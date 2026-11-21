// SPDX-License-Identifier: MPL-2.0
//! History and undo on a PC's agent threads, from a phone
//! (`docs/protocol/agents.md` §6): turns and what each changed, search,
//! marks and renames, forks, rewinding the conversation and its undo, and
//! sessions found on the PC brought in as threads.
//!
//! PC side: the core checks the toggles and every limit before anything
//! reaches the platform, refuses a phone's rewind that names files, follows
//! the host's pages of a patch up to 1 MiB, and makes every answer fit the
//! phone (no path, no restore plan's file list). Phone side: it sends the
//! requests, keeps what passes in the saved threads (`agent_cache`), holds
//! messages written while the PC can't be reached and sends them, once and
//! in order, when it's back.

use std::{sync::Arc, time::Instant};

pub use g7link_protocol::messages::{
    AgentCut, AgentDiffRequest, AgentForkRequest, AgentForkedFrom, AgentFound, AgentFoundItems,
    AgentFoundList, AgentFoundPreview, AgentFoundRequest, AgentHit, AgentHits, AgentImportJob,
    AgentImportProgress, AgentImportRequest, AgentMark, AgentPatch, AgentPatchRequest, AgentRename,
    AgentRewindApply, AgentRewindPlan, AgentRewindPreview, AgentRewindUndo, AgentSearchRequest,
    AgentThreadsRequest, AgentTurn, AgentTurnEvent, AgentTurnFile, AgentTurnFiles, AgentTurns,
    AgentTurnsRequest, AgentUndo, history, is_send_key,
};
use g7link_protocol::{
    DeviceId, Envelope, ErrorCode,
    messages::{
        AgentEvent, AgentItem, AgentItems, AgentSend, AgentThread, AgentThreads, Empty, agent, cut_text,
        one_line, types,
    },
};

pub use crate::agent_cache::{AgentHeld, Cached, CachedItems, HeldState, HeldWhy, MAX_HELD};
use crate::{
    Error, Node, Result,
    agent_cache::AgentCache,
    agents::{self, AgentFailure, AgentReply, AgentRequest},
    dispatch::Responder,
    events::NodeEvent,
    node::Shared,
};

/// A turn, or a range of turns from `from` to `to` (`agent.diff.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnRange {
    Turn(u32),
    Span { from: u32, to: u32 },
}

impl TurnRange {
    /// The range a request names, if it names one well.
    pub fn of(turn: Option<u32>, from: Option<u32>, to: Option<u32>) -> Option<Self> {
        match (turn, from, to) {
            (Some(turn), None, None) if turn >= 1 => Some(Self::Turn(turn)),
            (None, Some(from), Some(to)) if from >= 1 && from <= to => Some(Self::Span { from, to }),
            _ => None,
        }
    }

    /// As the wire carries it: `turn`, `from`, `to`.
    pub fn wire(self) -> (Option<u32>, Option<u32>, Option<u32>) {
        match self {
            Self::Turn(turn) => (Some(turn), None, None),
            Self::Span { from, to } => (None, Some(from), Some(to)),
        }
    }
}

/// One page of a file's patch as the host gives it (`turn.patch`): ask
/// again from `next` for the rest.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct AgentPatchPage {
    pub patch: String,
    pub next: Option<u64>,
    pub truncated: bool,
    pub binary: bool,
}

/// Never prints the patch.
impl std::fmt::Debug for AgentPatchPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPatchPage")
            .field("bytes", &self.patch.len())
            .field("next", &self.next)
            .field("truncated", &self.truncated)
            .field("binary", &self.binary)
            .finish()
    }
}

/// Host pages of one patch the core follows at most.
const MAX_PATCH_PAGES: usize = 64;

fn now_ms() -> u64 {
    u64::try_from(crate::now_unix_ms()).unwrap_or(0)
}

// ---- PC side ----

/// Whether `t` is one of §6's requests.
pub(crate) fn is_request(t: &str) -> bool {
    matches!(
        t,
        types::AGENT_TURNS
            | types::AGENT_DIFF_FILES
            | types::AGENT_DIFF_PATCH
            | types::AGENT_SEARCH
            | types::AGENT_MARK
            | types::AGENT_RENAME
            | types::AGENT_FORK
            | types::AGENT_REWIND_PREVIEW
            | types::AGENT_REWIND_APPLY
            | types::AGENT_REWIND_UNDO
            | types::AGENT_FOUND
            | types::AGENT_FOUND_PREVIEW
            | types::AGENT_IMPORT
    )
}

/// Whether `t` is one of §6's events (PC → phone).
pub(crate) fn is_event(t: &str) -> bool {
    matches!(t, types::AGENT_TURN | types::AGENT_CUT | types::AGENT_IMPORT_PROGRESS)
}

/// Whether `t` needs `agent_sessions` on too: what was found on the PC.
fn needs_sessions(t: &str) -> bool {
    matches!(t, types::AGENT_FOUND | types::AGENT_FOUND_PREVIEW | types::AGENT_IMPORT)
}

/// A PC's answer to one of §6's requests: one envelope, or a list that may
/// go in parts.
enum Answer {
    Plain(Envelope),
    Threads(AgentThreads),
    Turns(AgentTurns),
    Files(AgentTurnFiles),
    Patch(AgentPatch),
    Hits(AgentHits),
    Found(AgentFoundList),
    Items(AgentFoundItems),
}

/// What the platform must answer a request with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expects {
    Ok,
    Turns,
    Files,
    Hits,
    Threads,
    Thread,
    Plan,
    Found,
    Items,
    Job,
}

impl Expects {
    fn of(request: &AgentRequest) -> Self {
        match request {
            AgentRequest::Turns { .. } => Self::Turns,
            AgentRequest::DiffFiles { .. } => Self::Files,
            AgentRequest::Search { .. } => Self::Hits,
            AgentRequest::Mark { .. } | AgentRequest::ThreadsPage { .. } => Self::Threads,
            AgentRequest::Rename { .. }
            | AgentRequest::Fork { .. }
            | AgentRequest::RewindApply { .. }
            | AgentRequest::RewindUndo { .. } => Self::Thread,
            AgentRequest::RewindPreview { .. } => Self::Plan,
            AgentRequest::Found { .. } => Self::Found,
            AgentRequest::FoundPreview { .. } => Self::Items,
            AgentRequest::Import { .. } => Self::Job,
            _ => Self::Ok,
        }
    }
}

fn plain(env: Envelope) -> Answer {
    Answer::Plain(env)
}

fn ok_with<T: serde::Serialize>(body: &T) -> Answer {
    plain(Envelope::new(types::OK, body).unwrap_or_else(|_| AgentFailure::Internal.reply("failed")))
}

fn bad() -> Answer {
    plain(Envelope::error(ErrorCode::BadMessage, "invalid agent request"))
}

/// The refusal of a rewind from a phone that names files: files go back on
/// the PC only.
fn files_refused() -> Answer {
    plain(Envelope::error(ErrorCode::Denied, "files are put back on the PC only"))
}

/// PC: answers one of §6's requests from `peer`.
pub(crate) async fn handle_request(
    shared: &Arc<Shared>,
    peer: DeviceId,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<()> {
    match answer(shared, peer, env).await {
        Answer::Plain(reply) => rsp.send(reply).await,
        Answer::Threads(body) => rsp.paged(body).await,
        Answer::Turns(body) => rsp.paged(body).await,
        Answer::Files(body) => rsp.paged(body).await,
        Answer::Patch(body) => rsp.paged(body).await,
        Answer::Hits(body) => rsp.paged(body).await,
        Answer::Found(body) => rsp.paged(body).await,
        Answer::Items(body) => rsp.paged(body).await,
    }
}

/// The request `env` asks for, after the checks of §6; the answer to send
/// instead when it's refused.
fn request_of(env: &Envelope) -> std::result::Result<AgentRequest, Answer> {
    let request = match env.t.as_str() {
        types::AGENT_TURNS => {
            let r = agents::decode::<AgentTurnsRequest>(env, AgentTurnsRequest::is_valid).ok_or_else(bad)?;
            let limit = r.limit();
            AgentRequest::Turns { thread: r.thread, before: r.before, limit }
        }
        types::AGENT_DIFF_FILES => {
            let r = agents::decode::<AgentDiffRequest>(env, AgentDiffRequest::is_valid).ok_or_else(bad)?;
            let range = TurnRange::of(r.turn, r.from, r.to).ok_or_else(bad)?;
            AgentRequest::DiffFiles { thread: r.thread, range }
        }
        types::AGENT_DIFF_PATCH => {
            let r = agents::decode::<AgentPatchRequest>(env, AgentPatchRequest::is_valid).ok_or_else(bad)?;
            let range = TurnRange::of(r.turn, r.from, r.to).ok_or_else(bad)?;
            AgentRequest::DiffPatch { thread: r.thread, range, path: r.path, offset: None }
        }
        types::AGENT_SEARCH => {
            let r =
                agents::decode::<AgentSearchRequest>(env, AgentSearchRequest::is_valid).ok_or_else(bad)?;
            let limit = r.limit();
            let query = one_line(&r.query).trim().to_owned();
            AgentRequest::Search { query, limit, cursor: r.cursor, archived: r.archived }
        }
        types::AGENT_MARK => {
            let m = agents::decode::<AgentMark>(env, AgentMark::is_valid).ok_or_else(bad)?;
            if !m.is_snooze_in_range(now_ms()) {
                return Err(bad());
            }
            AgentRequest::Mark {
                threads: m.threads,
                pinned: m.pinned,
                archived: m.archived,
                unread: m.unread,
                snooze: m.snooze,
            }
        }
        types::AGENT_RENAME => {
            let r = agents::decode::<AgentRename>(env, AgentRename::is_valid).ok_or_else(bad)?;
            let title = one_line(&r.title).trim().to_owned();
            if title.is_empty() {
                return Err(bad());
            }
            AgentRequest::Rename { thread: r.thread, title }
        }
        types::AGENT_FORK => {
            let r = agents::decode::<AgentForkRequest>(env, AgentForkRequest::is_valid).ok_or_else(bad)?;
            AgentRequest::Fork { thread: r.thread, turn: r.turn }
        }
        types::AGENT_REWIND_PREVIEW => {
            let r =
                agents::decode::<AgentRewindPreview>(env, AgentRewindPreview::is_valid).ok_or_else(bad)?;
            if r.files {
                return Err(files_refused());
            }
            AgentRequest::RewindPreview { thread: r.thread, turn: r.turn }
        }
        types::AGENT_REWIND_APPLY => {
            let r = agents::decode::<AgentRewindApply>(env, AgentRewindApply::is_valid).ok_or_else(bad)?;
            if r.include_later {
                return Err(files_refused());
            }
            AgentRequest::RewindApply { token: r.token, refill: r.refill }
        }
        types::AGENT_REWIND_UNDO => {
            let r = agents::decode::<AgentRewindUndo>(env, AgentRewindUndo::is_valid).ok_or_else(bad)?;
            AgentRequest::RewindUndo { thread: r.thread }
        }
        types::AGENT_FOUND => {
            let r = agents::decode::<AgentFoundRequest>(env, AgentFoundRequest::is_valid).ok_or_else(bad)?;
            let limit = r.limit();
            let query = r.query.map(|q| one_line(&q).trim().to_owned()).filter(|q| !q.is_empty());
            AgentRequest::Found { query, agent: r.agent, show: r.show, before_ms: r.before_ms, limit }
        }
        types::AGENT_FOUND_PREVIEW => {
            let r = agents::decode::<AgentFoundPreview>(env, AgentFoundPreview::is_valid).ok_or_else(bad)?;
            let limit = r.limit();
            AgentRequest::FoundPreview { id: r.id, limit }
        }
        types::AGENT_IMPORT => {
            let r =
                agents::decode::<AgentImportRequest>(env, AgentImportRequest::is_valid).ok_or_else(bad)?;
            match r.cancel {
                Some(job) => AgentRequest::ImportCancel { job },
                None => AgentRequest::Import { ids: r.ids },
            }
        }
        _ => return Err(plain(Envelope::error(ErrorCode::Unsupported, "unknown agent request"))),
    };
    Ok(request)
}

async fn answer(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) -> Answer {
    if let Some(refused) = agents::refusal(shared, peer, needs_sessions(&env.t)) {
        return plain(refused);
    }
    let request = match request_of(env) {
        Ok(request) => request,
        Err(answer) => return answer,
    };
    if let AgentRequest::DiffPatch { thread, range, path, .. } = request {
        return patch(shared, peer, thread, range, path).await;
    }
    let expects = Expects::of(&request);
    let items2 = shared.peer_offers(&peer, agent::ITEMS2);
    let control = shared.control_with(&peer);
    let reply = match agents::ask_platform(shared, peer, request).await {
        Ok(reply) => reply,
        Err(failure) => return plain(failure.reply(agents::failure_message(failure))),
    };
    let malformed = || plain(AgentFailure::Internal.reply("the host's answer was malformed"));
    match (expects, reply) {
        (Expects::Ok, AgentReply::Ok) => plain(Envelope::empty(types::OK)),
        (Expects::Turns, AgentReply::Turns(turns)) => Answer::Turns(AgentTurns { turns }.sanitized()),
        (Expects::Files, AgentReply::TurnFiles(files)) => Answer::Files(files.sanitized()),
        (Expects::Hits, AgentReply::Hits(hits)) => Answer::Hits(hits.sanitized()),
        (Expects::Threads, AgentReply::Threads(threads)) => {
            let threads = threads.into_iter().map(|t| agents::thread_for(t, control)).collect();
            Answer::Threads(AgentThreads { threads }.sanitized())
        }
        (Expects::Thread, AgentReply::Thread(thread)) => match thread.sanitized() {
            Some(thread) => ok_with(&agents::thread_for(thread, control)),
            None => malformed(),
        },
        (Expects::Plan, AgentReply::RewindPlan(plan)) => match plan.sanitized() {
            Some(plan) => ok_with(&plan),
            None => malformed(),
        },
        (Expects::Found, AgentReply::Found(found)) => Answer::Found(found.sanitized()),
        (Expects::Items, AgentReply::Items(page)) => {
            let items = page.items.into_iter().map(|item| item.for_phone(items2, control)).collect();
            Answer::Items(AgentFoundItems { items }.sanitized())
        }
        (Expects::Job, AgentReply::Job(job)) if job.is_valid() => ok_with(&job),
        (Expects::Job, AgentReply::Job(_)) => malformed(),
        _ => plain(AgentFailure::Internal.reply("the host answered something else")),
    }
}

/// PC: one file's patch, following the host's pages up to
/// [`history::MAX_PATCH_BYTES`]; a later page that fails ends it, cut.
async fn patch(
    shared: &Arc<Shared>,
    peer: DeviceId,
    thread: String,
    range: TurnRange,
    path: String,
) -> Answer {
    let started = Instant::now();
    let mut whole = AgentPatch::default();
    let mut offset = None;
    let mut ended = false;
    for _ in 0..MAX_PATCH_PAGES {
        let request = AgentRequest::DiffPatch { thread: thread.clone(), range, path: path.clone(), offset };
        let page = match agents::ask_platform(shared, peer, request).await {
            Ok(AgentReply::Patch(page)) => page,
            Ok(_) if offset.is_none() => {
                return plain(AgentFailure::Internal.reply("the host answered something else"));
            }
            Err(failure) if offset.is_none() => {
                return plain(failure.reply(agents::failure_message(failure)));
            }
            Ok(_) | Err(_) => break,
        };
        whole.binary |= page.binary;
        whole.truncated |= page.truncated;
        let room = history::MAX_PATCH_BYTES - whole.patch.len();
        if page.patch.len() > room {
            whole.patch.push_str(cut_text(&page.patch, room));
            whole.truncated = true;
            ended = true;
            break;
        }
        whole.patch.push_str(&page.patch);
        match page.next {
            None => {
                ended = true;
                break;
            }
            // A page that doesn't move on, or the time it may take is up: what came is sent, cut.
            Some(next)
                if offset.is_some_and(|at| next <= at) || started.elapsed() >= agents::PLATFORM_LIMIT =>
            {
                break;
            }
            Some(next) => offset = Some(next),
        }
    }
    if !ended {
        whole.truncated = true;
    }
    Answer::Patch(whole.sanitized())
}

/// PC: tells every phone that gets agent events that a turn started, ended,
/// or its restore point settled (`agent.turn`).
pub(crate) fn send_turn(shared: &Arc<Shared>, event: AgentTurnEvent) -> Result<()> {
    let event = event.sanitized().ok_or_else(|| Error::Protocol("invalid agent.turn".into()))?;
    let env = Envelope::new(types::AGENT_TURN, &event)?;
    for session in shared.agent_recipients() {
        let _ = session.send_now(env.clone());
    }
    Ok(())
}

/// PC: tells every phone that gets agent events that items left a thread,
/// or came back (`agent.cut`).
pub(crate) fn send_cut(shared: &Arc<Shared>, cut: AgentCut) -> Result<()> {
    if !cut.is_valid() {
        return Err(Error::Protocol("invalid agent.cut".into()));
    }
    let env = Envelope::new(types::AGENT_CUT, &cut)?;
    for session in shared.agent_recipients() {
        let _ = session.send_now(env.clone());
    }
    Ok(())
}

/// PC: tells the phones that may see sessions found here how an import goes
/// (`agent.import.progress`).
pub(crate) fn send_import(shared: &Arc<Shared>, progress: AgentImportProgress) -> Result<()> {
    if !progress.is_valid() {
        return Err(Error::Protocol("invalid agent.import.progress".into()));
    }
    let env = Envelope::new(types::AGENT_IMPORT_PROGRESS, &progress)?;
    for session in shared.session_recipients() {
        let _ = session.send_now(env.clone());
    }
    Ok(())
}

// ---- Phone side: what arrives ----

/// Phone: one of §6's events from `peer`.
pub(crate) async fn handle_event(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) {
    if !shared.we_offer(agent::REMOTE) {
        return;
    }
    match env.t.as_str() {
        types::AGENT_TURN => {
            if let Ok(event) = env.body::<AgentTurnEvent>()
                && event.is_valid()
                && let Some(event) = event.sanitized()
            {
                shared.emit(NodeEvent::AgentTurn { device: peer, thread: event.thread, turn: event.turn });
            }
        }
        types::AGENT_CUT => {
            if let Ok(cut) = env.body::<AgentCut>()
                && cut.is_valid()
            {
                if !cut.restored {
                    let (thread, from, to) = (cut.thread.clone(), cut.from_seq, cut.to_seq);
                    with_cache(shared, move |cache| cache.cut(&peer, &thread, from, to)).await;
                }
                shared.emit(NodeEvent::AgentCut { device: peer, cut });
            }
        }
        types::AGENT_IMPORT_PROGRESS => {
            if let Ok(progress) = env.body::<AgentImportProgress>()
                && progress.is_valid()
            {
                shared.emit(NodeEvent::AgentImport { device: peer, progress });
            }
        }
        _ => {}
    }
}

/// Phone: a PC's catalog arrived; with agents in it, its held messages go.
pub(crate) fn catalog_arrived(shared: &Arc<Shared>, peer: DeviceId, has_agents: bool) {
    if has_agents {
        kick_flush(shared, peer);
    }
}

/// Phone: keeps what an `agent.event` says in the saved threads, and tries
/// held messages again when their thread changed.
pub(crate) async fn event_arrived(shared: &Arc<Shared>, peer: DeviceId, event: &AgentEvent) {
    let thread = event.thread.clone();
    match event.kind.as_str() {
        agent::event::STATE | agent::event::DONE => {
            let Some(state) = event.state.clone() else { return };
            let now = now_ms();
            let held = with_cache(shared, move |cache| {
                cache.save_thread(&peer, &state, now)?;
                cache.holds_for(&peer, &thread)
            })
            .await;
            if held == Some(true) {
                kick_flush(shared, peer);
            }
        }
        agent::event::ITEM => {
            if let Some(item) = event.item.clone() {
                with_cache(shared, move |cache| cache.save_item(&peer, &thread, &item)).await;
            }
        }
        agent::event::REMOVED => {
            with_cache(shared, move |cache| cache.remove_thread(&peer, &thread)).await;
        }
        _ => {}
    }
}

// ---- The saved threads ----

/// The cache, opened the first time it's needed on a phone; `None` on a PC
/// or when it can't be opened.
fn cache(shared: &Shared) -> Option<Arc<AgentCache>> {
    if !shared.we_offer(agent::REMOTE) {
        return None;
    }
    shared.agents.cache_or_open(|| {
        let cache = AgentCache::open(&shared.data_dir)?;
        let paired: Vec<DeviceId> = shared.store.list_peers()?.into_iter().map(|p| p.id).collect();
        cache.keep_only(&paired)?;
        Ok(cache)
    })
}

/// Runs `work` on the saved threads on a blocking thread; `None` when there
/// are none here or it failed (which is logged: a cache that fails costs
/// only what it would have saved).
pub(crate) async fn with_cache<T: Send + 'static>(
    shared: &Arc<Shared>,
    work: impl FnOnce(&AgentCache) -> Result<T> + Send + 'static,
) -> Option<T> {
    let shared = shared.clone();
    let done = tokio::task::spawn_blocking(move || cache(&shared).map(|cache| work(&cache))).await;
    match done {
        Ok(Some(Ok(value))) => Some(value),
        Ok(Some(Err(e))) => {
            tracing::warn!(error = %e, "the saved agent threads failed");
            None
        }
        Ok(None) | Err(_) => None,
    }
}

/// Like [`with_cache`], for the app's own calls: an error when the cache
/// can't be used.
async fn cache_call<T: Send + 'static>(
    shared: &Arc<Shared>,
    work: impl FnOnce(&AgentCache) -> Result<T> + Send + 'static,
) -> Result<T> {
    let shared = shared.clone();
    tokio::task::spawn_blocking(move || match cache(&shared) {
        Some(cache) => work(&cache),
        None => Err(Error::Unsupported),
    })
    .await
    .map_err(|e| Error::Internal(e.to_string()))?
}

/// Phone: a PC's list arrived: it's saved. Refused (`Denied`): that PC
/// turned agents off for this phone, so nothing of it is kept.
pub(crate) async fn threads_arrived(
    shared: &Arc<Shared>,
    peer: DeviceId,
    threads: &Result<Vec<AgentThread>>,
) {
    match threads {
        Ok(threads) => {
            let (threads, now) = (threads.clone(), now_ms());
            with_cache(shared, move |cache| cache.save_threads(&peer, &threads, now)).await;
        }
        Err(Error::Denied) => {
            with_cache(shared, move |cache| cache.drop_pc(&peer)).await;
            emit_outbox(shared, peer).await;
        }
        Err(_) => {}
    }
}

/// Phone: a page of a thread arrived; the first page means it was opened.
pub(crate) async fn page_arrived(shared: &Arc<Shared>, peer: DeviceId, page: &AgentItems, opened: bool) {
    let (page, now) = (page.clone(), now_ms());
    with_cache(shared, move |cache| cache.save_page(&peer, &page, now, opened)).await;
}

/// Phone: a thread came back from a request (start, configure, mark…).
pub(crate) async fn thread_arrived(shared: &Arc<Shared>, peer: DeviceId, thread: &AgentThread) {
    let (thread, now) = (thread.clone(), now_ms());
    with_cache(shared, move |cache| cache.save_thread(&peer, &thread, now)).await;
}

// ---- Held messages ----

/// Phone: tells the app how `peer`'s held messages stand.
async fn emit_outbox(shared: &Arc<Shared>, peer: DeviceId) {
    if let Some(held) = with_cache(shared, move |cache| cache.held(&peer)).await {
        shared.emit(NodeEvent::AgentOutbox { device: peer, held });
    }
}

/// A fresh key for a message: 16 lowercase hex digits.
fn fresh_key() -> String {
    format!("{:016x}", rand::random::<u64>())
}

/// Whether `err` means the message didn't reach the PC (or its answer didn't
/// come back): then it's held, and sent again with the same key.
fn unreachable(err: &Error) -> bool {
    matches!(err, Error::Offline | Error::Timeout | Error::Network(_))
}

/// Phone: sends a message, holding it when the PC can't be reached (`hold`).
/// A held message is sent once, in order, when the PC is back: `Some` says
/// it's waiting. A message with images is never held: it fails offline. A
/// message for a thread with messages already held waits behind them.
pub(crate) async fn send_held(
    shared: &Arc<Shared>,
    peer: DeviceId,
    mut body: AgentSend,
    hold: bool,
) -> Result<Option<AgentHeld>> {
    if !hold || !body.images.is_empty() {
        agents::send(shared, peer, body).await?;
        return Ok(None);
    }
    if !body.is_valid() {
        return Err(agents::invalid("agent.send"));
    }
    let key = fresh_key();
    body.key = Some(key.clone());
    let thread = body.thread.clone();
    let queued_behind = with_cache(shared, {
        let thread = thread.clone();
        move |cache| cache.holds_for(&peer, &thread)
    })
    .await
    .unwrap_or(false);
    if !queued_behind && shared.session(&peer).is_some() {
        match agents::send_keyed(shared, peer, body.clone()).await {
            Ok(_) => return Ok(None),
            Err(e) if unreachable(&e) => {}
            Err(e) => return Err(e),
        }
    }
    let (text, now) = (body.text.clone(), now_ms());
    let held = cache_call(shared, move |cache| cache.hold(&peer, &key, &thread, &text, now)).await?;
    emit_outbox(shared, peer).await;
    if queued_behind {
        kick_flush(shared, peer);
    }
    Ok(Some(held))
}

/// Phone: starts sending `peer`'s held messages, or, while that runs, has it
/// look once more when it's done.
pub(crate) fn kick_flush(shared: &Arc<Shared>, peer: DeviceId) {
    if !shared.we_offer(agent::REMOTE) || !shared.agents.begin_flush(peer) {
        return;
    }
    let shared = shared.clone();
    shared.runtime.clone().spawn(async move {
        loop {
            flush(&shared, peer).await;
            if !shared.agents.end_flush(peer) {
                break;
            }
        }
    });
}

/// How a held message's thread stands now.
enum Standing {
    /// It can take a message: as a turn (`false`), or queued behind the one
    /// that runs (`true`).
    Takes(bool),
    Blocked(HeldWhy),
    NotSent(HeldWhy),
    /// The PC can't be reached, or answered otherwise: try later.
    Later,
}

/// What `thread` takes now, read from the PC.
async fn standing(shared: &Arc<Shared>, peer: DeviceId, thread: &str) -> Standing {
    match agents::read(shared, peer, thread.to_owned(), None, 1).await {
        Ok(page) => {
            let t = &page.thread;
            if t.readonly.is_some() {
                Standing::NotSent(HeldWhy::ReadOnly)
            } else if t.state == agent::state::WAITING || t.asks > 0 || t.asking > 0 {
                Standing::Blocked(HeldWhy::AnswerFirst)
            } else {
                Standing::Takes(t.is_running())
            }
        }
        Err(Error::NotFound) => Standing::NotSent(HeldWhy::Gone),
        Err(_) => Standing::Later,
    }
}

/// Phone: sends `peer`'s held messages that may go: per thread, in the
/// order written, one at a time, each with its key. One older than a day
/// isn't sent by itself.
async fn flush(shared: &Arc<Shared>, peer: DeviceId) {
    if shared.session(&peer).is_none() {
        return;
    }
    let now = now_ms();
    let Some((expired, held)) =
        with_cache(shared, move |cache| Ok((cache.expire(&peer, now)?, cache.held(&peer)?))).await
    else {
        return;
    };
    let mut threads: Vec<String> = Vec::new();
    for h in held.iter().filter(|h| h.state != HeldState::NotSent) {
        if !threads.contains(&h.thread) {
            threads.push(h.thread.clone());
        }
    }
    if threads.is_empty() {
        // Nothing to send: the app hears only of messages that just stopped waiting.
        if expired {
            emit_outbox(shared, peer).await;
        }
        return;
    }
    'threads: for thread in threads {
        let rows: Vec<&AgentHeld> =
            held.iter().filter(|h| h.thread == thread && h.state != HeldState::NotSent).collect();
        let mut running = match standing(shared, peer, &thread).await {
            Standing::Takes(running) => running,
            Standing::Blocked(why) => {
                set_all(shared, peer, &rows, HeldState::Blocked, Some(why)).await;
                continue;
            }
            Standing::NotSent(why) => {
                set_all(shared, peer, &rows, HeldState::NotSent, Some(why)).await;
                continue;
            }
            Standing::Later => {
                if shared.session(&peer).is_none() {
                    break;
                }
                continue;
            }
        };
        for row in rows {
            if running && !agents::can_control(shared, &peer) {
                // Only a PC with §5 queues behind a running turn: this waits for the turn to end.
                set_one(shared, peer, &row.key, HeldState::Waiting, None).await;
                continue 'threads;
            }
            set_one(shared, peer, &row.key, HeldState::Sending, None).await;
            let body = AgentSend {
                thread: row.thread.clone(),
                text: row.text.clone(),
                dispatch: running.then(|| agent::dispatch::QUEUE.to_owned()),
                images: Vec::new(),
                key: Some(row.key.clone()),
            };
            match agents::send_keyed(shared, peer, body).await {
                Ok(_) => {
                    let key = row.key.clone();
                    with_cache(shared, move |cache| cache.take(&peer, &key)).await;
                    emit_outbox(shared, peer).await;
                    running = true;
                }
                Err(e) if unreachable(&e) => {
                    set_one(shared, peer, &row.key, HeldState::Waiting, None).await;
                    break 'threads;
                }
                Err(Error::Protocol(msg)) if msg.starts_with("BUSY") => {
                    let why = match standing(shared, peer, &thread).await {
                        Standing::Blocked(why) => why,
                        _ => HeldWhy::QueueFull,
                    };
                    set_one(shared, peer, &row.key, HeldState::Blocked, Some(why)).await;
                    continue 'threads;
                }
                Err(Error::NotFound) => {
                    set_one(shared, peer, &row.key, HeldState::NotSent, Some(HeldWhy::Gone)).await;
                }
                Err(e) => {
                    tracing::debug!(peer = %peer.short(), error = %e, "a held message was refused");
                    set_one(shared, peer, &row.key, HeldState::NotSent, Some(HeldWhy::Refused)).await;
                }
            }
        }
    }
    emit_outbox(shared, peer).await;
}

async fn set_one(shared: &Arc<Shared>, peer: DeviceId, key: &str, state: HeldState, why: Option<HeldWhy>) {
    let key = key.to_owned();
    with_cache(shared, move |cache| cache.set_held(&peer, &key, state, why)).await;
}

async fn set_all(
    shared: &Arc<Shared>,
    peer: DeviceId,
    rows: &[&AgentHeld],
    state: HeldState,
    why: Option<HeldWhy>,
) {
    let keys: Vec<String> = rows.iter().map(|h| h.key.clone()).collect();
    with_cache(shared, move |cache| {
        for key in &keys {
            cache.set_held(&peer, key, state, why)?;
        }
        Ok(())
    })
    .await;
    emit_outbox(shared, peer).await;
}

// ---- Phone side: requests ----

fn invalid(what: &str) -> Error {
    agents::invalid(what)
}

async fn paged<T>(
    shared: &Arc<Shared>,
    peer: DeviceId,
    t: &str,
    body: &impl serde::Serialize,
    cap: &str,
) -> Result<T>
where
    T: g7link_protocol::lanes::Paged + serde::de::DeserializeOwned + Send + 'static,
{
    agents::request_paged_of(shared, peer, Envelope::new(t, body)?, cap).await
}

async fn single<T: serde::de::DeserializeOwned>(
    shared: &Arc<Shared>,
    peer: DeviceId,
    t: &str,
    body: &impl serde::Serialize,
    cap: &str,
) -> Result<T> {
    agents::request_of(shared, peer, Envelope::new(t, body)?, cap).await
}

/// A thread a PC answered with, checked and kept.
async fn thread_reply(shared: &Arc<Shared>, peer: DeviceId, thread: AgentThread) -> Result<AgentThread> {
    let thread = thread.sanitized().ok_or_else(|| invalid("agent thread reply"))?;
    thread_arrived(shared, peer, &thread).await;
    Ok(thread)
}

/// The node's §6 methods. PC: what the platform pushes. Phone: requests to a
/// PC (`peer`), its saved threads and its held messages.
impl Node {
    /// PC: a turn of a thread started, ended, or its restore point settled;
    /// every phone with the `agents` toggle on gets it (`agent.turn`).
    pub async fn agent_turn(&self, thread: String, turn: AgentTurn) -> Result<()> {
        send_turn(self.shared(), AgentTurnEvent { thread, turn })
    }

    /// PC: items left a thread (a rewind), or came back (`restored`, its undo).
    pub async fn agent_cut(&self, cut: AgentCut) -> Result<()> {
        send_cut(self.shared(), cut)
    }

    /// PC: how an import of sessions found here goes, to the phones that may
    /// see them (both toggles on). At most 4 a second; the last has
    /// `done + failed == total`.
    pub async fn agent_import_progress(&self, progress: AgentImportProgress) -> Result<()> {
        send_import(self.shared(), progress)
    }

    /// Phone: a PC's threads on one shelf (`show`), or the next page
    /// (`before_ms`). The main list's newest is [`Node::agent_threads`].
    pub async fn agent_threads_shown(
        &self,
        peer: DeviceId,
        request: AgentThreadsRequest,
    ) -> Result<Vec<AgentThread>> {
        if !request.is_valid() {
            return Err(invalid("agent.threads"));
        }
        let shared = self.shared();
        let reply: AgentThreads = paged(shared, peer, types::AGENT_THREADS, &request, agent::HOST).await?;
        let threads = reply.sanitized().threads;
        let result = Ok(threads);
        threads_arrived(shared, peer, &result).await;
        result
    }

    /// Phone: a thread's turns, newest first: before turn `before`, at most `limit` (1 to 100; default 30).
    pub async fn agent_turns(
        &self,
        peer: DeviceId,
        thread: String,
        before: Option<u32>,
        limit: Option<u32>,
    ) -> Result<Vec<AgentTurn>> {
        let body = AgentTurnsRequest { thread, before, limit };
        if !body.is_valid() {
            return Err(invalid("agent.turns"));
        }
        let reply: AgentTurns = paged(self.shared(), peer, types::AGENT_TURNS, &body, agent::HOST).await?;
        Ok(reply.sanitized().turns)
    }

    /// Phone: the files a turn, or a range of turns, changed.
    pub async fn agent_diff_files(
        &self,
        peer: DeviceId,
        thread: String,
        range: TurnRange,
    ) -> Result<AgentTurnFiles> {
        let (turn, from, to) = range.wire();
        let body = AgentDiffRequest { thread, turn, from, to };
        if !body.is_valid() {
            return Err(invalid("agent.diff.files"));
        }
        let reply: AgentTurnFiles =
            paged(self.shared(), peer, types::AGENT_DIFF_FILES, &body, agent::HOST).await?;
        Ok(reply.sanitized())
    }

    /// Phone: one listed file's patch in a turn or a range, at most 1 MiB
    /// (`truncated` past that).
    pub async fn agent_diff_patch(
        &self,
        peer: DeviceId,
        thread: String,
        range: TurnRange,
        path: String,
    ) -> Result<AgentPatch> {
        let (turn, from, to) = range.wire();
        let body = AgentPatchRequest { thread, turn, from, to, path };
        if !body.is_valid() {
            return Err(invalid("agent.diff.patch"));
        }
        let reply: AgentPatch =
            paged(self.shared(), peer, types::AGENT_DIFF_PATCH, &body, agent::HOST).await?;
        Ok(reply.sanitized())
    }

    /// Phone: searches what a PC holds. The query is made one line here.
    pub async fn agent_search(&self, peer: DeviceId, mut request: AgentSearchRequest) -> Result<AgentHits> {
        request.query = one_line(&request.query).trim().to_owned();
        if !request.is_valid() {
            return Err(invalid("agent.search"));
        }
        let reply: AgentHits = paged(self.shared(), peer, types::AGENT_SEARCH, &request, agent::HOST).await?;
        Ok(reply.sanitized())
    }

    /// Phone: pins, archives, marks unread or snoozes up to 50 threads; the
    /// threads that changed.
    pub async fn agent_mark(&self, peer: DeviceId, mark: AgentMark) -> Result<Vec<AgentThread>> {
        if !mark.is_valid() || !mark.is_snooze_in_range(now_ms()) {
            return Err(invalid("agent.mark"));
        }
        let shared = self.shared();
        let reply: AgentThreads = paged(shared, peer, types::AGENT_MARK, &mark, agent::HOST).await?;
        let threads = reply.sanitized().threads;
        let (saved, now) = (threads.clone(), now_ms());
        with_cache(shared, move |cache| cache.save_threads(&peer, &saved, now)).await;
        Ok(threads)
    }

    /// Phone: renames a thread (1 to 80 bytes, one line).
    pub async fn agent_rename(&self, peer: DeviceId, thread: String, title: String) -> Result<AgentThread> {
        let body = AgentRename { thread, title: one_line(&title).trim().to_owned() };
        if !body.is_valid() {
            return Err(invalid("agent.rename"));
        }
        let shared = self.shared();
        let thread: AgentThread = single(shared, peer, types::AGENT_RENAME, &body, agent::HOST).await?;
        thread_reply(shared, peer, thread).await
    }

    /// Phone: a new thread from a copy of this one up to `turn` (the latest
    /// without it). Files aren't touched.
    pub async fn agent_fork(&self, peer: DeviceId, thread: String, turn: Option<u32>) -> Result<AgentThread> {
        let body = AgentForkRequest { thread, turn };
        if !body.is_valid() {
            return Err(invalid("agent.fork"));
        }
        let shared = self.shared();
        let thread: AgentThread = single(shared, peer, types::AGENT_FORK, &body, agent::HOST).await?;
        thread_reply(shared, peer, thread).await
    }

    /// Phone: what rewinding the conversation to before `turn` would leave
    /// out. Files are never changed from a phone.
    pub async fn agent_rewind_preview(
        &self,
        peer: DeviceId,
        thread: String,
        turn: u32,
    ) -> Result<AgentRewindPlan> {
        let body = AgentRewindPreview { thread, turn, files: false };
        if !body.is_valid() {
            return Err(invalid("agent.rewind.preview"));
        }
        let plan: AgentRewindPlan =
            single(self.shared(), peer, types::AGENT_REWIND_PREVIEW, &body, agent::HOST).await?;
        plan.sanitized().ok_or_else(|| invalid("agent.rewind.preview reply"))
    }

    /// Phone: rewinds as the preview said; `refill` puts the message back as
    /// the thread's draft (edit from here).
    pub async fn agent_rewind_apply(
        &self,
        peer: DeviceId,
        token: String,
        refill: bool,
    ) -> Result<AgentThread> {
        let body = AgentRewindApply { token, refill, include_later: false };
        if !body.is_valid() {
            return Err(invalid("agent.rewind.apply"));
        }
        let shared = self.shared();
        let thread: AgentThread = single(shared, peer, types::AGENT_REWIND_APPLY, &body, agent::HOST).await?;
        thread_reply(shared, peer, thread).await
    }

    /// Phone: brings back what the last rewind left out, while the thread's
    /// `undo` is set and doesn't need the PC (`undo.files`).
    pub async fn agent_rewind_undo(&self, peer: DeviceId, thread: String) -> Result<AgentThread> {
        let body = AgentRewindUndo { thread };
        if !body.is_valid() {
            return Err(invalid("agent.rewind.undo"));
        }
        let shared = self.shared();
        let thread: AgentThread = single(shared, peer, types::AGENT_REWIND_UNDO, &body, agent::HOST).await?;
        thread_reply(shared, peer, thread).await
    }

    /// Phone: the Claude and Codex sessions found on a PC (needs its
    /// `agent_sessions` toggle).
    pub async fn agent_found(
        &self,
        peer: DeviceId,
        mut request: AgentFoundRequest,
    ) -> Result<AgentFoundList> {
        request.query = request.query.map(|q| one_line(&q).trim().to_owned()).filter(|q| !q.is_empty());
        if !request.is_valid() {
            return Err(invalid("agent.found"));
        }
        let reply: AgentFoundList =
            paged(self.shared(), peer, types::AGENT_FOUND, &request, agent::SESSIONS).await?;
        Ok(reply.sanitized())
    }

    /// Phone: a found session's newest messages, read now and kept nowhere.
    pub async fn agent_found_preview(
        &self,
        peer: DeviceId,
        id: String,
        limit: Option<u32>,
    ) -> Result<Vec<AgentItem>> {
        let body = AgentFoundPreview { id, limit };
        if !body.is_valid() {
            return Err(invalid("agent.found.preview"));
        }
        let reply: AgentFoundItems =
            paged(self.shared(), peer, types::AGENT_FOUND_PREVIEW, &body, agent::SESSIONS).await?;
        Ok(reply.sanitized().items)
    }

    /// Phone: brings found sessions in as threads (1 to 50); progress comes
    /// as [`NodeEvent::AgentImport`].
    pub async fn agent_import(&self, peer: DeviceId, ids: Vec<String>) -> Result<AgentImportJob> {
        let body = AgentImportRequest { ids, cancel: None };
        if !body.is_valid() {
            return Err(invalid("agent.import"));
        }
        let job: AgentImportJob =
            single(self.shared(), peer, types::AGENT_IMPORT, &body, agent::SESSIONS).await?;
        if !job.is_valid() {
            return Err(invalid("agent.import reply"));
        }
        Ok(job)
    }

    /// Phone: stops an import job.
    pub async fn agent_import_cancel(&self, peer: DeviceId, job: String) -> Result<()> {
        let body = AgentImportRequest { ids: Vec::new(), cancel: Some(job) };
        if !body.is_valid() {
            return Err(invalid("agent.import"));
        }
        single::<Empty>(self.shared(), peer, types::AGENT_IMPORT, &body, agent::SESSIONS).await.map(|_| ())
    }

    /// Phone: sends a message; with `hold`, one the PC can't take now waits
    /// on this phone (`Some`) and goes once it's back. See [`send_held`].
    pub async fn agent_send_held(
        &self,
        peer: DeviceId,
        send: AgentSend,
        hold: bool,
    ) -> Result<Option<AgentHeld>> {
        send_held(self.shared(), peer, send, hold).await
    }

    /// Phone: a PC's saved threads, newest first, at once and offline.
    pub async fn agent_cached_threads(&self, pc: DeviceId) -> Result<Cached<Vec<AgentThread>>> {
        cache_call(self.shared(), move |cache| cache.threads(&pc)).await
    }

    /// Phone: a thread's saved items, oldest first.
    pub async fn agent_cached_items(&self, pc: DeviceId, thread: String) -> Result<Cached<CachedItems>> {
        cache_call(self.shared(), move |cache| cache.items(&pc, &thread)).await
    }

    /// Phone: a PC's held messages, in the order written.
    pub async fn agent_outbox(&self, pc: DeviceId) -> Result<Vec<AgentHeld>> {
        cache_call(self.shared(), move |cache| cache.held(&pc)).await
    }

    /// Phone: sends a held message that wasn't sent ("Send"): it waits again
    /// and goes when its PC and thread can take it.
    pub async fn agent_outbox_send(&self, pc: DeviceId, key: String) -> Result<()> {
        let now = now_ms();
        let held = cache_call(self.shared(), move |cache| cache.retry(&pc, &key, now)).await?;
        if !held {
            return Err(Error::NotFound);
        }
        emit_outbox(self.shared(), pc).await;
        kick_flush(self.shared(), pc);
        Ok(())
    }

    /// Phone: forgets a held message ("Discard").
    pub async fn agent_outbox_discard(&self, pc: DeviceId, key: String) -> Result<()> {
        let taken = cache_call(self.shared(), move |cache| cache.take(&pc, &key)).await?;
        if !taken {
            return Err(Error::NotFound);
        }
        emit_outbox(self.shared(), pc).await;
        Ok(())
    }

    /// Phone: forgets every saved thread ("Clear saved threads"). Held
    /// messages stay.
    pub async fn agent_cache_clear(&self) -> Result<()> {
        cache_call(self.shared(), AgentCache::clear).await
    }

    /// Phone: what the saved threads take on disk.
    pub async fn agent_cache_bytes(&self) -> Result<u64> {
        cache_call(self.shared(), AgentCache::bytes).await
    }

    /// Phone: "Keep threads on this phone". Off forgets what was kept and
    /// keeps nothing more; held messages still wait and go.
    pub async fn agent_cache_keep(&self, keep: bool) -> Result<()> {
        cache_call(self.shared(), move |cache| cache.set_keeps(keep)).await
    }

    /// Phone: whether threads are kept on this phone.
    pub async fn agent_cache_keeps(&self) -> Result<bool> {
        cache_call(self.shared(), AgentCache::keeps).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_is_a_turn_or_a_span() {
        assert_eq!(TurnRange::of(Some(2), None, None), Some(TurnRange::Turn(2)));
        assert_eq!(TurnRange::of(None, Some(1), Some(3)), Some(TurnRange::Span { from: 1, to: 3 }));
        assert_eq!(TurnRange::of(Some(0), None, None), None);
        assert_eq!(TurnRange::of(None, Some(3), Some(1)), None);
        assert_eq!(TurnRange::of(Some(1), Some(1), Some(2)), None);
        assert_eq!(TurnRange::Span { from: 1, to: 3 }.wire(), (None, Some(1), Some(3)));
    }

    #[test]
    fn every_request_and_event_is_named() {
        for t in [
            types::AGENT_TURNS,
            types::AGENT_DIFF_FILES,
            types::AGENT_DIFF_PATCH,
            types::AGENT_SEARCH,
            types::AGENT_MARK,
            types::AGENT_RENAME,
            types::AGENT_FORK,
            types::AGENT_REWIND_PREVIEW,
            types::AGENT_REWIND_APPLY,
            types::AGENT_REWIND_UNDO,
            types::AGENT_FOUND,
            types::AGENT_FOUND_PREVIEW,
            types::AGENT_IMPORT,
        ] {
            assert!(is_request(t), "{t}");
            assert!(g7link_protocol::lanes::rpc_of(t).is_some(), "{t}");
        }
        for t in [types::AGENT_TURN, types::AGENT_CUT, types::AGENT_IMPORT_PROGRESS] {
            assert!(is_event(t) && !is_request(t), "{t}");
        }
        assert!(needs_sessions(types::AGENT_IMPORT) && !needs_sessions(types::AGENT_MARK));
    }

    /// The error code `request_of` refuses `body` with, as `t`.
    fn refused<T: serde::Serialize>(t: &str, body: &T) -> Option<ErrorCode> {
        match request_of(&Envelope::new(t, body).unwrap()) {
            Err(Answer::Plain(env)) => {
                env.body::<g7link_protocol::messages::ErrorBody>().map(|b| b.code).ok()
            }
            _ => None,
        }
    }

    #[test]
    fn a_rewind_naming_files_is_refused_before_the_platform() {
        let preview = AgentRewindPreview { thread: "abc".into(), turn: 2, files: true };
        assert_eq!(refused(types::AGENT_REWIND_PREVIEW, &preview), Some(ErrorCode::Denied));
        let apply = AgentRewindApply { token: "abc".into(), refill: false, include_later: true };
        assert_eq!(refused(types::AGENT_REWIND_APPLY, &apply), Some(ErrorCode::Denied));
        let later = now_ms() + 31 * 86_400_000;
        let mark = AgentMark { threads: vec!["abc".into()], snooze: Some(later), ..Default::default() };
        assert_eq!(refused(types::AGENT_MARK, &mark), Some(ErrorCode::BadMessage));
        let rename = AgentRename { thread: "abc".into(), title: "\u{202E} ".into() };
        assert_eq!(refused(types::AGENT_RENAME, &rename), Some(ErrorCode::BadMessage));
        let search = AgentSearchRequest { query: "x".repeat(201), ..Default::default() };
        assert_eq!(refused(types::AGENT_SEARCH, &search), Some(ErrorCode::BadMessage));
        let path = "a/".repeat(257);
        let patch = AgentPatchRequest { thread: "abc".into(), turn: Some(1), path, ..Default::default() };
        assert_eq!(refused(types::AGENT_DIFF_PATCH, &patch), Some(ErrorCode::BadMessage));
        let fine = AgentRewindPreview { thread: "abc".into(), turn: 2, files: false };
        let fine = request_of(&Envelope::new(types::AGENT_REWIND_PREVIEW, &fine).unwrap());
        assert!(matches!(fine, Ok(AgentRequest::RewindPreview { turn: 2, .. })));
        let cancel = AgentImportRequest { ids: Vec::new(), cancel: Some("0123456789abcdef".into()) };
        let cancel = request_of(&Envelope::new(types::AGENT_IMPORT, &cancel).unwrap());
        assert!(matches!(cancel, Ok(AgentRequest::ImportCancel { .. })));
    }

    #[test]
    fn a_patch_page_never_prints_its_text() {
        let page = AgentPatchPage { patch: "+secret".into(), next: Some(4), truncated: false, binary: false };
        assert!(!format!("{page:?}").contains("secret"));
    }
}
