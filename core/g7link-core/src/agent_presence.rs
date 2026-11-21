// SPDX-License-Identifier: MPL-2.0
//! Presence (`docs/protocol/agents.md` §7): a PC tells its phones whether
//! someone is at it, whether it is locked or in Do Not Disturb, and which of
//! their threads and sessions are on its screen; a phone tells its PC which
//! thread or session it shows. A phone uses it to hold a notification for a
//! thread you're reading on the PC; a PC, not to notify about a thread you're
//! reading on the phone.
//!
//! PC side: the platform hands the core its presence (G7 Link desktop, from
//! the agent host's `ui.presence`); the core sends it to every phone that
//! gets agent pushes, sessions only to those that may see them, and again,
//! with what's left of its time, to a phone that connects. It keeps what
//! each phone shows, answers `agent.viewing` itself, and never prompts for
//! it. Phone side: it keeps each PC's presence until its time is up or the
//! link drops, and sends what this phone shows one at a time per PC, in
//! order, at most one every 2 s (a change waits for its turn, the newest
//! wins; `visible: false` doesn't wait). Nothing here is retried, queued
//! while offline, or carried over Bluetooth.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub use g7link_protocol::messages::{AgentPresence, AgentViewing, presence};
use g7link_protocol::{
    DeviceId, Envelope, ErrorCode,
    messages::{Empty, agent, types},
};

use crate::{
    Error, Node, Result,
    agents::{self, SESSIONS_TOGGLE, TOGGLE},
    dispatch::Responder,
    events::NodeEvent,
    node::Shared,
    session::Session,
};

/// How long a phone waits for its PC to take one `agent.viewing`.
const VIEWING_WAIT: Duration = Duration::from_secs(10);
const VIEW_GAP: Duration = Duration::from_millis(presence::VIEW_GAP_MS);
const VIEW_LIFE: Duration = Duration::from_millis(presence::VIEW_LIFE_MS);

/// Phone: what a PC says about itself, as this phone has it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentPcPresence {
    /// Someone used the PC in the last 30 s, and it isn't locked.
    pub at_pc: bool,
    pub locked: bool,
    /// The PC shows no notifications.
    pub dnd: bool,
    /// Threads on the PC's screen, newest first; at most 4.
    pub viewing: Vec<String>,
    /// Sessions on the PC's screen, newest first; at most 4, and none
    /// unless this phone may see sessions.
    pub sessions: Vec<String>,
    /// Unix ms, this phone's clock: when it came, plus its `ttl_ms`. It
    /// says nothing after that.
    pub until_ms: i64,
}

/// PC: the thread or session a phone shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentView {
    pub thread: Option<String>,
    pub session: Option<String>,
    /// One of [`presence::pane`].
    pub pane: Option<String>,
    /// When the phone last said so: Unix ms, this PC's clock.
    pub at_ms: i64,
}

/// Phone: what goes to one PC as `agent.viewing`.
#[derive(Debug)]
struct ViewGate {
    /// Tells this gate from one made after it for the same PC, so a sender
    /// left from an earlier one stops.
    serial: u64,
    /// What was handed to the wire last, and when.
    last: Option<(Instant, AgentViewing)>,
    /// The newest view waiting for its turn.
    pending: Option<AgentViewing>,
    /// A sender runs for this gate.
    running: bool,
}

#[derive(Debug, Default)]
struct State {
    /// PC: the presence the platform gave last, and when.
    last: Option<(AgentPresence, Instant)>,
    /// PC: what each phone shows, and when it last said so.
    views: HashMap<DeviceId, (AgentView, Instant)>,
    /// Phone: each PC's presence, and when it stops holding.
    pcs: HashMap<DeviceId, (AgentPcPresence, Instant)>,
    /// Phone: `agent.viewing` to each PC.
    gates: HashMap<DeviceId, ViewGate>,
    /// Phone: the next gate's serial.
    next_serial: u64,
}

/// Presence state, on both sides.
#[derive(Debug, Default)]
pub(crate) struct Presence {
    state: Mutex<State>,
}

impl Presence {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A session with `peer` ended: its view, its presence, and what waited
    /// to go to it go. (Nothing is said: a PC's [`Node::agent_views`] and a
    /// phone's [`Node::agent_presence_of`] leave out a device that's offline.)
    pub(crate) fn session_ended(&self, peer: &DeviceId) {
        let mut state = self.lock();
        state.views.remove(peer);
        state.pcs.remove(peer);
        if let Some(gate) = state.gates.get_mut(peer) {
            gate.pending = None;
            gate.last = None;
        }
    }

    /// `peer` was unpaired.
    pub(crate) fn remove_peer(&self, peer: &DeviceId) {
        self.session_ended(peer);
        self.lock().gates.remove(peer);
    }

    /// PC: `peer` shows nothing any more; whether it showed something.
    pub(crate) fn forget_view(&self, peer: &DeviceId) -> bool {
        self.lock().views.remove(peer).is_some()
    }

    /// PC: `peer` may no longer see sessions; whether the session it showed went.
    pub(crate) fn forget_session_view(&self, peer: &DeviceId) -> bool {
        let mut state = self.lock();
        let shows_session = state.views.get(peer).is_some_and(|(view, _)| view.session.is_some());
        if shows_session {
            state.views.remove(peer);
        }
        shows_session
    }

    /// Phone: `pc`'s presence says nothing any more.
    pub(crate) fn forget_pc(&self, pc: &DeviceId) {
        self.lock().pcs.remove(pc);
    }

    /// PC: `peer` said what it shows; the view to report, unless it changed
    /// nothing (it left a thread it no longer showed).
    fn viewed(&self, peer: DeviceId, viewing: AgentViewing) -> Option<Option<AgentView>> {
        let mut state = self.lock();
        if viewing.visible {
            let view = AgentView {
                thread: viewing.thread,
                session: viewing.session,
                pane: viewing.pane,
                at_ms: crate::now_unix_ms(),
            };
            state.views.insert(peer, (view.clone(), Instant::now()));
            return Some(Some(view));
        }
        let other = state.views.get(&peer).is_some_and(|(view, _)| {
            !viewing.names_nothing() && (view.thread != viewing.thread || view.session != viewing.session)
        });
        if other {
            return None;
        }
        state.views.remove(&peer);
        Some(None)
    }
}

/// Where `new` leaves the view waiting to go: a visible one replaces what
/// waited; leaving the thread or session that waits (or leaving whatever
/// it shows) replaces it too; leaving another is dropped, as what waits
/// replaces it on the PC anyway.
fn merge(pending: Option<AgentViewing>, new: AgentViewing) -> AgentViewing {
    match pending {
        Some(waiting)
            if !new.visible && waiting.visible && !new.names_nothing() && !new.same_target(&waiting) =>
        {
            waiting
        }
        _ => new,
    }
}

// ---- PC side ----

/// Whether `peer` may see this PC's sessions: it gets §3's pushes.
fn may_see_sessions(shared: &Shared, peer: &DeviceId) -> bool {
    shared.we_offer(agent::SESSIONS)
        && shared.toggle_on(peer, TOGGLE)
        && shared.toggle_on(peer, SESSIONS_TOGGLE)
        && shared.peer_offers_cap(peer, agent::FOLLOW)
}

/// `presence` as `peer` gets it: sessions only when it may see them.
fn for_phone(shared: &Shared, peer: &DeviceId, presence: &AgentPresence) -> AgentPresence {
    let mut body = presence.clone();
    if !may_see_sessions(shared, peer) {
        body.sessions.clear();
    }
    body
}

/// PC: sends the PC's presence to every phone that gets agent pushes, and
/// keeps it for the phones that connect while it holds.
pub(crate) fn send(shared: &Arc<Shared>, presence: AgentPresence) -> Result<()> {
    let presence = presence.sanitized();
    shared.agents.presence.lock().last = Some((presence.clone(), Instant::now()));
    for session in shared.agent_recipients() {
        let env = Envelope::new(types::AGENT_PRESENCE, &for_phone(shared, &session.peer, &presence))?;
        let _ = session.send_lossy(env);
    }
    Ok(())
}

/// PC: sends a phone that just connected (or was let in) the presence that
/// still holds, with what's left of its time.
pub(crate) async fn send_last(shared: &Arc<Shared>, session: &Arc<Session>) {
    let Some((presence, given)) = shared.agents.presence.lock().last.clone() else { return };
    let spent = u64::try_from(given.elapsed().as_millis()).unwrap_or(u64::MAX);
    let ttl_ms = presence.ttl_ms.saturating_sub(spent);
    if ttl_ms < presence::MIN_TTL_MS {
        return;
    }
    let body = AgentPresence { ttl_ms, ..for_phone(shared, &session.peer, &presence) };
    if let Ok(env) = Envelope::new(types::AGENT_PRESENCE, &body) {
        let _ = session.send(env).await;
    }
}

/// PC: the answer to a phone's `agent.viewing`. Refused without a prompt:
/// a phone saying what it shows isn't asking for anything.
fn answer_viewing(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) -> Envelope {
    if !shared.we_offer(agent::HOST) {
        return Envelope::error(ErrorCode::Unsupported, "agents are not available here");
    }
    if !shared.toggle_on(&peer, TOGGLE) {
        return Envelope::error(ErrorCode::Denied, "agents are turned off for this device");
    }
    let Some(viewing) = agents::decode::<AgentViewing>(env, AgentViewing::is_valid) else {
        return Envelope::error(ErrorCode::BadMessage, "invalid agent request");
    };
    if viewing.session.is_some() && !may_see_sessions(shared, &peer) {
        return Envelope::error(ErrorCode::Denied, "sessions are turned off for this device");
    }
    if let Some(view) = shared.agents.presence.viewed(peer, viewing.sanitized()) {
        shared.emit(NodeEvent::AgentViewing { device: peer, view });
    }
    Envelope::empty(types::OK)
}

// ---- Both ----

/// Whether `t` is one of §7's messages.
pub(crate) fn is_message(t: &str) -> bool {
    matches!(t, types::AGENT_PRESENCE | types::AGENT_VIEWING)
}

/// One of §7's messages from `peer`: a phone's `agent.viewing` on a PC, a
/// PC's `agent.presence` on a phone.
pub(crate) async fn handle(
    shared: &Arc<Shared>,
    peer: DeviceId,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<()> {
    match env.t.as_str() {
        types::AGENT_VIEWING => rsp.send(answer_viewing(shared, peer, env)).await,
        types::AGENT_PRESENCE => {
            presence_arrived(shared, peer, env);
            Ok(())
        }
        _ => Ok(()),
    }
}

// ---- Phone side ----

/// Phone: a PC's `agent.presence`. One out of its limits is dropped.
fn presence_arrived(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) {
    if !shared.we_offer(agent::REMOTE) {
        return;
    }
    let Some(body) = agents::decode::<AgentPresence>(env, AgentPresence::is_valid) else {
        tracing::debug!(peer = %peer.short(), "dropped an agent.presence out of its limits");
        return;
    };
    let ttl_ms = i64::try_from(body.ttl_ms).unwrap_or(i64::MAX);
    let presence = AgentPcPresence {
        at_pc: body.at_pc && !body.locked,
        locked: body.locked,
        dnd: body.dnd,
        viewing: body.viewing,
        sessions: body.sessions,
        until_ms: crate::now_unix_ms().saturating_add(ttl_ms),
    };
    let ends = Instant::now() + Duration::from_millis(body.ttl_ms);
    shared.agents.presence.lock().pcs.insert(peer, (presence.clone(), ends));
    shared.emit(NodeEvent::AgentPresence { device: peer, presence });
}

/// Phone: what this phone shows goes to `pc` in its turn.
fn viewing(shared: &Arc<Shared>, pc: DeviceId, view: AgentViewing) -> Result<()> {
    if !view.is_valid() {
        return Err(agents::invalid("agent.viewing"));
    }
    if shared.session(&pc).is_none() {
        return Err(Error::Offline);
    }
    let view = view.sanitized();
    let start = {
        let mut state = shared.agents.presence.lock();
        let serial = state.next_serial;
        let gate = state.gates.entry(pc).or_insert_with(|| ViewGate {
            serial,
            last: None,
            pending: None,
            running: false,
        });
        let fresh = gate.serial == serial;
        let next = merge(gate.pending.take(), view);
        // The PC was told this in the last 2 s.
        let told = next.visible
            && gate.last.as_ref().is_some_and(|(at, last)| *last == next && at.elapsed() < VIEW_GAP);
        if !told {
            gate.pending = Some(next);
        }
        let start = gate.pending.is_some() && !gate.running;
        if start {
            gate.running = true;
        }
        let serial = gate.serial;
        if fresh {
            state.next_serial += 1;
        }
        start.then_some(serial)
    };
    if let Some(serial) = start {
        let shared = shared.clone();
        shared.runtime.clone().spawn(async move { run_gate(&shared, pc, serial).await });
    }
    Ok(())
}

/// Phone: sends what waits for `pc`, one at a time, until nothing does.
async fn run_gate(shared: &Arc<Shared>, pc: DeviceId, serial: u64) {
    loop {
        let next = {
            let mut state = shared.agents.presence.lock();
            let Some(gate) = state.gates.get_mut(&pc).filter(|gate| gate.serial == serial) else { return };
            let Some(view) = gate.pending.take() else {
                gate.running = false;
                return;
            };
            let now = Instant::now();
            let wait = match &gate.last {
                Some((at, _)) if view.visible => (*at + VIEW_GAP).saturating_duration_since(now),
                _ => Duration::ZERO,
            };
            if wait.is_zero() {
                gate.last = Some((now, view.clone()));
                Ok(view)
            } else {
                gate.pending = Some(view);
                Err(wait)
            }
        };
        match next {
            Ok(view) => send_viewing(shared, pc, &view).await,
            Err(wait) => tokio::time::sleep(wait).await,
        }
    }
}

async fn send_viewing(shared: &Arc<Shared>, pc: DeviceId, view: &AgentViewing) {
    let Ok(env) = Envelope::new(types::AGENT_VIEWING, view) else { return };
    let sent =
        tokio::time::timeout(VIEWING_WAIT, agents::request_of::<Empty>(shared, pc, env, agent::HOST)).await;
    match sent {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::debug!(pc = %pc.short(), error = %e, "agent.viewing didn't go"),
        Err(_) => tracing::debug!(pc = %pc.short(), "agent.viewing wasn't answered"),
    }
}

/// The node's §7 methods.
impl Node {
    /// PC: what this PC says about itself to its phones (`agent.presence`):
    /// to each phone with the `agents` toggle on, sessions only to those
    /// that may see them. Give it on every change and at least every 20 s
    /// ([`presence::HEARTBEAT_MS`]), with `ttl_ms` [`presence::TTL_MS`]. It
    /// is made to fit: ids that aren't ids left out, 4 of each at most, not
    /// at the PC while locked.
    pub fn agent_presence(&self, presence: AgentPresence) -> Result<()> {
        send(self.shared(), presence)
    }

    /// PC: what each phone with the `agents` toggle on shows now, newest
    /// first: said in the last 60 s ([`presence::VIEW_LIFE_MS`]), over a
    /// session that's up; a session only from a phone that may see them.
    pub fn agent_views(&self) -> Vec<(DeviceId, AgentView)> {
        let shared = self.shared();
        let views: Vec<(DeviceId, AgentView)> = shared
            .agents
            .presence
            .lock()
            .views
            .iter()
            .filter(|(_, (_, said))| said.elapsed() < VIEW_LIFE)
            .map(|(peer, (view, _))| (*peer, view.clone()))
            .collect();
        let mut views: Vec<(DeviceId, AgentView)> = views
            .into_iter()
            .filter(|(peer, view)| {
                shared.session(peer).is_some()
                    && shared.toggle_on(peer, TOGGLE)
                    && (view.session.is_none() || may_see_sessions(shared, peer))
            })
            .collect();
        views.sort_by_key(|(_, view)| std::cmp::Reverse(view.at_ms));
        views
    }

    /// Phone: what PC `pc` last said about itself, while it holds: `None`
    /// once `until_ms` is past, while the link is down, or before it said
    /// anything. No event says it lapsed.
    pub fn agent_presence_of(&self, pc: DeviceId) -> Option<AgentPcPresence> {
        let shared = self.shared();
        shared.session(&pc)?;
        let state = shared.agents.presence.lock();
        let (presence, ends) = state.pcs.get(&pc)?;
        (Instant::now() < *ends && crate::now_unix_ms() < presence.until_ms).then(|| presence.clone())
    }

    /// Phone: this phone shows a thread or session of PC `pc` (`visible`),
    /// or stopped (`visible: false`; with no thread or session: whatever it
    /// showed). Say it when a screen comes to the front or leaves, when the
    /// screen turns on or off, and every 30 s while it shows. Sent in order,
    /// at most one every 2 s: a repeat inside that is dropped, a change
    /// waits for its turn (the newest wins), `visible: false` goes at once.
    /// Never retried; `Offline` while the link is down.
    pub fn agent_viewing(&self, pc: DeviceId, view: AgentViewing) -> Result<()> {
        viewing(self.shared(), pc, view)
    }

    /// Sends an `agent.*` lane message as is, without the checks the methods
    /// above make, so tests can see what a phone drops.
    #[doc(hidden)]
    pub fn debug_agent_push<T: serde::Serialize>(&self, peer: DeviceId, t: &str, body: &T) -> Result<()> {
        let session = self.shared().session(&peer).ok_or(Error::Offline)?;
        session.send_now(Envelope::new(t, body)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0123456789abcdef0123456789abcdef";
    const B: &str = "fedcba9876543210fedcba9876543210";

    fn view(thread: Option<&str>, visible: bool) -> AgentViewing {
        AgentViewing { thread: thread.map(Into::into), session: None, visible, pane: None }
    }

    #[test]
    fn leaving_another_thread_never_undoes_the_newest_view() {
        // Opened B, then the screen of A said it left: B still goes.
        assert_eq!(merge(Some(view(Some(B), true)), view(Some(A), false)), view(Some(B), true));
        // Left B itself, or left whatever it showed: that goes instead.
        assert_eq!(merge(Some(view(Some(B), true)), view(Some(B), false)), view(Some(B), false));
        assert_eq!(merge(Some(view(Some(B), true)), view(None, false)), view(None, false));
        // A newer view replaces what waited, shown or not.
        assert_eq!(merge(Some(view(None, false)), view(Some(A), true)), view(Some(A), true));
        assert_eq!(merge(Some(view(Some(B), true)), view(Some(A), true)), view(Some(A), true));
        assert_eq!(merge(None, view(Some(A), false)), view(Some(A), false));
    }

    #[test]
    fn a_pc_keeps_the_newest_view_and_a_late_leave_of_another_changes_nothing() {
        let peer = DeviceId([7; 32]);
        let presence = Presence::default();
        let shown = presence.viewed(peer, view(Some(B), true)).expect("reported").expect("a view");
        assert_eq!(shown.thread.as_deref(), Some(B));
        assert_eq!(presence.viewed(peer, view(Some(A), false)), None, "left A: B still shows");
        assert_eq!(presence.viewed(peer, view(Some(B), false)), Some(None));
        assert!(!presence.forget_view(&peer));
        presence.viewed(peer, view(Some(A), true));
        assert_eq!(presence.viewed(peer, view(None, false)), Some(None), "left whatever it showed");
        let session = AgentViewing { session: Some("sc0123".into()), ..view(None, true) };
        presence.viewed(peer, session);
        assert!(presence.forget_session_view(&peer));
        presence.viewed(peer, view(Some(A), true));
        assert!(!presence.forget_session_view(&peer), "a thread stays");
        presence.session_ended(&peer);
        assert!(!presence.forget_view(&peer));
    }
}
