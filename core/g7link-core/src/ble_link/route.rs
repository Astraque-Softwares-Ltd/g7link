// SPDX-License-Identifier: MPL-2.0
//! Phone: which path the touchpad and keyboard take to a PC, its session or
//! the Bluetooth link (docs/protocol/bluetooth.md §4.6). The user picks a
//! route per PC: Auto, Network or Bluetooth. The chosen path is used while
//! it's up and the other one when it isn't; Auto measures both and moves to
//! the clearly faster one. A path never changes in the middle of a gesture,
//! and discrete input goes one at a time, so order holds across a switch.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

use g7link_protocol::{
    DeviceId, Envelope,
    messages::{ButtonAction, Ping, RemoteInput, types},
};
use tokio_util::sync::CancellationToken;

use super::lock;
use crate::{Error, Result, events::LinkState, events::NodeEvent, node::Shared};

/// The route the user picked for a PC's touchpad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RemoteRoute {
    /// The faster of the two, measured.
    #[default]
    Auto,
    /// The session (Wi-Fi, USB, Away); the link only when there's none.
    Network,
    /// The Bluetooth link; the session only when it's down.
    Bluetooth,
}

/// The way input reaches a PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemotePath {
    Session,
    Ble,
}

impl RemotePath {
    fn other(self) -> Self {
        match self {
            RemotePath::Session => RemotePath::Ble,
            RemotePath::Ble => RemotePath::Session,
        }
    }
}

/// Where a PC's touchpad route stands, for the phone's header and picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RouteState {
    pub route: RemoteRoute,
    /// The path input takes now; `None` when neither is up.
    pub active: Option<RemotePath>,
    /// The chosen route's path is down and the other one is used.
    pub fallback: bool,
    pub session_rtt_ms: Option<u32>,
    pub ble_rtt_ms: Option<u32>,
    /// The PC offers `ble.remote`: the picker applies.
    pub ble_remote: bool,
    /// A Bluetooth link with that PC is up.
    pub ble_linked: bool,
}

impl RouteState {
    /// Whether `other` says something this doesn't: a round-trip time counts
    /// once it moves by 5 ms or more, as the session's does.
    fn differs(&self, other: &RouteState) -> bool {
        fn moved(a: Option<u32>, b: Option<u32>) -> bool {
            match (a, b) {
                (Some(a), Some(b)) => a.abs_diff(b) >= SHOWN_STEP_MS,
                (a, b) => a.is_some() != b.is_some(),
            }
        }
        self.route != other.route
            || self.active != other.active
            || self.fallback != other.fallback
            || self.ble_remote != other.ble_remote
            || self.ble_linked != other.ble_linked
            || moved(self.session_rtt_ms, other.session_rtt_ms)
            || moved(self.ble_rtt_ms, other.ble_rtt_ms)
    }
}

/// Round-trip samples kept per path.
const SAMPLES: usize = 8;
/// How often a touchpad that shows measures both paths, and Auto looks again.
const EVALUATE_EVERY: Duration = Duration::from_secs(2);
/// A probe with no answer by then counts as this long.
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
/// Auto moves when the other path wins this many evaluations in a row, by
/// this much (its score at most this share of the one in use's, and this many
/// ms less), at most once in this long.
const WINS_NEEDED: u8 = 3;
const WIN_RATIO: f64 = 0.7;
const WIN_MARGIN_MS: f64 = 15.0;
const MIN_DWELL: Duration = Duration::from_secs(20);
/// Quiet: this long since the last motion, and for Auto since the last key
/// or click.
const MOTION_QUIET: Duration = Duration::from_millis(150);
const INPUT_QUIET: Duration = Duration::from_millis(300);
/// A button still counted as held after this long with no input at all was
/// let go of where nothing saw it.
const HELD_FORGOTTEN: Duration = Duration::from_secs(10);
/// A discrete input on the link with no answer this long, or this many
/// pings lost in a row, mark the link stalled for a while: input moves to
/// the session meanwhile.
pub(crate) const STALL_AFTER: Duration = Duration::from_millis(1500);
const STALL_FOR: Duration = Duration::from_secs(10);
const PINGS_LOST: u8 = 2;
/// A shown round-trip time changes by at least this much.
const SHOWN_STEP_MS: u32 = 5;

/// The last round-trip times of one path, in ms.
#[derive(Debug, Default)]
struct Rtt {
    samples: VecDeque<u32>,
}

impl Rtt {
    fn add(&mut self, ms: u32) {
        if self.samples.len() == SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(ms);
    }

    fn clear(&mut self) {
        self.samples.clear();
    }

    fn median(&self) -> Option<u32> {
        let mut sorted = [0; SAMPLES];
        let n = self.samples.len().min(SAMPLES);
        for (slot, ms) in sorted.iter_mut().zip(&self.samples) {
            *slot = *ms;
        }
        let sorted = &mut sorted[..n];
        sorted.sort_unstable();
        sorted.get(n / 2).copied()
    }

    /// What Auto compares: mostly the median, but spikes count.
    fn score(&self) -> Option<f64> {
        let max = self.samples.iter().copied().max()?;
        Some(0.7 * f64::from(self.median()?) + 0.3 * f64::from(max))
    }
}

/// What's up between this phone and one PC, read when a decision is made.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Paths {
    pub session: bool,
    /// The link is up (stalled or not).
    pub ble: bool,
}

/// One PC's route and what it's decided from.
#[derive(Debug)]
pub(crate) struct PeerRoute {
    route: RemoteRoute,
    /// The PC offers `ble.remote`. Without it the route doesn't apply: input
    /// takes the link only when there's no session, as it did before.
    ble_remote: bool,
    /// Auto's pick while both paths are up.
    preferred: RemotePath,
    /// When Auto last changed its pick.
    last_switch: Option<Instant>,
    /// Evaluations in a row the other path won.
    wins: u8,
    /// The path input took last.
    active: Option<RemotePath>,
    session: Rtt,
    ble: Rtt,
    held_buttons: u8,
    in_flight: u8,
    last_motion: Option<Instant>,
    last_discrete: Option<Instant>,
    last_input: Option<Instant>,
    stalled_until: Option<Instant>,
    pings_lost: u8,
    /// Discrete input goes one at a time, from choosing its path until its
    /// answer, so the PC runs it in order whatever path each took.
    lane: Arc<tokio::sync::Mutex<()>>,
    /// Stops the probes run while a touchpad shows.
    probing: Option<CancellationToken>,
    /// The state last told to the app.
    shown: Option<RouteState>,
}

impl PeerRoute {
    pub(crate) fn new(ble_remote: bool) -> Self {
        PeerRoute {
            route: RemoteRoute::Auto,
            ble_remote,
            preferred: RemotePath::Session,
            last_switch: None,
            wins: 0,
            active: None,
            session: Rtt::default(),
            ble: Rtt::default(),
            held_buttons: 0,
            in_flight: 0,
            last_motion: None,
            last_discrete: None,
            last_input: None,
            stalled_until: None,
            pings_lost: 0,
            lane: Arc::default(),
            probing: None,
            shown: None,
        }
    }

    fn stalled(&self, now: Instant) -> bool {
        self.stalled_until.is_some_and(|until| now < until)
    }

    /// The link counts as up for input: up and not stalled.
    fn ble_up(&self, paths: Paths, now: Instant) -> bool {
        paths.ble && !self.stalled(now)
    }

    fn is_up(&self, path: RemotePath, paths: Paths, now: Instant) -> bool {
        match path {
            RemotePath::Session => paths.session,
            RemotePath::Ble => self.ble_up(paths, now),
        }
    }

    /// The path input should take now, gestures aside.
    fn desired(&self, paths: Paths, now: Instant) -> Option<RemotePath> {
        let (session, ble) = (paths.session, self.ble_up(paths, now));
        match (session, ble) {
            (false, false) => None,
            (true, false) => Some(RemotePath::Session),
            (false, true) => Some(RemotePath::Ble),
            (true, true) if !self.ble_remote => Some(RemotePath::Session),
            (true, true) => Some(match self.route {
                RemoteRoute::Network => RemotePath::Session,
                RemoteRoute::Bluetooth => RemotePath::Ble,
                RemoteRoute::Auto => self.preferred,
            }),
        }
    }

    /// Nothing would notice a switch now: no input waiting for its answer,
    /// no button held, no motion in the last 150 ms, and for Auto no key or
    /// click in the last 300 ms.
    fn quiet(&self, now: Instant, auto: bool) -> bool {
        let since =
            |at: Option<Instant>, gap: Duration| at.is_none_or(|t| now.saturating_duration_since(t) >= gap);
        self.in_flight == 0
            && (self.held_buttons == 0 || since(self.last_input, HELD_FORGOTTEN))
            && since(self.last_motion, MOTION_QUIET)
            && (!auto || since(self.last_discrete, INPUT_QUIET))
    }

    /// The path input takes now: the one in use while it's up and a switch
    /// would land mid-gesture, otherwise the one wanted. A path that went
    /// down is left at once.
    pub(crate) fn settle(&mut self, paths: Paths, now: Instant) -> Option<RemotePath> {
        let desired = self.desired(paths, now);
        let current_up = self.active.is_some_and(|path| self.is_up(path, paths, now));
        if current_up && desired != self.active && !self.quiet(now, false) {
            return self.active;
        }
        self.active = desired;
        desired
    }

    /// Auto's look at the two paths' times, every [`EVALUATE_EVERY`].
    pub(crate) fn evaluate(&mut self, paths: Paths, now: Instant) {
        let both = paths.session && self.ble_up(paths, now);
        let scores = self.session.score().zip(self.ble.score());
        let Some((session, ble)) =
            scores.filter(|_| both && self.ble_remote && self.route == RemoteRoute::Auto)
        else {
            self.wins = 0;
            return;
        };
        let (current, challenger) = match self.preferred {
            RemotePath::Session => (session, ble),
            RemotePath::Ble => (ble, session),
        };
        if challenger <= WIN_RATIO * current && current - challenger >= WIN_MARGIN_MS {
            self.wins = self.wins.saturating_add(1);
        } else {
            self.wins = 0;
        }
        let dwelt = self.last_switch.is_none_or(|at| now.saturating_duration_since(at) >= MIN_DWELL);
        if self.wins >= WINS_NEEDED && dwelt && self.quiet(now, true) {
            self.preferred = self.preferred.other();
            self.last_switch = Some(now);
            self.wins = 0;
        }
    }

    fn state(&self, paths: Paths, session_rtt: Option<u32>) -> RouteState {
        let fallback = self.ble_remote
            && matches!(
                (self.route, self.active),
                (RemoteRoute::Network, Some(RemotePath::Ble))
                    | (RemoteRoute::Bluetooth, Some(RemotePath::Session))
            );
        RouteState {
            route: self.route,
            active: self.active,
            fallback,
            session_rtt_ms: paths.session.then(|| self.session.median().or(session_rtt)).flatten(),
            ble_rtt_ms: paths.ble.then(|| self.ble.median()).flatten(),
            ble_remote: self.ble_remote,
            ble_linked: paths.ble,
        }
    }

    /// A discrete input is going out.
    fn input_started(&mut self, event: &RemoteInput, now: Instant) {
        if self.last_input.is_some_and(|t| now.saturating_duration_since(t) >= HELD_FORGOTTEN) {
            self.held_buttons = 0;
        }
        match event {
            RemoteInput::Button { action: ButtonAction::Down, .. } => {
                self.held_buttons = self.held_buttons.saturating_add(1).min(3);
            }
            RemoteInput::Button { action: ButtonAction::Up, .. } => {
                self.held_buttons = self.held_buttons.saturating_sub(1);
            }
            _ => {}
        }
        self.in_flight = self.in_flight.saturating_add(1);
        self.last_discrete = Some(now);
        self.last_input = Some(now);
    }

    /// A discrete input was answered (or failed) after `took`; a quick one's
    /// round trip is a sample of its path.
    fn input_done(&mut self, path: RemotePath, sample: Option<Duration>) {
        self.in_flight = self.in_flight.saturating_sub(1);
        if let Some(took) = sample {
            self.sample(path, took);
        }
    }

    fn motion(&mut self, now: Instant) {
        self.last_motion = Some(now);
        self.last_input = Some(now);
    }

    fn sample(&mut self, path: RemotePath, took: Duration) {
        let ms = u32::try_from(took.as_millis()).unwrap_or(u32::MAX);
        match path {
            RemotePath::Session => self.session.add(ms),
            RemotePath::Ble => self.ble.add(ms),
        }
    }

    /// A ping's answer on `path`, or `None` when none came in time.
    fn probed(&mut self, path: RemotePath, took: Option<Duration>, now: Instant) {
        self.sample(path, took.unwrap_or(PROBE_TIMEOUT));
        if path == RemotePath::Ble {
            match took {
                Some(_) => self.pings_lost = 0,
                None => {
                    self.pings_lost = self.pings_lost.saturating_add(1);
                    if self.pings_lost >= PINGS_LOST {
                        self.stall(now);
                    }
                }
            }
        }
    }

    fn stall(&mut self, now: Instant) {
        self.stalled_until = Some(now + STALL_FOR);
    }

    /// A path went down: what was measured on it no longer says anything.
    fn path_down(&mut self, path: RemotePath) {
        match path {
            RemotePath::Session => self.session.clear(),
            RemotePath::Ble => {
                self.ble.clear();
                self.stalled_until = None;
                self.pings_lost = 0;
            }
        }
    }
}

/// Whether `event`'s round trip says how fast its path is: a click or a key
/// is answered at once, but typed text takes as long as the typing.
fn measures(event: &RemoteInput) -> bool {
    matches!(
        event,
        RemoteInput::Button { .. }
            | RemoteInput::Key { .. }
            | RemoteInput::Scroll { .. }
            | RemoteInput::Slide { .. }
            | RemoteInput::Laser { .. }
    )
}

/// The routes of this phone's PCs.
#[derive(Debug, Default)]
pub(crate) struct Routes {
    peers: HashMap<DeviceId, PeerRoute>,
}

impl Routes {
    pub(crate) fn remove_peer(&mut self, peer: &DeviceId) {
        if let Some(route) = self.peers.remove(peer)
            && let Some(probing) = route.probing
        {
            probing.cancel();
        }
    }
}

impl Shared {
    /// Runs `f` on `peer`'s route, made first if there's none (only for a
    /// paired device). Reads the store: never call with a lock held.
    fn with_route<T>(&self, peer: &DeviceId, f: impl FnOnce(&mut PeerRoute) -> T) -> Result<T> {
        let exists = lock(&self.ble.routes).peers.contains_key(peer);
        let ble_remote = if exists {
            None
        } else {
            if !self.store.is_paired(peer)? {
                return Err(Error::NotPaired);
            }
            Some(self.peer_offers_ble_remote(peer))
        };
        let mut routes = lock(&self.ble.routes);
        let route = routes.peers.entry(*peer).or_insert_with(|| PeerRoute::new(ble_remote.unwrap_or(false)));
        Ok(f(route))
    }

    /// The paths to `peer` as its link state tells them: what the app is
    /// shown, and safe to read with the sessions locked. Also the session's
    /// own round-trip time, for before anything was measured here.
    fn shown_paths(&self, peer: &DeviceId) -> (Paths, Option<u32>) {
        let (session, rtt) = match self.link(peer, None) {
            LinkState::Online { rtt_ms, .. } => (true, (rtt_ms > 0).then_some(rtt_ms)),
            _ => (false, None),
        };
        (Paths { session, ble: self.ble_link(peer).is_some() }, rtt)
    }

    /// Settles `peer`'s route against `paths` and tells the app when what it
    /// shows changed.
    fn route_settled(&self, peer: &DeviceId, paths: Paths, session_rtt: Option<u32>, now: Instant) {
        let changed = {
            let mut routes = lock(&self.ble.routes);
            let Some(route) = routes.peers.get_mut(peer) else { return };
            route.settle(paths, now);
            let state = route.state(paths, session_rtt);
            let changed = route.shown.is_none_or(|shown| shown.differs(&state));
            if changed {
                route.shown = Some(state);
            }
            changed.then_some(state)
        };
        if let Some(state) = changed {
            self.emit(NodeEvent::RemoteRouteChanged { device: *peer, state });
        }
    }

    /// Something about the paths to `peer` changed (a session or a link came
    /// or went, a round-trip time moved). Takes no lock it could wait on:
    /// it's called with the sessions locked.
    pub(crate) fn route_refresh(&self, peer: &DeviceId) {
        let (paths, session_rtt) = self.shown_paths(peer);
        if !paths.session && !paths.ble {
            // The PC may restart before it's back: its states start over.
            crate::remote::forget_text_state(self, peer);
        }
        {
            let mut routes = lock(&self.ble.routes);
            let Some(route) = routes.peers.get_mut(peer) else { return };
            if !paths.session {
                route.path_down(RemotePath::Session);
            }
            if !paths.ble {
                route.path_down(RemotePath::Ble);
            }
        }
        self.route_settled(peer, paths, session_rtt, Instant::now());
    }

    /// `peer` announced what it offers again: whether it takes `ble.remote`.
    pub(crate) fn route_caps_changed(&self, peer: &DeviceId) {
        if !lock(&self.ble.routes).peers.contains_key(peer) {
            return;
        }
        let ble_remote = self.peer_offers_ble_remote(peer);
        if let Some(route) = lock(&self.ble.routes).peers.get_mut(peer) {
            route.ble_remote = ble_remote;
        }
        self.route_refresh(peer);
    }

    /// The path input for the PC `peer` takes now (the route's, or the other
    /// when that one is down). `Offline` when neither is up.
    pub(crate) fn path_for(&self, peer: &DeviceId) -> Result<RemotePath> {
        let paths = Paths { session: self.session(peer).is_some(), ble: self.ble_link(peer).is_some() };
        let now = Instant::now();
        let path = self.with_route(peer, |route| route.settle(paths, now))?;
        let (_, session_rtt) = self.shown_paths(peer);
        self.route_settled(peer, paths, session_rtt, now);
        path.ok_or(Error::Offline)
    }

    /// Whether this device can reach `peer` for input: a session or a link.
    pub(crate) fn input_reachable(&self, peer: &DeviceId) -> bool {
        self.session(peer).is_some() || self.ble_link(peer).is_some()
    }

    /// The lane `peer`'s discrete input waits its turn in.
    pub(crate) fn input_lane(&self, peer: &DeviceId) -> Result<Arc<tokio::sync::Mutex<()>>> {
        self.with_route(peer, |route| route.lane.clone())
    }

    /// A discrete input for `peer` goes out now; what [`input_done`] needs back.
    ///
    /// [`input_done`]: Self::input_done
    pub(crate) fn input_started(&self, peer: &DeviceId, event: &RemoteInput) -> (Instant, bool) {
        let now = Instant::now();
        let _ = self.with_route(peer, |route| route.input_started(event, now));
        (now, measures(event))
    }

    /// The discrete input started at `started` on `path` ended with `result`.
    pub(crate) fn input_done(
        &self,
        peer: &DeviceId,
        path: RemotePath,
        started: (Instant, bool),
        result: &Result<()>,
    ) {
        let (at, measures) = started;
        let sample = match result {
            Ok(()) if measures => Some(at.elapsed()),
            Err(Error::Timeout) => Some(PROBE_TIMEOUT),
            _ => None,
        };
        let _ = self.with_route(peer, |route| route.input_done(path, sample));
        if sample.is_some() {
            let (paths, session_rtt) = self.shown_paths(peer);
            self.route_settled(peer, paths, session_rtt, Instant::now());
        }
    }

    /// Pointer motion, a scroll or the laser goes out to `peer` now.
    pub(crate) fn input_moved(&self, peer: &DeviceId) {
        let now = Instant::now();
        let _ = self.with_route(peer, |route| route.motion(now));
    }

    /// A discrete input on the link to `peer` has had no answer for
    /// [`STALL_AFTER`]: input goes on the session for a while.
    pub(crate) fn ble_stalled(&self, peer: &DeviceId) {
        let now = Instant::now();
        if let Some(route) = lock(&self.ble.routes).peers.get_mut(peer) {
            route.stall(now);
        }
        tracing::debug!(peer = %peer.short(), "the Bluetooth link stalled; input takes the session");
        self.route_refresh(peer);
    }

    /// The route the user picked for the PC `peer`. The path changes once
    /// nothing is in the middle of a gesture.
    pub(crate) fn set_remote_route(&self, peer: &DeviceId, route: RemoteRoute) -> Result<()> {
        self.with_route(peer, |r| {
            r.route = route;
            r.wins = 0;
        })?;
        self.route_refresh(peer);
        Ok(())
    }

    /// Where `peer`'s route stands; the default for a device with none.
    pub(crate) fn remote_route_state(&self, peer: &DeviceId) -> RouteState {
        let (paths, session_rtt) = self.shown_paths(peer);
        if self.store.is_paired(peer).unwrap_or(false) {
            let now = Instant::now();
            if let Ok(state) = self.with_route(peer, |route| {
                route.settle(paths, now);
                route.state(paths, session_rtt)
            }) {
                return state;
            }
        }
        RouteState::default()
    }

    /// A touchpad for `peer` shows (`on`) or no longer does: while it shows,
    /// both paths are measured every 2 s and Auto looks at them.
    pub(crate) fn set_remote_active(self: &Arc<Self>, peer: &DeviceId, on: bool) -> Result<()> {
        let token = self.cancel.child_token();
        let started = self.with_route(peer, |route| {
            if !on {
                if let Some(probing) = route.probing.take() {
                    probing.cancel();
                }
                return false;
            }
            if route.probing.is_some() {
                return false;
            }
            // Each visit measures afresh.
            route.session.clear();
            route.ble.clear();
            route.probing = Some(token.clone());
            true
        })?;
        if started {
            self.runtime.spawn(probe(Arc::downgrade(self), *peer, token));
        }
        self.route_refresh(peer);
        Ok(())
    }
}

/// While a touchpad shows: pings both paths to `peer` every 2 s and lets
/// Auto look at the times, until cancelled.
async fn probe(shared: Weak<Shared>, peer: DeviceId, cancel: CancellationToken) {
    let mut tick = tokio::time::interval(EVALUATE_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            _ = tick.tick() => {}
        }
        let Some(shared) = shared.upgrade() else { return };
        let ble_remote = lock(&shared.ble.routes).peers.get(&peer).is_some_and(|r| r.ble_remote);
        if ble_remote {
            let (session, ble) = tokio::join!(ping_session(&shared, &peer), ping_ble(&shared, &peer));
            let now = Instant::now();
            if let Some(route) = lock(&shared.ble.routes).peers.get_mut(&peer) {
                if let Some(took) = session {
                    route.probed(RemotePath::Session, took, now);
                }
                if let Some(took) = ble {
                    route.probed(RemotePath::Ble, took, now);
                }
            }
        }
        let paths = Paths { session: shared.session(&peer).is_some(), ble: shared.ble_link(&peer).is_some() };
        let now = Instant::now();
        if let Some(route) = lock(&shared.ble.routes).peers.get_mut(&peer) {
            route.evaluate(paths, now);
        }
        let (_, session_rtt) = shared.shown_paths(&peer);
        shared.route_settled(&peer, paths, session_rtt, now);
    }
}

/// The session's round trip to `peer`: `None` without a session, `Some(None)`
/// when the ping wasn't answered in time.
async fn ping_session(shared: &Shared, peer: &DeviceId) -> Option<Option<Duration>> {
    let session = shared.session(peer)?;
    let ping = Envelope::new(types::PING, &Ping { ts: 0 }).ok()?;
    let started = Instant::now();
    match session.request(ping, PROBE_TIMEOUT).await {
        Ok(reply) if reply.t == types::PONG => Some(Some(started.elapsed())),
        Ok(_) => None,
        Err(_) => Some(None),
    }
}

/// The link's round trip to `peer`, as [`ping_session`].
async fn ping_ble(shared: &Shared, peer: &DeviceId) -> Option<Option<Duration>> {
    shared.ble_link(peer)?;
    let started = Instant::now();
    match shared.ble_request(*peer, Envelope::empty(types::PING), PROBE_TIMEOUT).await {
        Ok(reply) if reply.t == types::PONG => Some(Some(started.elapsed())),
        Ok(_) | Err(Error::Offline) => None,
        Err(_) => Some(None),
    }
}

#[cfg(test)]
mod tests {
    use g7link_protocol::messages::MouseButton;

    use super::*;

    const BOTH: Paths = Paths { session: true, ble: true };
    const SESSION: Paths = Paths { session: true, ble: false };
    const LINK: Paths = Paths { session: false, ble: true };
    const NONE: Paths = Paths { session: false, ble: false };

    fn route(route: RemoteRoute) -> PeerRoute {
        let mut r = PeerRoute::new(true);
        r.route = route;
        r
    }

    #[test]
    fn each_route_takes_its_path_and_falls_back_to_the_other() {
        use RemotePath::{Ble, Session};
        let now = Instant::now();
        let table = [
            (RemoteRoute::Network, [Some(Session), Some(Session), Some(Ble), None]),
            (RemoteRoute::Bluetooth, [Some(Ble), Some(Session), Some(Ble), None]),
            (RemoteRoute::Auto, [Some(Session), Some(Session), Some(Ble), None]),
        ];
        for (picked, wanted) in table {
            for (paths, wanted) in [BOTH, SESSION, LINK, NONE].into_iter().zip(wanted) {
                let mut r = route(picked);
                assert_eq!(r.settle(paths, now), wanted, "{picked:?} with {paths:?}");
                let state = r.state(paths, None);
                let fallback = matches!(
                    (picked, wanted),
                    (RemoteRoute::Network, Some(Ble)) | (RemoteRoute::Bluetooth, Some(Session))
                );
                assert_eq!(state.fallback, fallback, "{picked:?} with {paths:?}");
            }
        }
        // A PC without `ble.remote`: the link only without a session, as before.
        for picked in [RemoteRoute::Auto, RemoteRoute::Network, RemoteRoute::Bluetooth] {
            let mut old = PeerRoute::new(false);
            old.route = picked;
            assert_eq!(old.settle(BOTH, now), Some(Session));
            assert_eq!(old.settle(LINK, now), Some(Ble));
            assert!(!old.state(LINK, None).fallback);
        }
    }

    /// Feeds both paths a round of samples and evaluates, `at` seconds in.
    fn round(r: &mut PeerRoute, start: Instant, at: u64, session: u32, ble: u32) {
        let now = start + Duration::from_secs(at);
        r.sample(RemotePath::Session, Duration::from_millis(session.into()));
        r.sample(RemotePath::Ble, Duration::from_millis(ble.into()));
        r.evaluate(BOTH, now);
        r.settle(BOTH, now);
    }

    #[test]
    fn auto_stays_on_good_wifi_and_moves_on_a_bad_hotspot_day() {
        let start = Instant::now();
        let mut r = route(RemoteRoute::Auto);
        // Wi-Fi at 9-12 ms beats Bluetooth at about 35 ms: it stays.
        for (i, wifi) in [9, 12, 10, 11, 9, 12, 10, 11, 9, 10].into_iter().enumerate() {
            round(&mut r, start, 2 * i as u64, wifi, 35);
            assert_eq!(r.active, Some(RemotePath::Session));
        }

        // A hotspot at 75-120 ms with a 540 ms spike: Bluetooth wins on the
        // third evaluation in a row.
        let mut r = route(RemoteRoute::Auto);
        let hotspot = [80, 540, 100];
        for (i, ms) in hotspot.into_iter().enumerate() {
            assert_eq!(r.active.unwrap_or(RemotePath::Session), RemotePath::Session, "not before the third");
            round(&mut r, start, 2 * i as u64, ms, 45);
        }
        assert_eq!(r.active, Some(RemotePath::Ble));

        // Wi-Fi recovers at once: it doesn't come back before 20 s have passed.
        r.session.clear();
        for at in (6..24).step_by(2) {
            round(&mut r, start, at, 9, 45);
            assert_eq!(r.active, Some(RemotePath::Ble), "{at} s");
        }
        assert!(r.wins >= WINS_NEEDED);
        round(&mut r, start, 24, 9, 45);
        assert_eq!(r.active, Some(RemotePath::Session), "back 20 s after the switch");
    }

    #[test]
    fn a_switch_waits_for_the_gesture_to_end() {
        let start = Instant::now();
        let mut r = route(RemoteRoute::Network);
        assert_eq!(r.settle(BOTH, start), Some(RemotePath::Session));

        // A button held: the user's pick waits.
        let down = RemoteInput::Button { button: MouseButton::Left, action: ButtonAction::Down };
        r.input_started(&down, start);
        r.input_done(RemotePath::Session, None);
        r.route = RemoteRoute::Bluetooth;
        assert_eq!(r.settle(BOTH, start), Some(RemotePath::Session));
        let up = RemoteInput::Button { button: MouseButton::Left, action: ButtonAction::Up };
        r.input_started(&up, start);
        assert_eq!(r.settle(BOTH, start), Some(RemotePath::Session), "an input in flight");
        r.input_done(RemotePath::Session, None);
        assert_eq!(r.settle(BOTH, start), Some(RemotePath::Ble), "let go, answered");

        // Motion under 150 ms old.
        r.route = RemoteRoute::Network;
        let moved = start + Duration::from_secs(1);
        r.motion(moved);
        assert_eq!(r.settle(BOTH, moved + Duration::from_millis(100)), Some(RemotePath::Ble));
        assert_eq!(r.settle(BOTH, moved + MOTION_QUIET), Some(RemotePath::Session));

        // A failover doesn't wait: the link stalls mid-drag.
        r.route = RemoteRoute::Bluetooth;
        let later = moved + Duration::from_secs(1);
        assert_eq!(r.settle(BOTH, later), Some(RemotePath::Ble));
        r.input_started(&down, later);
        r.stall(later);
        assert_eq!(r.settle(BOTH, later), Some(RemotePath::Session), "a stalled link is down");
        assert!(r.state(BOTH, None).fallback);
        assert_eq!(r.settle(BOTH, later + STALL_FOR), Some(RemotePath::Session), "not mid-gesture");
        r.input_done(RemotePath::Ble, None);
        let dragging = later + Duration::from_secs(9);
        r.motion(dragging);
        assert_eq!(r.settle(BOTH, later + STALL_FOR), Some(RemotePath::Session), "the button is still down");
        assert_eq!(
            r.settle(BOTH, dragging + HELD_FORGOTTEN),
            Some(RemotePath::Ble),
            "a button nothing saw let go of is forgotten"
        );
    }

    #[test]
    fn auto_waits_for_typing_to_pause() {
        let start = Instant::now();
        let mut r = route(RemoteRoute::Auto);
        let key = RemoteInput::Key { key: "a".into(), mods: Vec::new() };
        for i in 0..3u64 {
            let now = start + Duration::from_secs(2 * i);
            r.sample(RemotePath::Session, Duration::from_millis(300));
            r.sample(RemotePath::Ble, Duration::from_millis(40));
            r.input_started(&key, now);
            r.input_done(RemotePath::Session, None);
            r.evaluate(BOTH, now + Duration::from_millis(100));
        }
        assert_eq!(r.preferred, RemotePath::Session, "a key 100 ms ago");
        r.evaluate(BOTH, start + Duration::from_secs(4) + INPUT_QUIET);
        assert_eq!(r.preferred, RemotePath::Ble);
    }

    #[test]
    fn two_lost_pings_stall_the_link() {
        let now = Instant::now();
        let mut r = route(RemoteRoute::Bluetooth);
        r.probed(RemotePath::Ble, None, now);
        assert_eq!(r.settle(BOTH, now), Some(RemotePath::Ble));
        r.probed(RemotePath::Ble, Some(Duration::from_millis(30)), now);
        r.probed(RemotePath::Ble, None, now);
        assert_eq!(r.settle(BOTH, now), Some(RemotePath::Ble), "not in a row");
        r.probed(RemotePath::Ble, None, now);
        assert_eq!(r.settle(BOTH, now), Some(RemotePath::Session));
        assert_eq!(r.settle(BOTH, now + STALL_FOR), Some(RemotePath::Ble));
        assert_eq!(r.ble.median(), Some(1500), "a lost ping counts as its timeout");
    }

    #[test]
    fn shown_times_change_by_five_ms_or_more() {
        let a = RouteState { session_rtt_ms: Some(10), ..RouteState::default() };
        assert!(!a.differs(&RouteState { session_rtt_ms: Some(14), ..a }));
        assert!(a.differs(&RouteState { session_rtt_ms: Some(15), ..a }));
        assert!(a.differs(&RouteState { session_rtt_ms: None, ..a }));
        assert!(a.differs(&RouteState { ble_linked: true, ..a }));
        let mut rtt = Rtt::default();
        for ms in [9, 12, 10, 11, 9, 12, 10, 11, 540] {
            rtt.add(ms);
        }
        assert_eq!(rtt.samples.len(), SAMPLES);
        assert_eq!(rtt.median(), Some(11));
        assert_eq!(rtt.score(), Some(0.7 * 11.0 + 0.3 * 540.0));
    }
}
