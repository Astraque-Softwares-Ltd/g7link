// SPDX-License-Identifier: GPL-3.0-or-later
//! Who is at this PC, for agents (phase 5 §V4.1): three facts and what the
//! phones show.
//!
//! - **at the PC**: not locked, and someone used the keyboard or mouse in the
//!   last 30 s. An idle time the desktop can't give is never "at the PC".
//! - **locked**: logind's `LockedHint`.
//! - **Do Not Disturb**: the desktop shows no notification banners.
//!
//! A task of its own reads them every 3 s with what the phones show (the
//! core keeps their `agent.viewing`) and tells the host (`ui.presence`) when
//! anything changed, at most once a second, and every 20 s regardless while
//! connected. Phones with agents on hear the host's `ui.presence` event
//! without the phones' views (`agent.presence`), and again every 20 s.
//! Nothing here sends a message, starts a thread or answers anything:
//! presence only holds and releases notifications and names the phone in the
//! thread's header.
//!
//! Every unknown fails toward notifying: no idle time, a stale view, a phone
//! that went offline or a host that can't be reached all read as "not here".

use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard, OnceLock},
    time::{Duration, Instant},
};

use g7_agents::api::{
    self, MAX_DEVICE_NAME, MAX_PRESENCE_VIEWING, MAX_REMOTE_VIEWS, Pane, PresenceEvent, RemoteView,
    UiPresenceParams, UiViewingParams,
};
use g7link_core::{AgentPresence, AgentView, DeviceId, Node, presence as wire};
use tokio::sync::Notify;

use crate::core_host;

/// Input this recent, and not locked, is someone at the PC. Reply on the
/// PC's "in use" (5 s) is a different constant, never shared (§V2).
pub(crate) const AT_PC_IDLE_MS: u64 = 30_000;
/// How often the three facts and the phones' views are read.
const TICK: Duration = Duration::from_secs(3);
/// Sent again this often while connected, whether or not anything changed.
const HEARTBEAT: Duration = Duration::from_millis(wire::HEARTBEAT_MS);
/// A change is sent at most this often.
const MIN_GAP: Duration = Duration::from_secs(1);
/// A phone's view counts this long after it last said so.
const VIEW_LIFE_MS: i64 = wire::VIEW_LIFE_MS as i64;

// ---- The three facts (pure) ----

/// Who is at this PC.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Here {
    pub at_pc: bool,
    pub locked: bool,
    pub dnd: bool,
}

/// Where the three facts come from: this PC, or a test.
pub(crate) trait Sources {
    /// The session is locked; false where logind can't say.
    fn locked(&self) -> bool;
    /// Milliseconds since the last keyboard or mouse input; `None` where the
    /// desktop can't say.
    fn idle_ms(&self) -> Option<u64>;
    /// The desktop shows notification banners (not Do Not Disturb).
    fn banners(&self) -> bool;
}

impl Here {
    pub(crate) fn read(sources: &impl Sources) -> Self {
        let locked = sources.locked();
        // A locked PC has nobody at it, whatever its idle time says.
        let at_pc = !locked && sources.idle_ms().is_some_and(|ms| ms <= AT_PC_IDLE_MS);
        Here { at_pc, locked, dnd: !sources.banners() }
    }
}

/// This PC: logind, the desktop's idle monitor and its notification server.
/// Each may block on D-Bus or a helper process: read off the runtime.
struct ThisPc;

impl Sources for ThisPc {
    fn locked(&self) -> bool {
        crate::sys::session_locked()
    }

    fn idle_ms(&self) -> Option<u64> {
        crate::sys::input::idle_ms()
    }

    fn banners(&self) -> bool {
        crate::sys::toast::enabled()
    }
}

// ---- What the phones show (pure) ----

/// A pane's word; one this version doesn't know is absent.
pub(crate) fn pane(word: &str) -> Option<Pane> {
    match word {
        wire::pane::THREAD => Some(Pane::Thread),
        wire::pane::CHANGES => Some(Pane::Changes),
        wire::pane::PLAN => Some(Pane::Plan),
        wire::pane::REQUESTS => Some(Pane::Requests),
        _ => None,
    }
}

/// The phones' views as the host takes them (§V4.2): said in the last 60 s
/// (`now_ms`, this PC's clock), naming a thread or a session by the host's
/// own id, newest first, one per phone, at most [`MAX_REMOTE_VIEWS`].
/// `phone` gives a phone's name and whether it's in Do Not Disturb.
pub(crate) fn remote_views(
    mut views: Vec<(DeviceId, AgentView)>,
    now_ms: i64,
    phone: impl Fn(DeviceId) -> (String, bool),
) -> Vec<RemoteView> {
    views.retain(|(_, v)| now_ms.saturating_sub(v.at_ms) < VIEW_LIFE_MS);
    views.sort_by_key(|(_, v)| std::cmp::Reverse(v.at_ms));
    let mut out: Vec<RemoteView> = Vec::with_capacity(views.len().min(MAX_REMOTE_VIEWS));
    for (device, view) in views {
        if out.len() == MAX_REMOTE_VIEWS {
            break;
        }
        let device_id = device.to_string();
        let thread_id = view.thread.filter(|id| api::is_valid_thread_id(id));
        let session_id = view.session.filter(|id| api::is_valid_session_id(id));
        if (thread_id.is_none() && session_id.is_none()) || out.iter().any(|v| v.device_id == device_id) {
            continue;
        }
        let (name, dnd) = phone(device);
        out.push(RemoteView {
            device_id,
            device_name: api::cut(&name, MAX_DEVICE_NAME).to_owned(),
            thread_id,
            session_id,
            pane: view.pane.as_deref().and_then(pane),
            dnd,
        });
    }
    out
}

/// The phone showing this thread (or session) now, by name, for the
/// thread's header: "open on Pixel ya Amani".
pub(crate) fn open_on<'a>(
    views: &'a [RemoteView],
    thread: Option<&str>,
    session: Option<&str>,
) -> Option<&'a str> {
    views
        .iter()
        .find(|v| {
            thread.is_some_and(|t| v.thread_id.as_deref() == Some(t))
                || session.is_some_and(|s| v.session_id.as_deref() == Some(s))
        })
        .map(|v| v.device_name.as_str())
}

// ---- What phones hear (pure) ----

/// `agent.presence` (§V4.3) from the host's event: the three facts and what
/// this PC shows, never a phone's view, an idle time, a window or an app.
pub(crate) fn told_heard(event: &PresenceEvent) -> AgentPresence {
    let first = |ids: &[String]| ids.iter().take(MAX_PRESENCE_VIEWING).cloned().collect();
    AgentPresence {
        at_pc: event.at_pc,
        locked: event.locked,
        dnd: event.dnd,
        viewing: first(&event.viewing),
        sessions: first(&event.viewing_sessions),
        ttl_ms: wire::TTL_MS,
    }
}

/// Before the host's first event, or from a host without presence: this
/// PC's facts and what the Agents page shows.
pub(crate) fn told_here(here: Here, page: &UiViewingParams) -> AgentPresence {
    AgentPresence {
        at_pc: here.at_pc,
        locked: here.locked,
        dnd: here.dnd,
        viewing: page.thread_id.iter().cloned().collect(),
        sessions: page.session_id.iter().cloned().collect(),
        ttl_ms: wire::TTL_MS,
    }
}

// ---- When to send (pure) ----

/// Sends a value when it changed, at most once per [`MIN_GAP`], and again
/// every [`HEARTBEAT`] when it didn't.
#[derive(Debug)]
struct Pacer<T> {
    sent: Option<(T, Instant)>,
    /// A change came too soon after the last send.
    waiting: bool,
}

impl<T> Default for Pacer<T> {
    fn default() -> Self {
        Self { sent: None, waiting: false }
    }
}

impl<T: Clone + PartialEq> Pacer<T> {
    /// `value` to send now, or `None`: it waits for the gap, or it's the
    /// same and not due.
    fn offer(&mut self, value: &T, now: Instant) -> Option<T> {
        let due = match &self.sent {
            None => true,
            Some((sent, at)) => {
                let since = now.saturating_duration_since(*at);
                if sent == value { since >= HEARTBEAT } else { since >= MIN_GAP }
            }
        };
        if due {
            self.sent = Some((value.clone(), now));
            self.waiting = false;
            return Some(value.clone());
        }
        self.waiting = self.sent.as_ref().is_some_and(|(sent, _)| sent != value);
        None
    }

    /// When the next send may be due.
    fn next(&self) -> Option<Instant> {
        let (_, at) = self.sent.as_ref()?;
        Some(*at + if self.waiting { MIN_GAP } else { HEARTBEAT })
    }
}

/// What one reading sends.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Sends {
    /// `ui.presence` to the host.
    pub host: Option<UiPresenceParams>,
    /// `agent.presence` to the phones.
    pub phones: Option<AgentPresence>,
    /// The phones' views changed: the thread's header reads them again.
    pub header: bool,
}

/// The host connection presence goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Conn {
    id: u64,
    /// The host takes `ui.presence` and sends its event (`cap::PRESENCE`).
    presence: bool,
}

/// Presence's state and rules, without the clock, the sources or the node:
/// tests drive it with their own.
#[derive(Debug, Default)]
pub(crate) struct Engine {
    /// The phones' views as last read.
    shown: Vec<RemoteView>,
    conn: Option<Conn>,
    host: Pacer<UiPresenceParams>,
    phones: Pacer<AgentPresence>,
    /// The host's newest event, as phones hear it.
    heard: Option<AgentPresence>,
}

impl Engine {
    /// A connection to the host is up: it and the phones hear at once.
    pub(crate) fn connected(&mut self, id: u64, presence: bool) {
        self.conn = Some(Conn { id, presence });
        self.host = Pacer::default();
        self.phones = Pacer::default();
        self.heard = None;
    }

    /// The connection `id` closed (a later one may already be up).
    pub(crate) fn disconnected(&mut self, id: u64) {
        if self.conn.is_some_and(|c| c.id == id) {
            self.conn = None;
            self.heard = None;
        }
    }

    /// One reading: the three facts, the phones' views ([`remote_views`])
    /// and what the Agents page shows.
    pub(crate) fn tick(
        &mut self,
        now: Instant,
        here: Here,
        views: Vec<RemoteView>,
        page: &UiViewingParams,
    ) -> Sends {
        let header = views != self.shown;
        let mut sends = Sends { header, ..Sends::default() };
        let Some(conn) = self.conn else {
            self.shown = views;
            return sends;
        };
        if conn.presence {
            let params = UiPresenceParams {
                at_pc: here.at_pc,
                locked: here.locked,
                dnd: here.dnd,
                remote: views.clone(),
            };
            sends.host = self.host.offer(&params, now);
        }
        self.shown = views;
        // Until the host's first event, and always from a host without
        // presence, phones hear this PC's facts and what the page shows.
        let told = self.heard.clone().unwrap_or_else(|| told_here(here, page));
        sends.phones = self.phones.offer(&told, now);
        sends
    }

    /// The host's event: what phones hear, if it's due now.
    pub(crate) fn heard(&mut self, event: &PresenceEvent, now: Instant) -> Option<AgentPresence> {
        let told = told_heard(event);
        self.heard = Some(told.clone());
        self.conn?;
        self.phones.offer(&told, now)
    }

    /// When to read again at the latest; `None` while there's no connection.
    pub(crate) fn next(&self, now: Instant) -> Option<Instant> {
        self.conn.map(|_| {
            [self.host.next(), self.phones.next()].into_iter().flatten().fold(now + TICK, Instant::min)
        })
    }

    pub(crate) fn shown(&self) -> &[RemoteView] {
        &self.shown
    }
}

// ---- The task ----

struct Shared {
    engine: Mutex<Engine>,
    wake: Notify,
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| Shared { engine: Mutex::default(), wake: Notify::new() })
}

fn engine() -> MutexGuard<'static, Engine> {
    shared().engine.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts the task; it waits for the node, then for a host connection.
pub(crate) fn start() {
    core_host::spawn(run());
}

/// A host connection is up (`presence`: it offers `cap::PRESENCE`).
pub(crate) fn connected(conn: u64, presence: bool) {
    engine().connected(conn, presence);
    shared().wake.notify_one();
}

pub(crate) fn disconnected(conn: u64) {
    engine().disconnected(conn);
}

/// Something presence reads changed (a phone said what it shows): read now.
pub(crate) fn wake() {
    shared().wake.notify_one();
}

/// The host's `ui.presence` event: what phones hear now, if it's due.
pub(crate) fn heard(event: &PresenceEvent) -> Option<AgentPresence> {
    engine().heard(event, Instant::now())
}

/// The phones' views as last read, for the header ([`open_on`]).
pub(crate) fn shown() -> Vec<RemoteView> {
    engine().shown().to_vec()
}

/// Phones with agents on hear presence (`agent.presence`): the core picks
/// them, leaves sessions out for a phone that may not see them, gives a
/// phone that connects later what's left of it, and never sends it over
/// Bluetooth.
pub(crate) fn tell_phones(node: &Node, told: AgentPresence) {
    if let Err(e) = node.agent_presence(told) {
        tracing::debug!(error = %e, "presence didn't reach the phones");
    }
}

/// What the phones show now, from the core, with their names and Do Not
/// Disturb.
fn phone_views(node: &Node) -> Vec<RemoteView> {
    let views = node.agent_views();
    if views.is_empty() {
        return Vec::new();
    }
    let names: HashMap<DeviceId, String> =
        node.paired_devices().unwrap_or_default().into_iter().map(|d| (d.id, d.info.name)).collect();
    let now_ms = i64::try_from(super::now_ms()).unwrap_or(i64::MAX);
    remote_views(views, now_ms, |id| {
        let name = names.get(&id).cloned().unwrap_or_else(|| id.short());
        (name, node.phone_toggles(id).is_some_and(|t| t.dnd))
    })
}

async fn run() {
    let Some(node) = core_host::wait_for_node().await else { return };
    let shared = shared();
    loop {
        let next = engine().next(Instant::now());
        match next {
            Some(at) => {
                let _ = tokio::time::timeout_at(at.into(), shared.wake.notified()).await;
            }
            None => shared.wake.notified().await,
        }
        let (client, page) = {
            let s = super::lock();
            (s.client.clone(), super::wanted_viewing(&s))
        };
        let Some(client) = client else { continue };
        let here = tokio::task::spawn_blocking(|| Here::read(&ThisPc)).await.unwrap_or_default();
        let views = phone_views(&node);
        let sends = engine().tick(Instant::now(), here, views, &page);
        if sends.header {
            super::changed();
        }
        if let Some(params) = sends.host
            && let Err(e) = client.ui_presence(params).await
        {
            tracing::debug!(error = %e, "the host didn't take presence");
        }
        if let Some(told) = sends.phones {
            tell_phones(&node, told);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    struct Fake {
        locked: bool,
        idle_ms: Option<u64>,
        banners: bool,
        asked_idle: Cell<bool>,
    }

    impl Fake {
        fn new(locked: bool, idle_ms: Option<u64>, banners: bool) -> Self {
            Fake { locked, idle_ms, banners, asked_idle: Cell::new(false) }
        }
    }

    impl Sources for Fake {
        fn locked(&self) -> bool {
            self.locked
        }
        fn idle_ms(&self) -> Option<u64> {
            self.asked_idle.set(true);
            self.idle_ms
        }
        fn banners(&self) -> bool {
            self.banners
        }
    }

    fn phone(n: u8) -> DeviceId {
        DeviceId([n; 32])
    }

    fn thread(c: char) -> String {
        c.to_string().repeat(32)
    }

    fn session(c: char) -> String {
        format!("sc{}", thread(c))
    }

    fn seen(thread_id: &str, at_ms: i64) -> AgentView {
        AgentView { thread: Some(thread_id.into()), session: None, pane: None, at_ms }
    }

    /// Phone `n` is "P<n>", in Do Not Disturb when `n` is odd.
    fn named(id: DeviceId) -> (String, bool) {
        (format!("P{}", id.0[0]), id.0[0] % 2 == 1)
    }

    fn remote(device: u8, thread_id: &str) -> RemoteView {
        let (device_name, dnd) = named(phone(device));
        RemoteView {
            device_id: phone(device).to_string(),
            device_name,
            thread_id: Some(thread_id.into()),
            session_id: None,
            pane: None,
            dnd,
        }
    }

    const AT: Here = Here { at_pc: true, locked: false, dnd: false };
    const AWAY: Here = Here { at_pc: false, locked: false, dnd: false };
    const NONE: UiViewingParams = UiViewingParams { thread_id: None, session_id: None };

    #[test]
    fn at_the_pc_needs_recent_input_an_unlocked_session_and_a_known_idle_time() {
        assert_eq!(Here::read(&Fake::new(false, Some(1_000), true)), AT);
        assert_eq!(Here::read(&Fake::new(false, Some(AT_PC_IDLE_MS), true)), AT);
        assert_eq!(Here::read(&Fake::new(false, Some(AT_PC_IDLE_MS + 1), true)), AWAY);
        // The desktop can't say how long it's been idle: nobody is there.
        assert_eq!(Here::read(&Fake::new(false, None, true)), AWAY);
        // Locked: nobody, whatever the idle time, and it isn't even asked.
        let locked = Fake::new(true, Some(0), true);
        assert_eq!(Here::read(&locked), Here { at_pc: false, locked: true, dnd: false });
        assert!(!locked.asked_idle.get());
        // No banners: Do Not Disturb.
        assert_eq!(Here::read(&Fake::new(false, Some(0), false)), Here { dnd: true, ..AT });
    }

    #[test]
    fn a_change_is_sent_at_most_once_a_second() {
        let t0 = Instant::now();
        let mut e = Engine::default();
        assert_eq!(e.tick(t0, AT, Vec::new(), &NONE), Sends::default(), "nothing without a host");
        assert_eq!(e.next(t0), None, "and no reading until one is up");

        e.connected(1, true);
        let first = e.tick(t0, AT, Vec::new(), &NONE);
        assert_eq!(
            first.host,
            Some(UiPresenceParams { at_pc: true, ..Default::default() }),
            "a new host hears at once"
        );
        assert_eq!(first.phones, Some(AgentPresence { at_pc: true, ttl_ms: 60_000, ..Default::default() }));

        // Away half a second later: it waits for the second to pass.
        let soon = t0 + Duration::from_millis(500);
        assert_eq!(e.tick(soon, AWAY, Vec::new(), &NONE).host, None);
        assert_eq!(e.next(soon), Some(t0 + MIN_GAP), "read again when it may go");
        let later = t0 + MIN_GAP;
        let sent = e.tick(later, AWAY, Vec::new(), &NONE).host.expect("sent once the second passed");
        assert!(!sent.at_pc);
        // Back and away again within the second: only the last value counts.
        assert_eq!(e.tick(later + Duration::from_millis(100), AT, Vec::new(), &NONE).host, None);
        assert_eq!(e.tick(later + Duration::from_millis(200), AWAY, Vec::new(), &NONE).host, None);
        assert_eq!(e.tick(later + MIN_GAP, AWAY, Vec::new(), &NONE).host, None, "unchanged: nothing to send");
        assert_eq!(e.next(later + MIN_GAP), Some(later + MIN_GAP + TICK));
    }

    #[test]
    fn the_host_and_phones_hear_it_every_20_seconds_while_connected() {
        let t0 = Instant::now();
        let mut e = Engine::default();
        e.connected(1, true);
        assert!(e.tick(t0, AT, Vec::new(), &NONE).host.is_some());
        let quiet = e.tick(t0 + TICK, AT, Vec::new(), &NONE);
        assert_eq!((quiet.host, quiet.phones), (None, None));
        assert_eq!(e.next(t0 + TICK), Some(t0 + TICK + TICK), "read every 3 s");
        assert_eq!(e.next(t0 + HEARTBEAT - Duration::from_secs(1)), Some(t0 + HEARTBEAT));
        let beat = e.tick(t0 + HEARTBEAT, AT, Vec::new(), &NONE);
        assert!(beat.host.is_some() && beat.phones.is_some(), "the same value again after 20 s");

        // The connection closes: nothing more until the next one, which hears at once.
        e.disconnected(1);
        assert_eq!(e.tick(t0 + HEARTBEAT * 2, AT, Vec::new(), &NONE), Sends::default());
        e.connected(2, true);
        assert!(e.tick(t0 + HEARTBEAT * 2, AT, Vec::new(), &NONE).host.is_some());
        // An older connection closing late doesn't stop the new one.
        e.disconnected(1);
        assert!(e.next(t0 + HEARTBEAT * 2).is_some());
    }

    #[test]
    fn phones_hear_the_hosts_event_without_the_phones_views() {
        let t0 = Instant::now();
        let page = UiViewingParams { thread_id: Some(thread('a')), session_id: None };
        let mut e = Engine::default();
        assert_eq!(e.heard(&PresenceEvent::default(), t0), None, "nothing without a connection");
        e.connected(1, true);
        // Before the host's first event: this PC's facts and the page's thread.
        let first = e.tick(t0, AT, Vec::new(), &page).phones.unwrap();
        assert_eq!(first.viewing, vec![thread('a')]);

        let event = PresenceEvent {
            at_pc: true,
            locked: false,
            dnd: true,
            viewing: "abcdef".chars().map(thread).collect(),
            viewing_sessions: vec![session('1')],
            remote: vec![remote(2, &thread('b'))],
        };
        let told = e.heard(&event, t0 + MIN_GAP).expect("an event goes to phones");
        assert_eq!(told.viewing.len(), MAX_PRESENCE_VIEWING);
        assert_eq!(told.sessions, vec![session('1')]);
        assert!(told.dnd);
        assert_eq!(told.ttl_ms, 60_000);
        assert!(!serde_json::to_string(&told).unwrap().contains("P2"), "never a phone's view");
        // The heartbeat repeats the event, not this PC's own reading.
        let beat = e.tick(t0 + MIN_GAP + HEARTBEAT, AWAY, Vec::new(), &page).phones.unwrap();
        assert_eq!(beat, told);
    }

    #[test]
    fn an_older_host_hears_nothing_but_phones_still_get_this_pcs_facts() {
        let t0 = Instant::now();
        let page = UiViewingParams { thread_id: None, session_id: Some(session('2')) };
        let mut e = Engine::default();
        e.connected(1, false);
        let sends = e.tick(t0, Here { at_pc: false, locked: true, dnd: false }, Vec::new(), &page);
        assert_eq!(sends.host, None);
        assert_eq!(
            sends.phones,
            Some(AgentPresence {
                locked: true,
                sessions: vec![session('2')],
                ttl_ms: 60_000,
                ..Default::default()
            })
        );
    }

    #[test]
    fn a_phones_view_counts_for_60_seconds() {
        let now = 1_000_000;
        let views =
            vec![(phone(1), seen(&thread('a'), now - 59_999)), (phone(2), seen(&thread('b'), now - 60_000))];
        assert_eq!(remote_views(views, now, named), vec![remote(1, &thread('a'))]);

        // The host hears it go, and the header with it.
        let t0 = Instant::now();
        let mut e = Engine::default();
        e.connected(1, true);
        let shown = e.tick(t0, AT, vec![remote(1, &thread('a'))], &NONE);
        assert!(shown.header);
        assert_eq!(shown.host.unwrap().remote, vec![remote(1, &thread('a'))]);
        assert_eq!(open_on(e.shown(), Some(&thread('a')), None), Some("P1"));
        let gone = e.tick(t0 + TICK, AT, Vec::new(), &NONE);
        assert!(gone.header);
        assert_eq!(gone.host.unwrap().remote, Vec::new());
        assert_eq!(open_on(e.shown(), Some(&thread('a')), None), None);
        assert!(!e.tick(t0 + TICK * 2, AT, Vec::new(), &NONE).header);
    }

    #[test]
    fn the_host_gets_only_ids_it_issued_and_names_that_fit() {
        let now = 1_000_000;
        let view = |thread: Option<&str>, session: Option<&str>| AgentView {
            thread: thread.map(Into::into),
            session: session.map(Into::into),
            pane: Some("changes".into()),
            at_ms: now,
        };
        // Not the host's ids: left out, so the host never refuses the whole call.
        for (n, (t, s)) in
            [(Some("abc"), None), (Some(&*"A".repeat(32)), None), (None, Some(&*thread('a'))), (None, None)]
                .into_iter()
                .enumerate()
        {
            let n = u8::try_from(n).unwrap();
            assert_eq!(remote_views(vec![(phone(n), view(t, s))], now, named), Vec::new(), "{t:?} {s:?}");
        }
        let long = |_| ("é".repeat(40), false);
        let views = remote_views(vec![(phone(1), view(None, Some(&session('c'))))], now, long);
        assert_eq!(views[0].session_id, Some(session('c')));
        assert_eq!(views[0].pane, Some(Pane::Changes));
        assert!(
            views[0].device_name.len() <= MAX_DEVICE_NAME && views[0].device_name.chars().all(|c| c == 'é')
        );
    }

    #[test]
    fn at_most_eight_phones_the_newest_first() {
        let now = 1_000_000;
        let views = (0..10).map(|n| (phone(n), seen(&thread('a'), now - 1_000 + i64::from(n)))).collect();
        let views = remote_views(views, now, named);
        assert_eq!(views.len(), MAX_REMOTE_VIEWS);
        assert_eq!(views[0].device_name, "P9");
        assert!(views[0].dnd, "each with its own Do Not Disturb");
        assert_eq!(views[7].device_name, "P2");
        assert!(!views[7].dnd);
    }

    #[test]
    fn the_header_names_the_phone_showing_this_thread() {
        let mut tab = remote(2, &thread('c'));
        tab.thread_id = None;
        tab.session_id = Some(session('c'));
        let views = vec![remote(3, &thread('a')), remote(1, &thread('a')), tab];
        assert_eq!(open_on(&views, Some(&thread('a')), None), Some("P3"), "the newest");
        assert_eq!(open_on(&views, Some(&thread('b')), None), None);
        assert_eq!(open_on(&views, None, Some(&session('c'))), Some("P2"));
        assert_eq!(open_on(&views, Some(&session('c')), None), None, "a thread is not a session");
        assert_eq!(open_on(&views, None, None), None);
    }

    #[test]
    fn pane_words_this_version_doesnt_know_are_absent() {
        assert_eq!(pane("changes"), Some(Pane::Changes));
        assert_eq!(pane("requests"), Some(Pane::Requests));
        assert_eq!(pane("beside"), None);
        assert_eq!(pane(""), None);
    }
}
