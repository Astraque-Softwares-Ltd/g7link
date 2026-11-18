// SPDX-License-Identifier: MPL-2.0
//! PC: a phone asking it to keep a Bluetooth link while its touchpad shows
//! (`ble.use`, docs/protocol/bluetooth.md §4.5). A hold lasts a lease the
//! phone renews; it lets the phone connect now, keeps the link once the
//! session is back, and connects again after a drop, within limits. A phone
//! that may not use Bluetooth yet can have the PC ask its user, at most once
//! a minute.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use g7link_protocol::{
    DeviceId, Envelope, ErrorCode,
    messages::{BleUse, BleUseReply, BleUseState, types},
};

use super::{BLE_LINK_CAPABILITY, BLE_LINK_TOGGLE, lock, wire_code};
use crate::{Error, Result, events::NodeEvent, features::Role, node::Shared};

/// How long a hold lasts unless renewed (the phone renews every 20 s).
pub(crate) const LEASE: Duration = Duration::from_secs(45);
/// A phone's ask raises the PC's prompt at most this often.
const PROMPT_GAP: Duration = Duration::from_secs(60);
/// A prompt counts as showing this long when the app never says it closed.
const PROMPT_SHOWS_FOR: Duration = Duration::from_secs(60);
/// Windows a hold opens by itself, at most this many in this long for one
/// phone: two minutes of advertising in five at worst.
const WINDOWS: usize = 4;
const WINDOWS_SPAN: Duration = Duration::from_secs(5 * 60);
/// A window opened this recently is still open: asking again opens none.
const WINDOW_STILL_OPEN: Duration = Duration::from_secs(20);

/// The phones holding a link with this PC, and who was asked what.
#[derive(Debug, Default)]
pub(crate) struct Holds {
    /// By phone: when its lease ends, and which hold this is (numbered, so a
    /// watch on an earlier one ends).
    held: HashMap<DeviceId, (Instant, u64)>,
    started: u64,
    /// When each phone last raised the prompt.
    prompted: HashMap<DeviceId, Instant>,
    /// Prompts showing, since when.
    showing: HashMap<DeviceId, Instant>,
    /// Windows each phone's holds opened, the oldest first.
    windows: HashMap<DeviceId, VecDeque<Instant>>,
}

impl Holds {
    /// Holds `peer`'s link for another lease from `now`: the hold's number
    /// when it's a new one.
    fn hold(&mut self, peer: DeviceId, now: Instant) -> Option<u64> {
        let until = now + LEASE;
        if let Some((lease, _)) = self.held.get_mut(&peer) {
            *lease = until;
            return None;
        }
        self.started += 1;
        self.held.insert(peer, (until, self.started));
        Some(self.started)
    }

    /// Ends `peer`'s hold: whether there was one.
    fn release(&mut self, peer: &DeviceId) -> bool {
        self.held.remove(peer).is_some()
    }

    /// When hold `number` of `peer`'s lease ends; `None` once it's over.
    fn lease(&self, peer: &DeviceId, number: u64) -> Option<Instant> {
        self.held.get(peer).filter(|(_, n)| *n == number).map(|(until, _)| *until)
    }

    /// Ends hold `number` of `peer` if its lease has run out by `now`.
    fn expire(&mut self, peer: &DeviceId, number: u64, now: Instant) -> bool {
        if self.lease(peer, number).is_some_and(|until| until <= now) {
            self.held.remove(peer);
            return true;
        }
        false
    }

    fn is_held(&self, peer: &DeviceId) -> bool {
        self.held.contains_key(peer)
    }

    /// Whether `peer` may raise the prompt now: the first time, and again a
    /// minute after the last. It's recorded as showing when it may.
    fn may_prompt(&mut self, peer: DeviceId, now: Instant) -> bool {
        if self.prompted.get(&peer).is_some_and(|at| now.saturating_duration_since(*at) < PROMPT_GAP) {
            return false;
        }
        self.prompted.insert(peer, now);
        self.showing.insert(peer, now);
        true
    }

    /// Whether `peer`'s prompt is showing.
    fn showing(&mut self, peer: &DeviceId, now: Instant) -> bool {
        match self.showing.get(peer) {
            Some(since) if now.saturating_duration_since(*since) < PROMPT_SHOWS_FOR => true,
            Some(_) => {
                self.showing.remove(peer);
                false
            }
            None => false,
        }
    }

    fn prompt_closed(&mut self, peer: &DeviceId) {
        self.showing.remove(peer);
    }

    /// Whether a window may open for `peer` now: not while one it opened is
    /// still open, and past [`WINDOWS`] in [`WINDOWS_SPAN`] only for one the
    /// phone's user asked for (`user`). Recorded when it may.
    fn window(&mut self, peer: DeviceId, now: Instant, user: bool) -> bool {
        let opened = self.windows.entry(peer).or_default();
        while opened.front().is_some_and(|at| now.saturating_duration_since(*at) >= WINDOWS_SPAN) {
            opened.pop_front();
        }
        if opened.back().is_some_and(|at| now.saturating_duration_since(*at) < WINDOW_STILL_OPEN) {
            return false;
        }
        if !user && opened.len() >= WINDOWS {
            return false;
        }
        if opened.len() >= WINDOWS {
            opened.pop_front();
        }
        opened.push_back(now);
        true
    }

    pub(crate) fn remove_peer(&mut self, peer: &DeviceId) {
        self.held.remove(peer);
        self.prompted.remove(peer);
        self.showing.remove(peer);
        self.windows.remove(peer);
    }
}

impl Shared {
    /// PC: whether `peer` holds its Bluetooth link with this PC.
    pub(crate) fn ble_held(&self, peer: &DeviceId) -> bool {
        lock(&self.ble.holds).is_held(peer)
    }

    /// PC: `peer`'s `ble.use` request, from a session or the link: the reply.
    pub(crate) fn ble_use_reply(self: &Arc<Self>, peer: DeviceId, env: &Envelope) -> Envelope {
        let Ok(request) = env.body::<BleUse>() else {
            return Envelope::error(ErrorCode::BadMessage, "invalid ble.use");
        };
        match self.answer_ble_use(peer, request, Instant::now()) {
            Ok(reply) => Envelope::new(types::BLE_USE, &reply)
                .unwrap_or_else(|_| Envelope::error(ErrorCode::Internal, "ble.use")),
            Err(error) => Envelope::error(wire_code(&error), error.to_string()),
        }
    }

    fn answer_ble_use(
        self: &Arc<Self>,
        peer: DeviceId,
        request: BleUse,
        now: Instant,
    ) -> Result<BleUseReply> {
        let roles = (self.local_role(), self.peer_role(&peer));
        if roles != (Some(Role::Desktop), Some(Role::Phone)) {
            return Err(Error::Unsupported);
        }
        let reply = |state| BleUseReply { state, lease: None };
        if !request.on {
            if lock(&self.ble.holds).release(&peer) {
                self.emit(NodeEvent::BleHoldChanged { device: peer, held: false });
            }
            return Ok(reply(BleUseState::Off));
        }
        if !self.we_offer(BLE_LINK_CAPABILITY) {
            return Ok(reply(BleUseState::Unavailable));
        }
        if !self.toggle_on(&peer, BLE_LINK_TOGGLE) {
            let mut holds = lock(&self.ble.holds);
            if holds.showing(&peer, now) {
                return Ok(reply(BleUseState::Asking));
            }
            if request.ask && holds.may_prompt(peer, now) {
                drop(holds);
                tracing::info!(peer = %peer.short(), "a phone asks to use Bluetooth with this PC");
                self.emit(NodeEvent::BleLinkRequested { device: peer });
                return Ok(reply(BleUseState::Asking));
            }
            return Ok(reply(BleUseState::Off));
        }
        if !self.ble_usable(&peer) {
            return Ok(reply(BleUseState::Unavailable));
        }
        if self.ble_linked_peers().iter().any(|other| *other != peer) {
            return Ok(reply(BleUseState::Busy));
        }
        let fresh = lock(&self.ble.holds).hold(peer, now);
        if let Some(number) = fresh {
            self.watch_lease(peer, number);
            self.emit(NodeEvent::BleHoldChanged { device: peer, held: true });
        }
        let lease = Some(u32::try_from(LEASE.as_secs()).unwrap_or(u32::MAX));
        if self.ble_link(&peer).is_some() {
            return Ok(BleUseReply { state: BleUseState::Linked, lease });
        }
        if lock(&self.ble.holds).window(peer, now, request.ask) {
            self.emit(NodeEvent::BleWanted { device: peer });
        }
        Ok(BleUseReply { state: BleUseState::Open, lease })
    }

    /// Ends hold `number` of `peer` once its lease runs out, renewals and all.
    fn watch_lease(self: &Arc<Self>, peer: DeviceId, number: u64) {
        let shared = Arc::downgrade(self);
        let cancel = self.cancel.clone();
        self.runtime.spawn(async move {
            loop {
                let until = shared.upgrade().and_then(|shared| lock(&shared.ble.holds).lease(&peer, number));
                let Some(until) = until else { return };
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = tokio::time::sleep_until(until.into()) => {}
                }
                let Some(shared) = shared.upgrade() else { return };
                if lock(&shared.ble.holds).expire(&peer, number, Instant::now()) {
                    tracing::debug!(peer = %peer.short(), "a Bluetooth hold ran out");
                    shared.emit(NodeEvent::BleHoldChanged { device: peer, held: false });
                    return;
                }
            }
        });
    }

    /// PC: the link with `peer` went. A phone that holds it gets a window to
    /// connect again, a few times in five minutes at most.
    pub(crate) fn held_link_down(&self, peer: DeviceId) {
        let reopen = {
            let mut holds = lock(&self.ble.holds);
            holds.is_held(&peer) && holds.window(peer, Instant::now(), false)
        };
        if reopen {
            self.emit(NodeEvent::BleWanted { device: peer });
        }
    }

    /// PC: the prompt for `peer` closed without its toggle being turned on.
    pub(crate) fn ble_prompt_closed(&self, peer: &DeviceId) {
        lock(&self.ble.holds).prompt_closed(peer);
    }

    /// The `ble_link` toggle for `peer` was set here: a prompt is answered,
    /// and off ends a hold.
    pub(crate) fn ble_toggled(&self, peer: DeviceId, on: bool) {
        let released = {
            let mut holds = lock(&self.ble.holds);
            holds.prompt_closed(&peer);
            !on && holds.release(&peer)
        };
        if released {
            self.emit(NodeEvent::BleHoldChanged { device: peer, held: false });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: DeviceId = DeviceId([1; 32]);
    const OTHER: DeviceId = DeviceId([2; 32]);

    #[test]
    fn a_hold_lasts_until_released_or_its_last_lease_runs_out() {
        let start = Instant::now();
        let at = |secs| start + Duration::from_secs(secs);
        let mut holds = Holds::default();
        let first = holds.hold(PHONE, at(0)).expect("a new hold");
        assert!(holds.is_held(&PHONE));
        assert_eq!(holds.hold(PHONE, at(20)), None, "renewed");
        assert!(!holds.expire(&PHONE, first, at(45)), "the first lease doesn't end it");
        assert_eq!(holds.lease(&PHONE, first), Some(at(65)));
        assert!(holds.expire(&PHONE, first, at(65)));
        assert!(!holds.is_held(&PHONE));
        assert_eq!(holds.lease(&PHONE, first), None);

        let second = holds.hold(PHONE, at(70)).expect("a new hold");
        assert!(holds.release(&PHONE));
        assert!(!holds.release(&PHONE));
        let third = holds.hold(PHONE, at(80)).expect("a new hold");
        assert!(!holds.expire(&PHONE, second, at(200)), "an earlier hold's watch ends nothing");
        assert!(holds.expire(&PHONE, third, at(200)));
    }

    #[test]
    fn a_phone_raises_the_prompt_once_a_minute_at_most() {
        let start = Instant::now();
        let mut holds = Holds::default();
        assert!(holds.may_prompt(PHONE, start));
        assert!(holds.showing(&PHONE, start));
        assert!(!holds.may_prompt(PHONE, start + Duration::from_secs(59)));
        assert!(holds.may_prompt(OTHER, start), "each phone its own");
        holds.prompt_closed(&PHONE);
        assert!(!holds.showing(&PHONE, start), "Not now");
        assert!(!holds.may_prompt(PHONE, start + Duration::from_secs(30)), "not again so soon");
        assert!(holds.may_prompt(PHONE, start + PROMPT_GAP));
        // A prompt nobody answered stops counting as showing.
        assert!(!holds.showing(&PHONE, start + PROMPT_GAP + PROMPT_SHOWS_FOR));
        holds.remove_peer(&PHONE);
        assert!(holds.may_prompt(PHONE, start + PROMPT_GAP), "unpairing forgets it");
    }

    #[test]
    fn a_hold_opens_four_windows_in_five_minutes_at_most() {
        let start = Instant::now();
        let mut holds = Holds::default();
        let at = |secs| start + Duration::from_secs(secs);
        assert!(holds.window(PHONE, at(0), false));
        assert!(!holds.window(PHONE, at(10), false), "still open");
        assert!(!holds.window(PHONE, at(10), true), "still open, even when asked");
        for secs in [30, 60, 90] {
            assert!(holds.window(PHONE, at(secs), false), "{secs} s");
        }
        assert!(!holds.window(PHONE, at(120), false), "the fifth in five minutes");
        assert!(holds.window(OTHER, at(120), false), "each phone its own");
        assert!(holds.window(PHONE, at(150), true), "the phone's user asked again");
        assert!(!holds.window(PHONE, at(329), false));
        assert!(holds.window(PHONE, at(330), false), "the one at 30 s has aged out");
    }
}
