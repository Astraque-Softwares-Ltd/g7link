// SPDX-License-Identifier: GPL-3.0-or-later
//! Handing a thread over between this PC and a phone (phase 5 §V8.5).
//!
//! - **To a phone**: "Continue on Pixel" in the thread's menu, or the
//!   launcher's action (the host's `ui.handoff` event). The composer's draft
//!   is saved first; the phone opens the thread at the first message that
//!   was on this screen and loads the draft.
//! - **From a phone**: the Agents page opens at that thread and message with
//!   "Opened from Pixel". A locked PC keeps one such hand-off for 10 minutes
//!   and opens it when it's unlocked, once.
//! - **Walking away**: when this PC locks (not when it's merely idle) and a
//!   thread was on screen in the last 60 s that works, waits for you or has
//!   a draft, the phones are offered it quietly. Not while this PC or that
//!   phone is in Do Not Disturb (§V8.6).
//!
//! Nothing here unlocks the PC, types, sends a message or answers anything:
//! a hand-off only opens a view.

use std::{
    sync::{Mutex, MutexGuard, OnceLock},
    time::{Duration, Instant},
};

use g7_agents::api::{self, State};
use g7link_core::{
    AgentHandedOff, AgentHandoff, DeviceId, LinkState, Node, agent, agent_handoff::handoff as wire,
};

use super::presence::Here;
use crate::{core_host, i18n::tr};

/// A hand-off from a phone waits this long for the PC to be unlocked.
pub(crate) const PENDING_LIFE: Duration = Duration::from_secs(600);
/// A thread on screen this recently is the one offered when the PC locks.
pub(crate) const SEEN_WITHIN: Duration = Duration::from_secs(60);

// ---- The rules (pure) ----

/// A thread a phone handed over while this PC was locked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pending {
    pub thread: String,
    /// The first message on the phone's screen.
    pub seq: Option<u64>,
    /// The phone, by name, for "Opened from %1".
    pub from: String,
    at: Instant,
}

/// What a phone's hand-off did here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arrived {
    /// The page shows it now, and someone is at the PC.
    Opened,
    /// The page shows it now, but nobody has used the PC in the last 30 s.
    Away,
    /// The PC is locked: it opens when it's unlocked.
    Locked,
}

impl Arrived {
    /// The phone's answer (`agent.handoff.open`).
    pub(crate) fn wire(self) -> AgentHandedOff {
        match self {
            Arrived::Opened => AgentHandedOff::opened(),
            Arrived::Away => AgentHandedOff::away(),
            Arrived::Locked => AgentHandedOff::locked(),
        }
    }
}

/// What one reading asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// The PC was unlocked: open the hand-off that waited.
    Open(Pending),
    /// The PC just locked with this thread on screen a moment ago.
    Leaving(String),
}

/// Hand-offs on this PC: the one waiting for an unlock, the thread last on
/// screen, and whether the PC was locked at the last reading.
#[derive(Debug, Default)]
pub(crate) struct Handoffs {
    pending: Option<Pending>,
    seen: Option<(String, Instant)>,
    locked: Option<bool>,
    /// The phones "Continue on" offers, as last read.
    phones: Vec<(DeviceId, String)>,
}

impl Handoffs {
    /// A phone handed `thread` over: opened now, or kept (one, the newest)
    /// until the PC is unlocked.
    pub(crate) fn handed_over(
        &mut self,
        here: Here,
        thread: String,
        seq: Option<u64>,
        from: String,
        now: Instant,
    ) -> Arrived {
        if here.locked {
            self.pending = Some(Pending { thread, seq, from, at: now });
            Arrived::Locked
        } else if here.at_pc {
            Arrived::Opened
        } else {
            Arrived::Away
        }
    }

    /// One reading: whether the PC is locked and the thread the page shows
    /// in the active window (`None`: none).
    pub(crate) fn tick(&mut self, now: Instant, locked: bool, on_screen: Option<&str>) -> Vec<Step> {
        let mut steps = Vec::new();
        if let Some(thread) = on_screen.filter(|_| !locked) {
            self.seen = Some((thread.to_owned(), now));
        }
        let was = self.locked.replace(locked);
        // Locking, not idleness: only the change from unlocked counts.
        if was == Some(false)
            && locked
            && let Some((thread, at)) = self.seen.take()
            && now.saturating_duration_since(at) <= SEEN_WITHIN
        {
            steps.push(Step::Leaving(thread));
        }
        if self.pending.as_ref().is_some_and(|p| now.saturating_duration_since(p.at) > PENDING_LIFE) {
            self.pending = None;
        }
        if !locked && let Some(pending) = self.pending.take() {
            steps.push(Step::Open(pending));
        }
        steps
    }
}

/// The thread is worth offering when you walk away: it works, waits for
/// you, or has a draft. `state`: `None` when it isn't known here.
pub(crate) fn worth_offering(state: Option<State>, draft: bool) -> bool {
    state.is_some_and(State::is_running) || draft
}

/// Which phone the launcher's hand-off goes to: the one it named when that
/// can take it, else the first that can (`phones` in their paired order).
pub(crate) fn chosen(phones: &[(DeviceId, String)], named: Option<&str>) -> Option<DeviceId> {
    match named {
        Some(id) => phones.iter().find(|(d, _)| d.to_string() == id).map(|(d, _)| *d),
        None => phones.first().map(|(d, _)| *d),
    }
}

/// The phones a walk-away offer goes to: none while this PC is in Do Not
/// Disturb, and never one in Do Not Disturb itself (§V8.6).
pub(crate) fn offered_to(
    phones: &[(DeviceId, String)],
    pc_dnd: bool,
    phone_dnd: impl Fn(DeviceId) -> bool,
) -> Vec<DeviceId> {
    if pc_dnd {
        return Vec::new();
    }
    phones.iter().map(|(d, _)| *d).filter(|d| !phone_dnd(*d)).collect()
}

/// `agent.handoff` for `thread`: held 10 minutes on the phone.
pub(crate) fn handoff(thread: &str, seq: Option<u64>, offer: bool) -> AgentHandoff {
    AgentHandoff { thread: thread.to_owned(), seq, offer, ttl_ms: wire::TTL_MS }
}

// ---- The state ----

fn handoffs() -> MutexGuard<'static, Handoffs> {
    static HANDOFFS: OnceLock<Mutex<Handoffs>> = OnceLock::new();
    HANDOFFS.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// One reading of presence's task (every 3 s): opens a hand-off that waited
/// for the unlock, and offers the thread on screen to the phones when the PC
/// just locked. True when the phones "Continue on" offers changed.
pub(crate) fn tick(locked: bool, dnd: bool, on_screen: Option<&str>) -> bool {
    let phones = phones();
    let (steps, changed) = {
        let mut h = handoffs();
        let changed = h.phones != phones;
        h.phones = phones;
        (h.tick(Instant::now(), locked, on_screen), changed)
    };
    for step in steps {
        match step {
            Step::Open(pending) => opened_here(&pending.thread, pending.seq, &pending.from),
            Step::Leaving(thread) => core_host::spawn(walk_away(thread, dnd)),
        }
    }
    changed
}

/// The phones "Continue on" offers, as presence's task last read them.
pub(crate) fn shown_phones() -> Vec<(DeviceId, String)> {
    handoffs().phones.clone()
}

/// Opens `thread` at `seq` on the Agents page, as a notification's Open
/// does, with "Opened from %1".
fn opened_here(thread: &str, seq: Option<u64>, from: &str) {
    crate::bridge::app::show_page("agents");
    super::open_at(super::OpenKind::Thread, thread.to_owned(), seq);
    crate::bridge::app::show_message(tr!("Agents", "Opened from %1", from));
}

/// A phone handed `thread` over (at its message `seq`).
pub(crate) async fn from_phone(from: DeviceId, thread: String, seq: Option<u64>) -> Arrived {
    let name = super::device_name(&from);
    let here = tokio::task::spawn_blocking(super::presence::here_now).await.unwrap_or_default();
    let arrived = handoffs().handed_over(here, thread.clone(), seq, name.clone(), Instant::now());
    if arrived != Arrived::Locked {
        opened_here(&thread, seq, &name);
    }
    arrived
}

/// Phones a thread can be handed to now: paired, connected, with agents on
/// and G7 Link's agents, in their paired order, by name.
pub(crate) fn phones() -> Vec<(DeviceId, String)> {
    let Some(host) = core_host::try_host() else { return Vec::new() };
    let online: Vec<(DeviceId, String)> = host.hub.read(|s| {
        s.devices
            .iter()
            .filter(|d| matches!(d.link, LinkState::Online { .. }))
            .map(|d| (d.id, s.name_of(&d.id).unwrap_or_else(|| d.info.name.clone())))
            .collect()
    });
    if online.is_empty() {
        return online;
    }
    let Some(node) = core_host::node() else { return Vec::new() };
    online
        .into_iter()
        .filter(|(id, _)| {
            node.device_toggles(*id).is_ok_and(|t| t.contains(&(agent::TOGGLE, true)))
                && node.peer_offers(*id, agent::REMOTE)
        })
        .collect()
}

/// Sends `thread` to `device`: opened there when G7 Link is in front, else a
/// notification (`offer`: the quiet walk-away one). False when it didn't go.
async fn send(node: &Node, device: DeviceId, thread: &str, seq: Option<u64>, offer: bool) -> bool {
    match node.agent_handoff(Some(device), handoff(thread, seq, offer)).await {
        Ok(sent) => sent > 0,
        Err(e) => {
            tracing::debug!(error = %e, "a hand-off didn't reach the phone");
            false
        }
    }
}

/// "Continue on <phone>" from this PC's thread menu: the draft is saved,
/// then the phone opens the thread at `seq`.
pub(crate) async fn to_phone(device: DeviceId, thread: String, seq: Option<u64>) -> bool {
    let Some(node) = core_host::node() else { return false };
    if !phones().iter().any(|(d, _)| *d == device) {
        return false;
    }
    super::flush_draft(&thread).await;
    send(&node, device, &thread, seq, false).await
}

/// The host's `ui.handoff` event (the launcher's "Continue on"): the
/// launcher saved its draft first.
pub(crate) fn from_launcher(thread: String, device: Option<String>, seq: Option<u64>) {
    if !api::is_valid_thread_id(&thread) {
        return;
    }
    let Some(node) = core_host::node() else { return };
    let phones = phones();
    match chosen(&phones, device.as_deref()) {
        Some(device) => {
            core_host::spawn(async move {
                send(&node, device, &thread, seq, false).await;
            });
        }
        None => tracing::debug!("no phone can take the launcher's hand-off"),
    }
}

/// The PC locked with `thread` on screen a moment ago: offered to the
/// phones when it's worth it.
async fn walk_away(thread: String, pc_dnd: bool) {
    let Some(node) = core_host::node() else { return };
    let (state, seq) = super::walk_away_facts(&thread);
    let draft = super::flush_draft(&thread).await;
    if !worth_offering(state, draft) {
        return;
    }
    let phones = phones();
    let to = offered_to(&phones, pc_dnd, |id| node.phone_toggles(id).is_some_and(|t| t.dnd));
    for device in to {
        send(&node, device, &thread, seq, true).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(c: char) -> String {
        c.to_string().repeat(32)
    }

    fn phone(n: u8) -> DeviceId {
        DeviceId([n; 32])
    }

    const AT: Here = Here { at_pc: true, locked: false, dnd: false };
    const AWAY: Here = Here { at_pc: false, locked: false, dnd: false };
    const LOCKED: Here = Here { at_pc: false, locked: true, dnd: false };

    #[test]
    fn a_hand_off_to_a_locked_pc_opens_once_when_it_is_unlocked() {
        let t0 = Instant::now();
        let mut h = Handoffs::default();
        assert!(h.tick(t0, true, None).is_empty());
        let arrived = h.handed_over(LOCKED, thread('a'), Some(7), "Pixel".into(), t0);
        assert_eq!(arrived, Arrived::Locked);
        assert!(h.tick(t0 + Duration::from_secs(3), true, None).is_empty(), "still locked");
        let steps = h.tick(t0 + Duration::from_secs(6), false, None);
        let [Step::Open(p)] = steps.as_slice() else { panic!("{steps:?}") };
        assert_eq!((p.thread.as_str(), p.seq, p.from.as_str()), (thread('a').as_str(), Some(7), "Pixel"));
        // Never twice, even when the PC locks and unlocks again.
        assert!(h.tick(t0 + Duration::from_secs(9), false, None).is_empty());
        assert!(h.tick(t0 + Duration::from_secs(12), true, None).is_empty());
        assert!(h.tick(t0 + Duration::from_secs(15), false, None).is_empty());
    }

    #[test]
    fn an_unlocked_pc_opens_it_at_once_and_keeps_nothing() {
        let t0 = Instant::now();
        let mut h = Handoffs::default();
        assert_eq!(h.handed_over(AT, thread('a'), None, "Pixel".into(), t0), Arrived::Opened);
        assert_eq!(
            h.handed_over(AWAY, thread('b'), None, "Pixel".into(), t0),
            Arrived::Away,
            "open, nobody there"
        );
        assert!(h.tick(t0, false, None).is_empty());
        assert_eq!(Arrived::Opened.wire(), AgentHandedOff::opened());
        assert_eq!(Arrived::Away.wire(), AgentHandedOff::away());
        assert_eq!(Arrived::Locked.wire(), AgentHandedOff::locked());
    }

    #[test]
    fn a_waiting_hand_off_lasts_ten_minutes_and_the_newest_wins() {
        let t0 = Instant::now();
        let mut h = Handoffs::default();
        h.tick(t0, true, None);
        h.handed_over(LOCKED, thread('a'), None, "Pixel".into(), t0);
        h.handed_over(LOCKED, thread('b'), None, "Pixel".into(), t0 + Duration::from_secs(60));
        let steps = h.tick(t0 + Duration::from_secs(61), false, None);
        assert!(matches!(steps.as_slice(), [Step::Open(p)] if p.thread == thread('b')), "one, the newest");

        let mut h = Handoffs::default();
        h.tick(t0, true, None);
        h.handed_over(LOCKED, thread('a'), None, "Pixel".into(), t0);
        assert!(h.tick(t0 + PENDING_LIFE + Duration::from_secs(1), true, None).is_empty());
        assert!(h.tick(t0 + PENDING_LIFE + Duration::from_secs(4), false, None).is_empty(), "it ran out");
    }

    #[test]
    fn locking_offers_the_thread_seen_in_the_last_minute() {
        let t0 = Instant::now();
        let mut h = Handoffs::default();
        assert!(h.tick(t0, false, Some(&thread('a'))).is_empty());
        // The window lost the keyboard to the lock screen first.
        assert!(h.tick(t0 + Duration::from_secs(3), false, None).is_empty());
        let steps = h.tick(t0 + Duration::from_secs(6), true, None);
        assert_eq!(steps, vec![Step::Leaving(thread('a'))]);
        // Once per lock.
        assert!(h.tick(t0 + Duration::from_secs(9), true, None).is_empty());
    }

    #[test]
    fn idleness_a_stale_thread_or_a_first_reading_offers_nothing() {
        let t0 = Instant::now();
        // Seen 61 s before the lock.
        let mut h = Handoffs::default();
        h.tick(t0, false, Some(&thread('a')));
        h.tick(t0 + Duration::from_secs(30), false, None);
        assert!(h.tick(t0 + SEEN_WITHIN + Duration::from_secs(1), true, None).is_empty());
        // Already locked at the first reading: no change was seen.
        let mut h = Handoffs::default();
        assert!(h.tick(t0, true, Some(&thread('a'))).is_empty());
        // Unlocked and idle the whole time: nothing.
        let mut h = Handoffs::default();
        h.tick(t0, false, Some(&thread('a')));
        assert!(h.tick(t0 + Duration::from_secs(40), false, None).is_empty());
        // Nothing on screen before the lock.
        let mut h = Handoffs::default();
        h.tick(t0, false, None);
        assert!(h.tick(t0 + Duration::from_secs(3), true, None).is_empty());
    }

    #[test]
    fn a_thread_is_offered_when_it_works_waits_or_has_a_draft() {
        for (state, offered) in [
            (State::Working, true),
            (State::Waiting, true),
            (State::Done, false),
            (State::Failed, false),
            (State::Interrupted, false),
        ] {
            assert_eq!(worth_offering(Some(state), false), offered, "{state:?}");
            assert!(worth_offering(Some(state), true), "a draft: {state:?}");
        }
        assert!(!worth_offering(None, false));
        assert!(worth_offering(None, true));
    }

    #[test]
    fn do_not_disturb_on_either_side_holds_the_walk_away_offer() {
        let phones = vec![(phone(1), "A".to_owned()), (phone(2), "B".to_owned())];
        assert_eq!(offered_to(&phones, false, |_| false), vec![phone(1), phone(2)]);
        assert_eq!(offered_to(&phones, false, |d| d == phone(1)), vec![phone(2)]);
        assert_eq!(offered_to(&phones, true, |_| false), Vec::<DeviceId>::new());
    }

    #[test]
    fn the_launcher_hand_off_goes_to_the_phone_it_named_or_the_first() {
        let phones = vec![(phone(1), "A".to_owned()), (phone(2), "B".to_owned())];
        assert_eq!(chosen(&phones, None), Some(phone(1)));
        assert_eq!(chosen(&phones, Some(&phone(2).to_string())), Some(phone(2)));
        assert_eq!(chosen(&phones, Some(&phone(3).to_string())), None, "not one that can take it");
        assert_eq!(chosen(&[], None), None);
    }

    #[test]
    fn a_hand_off_holds_ten_minutes_on_the_phone() {
        let sent = handoff(&thread('a'), Some(4), true);
        assert_eq!((sent.seq, sent.offer, sent.ttl_ms), (Some(4), true, 600_000));
        assert!(sent.is_valid());
    }
}
