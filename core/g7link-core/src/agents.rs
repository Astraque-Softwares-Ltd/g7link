// SPDX-License-Identifier: MPL-2.0
//! Coding-agent threads on a PC, followed, started and answered from a phone
//! (`docs/protocol/agents.md`).
//!
//! PC side: the platform (G7 Link desktop, through the agent host) answers
//! requests in [`Platform::agent_request`](crate::Platform::agent_request)
//! and pushes the catalog, thread events and asks through the node. This
//! module checks the `agents` toggle, the limits and the mode before anything
//! reaches the platform, keeps each phone's watched thread, and paces text
//! deltas. Phone side: it sends requests and turns what arrives into
//! [`NodeEvent`]s.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub use g7link_protocol::messages::{
    AgentAnswer, AgentAsk, AgentAsked, AgentCancel, AgentCatalog, AgentCatalogAgent, AgentCatalogFolder,
    AgentEvent, AgentItem, AgentItems, AgentRead, AgentSend, AgentStart, AgentThread, AgentThreads,
    AgentTool, AgentWatch, agent, is_valid_agent_id,
};
use g7link_protocol::{DeviceId, Envelope, ErrorCode, ProtocolError, messages::types};

use crate::{
    Error, Result,
    events::NodeEvent,
    node::Shared,
    session::{REQUEST_TIMEOUT, Session},
};

/// The per-device toggle on the PC that governs the service (off by default).
pub const TOGGLE: &str = agent::TOGGLE;

/// How long the platform has to answer one request.
const PLATFORM_LIMIT: Duration = Duration::from_secs(15);
/// The least time between two `delta` events to one phone.
const DELTA_INTERVAL: Duration = Duration::from_millis(1000 / agent::MAX_DELTAS_PER_SEC as u64);

/// A phone's request, after the core checked the toggle, the limits and the
/// mode. Answered by [`Platform::agent_request`](crate::Platform::agent_request).
#[derive(Clone, PartialEq, Eq)]
pub enum AgentRequest {
    /// `agent.threads`: newest first, at most [`agent::MAX_THREADS`].
    Threads,
    /// `agent.start`. `mode` is always `"supervised"`.
    Start { agent: String, folder: String, prompt: String, mode: String },
    /// `agent.send`.
    Send { thread: String, text: String },
    /// `agent.cancel`: interrupt the running turn.
    Cancel { thread: String },
    /// `agent.read`: the newest `limit` items, or the ones before `before`.
    Read { thread: String, before: Option<u64>, limit: u32 },
    /// `agent.answer`, for this time only.
    Answer { token: String, allow: bool },
}

/// Never prints a prompt, text or token (protocol v0 §11).
impl std::fmt::Debug for AgentRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Threads => f.write_str("Threads"),
            Self::Start { agent, folder, mode, .. } => f
                .debug_struct("Start")
                .field("agent", agent)
                .field("folder", folder)
                .field("mode", mode)
                .finish_non_exhaustive(),
            Self::Send { thread, .. } => {
                f.debug_struct("Send").field("thread", thread).finish_non_exhaustive()
            }
            Self::Cancel { thread } => f.debug_struct("Cancel").field("thread", thread).finish(),
            Self::Read { thread, before, limit } => f
                .debug_struct("Read")
                .field("thread", thread)
                .field("before", before)
                .field("limit", limit)
                .finish(),
            Self::Answer { allow, .. } => {
                f.debug_struct("Answer").field("allow", allow).finish_non_exhaustive()
            }
        }
    }
}

/// The platform's answer to an [`AgentRequest`]: `Threads` for `Threads`,
/// `Thread` for `Start`, `Items` for `Read`, `Ok` for the rest. Threads carry
/// `from_phone` for the phone that asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentReply {
    Ok,
    Threads(Vec<AgentThread>),
    Thread(AgentThread),
    Items(AgentItems),
}

/// Why the platform refused an [`AgentRequest`]; each is the protocol error
/// of the same name (`docs/protocol/agents.md` §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentFailure {
    /// The host can't be reached, or the agent isn't installed or signed in.
    Unsupported,
    /// The folder was removed, or the host refused.
    Denied,
    BadMessage,
    /// Unknown agent, folder, thread or token, or a token already answered.
    NotFound,
    /// The thread is working, or too many threads are running.
    Busy,
    Internal,
}

impl AgentFailure {
    fn code(self) -> ErrorCode {
        match self {
            Self::Unsupported => ErrorCode::Unsupported,
            Self::Denied => ErrorCode::Denied,
            Self::BadMessage => ErrorCode::BadMessage,
            Self::NotFound => ErrorCode::NotFound,
            Self::Busy => ErrorCode::Busy,
            Self::Internal => ErrorCode::Internal,
        }
    }

    fn reply(self, message: &str) -> Envelope {
        Envelope::error(self.code(), message)
    }
}

/// A phone's delta pacing: when the last one went, and the one waiting for its turn.
#[derive(Debug, Default)]
struct Gate {
    last: Option<Instant>,
    pending: Option<AgentEvent>,
    scheduled: bool,
}

#[derive(Debug, Default)]
struct Inner {
    /// PC: what phones may start here; `None` while the host is gone.
    catalog: Option<AgentCatalog>,
    /// PC: the thread each phone shows, for this session.
    watching: HashMap<DeviceId, String>,
    /// PC: phones that already caused the one-time prompt.
    prompted: HashSet<DeviceId>,
    /// PC: delta pacing per phone.
    gates: HashMap<DeviceId, Gate>,
    /// Phone: the last catalog each PC sent, if it had agents.
    peer_catalogs: HashMap<DeviceId, AgentCatalog>,
}

/// Per-node agents state, on both sides.
#[derive(Debug, Default)]
pub(crate) struct Agents {
    inner: Mutex<Inner>,
}

impl Agents {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A session with `peer` ended: its watch and pending text go, and on a
    /// phone the PC's catalog (it may come back different).
    pub fn session_ended(&self, peer: &DeviceId) {
        let mut inner = self.lock();
        inner.watching.remove(peer);
        inner.gates.remove(peer);
        inner.peer_catalogs.remove(peer);
    }

    /// `peer` was unpaired.
    pub fn remove_peer(&self, peer: &DeviceId) {
        self.session_ended(peer);
        self.lock().prompted.remove(peer);
    }

    /// PC: `peer` now shows `thread` (or none). Text waiting for another thread goes.
    fn watch(&self, peer: DeviceId, thread: Option<String>) {
        let mut inner = self.lock();
        match thread {
            Some(thread) => {
                if inner.watching.get(&peer) != Some(&thread) {
                    inner.gates.remove(&peer);
                }
                inner.watching.insert(peer, thread);
            }
            None => {
                inner.watching.remove(&peer);
                inner.gates.remove(&peer);
            }
        }
    }

    /// The last catalog `peer` sent, if it had agents.
    pub fn peer_catalog(&self, peer: &DeviceId) -> Option<AgentCatalog> {
        self.lock().peer_catalogs.get(peer).cloned()
    }
}

impl Shared {
    fn offers_agents(&self) -> bool {
        self.we_offer(agent::HOST)
    }

    /// Whether this PC sends `peer` anything from the service.
    fn agents_reach(&self, peer: &DeviceId) -> bool {
        self.toggle_on(peer, TOGGLE)
            && self.store.get_peer(peer).ok().flatten().is_some_and(|p| p.caps.contains(agent::REMOTE))
    }

    /// The live sessions this PC sends agent pushes to.
    fn agent_recipients(&self) -> Vec<Arc<Session>> {
        if !self.offers_agents() {
            return Vec::new();
        }
        self.live_sessions().into_iter().filter(|s| self.agents_reach(&s.peer)).collect()
    }
}

fn catalog_env(catalog: Option<&AgentCatalog>) -> Option<Envelope> {
    let empty = AgentCatalog::default();
    Envelope::new(types::AGENT_CATALOG, catalog.unwrap_or(&empty)).ok()
}

/// PC: sends this PC's catalog to `session` when it may have it (on connect,
/// and when the toggle is turned on).
pub(crate) async fn send_catalog(shared: &Arc<Shared>, session: &Arc<Session>) {
    if !shared.offers_agents() || !shared.agents_reach(&session.peer) {
        return;
    }
    let env = catalog_env(shared.agents.lock().catalog.as_ref());
    if let Some(env) = env {
        let _ = session.send(env).await;
    }
}

/// PC: the `agents` toggle for `peer` was turned on or off.
pub(crate) fn toggled(shared: &Arc<Shared>, peer: DeviceId, on: bool) {
    let Some(session) = shared.session(&peer) else {
        if !on {
            shared.agents.session_ended(&peer);
        }
        return;
    };
    let shared = shared.clone();
    shared.runtime.clone().spawn(async move {
        if on {
            send_catalog(&shared, &session).await;
        } else {
            {
                let mut inner = shared.agents.lock();
                inner.watching.remove(&peer);
                inner.gates.remove(&peer);
            }
            // The last word: nothing can be started or followed from here now.
            if shared.offers_agents()
                && shared.store.get_peer(&peer).ok().flatten().is_some_and(|p| p.caps.contains(agent::REMOTE))
                && let Some(env) = catalog_env(None)
            {
                let _ = session.send(env).await;
            }
        }
    });
}

/// PC: replaces this PC's catalog (`None`: the host is gone) and sends it to
/// the phones that may have it when it changed.
pub(crate) fn set_catalog(shared: &Arc<Shared>, catalog: Option<AgentCatalog>) {
    let catalog = catalog.map(AgentCatalog::sanitized).map(|mut c| {
        c.modes = if c.agents.is_empty() { Vec::new() } else { vec![agent::mode::SUPERVISED.to_owned()] };
        c
    });
    let recipients = shared.agent_recipients();
    let mut inner = shared.agents.lock();
    if inner.catalog == catalog {
        return;
    }
    inner.catalog = catalog;
    if let Some(env) = catalog_env(inner.catalog.as_ref()) {
        for session in recipients {
            let _ = session.send_now(env.clone());
        }
    }
}

/// `event` for one phone: `from_phone` says whether that phone started the thread.
fn for_phone(event: &AgentEvent, peer: &DeviceId, started_by: Option<&DeviceId>) -> AgentEvent {
    let mut event = event.clone();
    if let Some(state) = &mut event.state {
        state.from_phone = started_by == Some(peer);
    }
    event
}

fn event_env(event: &AgentEvent) -> Option<Envelope> {
    Envelope::new(types::AGENT_EVENT, event).ok()
}

/// Whether `next` continues `pending`'s text, and the two still fit in one delta.
fn joins(pending: &AgentEvent, next: &AgentEvent) -> bool {
    let (Some(text), Some(next_text)) = (&pending.text, &next.text) else { return false };
    pending.thread == next.thread
        && pending.seq == next.seq
        && pending.item_id == next.item_id
        && pending
            .offset
            .zip(next.offset)
            .is_some_and(|(at, next_at)| u64::from(at) + text.len() as u64 == u64::from(next_at))
        && text.len() + next_text.len() <= agent::MAX_DELTA_BYTES
}

/// Sends `peer`'s waiting delta now (to keep it ahead of what follows) if
/// it belongs to `thread`.
fn flush_for(inner: &mut Inner, session: &Session, thread: &str) {
    if let Some(gate) = inner.gates.get_mut(&session.peer)
        && gate.pending.as_ref().is_some_and(|p| p.thread == thread)
        && let Some(env) = gate.pending.take().as_ref().and_then(event_env)
    {
        gate.last = Some(Instant::now());
        let _ = session.try_send_realtime(env);
    }
}

/// PC: passes a thread event to the phones that get it (`docs/protocol/agents.md`
/// §2.1): `delta` and `item` to the one watching that thread, the rest to all.
pub(crate) fn send_event(
    shared: &Arc<Shared>,
    event: AgentEvent,
    started_by: Option<DeviceId>,
) -> Result<()> {
    let event = event.sanitized().ok_or_else(|| Error::Protocol("invalid agent event".into()))?;
    let recipients = shared.agent_recipients();
    if recipients.is_empty() {
        return Ok(());
    }
    let mut schedule = Vec::new();
    {
        let mut inner = shared.agents.lock();
        for session in &recipients {
            let peer = session.peer;
            let watches = inner.watching.get(&peer).is_some_and(|t| *t == event.thread);
            match event.kind.as_str() {
                agent::event::DELTA => {
                    if !watches {
                        continue;
                    }
                    let now = Instant::now();
                    let gate = inner.gates.entry(peer).or_default();
                    if let Some(pending) = &mut gate.pending {
                        if joins(pending, &event) {
                            if let (Some(text), Some(more)) = (&mut pending.text, &event.text) {
                                text.push_str(more);
                            }
                        } else {
                            let earlier = gate.pending.replace(event.clone());
                            if let Some(env) = earlier.as_ref().and_then(event_env) {
                                let _ = session.try_send_realtime(env);
                            }
                            gate.last = Some(now);
                        }
                    } else if gate.last.is_none_or(|last| now.duration_since(last) >= DELTA_INTERVAL) {
                        gate.last = Some(now);
                        if let Some(env) = event_env(&event) {
                            let _ = session.try_send_realtime(env);
                        }
                    } else {
                        gate.pending = Some(event.clone());
                        if !gate.scheduled {
                            gate.scheduled = true;
                            let last = gate.last.unwrap_or(now);
                            schedule.push((peer, (last + DELTA_INTERVAL).saturating_duration_since(now)));
                        }
                    }
                }
                agent::event::ITEM => {
                    if !watches {
                        continue;
                    }
                    flush_for(&mut inner, session, &event.thread);
                    if let Some(env) = event_env(&event) {
                        let _ = session.send_now(env);
                    }
                }
                _ => {
                    flush_for(&mut inner, session, &event.thread);
                    if let Some(env) = event_env(&for_phone(&event, &peer, started_by.as_ref())) {
                        let _ = session.send_now(env);
                    }
                }
            }
        }
    }
    for (peer, wait) in schedule {
        let shared = shared.clone();
        shared.runtime.clone().spawn(async move {
            tokio::time::sleep(wait).await;
            let session = shared.session(&peer);
            let mut inner = shared.agents.lock();
            let Some(gate) = inner.gates.get_mut(&peer) else { return };
            gate.scheduled = false;
            let Some(pending) = gate.pending.take() else { return };
            gate.last = Some(Instant::now());
            let still_watching = inner.watching.get(&peer).is_some_and(|t| *t == pending.thread);
            if let (Some(session), true, Some(env)) = (session, still_watching, event_env(&pending)) {
                let _ = session.try_send_realtime(env);
            }
        });
    }
    Ok(())
}

/// PC: sends an ask to every phone that gets them.
pub(crate) fn send_ask(shared: &Arc<Shared>, ask: AgentAsk) -> Result<()> {
    let ask = ask.sanitized().ok_or_else(|| Error::Protocol("invalid agent ask".into()))?;
    let env = Envelope::new(types::AGENT_ASK, &ask)?;
    for session in shared.agent_recipients() {
        let _ = session.send_now(env.clone());
    }
    Ok(())
}

/// PC: tells every phone that gets asks that one ended.
pub(crate) fn send_asked(shared: &Arc<Shared>, asked: AgentAsked) -> Result<()> {
    let asked = asked.sanitized().ok_or_else(|| Error::Protocol("invalid agent.asked".into()))?;
    let env = Envelope::new(types::AGENT_ASKED, &asked)?;
    for session in shared.agent_recipients() {
        let _ = session.send_now(env.clone());
    }
    Ok(())
}

/// Text typed on the phone without control characters other than newline and tab.
fn clean_typed(text: String) -> String {
    if text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t')) {
        text.chars().filter(|&c| !c.is_control() || matches!(c, '\n' | '\t')).collect()
    } else {
        text
    }
}

/// A request's body if it decodes and is within its limits.
fn decode<T: serde::de::DeserializeOwned>(env: &Envelope, valid: impl Fn(&T) -> bool) -> Option<T> {
    env.body::<T>().ok().filter(|body| valid(body))
}

/// PC: what to answer a phone's `agent.*` request, checked in the order of
/// `docs/protocol/agents.md` and contract §6.2.
async fn answer(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) -> Envelope {
    if !shared.offers_agents() {
        return Envelope::error(ErrorCode::Unsupported, "agents are not available here");
    }
    if !shared.toggle_on(&peer, TOGGLE) {
        let first_time = shared.agents.lock().prompted.insert(peer);
        if first_time {
            shared.emit(NodeEvent::AgentsRequested { device: peer });
        }
        return Envelope::error(ErrorCode::Denied, "agents are turned off for this device");
    }
    let bad = || Envelope::error(ErrorCode::BadMessage, "invalid agent request");
    let request = match env.t.as_str() {
        types::AGENT_THREADS => AgentRequest::Threads,
        types::AGENT_START => {
            let Some(start) = decode::<AgentStart>(env, AgentStart::is_valid) else { return bad() };
            if start.mode != agent::mode::SUPERVISED {
                return Envelope::error(ErrorCode::Denied, "a phone starts supervised threads only");
            }
            AgentRequest::Start {
                agent: start.agent,
                folder: start.folder,
                prompt: clean_typed(start.prompt),
                mode: start.mode,
            }
        }
        types::AGENT_SEND => {
            let Some(send) = decode::<AgentSend>(env, AgentSend::is_valid) else { return bad() };
            AgentRequest::Send { thread: send.thread, text: clean_typed(send.text) }
        }
        types::AGENT_CANCEL => {
            let Some(cancel) = decode::<AgentCancel>(env, AgentCancel::is_valid) else { return bad() };
            AgentRequest::Cancel { thread: cancel.thread }
        }
        types::AGENT_READ => {
            let Some(read) = decode::<AgentRead>(env, AgentRead::is_valid) else { return bad() };
            let limit = read.limit();
            AgentRequest::Read { thread: read.thread, before: read.before, limit }
        }
        types::AGENT_ANSWER => {
            let Some(answer) = decode::<AgentAnswer>(env, AgentAnswer::is_valid) else { return bad() };
            if answer.scope.as_deref().is_some_and(|s| s != agent::scope::ONCE) {
                return Envelope::error(ErrorCode::Denied, "only this request can be answered from a phone");
            }
            let allow = answer.allow();
            AgentRequest::Answer { token: answer.token, allow }
        }
        types::AGENT_WATCH => {
            let Some(watch) = decode::<AgentWatch>(env, AgentWatch::is_valid) else { return bad() };
            shared.agents.watch(peer, watch.thread);
            return Envelope::empty(types::OK);
        }
        _ => return Envelope::error(ErrorCode::Unsupported, "unknown agent request"),
    };
    let expects = match &request {
        AgentRequest::Threads => types::AGENT_THREADS,
        AgentRequest::Start { .. } => types::AGENT_THREAD,
        AgentRequest::Read { .. } => types::AGENT_ITEMS,
        _ => types::OK,
    };
    let platform = shared.platform.clone();
    let task = tokio::task::spawn_blocking(move || platform.agent_request(&peer, request));
    let reply = match tokio::time::timeout(PLATFORM_LIMIT, task).await {
        Ok(Ok(reply)) => reply,
        Ok(Err(_)) | Err(_) => {
            tracing::warn!(peer = %peer.short(), "the agent host didn't answer");
            Err(AgentFailure::Internal)
        }
    };
    let built = match reply {
        Ok(AgentReply::Ok) if expects == types::OK => Ok(Envelope::empty(types::OK)),
        Ok(AgentReply::Threads(threads)) if expects == types::AGENT_THREADS => {
            Envelope::new(types::AGENT_THREADS, &AgentThreads { threads }.sanitized())
        }
        Ok(AgentReply::Thread(thread)) if expects == types::AGENT_THREAD => match thread.sanitized() {
            Some(thread) => Envelope::new(types::AGENT_THREAD, &thread),
            None => return AgentFailure::Internal.reply("the host's answer was malformed"),
        },
        Ok(AgentReply::Items(items)) if expects == types::AGENT_ITEMS => match items.sanitized() {
            Some(items) => Envelope::new(types::AGENT_ITEMS, &items),
            None => return AgentFailure::Internal.reply("the host's answer was malformed"),
        },
        Ok(_) => return AgentFailure::Internal.reply("the host answered something else"),
        Err(failure) => return failure.reply(failure_message(failure)),
    };
    built.unwrap_or_else(|_| AgentFailure::Internal.reply("failed"))
}

fn failure_message(failure: AgentFailure) -> &'static str {
    match failure {
        AgentFailure::Unsupported => "the agent can't run here now",
        AgentFailure::Denied => "refused",
        AgentFailure::BadMessage => "invalid agent request",
        AgentFailure::NotFound => "not found",
        AgentFailure::Busy => "busy",
        AgentFailure::Internal => "the agent host failed",
    }
}

/// Handles `agent.*` messages: requests on a PC, pushes on a phone.
pub(crate) async fn handle(shared: &Arc<Shared>, session: &Arc<Session>, env: &Envelope) -> Result<bool> {
    let peer = session.peer;
    if env.re.is_some() {
        // A reply that came after its request gave up.
        return Ok(true);
    }
    match env.t.as_str() {
        types::AGENT_THREADS
        | types::AGENT_START
        | types::AGENT_SEND
        | types::AGENT_CANCEL
        | types::AGENT_READ
        | types::AGENT_WATCH
        | types::AGENT_ANSWER => {
            let reply = answer(shared, peer, env).await;
            if env.id.is_some() {
                session.send(reply.reply_to(env.id)).await?;
            }
        }
        types::AGENT_CATALOG if shared.we_offer(agent::REMOTE) => {
            let Ok(catalog) = env.body::<AgentCatalog>() else { return Ok(true) };
            if !catalog.is_valid() {
                return Ok(true);
            }
            let catalog = catalog.sanitized();
            let catalog = (!catalog.agents.is_empty()).then_some(catalog);
            {
                let mut inner = shared.agents.lock();
                match &catalog {
                    Some(c) => inner.peer_catalogs.insert(peer, c.clone()),
                    None => inner.peer_catalogs.remove(&peer),
                };
            }
            shared.emit(NodeEvent::AgentCatalog { device: peer, catalog });
        }
        types::AGENT_EVENT if shared.we_offer(agent::REMOTE) => {
            if let Ok(event) = env.body::<AgentEvent>()
                && event.is_valid()
                && let Some(event) = event.sanitized()
            {
                shared.emit(NodeEvent::AgentEvent { device: peer, event: Box::new(event) });
            }
        }
        types::AGENT_ASK if shared.we_offer(agent::REMOTE) => {
            if let Ok(ask) = env.body::<AgentAsk>()
                && ask.is_valid()
                && let Some(ask) = ask.sanitized()
            {
                shared.emit(NodeEvent::AgentAsk { device: peer, ask });
            }
        }
        types::AGENT_ASKED if shared.we_offer(agent::REMOTE) => {
            if let Ok(asked) = env.body::<AgentAsked>()
                && asked.is_valid()
                && let Some(asked) = asked.sanitized()
            {
                shared.emit(NodeEvent::AgentAsked {
                    device: peer,
                    token: asked.token,
                    thread: asked.thread,
                    outcome: asked.outcome,
                    by_name: asked.by_name,
                });
            }
        }
        types::AGENT_CATALOG | types::AGENT_EVENT | types::AGENT_ASK | types::AGENT_ASKED => {}
        _ => return Ok(false),
    }
    Ok(true)
}

// ---- Phone side ----

/// A PC's error reply as the core's error. `BUSY` is the generic failure
/// here (`Error::Protocol` starting with `BUSY`), not `Denied`: `Denied`
/// means the toggle is off.
fn reply_error(err: ProtocolError) -> Error {
    match err {
        ProtocolError::Remote { code: ErrorCode::Busy, msg } => Error::Protocol(format!("BUSY: {msg}")),
        other => other.into(),
    }
}

async fn request<T: serde::de::DeserializeOwned>(
    shared: &Arc<Shared>,
    peer: DeviceId,
    env: Envelope,
    expected: &'static str,
) -> Result<T> {
    let session = shared.session(&peer).ok_or(Error::Offline)?;
    if !shared.store.get_peer(&peer)?.is_some_and(|p| p.caps.contains(agent::HOST)) {
        return Err(Error::Unsupported);
    }
    let reply = session.request(env, REQUEST_TIMEOUT).await?;
    reply.expect_body::<T>(expected).map_err(reply_error)
}

async fn request_ok(shared: &Arc<Shared>, peer: DeviceId, env: Envelope) -> Result<()> {
    request::<g7link_protocol::messages::Empty>(shared, peer, env, types::OK).await.map(|_| ())
}

fn invalid(what: &str) -> Error {
    Error::Protocol(format!("invalid {what}"))
}

pub(crate) async fn threads(shared: &Arc<Shared>, peer: DeviceId) -> Result<Vec<AgentThread>> {
    let env = Envelope::empty(types::AGENT_THREADS);
    let reply: AgentThreads = request(shared, peer, env, types::AGENT_THREADS).await?;
    if !reply.is_valid() {
        return Err(invalid("agent.threads reply"));
    }
    Ok(reply.sanitized().threads)
}

pub(crate) async fn start(shared: &Arc<Shared>, peer: DeviceId, start: AgentStart) -> Result<AgentThread> {
    if !start.is_valid() {
        return Err(invalid("agent.start"));
    }
    let env = Envelope::new(types::AGENT_START, &start)?;
    let thread: AgentThread = request(shared, peer, env, types::AGENT_THREAD).await?;
    if !thread.is_valid() {
        return Err(invalid("agent.thread reply"));
    }
    thread.sanitized().ok_or_else(|| invalid("agent.thread reply"))
}

pub(crate) async fn send(shared: &Arc<Shared>, peer: DeviceId, thread: String, text: String) -> Result<()> {
    let body = AgentSend { thread, text };
    if !body.is_valid() {
        return Err(invalid("agent.send"));
    }
    request_ok(shared, peer, Envelope::new(types::AGENT_SEND, &body)?).await
}

pub(crate) async fn cancel(shared: &Arc<Shared>, peer: DeviceId, thread: String) -> Result<()> {
    let body = AgentCancel { thread };
    if !body.is_valid() {
        return Err(invalid("agent.cancel"));
    }
    request_ok(shared, peer, Envelope::new(types::AGENT_CANCEL, &body)?).await
}

pub(crate) async fn read(
    shared: &Arc<Shared>,
    peer: DeviceId,
    thread: String,
    before: Option<u64>,
    limit: u32,
) -> Result<AgentItems> {
    let body = AgentRead { thread, before, limit: Some(limit.clamp(1, agent::MAX_ITEMS as u32)) };
    if !body.is_valid() {
        return Err(invalid("agent.read"));
    }
    let items: AgentItems =
        request(shared, peer, Envelope::new(types::AGENT_READ, &body)?, types::AGENT_ITEMS).await?;
    if !items.is_valid() {
        return Err(invalid("agent.items reply"));
    }
    items.sanitized().ok_or_else(|| invalid("agent.items reply"))
}

pub(crate) async fn watch(shared: &Arc<Shared>, peer: DeviceId, thread: Option<String>) -> Result<()> {
    let body = AgentWatch { thread };
    if !body.is_valid() {
        return Err(invalid("agent.watch"));
    }
    request_ok(shared, peer, Envelope::new(types::AGENT_WATCH, &body)?).await
}

pub(crate) async fn answer_ask(
    shared: &Arc<Shared>,
    peer: DeviceId,
    token: String,
    allow: bool,
) -> Result<()> {
    let decision = if allow { agent::decision::ALLOW } else { agent::decision::DENY };
    let body = AgentAnswer { token, decision: decision.into(), scope: None };
    if !body.is_valid() {
        return Err(invalid("agent.answer"));
    }
    request_ok(shared, peer, Envelope::new(types::AGENT_ANSWER, &body)?).await
}

/// Sends `body` as `t` to `peer` without checking it here, for tests of
/// what the PC refuses. A reply other than an error is `Ok`.
pub(crate) async fn raw<T: serde::Serialize>(
    shared: &Arc<Shared>,
    peer: DeviceId,
    t: &str,
    body: &T,
) -> Result<()> {
    let session = shared.session(&peer).ok_or(Error::Offline)?;
    let reply = session.request(Envelope::new(t, body)?, REQUEST_TIMEOUT).await?;
    if reply.t == types::ERROR {
        reply.expect(types::OK).map_err(reply_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(offset: u32, text: &str) -> AgentEvent {
        AgentEvent::delta("abc".into(), 3, "m:0".into(), offset, text.into())
    }

    #[test]
    fn deltas_join_only_when_they_continue_each_other() {
        assert!(joins(&delta(0, "Hello"), &delta(5, " world")));
        assert!(!joins(&delta(0, "Hello"), &delta(6, "world")), "a gap");
        let mut other_item = delta(5, "x");
        other_item.item_id = Some("m:1".into());
        assert!(!joins(&delta(0, "Hello"), &other_item));
        let big = "y".repeat(agent::MAX_DELTA_BYTES - 2);
        assert!(!joins(&delta(0, &big), &delta(big.len() as u32, "abc")), "too long joined");
    }

    #[test]
    fn typed_text_keeps_lines_and_loses_other_controls() {
        assert_eq!(clean_typed("a\nb\tc\u{0}\u{1b}d".into()), "a\nb\tcd");
    }

    #[test]
    fn a_busy_pc_is_a_generic_failure_not_a_refusal() {
        let busy = reply_error(ProtocolError::Remote { code: ErrorCode::Busy, msg: "working".into() });
        assert!(matches!(busy, Error::Protocol(ref m) if m.starts_with("BUSY")), "{busy:?}");
        let denied = reply_error(ProtocolError::Remote { code: ErrorCode::Denied, msg: String::new() });
        assert!(matches!(denied, Error::Denied));
    }

    #[test]
    fn requests_never_print_prompts_or_tokens() {
        let start = AgentRequest::Start {
            agent: "claude".into(),
            folder: "f1".into(),
            prompt: "secret plan".into(),
            mode: "supervised".into(),
        };
        let answer = AgentRequest::Answer { token: "0123abcd".into(), allow: true };
        let shown = format!("{start:?} {answer:?}");
        assert!(!shown.contains("secret") && !shown.contains("0123abcd"), "{shown}");
    }
}
