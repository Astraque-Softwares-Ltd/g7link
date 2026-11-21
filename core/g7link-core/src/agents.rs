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
//!
//! Sessions started on the PC outside G7 Link (§3) follow the same path
//! behind a second toggle, `agent_sessions`: the core keeps each phone's
//! watched session and sends a session's changes at most once per 2 s.
//!
//! An export (§4) is larger than a control message, so it travels on a
//! stream of its own, checked the same way.
//!
//! The control messages (§5) need `agent.control` on both sides. To a phone
//! without it the PC sends what phase 1 sent: no models, queue or
//! questions, plan mode as supervised, a plan as an answer and a question
//! as a tool step. Images travel on streams of their own, one upload at a
//! time per phone, refused when more bytes come than were announced.
//!
//! Presence (§7) is [`crate::agent_presence`]'s; its state lives here, in
//! `Agents`, so it ends with the session and the toggle.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub use g7link_protocol::messages::{
    AgentAnswer, AgentAsk, AgentAsked, AgentCancel, AgentCatalog, AgentCatalogAgent, AgentCatalogFolder,
    AgentCommand, AgentCommands, AgentCommandsRequest, AgentConfigure, AgentContext, AgentDraft,
    AgentDraftChanged, AgentDraftReply, AgentDraftRequest, AgentDraftSaved, AgentDraftSet, AgentEvent,
    AgentExportMeta, AgentExportRequest, AgentFileHit, AgentFiles, AgentFilesRequest, AgentImage,
    AgentImageMeta, AgentImageThumb, AgentImageThumbMeta, AgentImplement, AgentItem, AgentItemQuestion,
    AgentItems, AgentLimit, AgentMcp, AgentModel, AgentPlanStep, AgentQuestion, AgentQuestionAnswer,
    AgentQuestionAsk, AgentQuestionOption, AgentQuestionReply, AgentQuestioned, AgentQueueChange,
    AgentQueued, AgentRead, AgentSend, AgentSession, AgentSessionContinue, AgentSessionEvent,
    AgentSessionItem, AgentSessionItems, AgentSessionLimit, AgentSessionRead, AgentSessionWatch,
    AgentSessions, AgentStart, AgentThread, AgentThreads, AgentTool, AgentWatch, agent, clean_deny_message,
    is_valid_agent_id,
};
use g7link_protocol::{
    DeviceId, Envelope, ErrorCode, ProtocolError,
    messages::{
        AgentSent, StreamHeader, agent_export, agent_image, clean_agent_export, clean_export_file_name,
        clean_image_name, is_jpeg, types,
    },
    read_frame, write_frame,
};
use iroh::endpoint::{RecvStream, SendStream};

use crate::{
    Error, Result,
    agent_cache::AgentCache,
    agent_git::{
        AgentBranches, AgentGit, AgentGitPatchPage, AgentGitReviewed, AgentJob, AgentJobError, AgentPrRows,
        AgentPrView,
    },
    agent_history::{
        AgentFoundList, AgentHits, AgentImportJob, AgentPatchPage, AgentRewindPlan, AgentThreadsRequest,
        AgentTurn, AgentTurnFiles, TurnRange,
    },
    dispatch::Responder,
    events::NodeEvent,
    node::Shared,
    session::Session,
};

/// The per-device toggle on the PC that governs the service (off by default).
pub const TOGGLE: &str = agent::TOGGLE;
/// The per-device toggle on the PC for sessions started outside G7 Link
/// (off by default; counts only while [`TOGGLE`] is on).
pub const SESSIONS_TOGGLE: &str = agent::SESSIONS_TOGGLE;

/// How long the platform has to answer one request.
pub(crate) const PLATFORM_LIMIT: Duration = Duration::from_secs(15);
/// How long a phone waits for a PC's answer: longer than the PC gives its
/// host ([`PLATFORM_LIMIT`]), so the PC's own refusal arrives first.
const AGENT_WAIT: Duration = Duration::from_secs(20);
/// The least time between two `delta` events to one phone.
const DELTA_INTERVAL: Duration = Duration::from_millis(1000 / agent::MAX_DELTAS_PER_SEC as u64);
/// The least time between two `agent.session` changes of one session.
const SESSION_INTERVAL: Duration = Duration::from_secs(2);
/// Session gates kept before the idle ones are dropped.
const SESSION_GATES_KEPT: usize = 128;
/// How long an export stream waits for the request, or for the PC's answer.
const EXPORT_WAIT: Duration = Duration::from_secs(20);
/// How long an export's text may take to cross.
const EXPORT_SEND: Duration = Duration::from_secs(60);
/// Reasoning items remembered (thread, item id), so their deltas can be
/// kept from phones without `agent.items2`. The oldest go first.
const REASONING_KEPT: usize = 256;
/// Secret questions remembered (their tokens), so a phone's reply to one is
/// refused here. The oldest go first.
const SECRETS_KEPT: usize = 64;
/// How long an image stream waits for its meta, or for the PC's answer.
const IMAGE_WAIT: Duration = Duration::from_secs(20);
/// How long an image's bytes may take to cross (contract: an upload not
/// ended in 60 s is dropped).
const IMAGE_SEND: Duration = Duration::from_secs(60);
/// What one read of an image's bytes takes at most.
const IMAGE_CHUNK: usize = 64 * 1024;

/// What a phone does to one queued message (`agent.queue`).
#[derive(Clone, PartialEq, Eq)]
pub enum QueueAction {
    /// Replace its text (cleaned, 1 to [`agent::MAX_QUEUED_TEXT_BYTES`]).
    Edit(String),
    Remove,
    /// Steer with it now when the thread works and the agent can, else
    /// start a turn with it.
    SendNow,
}

impl std::fmt::Debug for QueueAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Edit(text) => f.debug_struct("Edit").field("bytes", &text.len()).finish(),
            Self::Remove => f.write_str("Remove"),
            Self::SendNow => f.write_str("SendNow"),
        }
    }
}

/// A phone's request, after the core checked the toggle, the limits and the
/// mode. Answered by [`Platform::agent_request`](crate::Platform::agent_request).
#[derive(Clone, PartialEq, Eq)]
pub enum AgentRequest {
    /// `agent.threads`: newest first, at most [`agent::MAX_THREADS`].
    Threads,
    /// `agent.threads` for a shelf (`show`: one of
    /// [`history::show`](g7link_protocol::messages::history::show)), or the
    /// next page (`before_ms`). Answered with `Threads`.
    ThreadsPage { show: Option<String>, before_ms: Option<u64> },
    /// `agent.start`. `mode` is `"supervised"`, or `"plan"` from a phone
    /// with `agent.control`; `model`, `effort` and `images` come only from
    /// such a phone too (`model` as one argument: never a flag, no space).
    /// `custom_model`: the user typed `model` (it needn't be listed).
    /// `key`: 16 hex digits; one the thread took in the last 24 h starts nothing.
    Start {
        agent: String,
        folder: String,
        prompt: String,
        mode: String,
        model: Option<String>,
        effort: Option<String>,
        images: Vec<String>,
        custom_model: bool,
        key: Option<String>,
    },
    /// `agent.send`. `dispatch` (one of [`agent::dispatch`]) and `images`
    /// only from a phone with `agent.control`; without `dispatch` the turn
    /// must have ended. `text` may be empty with images. `key`: 16 hex
    /// digits; one the thread took in the last 24 h is answered
    /// `Duplicate` and nothing is sent.
    Send { thread: String, text: String, dispatch: Option<String>, images: Vec<String>, key: Option<String> },
    /// `agent.configure`: changes for the thread's next turn. An empty
    /// `model` or `effort` goes back to the default; `mode` is
    /// `"supervised"` or `"plan"`. Answered with `Thread`.
    Configure {
        thread: String,
        model: Option<String>,
        effort: Option<String>,
        mode: Option<String>,
        custom_model: bool,
    },
    /// `agent.queue`: a change to the queued message `id` of `thread`.
    Queue { thread: String, id: String, action: QueueAction },
    /// `agent.reply` with answers, one per ask, by the asks' labels or the
    /// user's own words. Never for a secret question (the core refuses it).
    QuestionAnswer { token: String, answers: Vec<AgentQuestionAnswer> },
    /// `agent.reply` with `dismiss`: the user won't answer.
    QuestionDismiss { token: String },
    /// `agent.implement`: implement the thread's newest proposed plan
    /// (`item`), asking before edits and commands (supervised, always).
    Implement { thread: String, item: String },
    /// `agent.files`: files of a thread's folder, or of a folder, for `@`;
    /// at most `limit` (1 to [`agent::MAX_FILE_HITS`]). Paths relative to it.
    Files { thread: Option<String>, folder: Option<String>, query: String, limit: u32 },
    /// `agent.commands`: the slash commands `agent` offers in `folder`.
    Commands { agent: String, folder: String },
    /// `agent.draft.get`. Answered with `Draft`.
    DraftGet { key: String },
    /// `agent.draft.set`: store it when `rev` is the draft's (0: new).
    /// Answered with `DraftSaved`; tell the other phones with
    /// [`Node::agent_draft`](crate::Node::agent_draft).
    DraftSet { key: String, text: String, images: Vec<String>, rev: u64 },
    /// An image a phone sent on an `agent/image` stream, all its bytes (at
    /// most [`agent::MAX_IMAGE_BYTES`], not yet checked to be an image):
    /// hand it to the host's bounded decoder. `name` is a file name.
    /// Answered with `Image`.
    ImageAdd { name: String, bytes: Vec<u8> },
    /// A JPEG of image `id` no larger than `max` px a side and
    /// [`agent::MAX_THUMB_BYTES`]. Answered with `Thumb`.
    ImageThumb { id: String, max: u32 },
    /// `agent.cancel`: interrupt the running turn.
    Cancel { thread: String },
    /// `agent.read`: the newest `limit` items, or the ones before `before`.
    Read { thread: String, before: Option<u64>, limit: u32 },
    /// `agent.answer`, for this time only. `message`: what the user said
    /// when denying (never with `allow`), cleaned, 1 to
    /// [`agent::MAX_DENY_MESSAGE_BYTES`]; only while this PC offers
    /// [`agent::SAY_WHY`].
    Answer { token: String, allow: bool, message: Option<String> },
    /// `agent.sessions`: sessions started on this PC outside G7 Link, at
    /// most [`agent::MAX_SESSIONS`], in the host's order.
    Sessions,
    /// `agent.session.read`: the newest `limit` items, or the ones before `before`.
    SessionRead { session: String, before: Option<u64>, limit: u32 },
    /// `agent.session.watch`: the session whose items the phone shows now
    /// (`None`: none). The core keeps the watch too; it also sends `None`
    /// when the connection ends or a toggle goes off.
    SessionWatch { session: Option<String> },
    /// `agent.session.continue`: make the session a supervised thread
    /// started by the phone.
    SessionContinue { session: String },
    /// `agent.export`: a thread, or a session, as Markdown (exactly one of
    /// the two; a session only with `agent_sessions` on).
    Export { thread: Option<String>, session: Option<String> },
    // ---- History and undo (`docs/protocol/agents.md` §6) ----
    /// `agent.turns`: a thread's turns, newest first, before `before`; at
    /// most `limit` (1 to 100). Answered with `Turns`.
    Turns { thread: String, before: Option<u32>, limit: u32 },
    /// `agent.diff.files`: what a turn or a range of turns changed. Answered
    /// with `TurnFiles`.
    DiffFiles { thread: String, range: TurnRange },
    /// `agent.diff.patch`: one page of a file's patch, from `offset`; the
    /// core asks again from the page's `next`, up to 1 MiB. `path` is one
    /// the diff lists: look it up, never pass it on trust. Answered with `Patch`.
    DiffPatch { thread: String, range: TurnRange, path: String, offset: Option<u64> },
    /// `agent.search` (one line, 1 to 200 bytes; `limit` 1 to 50). Answered with `Hits`.
    Search { query: String, limit: u32, cursor: Option<String>, archived: bool },
    /// `agent.mark`: 1 to 50 threads; `snooze` in the future, at most 30
    /// days ahead, or 0. Answered with `Threads`: those that changed.
    Mark {
        threads: Vec<String>,
        pinned: Option<bool>,
        archived: Option<bool>,
        unread: Option<bool>,
        snooze: Option<u64>,
    },
    /// `agent.rename`: `title` one line, 1 to 80 bytes. Answered with `Thread`.
    Rename { thread: String, title: String },
    /// `agent.fork`: from `turn`, or the latest; supervised or plan from a
    /// phone. Answered with `Thread`, the new one.
    Fork { thread: String, turn: Option<u32> },
    /// `agent.rewind.preview`: the conversation only, always. Answered with `RewindPlan`.
    RewindPreview { thread: String, turn: u32 },
    /// `agent.rewind.apply`: the conversation only (`include_later` is
    /// refused here). Answered with `Thread`.
    RewindApply { token: String, refill: bool },
    /// `agent.rewind.undo`: a rewind that put no files back. Answered with `Thread`.
    RewindUndo { thread: String },
    /// `agent.found` (`agent_sessions` on). Answered with `Found`; never a `cwd`.
    Found {
        query: Option<String>,
        agent: Option<String>,
        show: Option<String>,
        before_ms: Option<u64>,
        limit: u32,
    },
    /// `agent.found.preview` (`agent_sessions` on). Answered with `Items`,
    /// of which only `items` is read.
    FoundPreview { id: String, limit: u32 },
    /// `agent.import` (`agent_sessions` on): 1 to 50 found ids. Answered with `Job`.
    Import { ids: Vec<String> },
    /// `agent.import` with `cancel`. Answered with `Ok`.
    ImportCancel { job: String },
    // ---- Source control (`docs/protocol/agents.md` §8) ----
    /// `agent.git.status`: the thread's folder's status. Answered with `Git`.
    GitStatus { thread: String },
    /// `agent.git.patch`: the page at `offset` of a listed file's
    /// uncommitted patch (the core follows the pages up to 1 MiB). Answered
    /// with `GitPatch`.
    GitPatch { thread: String, path: String, offset: Option<u64> },
    /// `agent.git.review`: pins 1 to 200 listed paths as they are now.
    /// Answered with `Review`.
    GitReview { thread: String, paths: Vec<String> },
    /// `agent.git.commit`, after the core checked that the phone was given
    /// every file of its review whole: `paths` are that review's, `review`
    /// the host's id of it (`git.review`'s `review`). Answered with `GitJob`,
    /// or `Refused` when the host refuses before anything runs.
    GitCommit { thread: String, paths: Vec<String>, message: String, review: String },
    /// `agent.git.push` (the phone's `agent_push` is on). As `GitCommit`.
    GitPush { thread: String, remote: Option<String> },
    /// `agent.git.fetch`. As `GitCommit`.
    GitFetch { thread: String },
    /// `agent.git.branches`. Answered with `Branches`.
    GitBranches { thread: String, query: Option<String> },
    /// `agent.git.switch`: never with changes taken along (a phone can't).
    /// As `GitCommit`.
    GitSwitch { thread: String, branch: String, create: bool, track: bool },
    /// `agent.git.job`: a job as it stands. Answered with `GitJob`.
    GitJob { job: String },
    /// `agent.git.job` with `stop`. Answered with `GitJob`.
    GitStop { job: String },
    /// `agent.pr.view`. Answered with `Pr`.
    PrView { thread: String, refresh: bool },
    /// `agent.pr.create` (the phone's `agent_push` is on). As `GitCommit`.
    PrCreate { thread: String, title: String, body: String, base: Option<String>, draft: bool },
    /// `agent.pr.list`: `show` one of `pr::show`, `folder` with `folder`.
    /// Answered with `PrRows`.
    PrList { show: String, folder: Option<String>, refresh: bool },
    /// `agent.write`: "Ask <agent> to write it", `purpose` one of
    /// `git::purpose` (the host's `thread.send { purpose }`). Answered with `Thread`.
    Ask { thread: String, purpose: String },
}

/// Never prints a prompt, text or token (protocol v0 §11).
impl std::fmt::Debug for AgentRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Threads => f.write_str("Threads"),
            Self::ThreadsPage { show, before_ms } => {
                f.debug_struct("ThreadsPage").field("show", show).field("before_ms", before_ms).finish()
            }
            Self::Start { agent, folder, mode, model, effort, images, custom_model, .. } => f
                .debug_struct("Start")
                .field("agent", agent)
                .field("folder", folder)
                .field("mode", mode)
                .field("model", model)
                .field("effort", effort)
                .field("images", &images.len())
                .field("custom_model", custom_model)
                .finish_non_exhaustive(),
            Self::Send { thread, dispatch, images, .. } => f
                .debug_struct("Send")
                .field("thread", thread)
                .field("dispatch", dispatch)
                .field("images", &images.len())
                .finish_non_exhaustive(),
            Self::Configure { thread, model, effort, mode, custom_model } => f
                .debug_struct("Configure")
                .field("thread", thread)
                .field("model", model)
                .field("effort", effort)
                .field("mode", mode)
                .field("custom_model", custom_model)
                .finish(),
            Self::Queue { thread, action, .. } => f
                .debug_struct("Queue")
                .field("thread", thread)
                .field("action", action)
                .finish_non_exhaustive(),
            Self::QuestionAnswer { answers, .. } => {
                f.debug_struct("QuestionAnswer").field("answers", answers).finish_non_exhaustive()
            }
            Self::QuestionDismiss { .. } => f.debug_struct("QuestionDismiss").finish_non_exhaustive(),
            Self::Implement { thread, .. } => {
                f.debug_struct("Implement").field("thread", thread).finish_non_exhaustive()
            }
            Self::Files { thread, folder, limit, .. } => f
                .debug_struct("Files")
                .field("thread", thread)
                .field("folder", folder)
                .field("limit", limit)
                .finish_non_exhaustive(),
            Self::Commands { agent, folder } => {
                f.debug_struct("Commands").field("agent", agent).field("folder", folder).finish()
            }
            Self::DraftGet { .. } => f.debug_struct("DraftGet").finish_non_exhaustive(),
            Self::DraftSet { text, images, rev, .. } => f
                .debug_struct("DraftSet")
                .field("bytes", &text.len())
                .field("images", &images.len())
                .field("rev", rev)
                .finish_non_exhaustive(),
            Self::ImageAdd { bytes, .. } => {
                f.debug_struct("ImageAdd").field("bytes", &bytes.len()).finish_non_exhaustive()
            }
            Self::ImageThumb { max, .. } => {
                f.debug_struct("ImageThumb").field("max", max).finish_non_exhaustive()
            }
            Self::Cancel { thread } => f.debug_struct("Cancel").field("thread", thread).finish(),
            Self::Read { thread, before, limit } => f
                .debug_struct("Read")
                .field("thread", thread)
                .field("before", before)
                .field("limit", limit)
                .finish(),
            Self::Answer { allow, message, .. } => f
                .debug_struct("Answer")
                .field("allow", allow)
                .field("message_bytes", &message.as_ref().map(String::len))
                .finish_non_exhaustive(),
            Self::Sessions => f.write_str("Sessions"),
            Self::SessionRead { session, before, limit } => f
                .debug_struct("SessionRead")
                .field("session", session)
                .field("before", before)
                .field("limit", limit)
                .finish(),
            Self::SessionWatch { session } => {
                f.debug_struct("SessionWatch").field("session", session).finish()
            }
            Self::SessionContinue { session } => {
                f.debug_struct("SessionContinue").field("session", session).finish()
            }
            Self::Export { thread, session } => {
                f.debug_struct("Export").field("thread", thread).field("session", session).finish()
            }
            Self::Turns { thread, before, limit } => f
                .debug_struct("Turns")
                .field("thread", thread)
                .field("before", before)
                .field("limit", limit)
                .finish(),
            Self::DiffFiles { thread, range } => {
                f.debug_struct("DiffFiles").field("thread", thread).field("range", range).finish()
            }
            Self::DiffPatch { thread, range, offset, .. } => f
                .debug_struct("DiffPatch")
                .field("thread", thread)
                .field("range", range)
                .field("offset", offset)
                .finish_non_exhaustive(),
            Self::Search { limit, cursor, archived, query } => f
                .debug_struct("Search")
                .field("bytes", &query.len())
                .field("limit", limit)
                .field("paged", &cursor.is_some())
                .field("archived", archived)
                .finish(),
            Self::Mark { threads, pinned, archived, unread, snooze } => f
                .debug_struct("Mark")
                .field("threads", &threads.len())
                .field("pinned", pinned)
                .field("archived", archived)
                .field("unread", unread)
                .field("snooze", snooze)
                .finish(),
            Self::Rename { thread, .. } => {
                f.debug_struct("Rename").field("thread", thread).finish_non_exhaustive()
            }
            Self::Fork { thread, turn } => {
                f.debug_struct("Fork").field("thread", thread).field("turn", turn).finish()
            }
            Self::RewindPreview { thread, turn } => {
                f.debug_struct("RewindPreview").field("thread", thread).field("turn", turn).finish()
            }
            Self::RewindApply { refill, .. } => {
                f.debug_struct("RewindApply").field("refill", refill).finish_non_exhaustive()
            }
            Self::RewindUndo { thread } => f.debug_struct("RewindUndo").field("thread", thread).finish(),
            Self::Found { agent, show, before_ms, limit, query } => f
                .debug_struct("Found")
                .field("query_bytes", &query.as_ref().map(String::len))
                .field("agent", agent)
                .field("show", show)
                .field("before_ms", before_ms)
                .field("limit", limit)
                .finish(),
            Self::FoundPreview { id, limit } => {
                f.debug_struct("FoundPreview").field("id", id).field("limit", limit).finish()
            }
            Self::Import { ids } => f.debug_struct("Import").field("ids", &ids.len()).finish(),
            Self::ImportCancel { job } => f.debug_struct("ImportCancel").field("job", job).finish(),
            Self::GitStatus { thread } => f.debug_struct("GitStatus").field("thread", thread).finish(),
            Self::GitPatch { thread, offset, .. } => f
                .debug_struct("GitPatch")
                .field("thread", thread)
                .field("offset", offset)
                .finish_non_exhaustive(),
            Self::GitReview { thread, paths } => {
                f.debug_struct("GitReview").field("thread", thread).field("paths", &paths.len()).finish()
            }
            Self::GitCommit { thread, paths, message, review } => f
                .debug_struct("GitCommit")
                .field("thread", thread)
                .field("paths", &paths.len())
                .field("bytes", &message.len())
                .field("review", review)
                .finish(),
            Self::GitPush { thread, remote } => {
                f.debug_struct("GitPush").field("thread", thread).field("remote", remote).finish()
            }
            Self::GitFetch { thread } => f.debug_struct("GitFetch").field("thread", thread).finish(),
            Self::GitBranches { thread, query } => f
                .debug_struct("GitBranches")
                .field("thread", thread)
                .field("query_bytes", &query.as_ref().map(String::len))
                .finish(),
            Self::GitSwitch { thread, create, track, .. } => f
                .debug_struct("GitSwitch")
                .field("thread", thread)
                .field("create", create)
                .field("track", track)
                .finish_non_exhaustive(),
            Self::GitJob { job } => f.debug_struct("GitJob").field("job", job).finish(),
            Self::GitStop { job } => f.debug_struct("GitStop").field("job", job).finish(),
            Self::PrView { thread, refresh } => {
                f.debug_struct("PrView").field("thread", thread).field("refresh", refresh).finish()
            }
            Self::PrCreate { thread, body, draft, .. } => f
                .debug_struct("PrCreate")
                .field("thread", thread)
                .field("body_bytes", &body.len())
                .field("draft", draft)
                .finish_non_exhaustive(),
            Self::PrList { show, folder, refresh } => f
                .debug_struct("PrList")
                .field("show", show)
                .field("folder", folder)
                .field("refresh", refresh)
                .finish(),
            Self::Ask { thread, purpose } => {
                f.debug_struct("Ask").field("thread", thread).field("purpose", purpose).finish()
            }
        }
    }
}

/// The platform's answer to an [`AgentRequest`]: `Threads` for `Threads`,
/// `Thread` for `Start`, `SessionContinue` and `Configure`, `Items` for
/// `Read`, `Sessions` for `Sessions`, `SessionItems` for `SessionRead`,
/// `Export` for `Export`, `Files` for `Files`, `Commands` for `Commands`,
/// `Draft` for `DraftGet`, `DraftSaved` for `DraftSet`, `Image` for
/// `ImageAdd`, `Thumb` for `ImageThumb`, `Ok` for the rest. Threads carry
/// `from_phone` for the phone that asked; the core makes everything fit the
/// phone that asked (legacy forms, no secret asks, relative paths only).
#[derive(Clone, PartialEq, Eq)]
pub enum AgentReply {
    Ok,
    Threads(Vec<AgentThread>),
    Thread(AgentThread),
    Items(AgentItems),
    Sessions(Vec<AgentSession>),
    SessionItems(AgentSessionItems),
    Export(AgentExport),
    Files(AgentFiles),
    Commands(Vec<AgentCommand>),
    Draft(Option<AgentDraft>),
    DraftSaved(AgentDraftSaved),
    Image(AgentImage),
    /// A JPEG.
    Thumb(Vec<u8>),
    /// `Send` with a `key` the thread had already taken: nothing was sent.
    Duplicate,
    /// `Turns`, newest first.
    Turns(Vec<AgentTurn>),
    /// `DiffFiles`.
    TurnFiles(AgentTurnFiles),
    /// One page of `DiffPatch`.
    Patch(AgentPatchPage),
    /// `Search`.
    Hits(AgentHits),
    /// `RewindPreview`: the conversation part only; no file ever.
    RewindPlan(AgentRewindPlan),
    /// `Found`.
    Found(AgentFoundList),
    /// `Import`.
    Job(AgentImportJob),
    /// `GitStatus`.
    Git(AgentGit),
    /// One page of `GitPatch`.
    GitPatch(AgentGitPatchPage),
    /// `GitReview`.
    Review(AgentGitReviewed),
    /// A job a request started, or `GitJob`/`GitStop` read: `by` is the
    /// device that asked for it (`None`: this PC).
    GitJob {
        job: AgentJob,
        by: Option<DeviceId>,
    },
    /// `GitBranches`.
    Branches(AgentBranches),
    /// `PrView`.
    Pr(AgentPrView),
    /// `PrList`.
    PrRows(AgentPrRows),
    /// The host refused a source-control request before anything ran
    /// (its error's `data.reason`, the words of §8): for one that starts a
    /// job the phone gets a job that already failed with it.
    Refused(AgentJobError),
}

/// Never prints a thumbnail's bytes.
impl std::fmt::Debug for AgentReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ok => f.write_str("Ok"),
            Self::Threads(threads) => f.debug_tuple("Threads").field(threads).finish(),
            Self::Thread(thread) => f.debug_tuple("Thread").field(thread).finish(),
            Self::Items(items) => f.debug_tuple("Items").field(items).finish(),
            Self::Sessions(sessions) => f.debug_tuple("Sessions").field(sessions).finish(),
            Self::SessionItems(items) => f.debug_tuple("SessionItems").field(items).finish(),
            Self::Export(export) => f.debug_tuple("Export").field(export).finish(),
            Self::Files(files) => f.debug_tuple("Files").field(&files.files.len()).finish(),
            Self::Commands(commands) => f.debug_tuple("Commands").field(&commands.len()).finish(),
            Self::Draft(draft) => f.debug_tuple("Draft").field(draft).finish(),
            Self::DraftSaved(saved) => f.debug_tuple("DraftSaved").field(saved).finish(),
            Self::Image(image) => f.debug_tuple("Image").field(image).finish(),
            Self::Thumb(bytes) => f.debug_struct("Thumb").field("bytes", &bytes.len()).finish(),
            Self::Duplicate => f.write_str("Duplicate"),
            Self::Turns(turns) => f.debug_tuple("Turns").field(&turns.len()).finish(),
            Self::TurnFiles(files) => f.debug_tuple("TurnFiles").field(&files.files.len()).finish(),
            Self::Patch(page) => f.debug_tuple("Patch").field(page).finish(),
            Self::Hits(hits) => f.debug_tuple("Hits").field(&hits.hits.len()).finish(),
            Self::RewindPlan(plan) => f.debug_tuple("RewindPlan").field(plan).finish(),
            Self::Found(found) => f.debug_tuple("Found").field(&found.found.len()).finish(),
            Self::Job(job) => f.debug_tuple("Job").field(job).finish(),
            Self::Git(status) => f.debug_tuple("Git").field(status).finish(),
            Self::GitPatch(page) => f.debug_tuple("GitPatch").field(page).finish(),
            Self::Review(review) => f.debug_tuple("Review").field(review).finish(),
            Self::GitJob { job, by } => {
                f.debug_struct("GitJob").field("job", job).field("by", &by.map(|d| d.short())).finish()
            }
            Self::Branches(branches) => f.debug_tuple("Branches").field(&branches.branches.len()).finish(),
            Self::Pr(view) => f.debug_tuple("Pr").field(view).finish(),
            Self::PrRows(rows) => f.debug_tuple("PrRows").field(&rows.prs.len()).finish(),
            Self::Refused(error) => f.debug_tuple("Refused").field(error).finish(),
        }
    }
}

/// A thread or a session as Markdown (`docs/protocol/agents.md` §4).
#[derive(Clone, PartialEq, Eq)]
pub struct AgentExport {
    /// At most [`agent::MAX_EXPORT_BYTES`], without control characters
    /// other than newline and tab.
    pub text: String,
    /// Items were left out to fit.
    pub truncated: bool,
    /// A name to save it as: `[A-Za-z0-9._-]`, not starting with a dot.
    pub file_name: String,
}

/// Never prints the text or the name (which carries the title).
impl std::fmt::Debug for AgentExport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentExport")
            .field("bytes", &self.text.len())
            .field("truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}

impl AgentExport {
    /// Within its limits: the text cleaned and cut (then `truncated`), the
    /// name made a file name.
    pub fn sanitized(self) -> Self {
        let (text, cut) = clean_agent_export(self.text);
        Self { text, truncated: self.truncated || cut, file_name: clean_export_file_name(&self.file_name) }
    }
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

    pub(crate) fn reply(self, message: &str) -> Envelope {
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

/// One session's pacing: when its last change went, and the newest one
/// waiting for its turn.
#[derive(Debug, Default)]
struct SessionGate {
    last: Option<Instant>,
    pending: Option<AgentSessionEvent>,
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
    /// PC: the session each phone shows, for this connection.
    session_watching: HashMap<DeviceId, String>,
    /// PC: phones that already caused the one-time sessions prompt.
    prompted_sessions: HashSet<DeviceId>,
    /// PC: `agent.session` pacing per session id.
    session_gates: HashMap<String, SessionGate>,
    /// Phone: the last catalog each PC sent, if it had agents, and what that
    /// PC offered when it was last told.
    peer_catalogs: HashMap<DeviceId, (AgentCatalog, PeerOffers)>,
    /// PC: reasoning items seen lately, as (thread, item id).
    reasoning: VecDeque<(String, String)>,
    /// PC: tokens of secret questions sent lately (a phone may not answer them).
    secrets: VecDeque<String>,
    /// PC: phones with an image upload running (one at a time each).
    uploading: HashSet<DeviceId>,
    /// PC: the form of the catalog each phone got last: with §5 or not.
    catalog_forms: HashMap<DeviceId, bool>,
    /// Phone: PCs whose held messages are being sent, and whether to look
    /// once more when that's done.
    flushing: HashMap<DeviceId, bool>,
}

/// Phone: what a PC offers on top of `agent.host`, as it last said.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PeerOffers {
    /// `agent.say_why`.
    pub say_why: bool,
    /// `agent.control`.
    pub control: bool,
}

impl Inner {
    /// Remembers that `item` of `thread` is a reasoning item.
    fn note_reasoning(&mut self, thread: &str, item: &AgentItem) {
        if !item.is_reasoning() || self.is_reasoning(thread, &item.id) {
            return;
        }
        if self.reasoning.len() == REASONING_KEPT {
            self.reasoning.pop_front();
        }
        self.reasoning.push_back((thread.to_owned(), item.id.clone()));
    }

    fn is_reasoning(&self, thread: &str, item_id: &str) -> bool {
        self.reasoning.iter().any(|(t, i)| t == thread && i == item_id)
    }

    /// Remembers that the question `token` is secret.
    fn note_secret(&mut self, token: &str) {
        if self.secrets.iter().any(|t| t == token) {
            return;
        }
        if self.secrets.len() == SECRETS_KEPT {
            self.secrets.pop_front();
        }
        self.secrets.push_back(token.to_owned());
    }

    fn is_secret(&self, token: &str) -> bool {
        self.secrets.iter().any(|t| t == token)
    }
}

/// PC: `peer`'s image upload, while it runs; it ends when this goes.
struct Upload {
    shared: Arc<Shared>,
    peer: DeviceId,
}

impl Upload {
    /// `None` while `peer` has one running.
    fn begin(shared: &Arc<Shared>, peer: DeviceId) -> Option<Self> {
        shared.agents.lock().uploading.insert(peer).then(|| Self { shared: shared.clone(), peer })
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        self.shared.agents.lock().uploading.remove(&self.peer);
    }
}

/// Phone: the saved threads, opened the first time they're needed.
#[derive(Default)]
enum CacheSlot {
    #[default]
    Unopened,
    Open(Arc<AgentCache>),
    /// It couldn't be opened; not tried again this run.
    Failed,
}

impl std::fmt::Debug for CacheSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unopened => "Unopened",
            Self::Open(_) => "Open",
            Self::Failed => "Failed",
        })
    }
}

/// Per-node agents state, on both sides.
#[derive(Debug, Default)]
pub(crate) struct Agents {
    inner: Mutex<Inner>,
    cache: Mutex<CacheSlot>,
    /// §7: the PC's presence, phones' views.
    pub(crate) presence: crate::agent_presence::Presence,
    /// §8: the reviews phones read before they commit.
    pub(crate) git: crate::agent_git::Reviews,
}

impl Agents {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A session with `peer` ended: its watches and pending text go, its
    /// presence and view (§7), and on a phone the PC's catalog (it may come
    /// back different). True when it
    /// watched a session the platform should stop sending
    /// ([`unwatch_on_platform`]).
    #[must_use]
    pub fn session_ended(&self, peer: &DeviceId) -> bool {
        self.presence.session_ended(peer);
        let mut inner = self.lock();
        inner.watching.remove(peer);
        inner.gates.remove(peer);
        inner.peer_catalogs.remove(peer);
        inner.catalog_forms.remove(peer);
        inner.session_watching.remove(peer).is_some()
    }

    /// `peer` was unpaired: on a phone nothing of it stays saved. True as
    /// for [`session_ended`](Self::session_ended).
    #[must_use]
    pub fn remove_peer(&self, peer: &DeviceId) -> bool {
        let watched = self.session_ended(peer);
        self.presence.remove_peer(peer);
        self.git.forget(peer);
        {
            let mut inner = self.lock();
            inner.prompted.remove(peer);
            inner.prompted_sessions.remove(peer);
            inner.flushing.remove(peer);
        }
        let cache = match &*self.cache.lock().unwrap_or_else(|e| e.into_inner()) {
            CacheSlot::Open(cache) => Some(cache.clone()),
            _ => None,
        };
        if let Some(cache) = cache
            && let Err(e) = cache.drop_pc(peer)
        {
            tracing::warn!(error = %e, "an unpaired PC's saved threads stayed");
        }
        watched
    }

    /// Phone: the saved threads, opened with `open` the first time.
    pub(crate) fn cache_or_open(&self, open: impl FnOnce() -> Result<AgentCache>) -> Option<Arc<AgentCache>> {
        let mut slot = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match &*slot {
            CacheSlot::Open(cache) => return Some(cache.clone()),
            CacheSlot::Failed => return None,
            CacheSlot::Unopened => {}
        }
        match open() {
            Ok(cache) => {
                let cache = Arc::new(cache);
                *slot = CacheSlot::Open(cache.clone());
                Some(cache)
            }
            Err(e) => {
                tracing::warn!(error = %e, "the saved agent threads can't be opened");
                *slot = CacheSlot::Failed;
                None
            }
        }
    }

    /// Phone: whether a flush of `peer`'s held messages may start now; while
    /// one runs, it's asked to look once more instead.
    pub(crate) fn begin_flush(&self, peer: DeviceId) -> bool {
        let mut inner = self.lock();
        match inner.flushing.get_mut(&peer) {
            Some(again) => {
                *again = true;
                false
            }
            None => {
                inner.flushing.insert(peer, false);
                true
            }
        }
    }

    /// Phone: a flush of `peer` ended; whether it should run once more.
    pub(crate) fn end_flush(&self, peer: DeviceId) -> bool {
        let mut inner = self.lock();
        match inner.flushing.get_mut(&peer) {
            Some(again) if *again => {
                *again = false;
                true
            }
            _ => {
                inner.flushing.remove(&peer);
                false
            }
        }
    }

    /// PC: `peer` stops watching its session; true when it watched one.
    fn unwatch_session(&self, peer: &DeviceId) -> bool {
        self.lock().session_watching.remove(peer).is_some()
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
        self.lock().peer_catalogs.get(peer).map(|(catalog, _)| catalog.clone())
    }

    /// PC: whether the question `token` is secret, as far as it was told.
    fn is_secret(&self, token: &str) -> bool {
        self.lock().is_secret(token)
    }
}

impl Shared {
    fn offers_agents(&self) -> bool {
        self.we_offer(agent::HOST)
    }

    /// Whether this PC sends `peer` anything from the service.
    fn agents_reach(&self, peer: &DeviceId) -> bool {
        self.toggle_on(peer, TOGGLE) && self.peer_offers(peer, agent::REMOTE)
    }

    /// Whether the paired `peer` offers `cap`, as it last said.
    pub(crate) fn peer_offers(&self, peer: &DeviceId, cap: &str) -> bool {
        self.store.get_peer(peer).ok().flatten().is_some_and(|p| p.caps.contains(cap))
    }

    /// Whether this PC offers §5 (`agent.control`).
    fn offers_control(&self) -> bool {
        self.we_offer(agent::CONTROL)
    }

    /// Whether this PC and `peer` both offer `agent.control`: only then does
    /// either side use §5.
    pub(crate) fn control_with(&self, peer: &DeviceId) -> bool {
        self.offers_control() && self.peer_offers(peer, agent::CONTROL)
    }

    /// The live sessions this PC sends agent pushes to.
    pub(crate) fn agent_recipients(&self) -> Vec<Arc<Session>> {
        self.agent_recipient_caps().into_iter().map(|r| r.session).collect()
    }

    /// [`agent_recipients`](Self::agent_recipients), each with what that phone takes.
    fn agent_recipient_caps(&self) -> Vec<Recipient> {
        if !self.offers_agents() {
            return Vec::new();
        }
        let sessions = self.live_sessions().into_iter().filter(|s| self.toggle_on(&s.peer, TOGGLE)).collect();
        self.with_caps(sessions, agent::REMOTE)
    }

    /// The phones that get §5's pushes (questions, drafts).
    fn control_recipients(&self) -> Vec<Arc<Session>> {
        self.agent_recipient_caps().into_iter().filter(|r| r.control).map(|r| r.session).collect()
    }

    /// `sessions` whose phone offers `cap`, each with what it takes.
    fn with_caps(&self, sessions: Vec<Arc<Session>>, cap: &str) -> Vec<Recipient> {
        let control = self.offers_control();
        sessions
            .into_iter()
            .filter_map(|s| {
                let caps = self.store.get_peer(&s.peer).ok().flatten()?.caps;
                caps.contains(cap).then(|| Recipient {
                    items2: caps.contains(agent::ITEMS2),
                    control: control && caps.contains(agent::CONTROL),
                    session: s,
                })
            })
            .collect()
    }

    /// The live sessions this PC sends §3's pushes to: both toggles on, and
    /// the phone shows sessions.
    pub(crate) fn session_recipients(&self) -> Vec<Arc<Session>> {
        self.session_recipient_caps().into_iter().map(|r| r.session).collect()
    }

    /// [`session_recipients`](Self::session_recipients), each with what that phone takes.
    fn session_recipient_caps(&self) -> Vec<Recipient> {
        if !self.we_offer(agent::SESSIONS) {
            return Vec::new();
        }
        let sessions = self
            .live_sessions()
            .into_iter()
            .filter(|s| self.toggle_on(&s.peer, TOGGLE) && self.toggle_on(&s.peer, SESSIONS_TOGGLE))
            .collect();
        self.with_caps(sessions, agent::FOLLOW)
    }
}

/// A phone the PC sends agent pushes to, and what it takes.
struct Recipient {
    session: Arc<Session>,
    /// `agent.items2`.
    items2: bool,
    /// `agent.control`, while this PC offers it too.
    control: bool,
}

/// PC: tells the platform that `peer` watches no session any more (its
/// connection ended, a toggle went off, or the session left).
pub(crate) fn unwatch_on_platform(shared: &Shared, peer: DeviceId) {
    let platform = shared.platform.clone();
    shared.runtime.spawn(async move {
        let request = AgentRequest::SessionWatch { session: None };
        let task = tokio::task::spawn_blocking(move || platform.agent_request(&peer, request));
        let _ = tokio::time::timeout(PLATFORM_LIMIT, task).await;
    });
}

/// The catalog as a phone takes it: in full with `agent.control`, its
/// legacy form otherwise; an empty one when there's none. `push`: that
/// phone's `agent_push` (§8).
fn catalog_env(catalog: Option<&AgentCatalog>, control: bool, push: bool) -> Option<Envelope> {
    let mut catalog = catalog.cloned().unwrap_or_default();
    catalog.push = push && !catalog.agents.is_empty();
    let catalog = if control { catalog } else { catalog.for_legacy() };
    Envelope::new(types::AGENT_CATALOG, &catalog).ok()
}

/// PC: sends this PC's catalog to `session` when it may have it (on connect,
/// and when the toggle is turned on).
pub(crate) async fn send_catalog(shared: &Arc<Shared>, session: &Arc<Session>) {
    if !shared.offers_agents() || !shared.agents_reach(&session.peer) {
        return;
    }
    let control = shared.control_with(&session.peer);
    let push = crate::agent_git::may_push(shared, &session.peer);
    let env = {
        let mut inner = shared.agents.lock();
        inner.catalog_forms.insert(session.peer, control);
        catalog_env(inner.catalog.as_ref(), control, push)
    };
    if let Some(env) = env {
        let _ = session.send(env).await;
    }
    crate::agent_presence::send_last(shared, session).await;
}

/// PC: sends the catalog again to the phones (all, or `only`) whose form of
/// it, with §5 or without, changed since they got it: after this PC's or a
/// phone's capabilities changed.
pub(crate) async fn refresh_catalogs(shared: &Arc<Shared>, only: Option<DeviceId>) {
    if !shared.offers_agents() {
        return;
    }
    for recipient in shared.agent_recipient_caps() {
        let peer = recipient.session.peer;
        if only.is_some_and(|only| only != peer) {
            continue;
        }
        let sent = shared.agents.lock().catalog_forms.get(&peer).copied();
        if sent.is_some_and(|sent| sent != recipient.control) {
            send_catalog(shared, &recipient.session).await;
        }
    }
}

/// PC: the `agents` toggle for `peer` was turned on or off.
pub(crate) fn toggled(shared: &Arc<Shared>, peer: DeviceId, on: bool) {
    if !on {
        shared.agents.git.forget(&peer);
    }
    if !on && shared.agents.presence.forget_view(&peer) {
        shared.emit(NodeEvent::AgentViewing { device: peer, view: None });
    }
    let Some(session) = shared.session(&peer) else {
        if !on && shared.agents.session_ended(&peer) {
            unwatch_on_platform(shared, peer);
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
            if shared.agents.unwatch_session(&peer) {
                unwatch_on_platform(&shared, peer);
            }
            // The last word: nothing can be started or followed from here now.
            if shared.offers_agents()
                && shared.peer_offers(&peer, agent::REMOTE)
                && let Some(env) = catalog_env(None, false, false)
            {
                let _ = session.send(env).await;
            }
        }
    });
}

/// PC: the `agent_sessions` toggle for `peer` was turned on or off. Off, its
/// watched session goes; nothing is sent (the phone asks again).
pub(crate) fn sessions_toggled(shared: &Arc<Shared>, peer: DeviceId, on: bool) {
    if !on && shared.agents.unwatch_session(&peer) {
        unwatch_on_platform(shared, peer);
    }
    if !on && shared.agents.presence.forget_session_view(&peer) {
        shared.emit(NodeEvent::AgentViewing { device: peer, view: None });
    }
}

/// PC: passes a session's change or removal to the phones that get §3
/// (`docs/protocol/agents.md` §3): a change at most once per 2 s per
/// session (the newest waits for its turn), a removal at once. A phone
/// watching a removed session stops.
pub(crate) fn send_session(shared: &Arc<Shared>, event: AgentSessionEvent) -> Result<()> {
    let event = event.sanitized().ok_or_else(|| Error::Protocol("invalid agent session".into()))?;
    let Some(id) = event.id().map(str::to_owned) else {
        return Err(Error::Protocol("invalid agent session".into()));
    };
    let recipients = shared.session_recipients();
    let now = Instant::now();
    let mut inner = shared.agents.lock();
    if event.removed.is_some() {
        inner.session_gates.remove(&id);
        let watchers: Vec<DeviceId> = inner
            .session_watching
            .iter()
            .filter(|(_, watched)| **watched == id)
            .map(|(peer, _)| *peer)
            .collect();
        for peer in &watchers {
            inner.session_watching.remove(peer);
        }
        broadcast_session(&recipients, &event);
        drop(inner);
        for peer in watchers {
            unwatch_on_platform(shared, peer);
        }
        return Ok(());
    }
    if inner.session_gates.len() > SESSION_GATES_KEPT {
        inner.session_gates.retain(|_, gate| {
            gate.pending.is_some()
                || gate.last.is_some_and(|last| now.duration_since(last) < SESSION_INTERVAL)
        });
    }
    let gate = inner.session_gates.entry(id.clone()).or_default();
    if gate.pending.is_none() && gate.last.is_none_or(|last| now.duration_since(last) >= SESSION_INTERVAL) {
        gate.last = Some(now);
        broadcast_session(&recipients, &event);
        return Ok(());
    }
    gate.pending = Some(event);
    if gate.scheduled {
        return Ok(());
    }
    gate.scheduled = true;
    let wait = (gate.last.unwrap_or(now) + SESSION_INTERVAL).saturating_duration_since(now);
    drop(inner);
    let shared = shared.clone();
    shared.runtime.clone().spawn(async move {
        tokio::time::sleep(wait).await;
        let recipients = shared.session_recipients();
        let mut inner = shared.agents.lock();
        let Some(gate) = inner.session_gates.get_mut(&id) else { return };
        gate.scheduled = false;
        let Some(pending) = gate.pending.take() else { return };
        gate.last = Some(Instant::now());
        broadcast_session(&recipients, &pending);
    });
    Ok(())
}

fn broadcast_session(recipients: &[Arc<Session>], event: &AgentSessionEvent) {
    if let Ok(env) = Envelope::new(types::AGENT_SESSION, event) {
        for session in recipients {
            let _ = session.send_now(env.clone());
        }
    }
}

/// PC: passes an item of a session to the phones watching it.
pub(crate) fn send_session_item(shared: &Arc<Shared>, session: String, item: AgentItem) -> Result<()> {
    let body = AgentSessionItem { session, item }
        .sanitized()
        .ok_or_else(|| Error::Protocol("invalid agent session item".into()))?;
    let recipients = shared.session_recipient_caps();
    if recipients.is_empty() {
        return Ok(());
    }
    let inner = shared.agents.lock();
    for recipient in recipients {
        if inner.session_watching.get(&recipient.session.peer) == Some(&body.session) {
            let item = body.item.clone().for_phone(recipient.items2, recipient.control);
            let env = Envelope::new(
                types::AGENT_SESSION_ITEM,
                &AgentSessionItem { session: body.session.clone(), item },
            )?;
            let _ = recipient.session.send_now(env);
        }
    }
    Ok(())
}

/// PC: replaces this PC's catalog (`None`: the host is gone) and sends it to
/// the phones that may have it when it changed. A phone may start
/// supervised threads, and plan-mode ones (an agent that can plan, a phone
/// with `agent.control`); what §5 added goes only to such a phone.
pub(crate) fn set_catalog(shared: &Arc<Shared>, catalog: Option<AgentCatalog>) {
    let catalog = catalog.map(AgentCatalog::sanitized).map(|mut c| {
        let plan = c.agents.iter().any(|a| a.can(agent::can::PLAN));
        c.modes = match (c.agents.is_empty(), plan) {
            (true, _) => Vec::new(),
            (false, false) => vec![agent::mode::SUPERVISED.to_owned()],
            (false, true) => vec![agent::mode::SUPERVISED.to_owned(), agent::mode::PLAN.to_owned()],
        };
        c
    });
    let recipients = shared.agent_recipient_caps();
    let mut inner = shared.agents.lock();
    if inner.catalog == catalog {
        return;
    }
    inner.catalog = catalog;
    for recipient in recipients {
        inner.catalog_forms.insert(recipient.session.peer, recipient.control);
        let push = crate::agent_git::may_push(shared, &recipient.session.peer);
        if let Some(env) = catalog_env(inner.catalog.as_ref(), recipient.control, push) {
            let _ = recipient.session.send_now(env);
        }
    }
}

/// `thread` as a phone takes it: its legacy form without `agent.control`.
pub(crate) fn thread_for(thread: AgentThread, control: bool) -> AgentThread {
    if control { thread } else { thread.for_legacy() }
}

/// `event` for one phone: `from_phone` says whether that phone started the
/// thread; the thread and the item as [`thread_for`] and
/// [`AgentItem::for_phone`] make them.
fn for_phone(
    event: &AgentEvent,
    peer: &DeviceId,
    started_by: Option<&DeviceId>,
    recipient: &Recipient,
) -> AgentEvent {
    let mut event = event.clone();
    if let Some(mut state) = event.state.take() {
        state.from_phone = started_by == Some(peer);
        event.state = Some(thread_for(state, recipient.control));
    }
    event.item = event.item.map(|item| item.for_phone(recipient.items2, recipient.control));
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
        let _ = session.send_lossy(env);
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
    let recipients = shared.agent_recipient_caps();
    let mut schedule = Vec::new();
    {
        let mut inner = shared.agents.lock();
        if let Some(item) = &event.item {
            inner.note_reasoning(&event.thread, item);
        }
        let reasoning_delta = event.kind == agent::event::DELTA
            && event.item_id.as_deref().is_some_and(|id| inner.is_reasoning(&event.thread, id));
        for recipient in &recipients {
            let (session, peer, items2) = (&recipient.session, recipient.session.peer, recipient.items2);
            let watches = inner.watching.get(&peer).is_some_and(|t| *t == event.thread);
            match event.kind.as_str() {
                agent::event::DELTA => {
                    if !watches || (reasoning_delta && !items2) {
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
                                let _ = session.send_lossy(env);
                            }
                            gate.last = Some(now);
                        }
                    } else if gate.last.is_none_or(|last| now.duration_since(last) >= DELTA_INTERVAL) {
                        gate.last = Some(now);
                        if let Some(env) = event_env(&event) {
                            let _ = session.send_lossy(env);
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
                    if let Some(env) = event_env(&for_phone(&event, &peer, None, recipient)) {
                        let _ = session.send_now(env);
                    }
                }
                _ => {
                    flush_for(&mut inner, session, &event.thread);
                    if let Some(env) = event_env(&for_phone(&event, &peer, started_by.as_ref(), recipient)) {
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
                let _ = session.send_lossy(env);
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

/// PC: sends a question to every phone that gets §5 (`docs/protocol/agents.md`
/// §5.2). A secret question goes without its asks, and the core refuses a
/// phone's reply to it from then on.
pub(crate) fn send_question(shared: &Arc<Shared>, question: AgentQuestion) -> Result<()> {
    let question = question.sanitized().ok_or_else(|| Error::Protocol("invalid agent question".into()))?;
    if question.secret {
        shared.agents.lock().note_secret(&question.token);
    }
    let env = Envelope::new(types::AGENT_QUESTION, &question.for_phone())?;
    for session in shared.control_recipients() {
        let _ = session.send_now(env.clone());
    }
    Ok(())
}

/// PC: tells every phone that gets §5 that a question ended.
pub(crate) fn send_questioned(shared: &Arc<Shared>, questioned: AgentQuestioned) -> Result<()> {
    let questioned =
        questioned.sanitized().ok_or_else(|| Error::Protocol("invalid agent.questioned".into()))?;
    let env = Envelope::new(types::AGENT_QUESTIONED, &questioned)?;
    for session in shared.control_recipients() {
        let _ = session.send_now(env.clone());
    }
    Ok(())
}

/// PC: tells every phone that gets §5, but `except` (the one that changed
/// it), that a draft changed.
pub(crate) fn send_draft(
    shared: &Arc<Shared>,
    key: String,
    rev: u64,
    except: Option<DeviceId>,
) -> Result<()> {
    let body = AgentDraftChanged { key, rev };
    if !body.is_valid() {
        return Err(Error::Protocol("invalid agent.draft".into()));
    }
    let env = Envelope::new(types::AGENT_DRAFT, &body)?;
    for session in shared.control_recipients() {
        if Some(session.peer) != except {
            let _ = session.send_now(env.clone());
        }
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
pub(crate) fn decode<T: serde::de::DeserializeOwned>(
    env: &Envelope,
    valid: impl Fn(&T) -> bool,
) -> Option<T> {
    env.body::<T>().ok().filter(|body| valid(body))
}

/// Whether `t` is one of §3's requests.
fn is_session_request(t: &str) -> bool {
    matches!(
        t,
        types::AGENT_SESSIONS
            | types::AGENT_SESSION_READ
            | types::AGENT_SESSION_WATCH
            | types::AGENT_SESSION_CONTINUE
    )
}

/// PC: the refusal for a request from `peer` (of §3 when `sessions`) while
/// this PC doesn't offer it or a toggle is off; the first refusal of each
/// kind shows the PC's user a prompt.
pub(crate) fn refusal(shared: &Arc<Shared>, peer: DeviceId, sessions: bool) -> Option<Envelope> {
    let offered = if sessions { shared.we_offer(agent::SESSIONS) } else { shared.offers_agents() };
    if !offered {
        return Some(Envelope::error(ErrorCode::Unsupported, "agents are not available here"));
    }
    if !shared.toggle_on(&peer, TOGGLE) {
        let first_time = shared.agents.lock().prompted.insert(peer);
        if first_time {
            shared.emit(NodeEvent::AgentsRequested { device: peer });
        }
        return Some(Envelope::error(ErrorCode::Denied, "agents are turned off for this device"));
    }
    if sessions && !shared.toggle_on(&peer, SESSIONS_TOGGLE) {
        let first_time = shared.agents.lock().prompted_sessions.insert(peer);
        if first_time {
            shared.emit(NodeEvent::AgentSessionsRequested { device: peer });
        }
        return Some(Envelope::error(ErrorCode::Denied, "sessions are turned off for this device"));
    }
    None
}

/// Whether `t` is one of §5's requests (they need `agent.control` on both sides).
fn is_control_request(t: &str) -> bool {
    matches!(
        t,
        types::AGENT_CONFIGURE
            | types::AGENT_QUEUE
            | types::AGENT_REPLY
            | types::AGENT_IMPLEMENT
            | types::AGENT_FILES
            | types::AGENT_COMMANDS
            | types::AGENT_DRAFT_GET
            | types::AGENT_DRAFT_SET
    )
}

/// The refusal of §5 from `peer` when this PC or the phone doesn't offer
/// `agent.control`.
fn control_refusal(shared: &Shared, peer: &DeviceId) -> Option<Envelope> {
    (!shared.control_with(peer))
        .then(|| Envelope::error(ErrorCode::Unsupported, "this PC or phone can't do that yet"))
}

/// The refusal of a mode a phone asked for: `supervised`, and `plan` with
/// `agent.control`, are the only ones.
fn mode_refusal(shared: &Shared, peer: &DeviceId, mode: &str) -> Option<Envelope> {
    let allowed = mode == agent::mode::SUPERVISED || (mode == agent::mode::PLAN && shared.control_with(peer));
    (!allowed).then(|| {
        Envelope::error(ErrorCode::Denied, "a phone asks before edits and commands, or plans; nothing else")
    })
}

/// PC: what to answer a phone's `agent.*` request, checked in the order of
/// `docs/protocol/agents.md` and contract §6.2 (0.1.14 contract §A4.2 for
/// §3; phase 2 contract §Q5 for §5).
/// A PC's answer to a phone's request: a list that may go in parts, or one envelope.
enum PcAnswer {
    Plain(Envelope),
    Threads(AgentThreads),
    Items(Box<AgentItems>),
    Sessions(AgentSessions),
    SessionItems(Box<AgentSessionItems>),
    Files(AgentFiles),
    Commands(AgentCommands),
}

impl From<Envelope> for PcAnswer {
    fn from(env: Envelope) -> Self {
        PcAnswer::Plain(env)
    }
}

async fn answer(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) -> PcAnswer {
    answer_envelope(shared, peer, env).await
}

async fn answer_envelope(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) -> PcAnswer {
    if let Some(refused) = refusal(shared, peer, is_session_request(&env.t)) {
        return refused.into();
    }
    if is_control_request(&env.t)
        && let Some(refused) = control_refusal(shared, &peer)
    {
        return refused.into();
    }
    let bad = || Envelope::error(ErrorCode::BadMessage, "invalid agent request");
    let request = match env.t.as_str() {
        types::AGENT_THREADS => {
            let Some(list) = decode::<AgentThreadsRequest>(env, AgentThreadsRequest::is_valid) else {
                return bad().into();
            };
            if list.is_paged() {
                AgentRequest::ThreadsPage { show: list.show, before_ms: list.before_ms }
            } else {
                AgentRequest::Threads
            }
        }
        types::AGENT_START => {
            let Some(start) = decode::<AgentStart>(env, AgentStart::is_valid) else { return bad().into() };
            if let Some(refused) = mode_refusal(shared, &peer, &start.mode) {
                return refused.into();
            }
            if start.uses_control()
                && let Some(refused) = control_refusal(shared, &peer)
            {
                return refused.into();
            }
            AgentRequest::Start {
                agent: start.agent,
                folder: start.folder,
                prompt: clean_typed(start.prompt),
                mode: start.mode,
                model: start.model,
                effort: start.effort,
                images: start.images,
                custom_model: start.custom_model,
                key: start.key,
            }
        }
        types::AGENT_SEND => {
            let Some(send) = decode::<AgentSend>(env, AgentSend::is_valid) else { return bad().into() };
            if send.uses_control()
                && let Some(refused) = control_refusal(shared, &peer)
            {
                return refused.into();
            }
            AgentRequest::Send {
                thread: send.thread,
                text: clean_typed(send.text),
                dispatch: send.dispatch,
                images: send.images,
                key: send.key,
            }
        }
        types::AGENT_CANCEL => {
            let Some(cancel) = decode::<AgentCancel>(env, AgentCancel::is_valid) else { return bad().into() };
            AgentRequest::Cancel { thread: cancel.thread }
        }
        types::AGENT_READ => {
            let Some(read) = decode::<AgentRead>(env, AgentRead::is_valid) else { return bad().into() };
            let limit = read.limit();
            AgentRequest::Read { thread: read.thread, before: read.before, limit }
        }
        types::AGENT_ANSWER => {
            let Some(answer) = decode::<AgentAnswer>(env, AgentAnswer::is_valid) else { return bad().into() };
            if answer.scope.as_deref().is_some_and(|s| s != agent::scope::ONCE) {
                return Envelope::error(ErrorCode::Denied, "only this request can be answered from a phone")
                    .into();
            }
            let message = match answer.message.as_deref() {
                None => None,
                Some(_) if !shared.we_offer(agent::SAY_WHY) => {
                    return Envelope::error(ErrorCode::Unsupported, "this PC can't pass a message on").into();
                }
                Some(message) => match clean_deny_message(message) {
                    Some(message) => Some(message),
                    None => return bad().into(),
                },
            };
            let allow = answer.allow();
            AgentRequest::Answer { token: answer.token, allow, message }
        }
        types::AGENT_WATCH => {
            let Some(watch) = decode::<AgentWatch>(env, AgentWatch::is_valid) else { return bad().into() };
            shared.agents.watch(peer, watch.thread);
            return Envelope::empty(types::OK).into();
        }
        types::AGENT_SESSIONS => AgentRequest::Sessions,
        types::AGENT_SESSION_READ => {
            let Some(read) = decode::<AgentSessionRead>(env, AgentSessionRead::is_valid) else {
                return bad().into();
            };
            let limit = read.limit();
            AgentRequest::SessionRead { session: read.session, before: read.before, limit }
        }
        types::AGENT_SESSION_WATCH => {
            let Some(watch) = decode::<AgentSessionWatch>(env, AgentSessionWatch::is_valid) else {
                return bad().into();
            };
            AgentRequest::SessionWatch { session: watch.session }
        }
        types::AGENT_SESSION_CONTINUE => {
            let Some(cont) = decode::<AgentSessionContinue>(env, AgentSessionContinue::is_valid) else {
                return bad().into();
            };
            AgentRequest::SessionContinue { session: cont.session }
        }
        types::AGENT_CONFIGURE => {
            let Some(c) = decode::<AgentConfigure>(env, AgentConfigure::is_valid) else {
                return bad().into();
            };
            if let Some(mode) = c.mode.as_deref()
                && let Some(refused) = mode_refusal(shared, &peer, mode)
            {
                return refused.into();
            }
            AgentRequest::Configure {
                thread: c.thread,
                model: c.model,
                effort: c.effort,
                mode: c.mode,
                custom_model: c.custom_model,
            }
        }
        types::AGENT_QUEUE => {
            let Some(change) = decode::<AgentQueueChange>(env, AgentQueueChange::is_valid) else {
                return bad().into();
            };
            let action = match (change.op.as_str(), change.text) {
                (agent::queue_op::EDIT, Some(text)) => QueueAction::Edit(clean_typed(text)),
                (agent::queue_op::REMOVE, None) => QueueAction::Remove,
                (agent::queue_op::SEND_NOW, None) => QueueAction::SendNow,
                _ => return bad().into(),
            };
            AgentRequest::Queue { thread: change.thread, id: change.id, action }
        }
        types::AGENT_REPLY => {
            let Some(reply) = decode::<AgentQuestionReply>(env, AgentQuestionReply::is_valid) else {
                return bad().into();
            };
            if shared.agents.is_secret(&reply.token) {
                return Envelope::error(ErrorCode::Denied, "answer this one on the PC").into();
            }
            if reply.dismiss {
                AgentRequest::QuestionDismiss { token: reply.token }
            } else {
                AgentRequest::QuestionAnswer { token: reply.token, answers: reply.answers }
            }
        }
        types::AGENT_IMPLEMENT => {
            let Some(implement) = decode::<AgentImplement>(env, AgentImplement::is_valid) else {
                return bad().into();
            };
            AgentRequest::Implement { thread: implement.thread, item: implement.item }
        }
        types::AGENT_FILES => {
            let Some(files) = decode::<AgentFilesRequest>(env, AgentFilesRequest::is_valid) else {
                return bad().into();
            };
            let limit = files.limit();
            AgentRequest::Files { thread: files.thread, folder: files.folder, query: files.query, limit }
        }
        types::AGENT_COMMANDS => {
            let Some(c) = decode::<AgentCommandsRequest>(env, AgentCommandsRequest::is_valid) else {
                return bad().into();
            };
            AgentRequest::Commands { agent: c.agent, folder: c.folder }
        }
        types::AGENT_DRAFT_GET => {
            let Some(get) = decode::<AgentDraftRequest>(env, AgentDraftRequest::is_valid) else {
                return bad().into();
            };
            AgentRequest::DraftGet { key: get.key }
        }
        types::AGENT_DRAFT_SET => {
            let Some(set) = decode::<AgentDraftSet>(env, AgentDraftSet::is_valid) else {
                return bad().into();
            };
            AgentRequest::DraftSet {
                key: set.key,
                text: clean_typed(set.text),
                images: set.images,
                rev: set.rev,
            }
        }
        _ => return Envelope::error(ErrorCode::Unsupported, "unknown agent request").into(),
    };
    let expects = match &request {
        AgentRequest::Threads | AgentRequest::ThreadsPage { .. } => types::AGENT_THREADS,
        AgentRequest::Start { .. }
        | AgentRequest::SessionContinue { .. }
        | AgentRequest::Configure { .. } => types::AGENT_THREAD,
        AgentRequest::Read { .. } => types::OK,
        AgentRequest::Sessions => types::AGENT_SESSIONS,
        AgentRequest::SessionRead { .. } => types::OK,
        AgentRequest::Files { .. } => types::AGENT_FILES,
        AgentRequest::Commands { .. } => types::AGENT_COMMANDS,
        AgentRequest::DraftGet { .. } => types::AGENT_DRAFT_GET,
        AgentRequest::DraftSet { .. } => types::AGENT_DRAFT_SET,
        _ => types::OK,
    };
    let watching = match &request {
        AgentRequest::SessionWatch { session } => Some(session.clone()),
        _ => None,
    };
    let items2 = shared.peer_offers(&peer, agent::ITEMS2);
    let control = shared.control_with(&peer);
    let reply = ask_platform(shared, peer, request).await;
    let built = match reply {
        Ok(AgentReply::Ok) if expects == types::OK => Ok(Envelope::empty(types::OK).into()),
        Ok(AgentReply::Duplicate) if env.t == types::AGENT_SEND => {
            Envelope::new(types::OK, &AgentSent { duplicate: true }).map(PcAnswer::from)
        }
        Ok(AgentReply::Threads(threads)) if expects == types::AGENT_THREADS => {
            let threads = threads.into_iter().map(|t| thread_for(t, control)).collect();
            Ok(PcAnswer::Threads(AgentThreads { threads }.sanitized()))
        }
        Ok(AgentReply::Thread(thread)) if expects == types::AGENT_THREAD => {
            match thread.sanitized().map(|t| thread_for(t, control)) {
                Some(thread) => Envelope::new(types::OK, &thread).map(PcAnswer::from),
                None => return AgentFailure::Internal.reply("the host's answer was malformed").into(),
            }
        }
        Ok(AgentReply::Items(items)) if expects == types::OK => {
            {
                let mut inner = shared.agents.lock();
                for item in &items.items {
                    inner.note_reasoning(&items.thread.id, item);
                }
                for question in items.questions.iter().filter(|q| q.secret) {
                    inner.note_secret(&question.token);
                }
            }
            match items.sanitized() {
                Some(items) => Ok(PcAnswer::Items(Box::new(items.for_phone(items2, control)))),
                None => return AgentFailure::Internal.reply("the host's answer was malformed").into(),
            }
        }
        Ok(AgentReply::Sessions(sessions)) if expects == types::AGENT_SESSIONS => {
            Ok(PcAnswer::Sessions(AgentSessions { sessions }.sanitized()))
        }
        Ok(AgentReply::SessionItems(mut items)) if expects == types::OK => {
            items.items = items.items.into_iter().map(|item| item.for_phone(items2, control)).collect();
            match items.sanitized() {
                Some(items) => Ok(PcAnswer::SessionItems(Box::new(items))),
                None => return AgentFailure::Internal.reply("the host's answer was malformed").into(),
            }
        }
        Ok(AgentReply::Files(files)) if expects == types::AGENT_FILES => {
            Ok(PcAnswer::Files(files.sanitized()))
        }
        Ok(AgentReply::Commands(commands)) if expects == types::AGENT_COMMANDS => {
            Ok(PcAnswer::Commands(AgentCommands { commands }.sanitized()))
        }
        Ok(AgentReply::Draft(draft)) if expects == types::AGENT_DRAFT_GET => {
            Envelope::new(types::OK, &AgentDraftReply { draft: draft.and_then(AgentDraft::sanitized) })
                .map(PcAnswer::from)
        }
        Ok(AgentReply::DraftSaved(saved)) if expects == types::AGENT_DRAFT_SET => {
            Envelope::new(types::OK, &saved.sanitized()).map(PcAnswer::from)
        }
        Ok(_) => return AgentFailure::Internal.reply("the host answered something else").into(),
        Err(failure) => {
            if watching.is_some() {
                // Refused (a third phone, say): this phone watches nothing now.
                shared.agents.unwatch_session(&peer);
            }
            return failure.reply(failure_message(failure)).into();
        }
    };
    if let Some(session) = watching {
        let mut inner = shared.agents.lock();
        match session {
            Some(session) => inner.session_watching.insert(peer, session),
            None => inner.session_watching.remove(&peer),
        };
    }
    built.unwrap_or_else(|_| AgentFailure::Internal.reply("failed").into())
}

/// Hands `request` to the platform on a blocking thread, giving up after
/// [`PLATFORM_LIMIT`].
pub(crate) async fn ask_platform(
    shared: &Arc<Shared>,
    peer: DeviceId,
    request: AgentRequest,
) -> std::result::Result<AgentReply, AgentFailure> {
    let platform = shared.platform.clone();
    let task = tokio::task::spawn_blocking(move || platform.agent_request(&peer, request));
    match tokio::time::timeout(PLATFORM_LIMIT, task).await {
        Ok(Ok(reply)) => reply,
        Ok(Err(_)) | Err(_) => {
            tracing::warn!(peer = %peer.short(), "the agent host didn't answer");
            Err(AgentFailure::Internal)
        }
    }
}

pub(crate) fn failure_message(failure: AgentFailure) -> &'static str {
    match failure {
        AgentFailure::Unsupported => "the agent can't run here now",
        AgentFailure::Denied => "refused",
        AgentFailure::BadMessage => "invalid agent request",
        AgentFailure::NotFound => "not found",
        AgentFailure::Busy => "busy",
        AgentFailure::Internal => "the agent host failed",
    }
}

/// PC: the export `env` asks for, if `peer` may have it: the error reply otherwise.
async fn export_for(
    shared: &Arc<Shared>,
    peer: DeviceId,
    env: &Envelope,
) -> std::result::Result<AgentExport, Envelope> {
    let body = env.expect_body::<AgentExportRequest>(types::AGENT_EXPORT).ok();
    if let Some(refused) = refusal(shared, peer, body.as_ref().is_some_and(AgentExportRequest::is_session)) {
        return Err(refused);
    }
    let Some(body) = body.filter(AgentExportRequest::is_valid) else {
        return Err(Envelope::error(ErrorCode::BadMessage, "invalid agent.export"));
    };
    let request = AgentRequest::Export { thread: body.thread, session: body.session };
    let platform = shared.platform.clone();
    let task = tokio::task::spawn_blocking(move || platform.agent_request(&peer, request));
    let reply = match tokio::time::timeout(PLATFORM_LIMIT, task).await {
        Ok(Ok(reply)) => reply,
        Ok(Err(_)) | Err(_) => {
            tracing::warn!(peer = %peer.short(), "the agent host didn't answer an export");
            Err(AgentFailure::Internal)
        }
    };
    match reply {
        Ok(AgentReply::Export(export)) => Ok(export.sanitized()),
        Ok(_) => Err(AgentFailure::Internal.reply("the host answered something else")),
        Err(failure) => Err(failure.reply(failure_message(failure))),
    }
}

/// PC: the `agent/export` stream (its header already read): one thread or
/// session as Markdown (`docs/protocol/agents.md` §4).
pub(crate) async fn serve_export(
    shared: Arc<Shared>,
    peer: DeviceId,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let Ok(Ok(Some(bytes))) = tokio::time::timeout(EXPORT_WAIT, read_frame(&mut recv)).await else {
        return;
    };
    let Ok(env) = Envelope::from_cbor(&bytes) else { return };
    let export = match export_for(&shared, peer, &env).await {
        Ok(export) => export,
        Err(reply) => {
            let _ = write_frame(&mut send, &reply.reply_to(env.id).to_cbor()).await;
            let _ = send.finish();
            return;
        }
    };
    let meta = AgentExportMeta {
        length: export.text.len() as u64,
        truncated: export.truncated,
        file_name: export.file_name,
    };
    let Ok(reply) = Envelope::new(types::AGENT_EXPORT_META, &meta).map(|e| e.reply_to(env.id)) else {
        return;
    };
    let sent = tokio::time::timeout(EXPORT_SEND, async {
        write_frame(&mut send, &reply.to_cbor()).await.is_ok()
            && send.write_all(export.text.as_bytes()).await.is_ok()
    })
    .await;
    if sent == Ok(true) {
        let _ = send.finish();
    }
}

/// Writes `reply` (an answer to request `id`) and ends the stream.
async fn end_with(send: &mut SendStream, reply: Envelope, id: Option<u64>) {
    let _ = write_frame(send, &reply.reply_to(id).to_cbor()).await;
    let _ = send.finish();
}

/// What `peer` may not do on an image stream, if anything: the toggle, and
/// `agent.control` on both sides.
fn image_refusal(shared: &Arc<Shared>, peer: DeviceId) -> Option<Envelope> {
    refusal(shared, peer, false).or_else(|| control_refusal(shared, &peer))
}

/// Reads exactly `length` bytes of an image from `recv`, then its end. `Err`
/// with the reply when more come than were announced, or the stream fails
/// or stalls (then `None`: nothing to say).
async fn read_image(recv: &mut RecvStream, length: usize) -> std::result::Result<Vec<u8>, Option<Envelope>> {
    let read = async {
        let mut data = Vec::with_capacity(length);
        loop {
            match recv.read_chunk(IMAGE_CHUNK).await {
                Ok(Some(chunk)) => {
                    if data.len() + chunk.len() > length {
                        return Err(Some(Envelope::error(
                            ErrorCode::BadMessage,
                            "more bytes than agent.image.meta said",
                        )));
                    }
                    data.extend_from_slice(&chunk);
                }
                Ok(None) if data.len() == length => return Ok(data),
                Ok(None) | Err(_) => return Err(None),
            }
        }
    };
    tokio::time::timeout(IMAGE_SEND, read).await.unwrap_or(Err(None))
}

/// PC: the `agent/image` stream (its header already read): one image a
/// phone sends for a message (`docs/protocol/agents.md` §5.3). One at a
/// time per phone; the bytes go to the platform only when exactly as many
/// came as announced.
pub(crate) async fn serve_image(
    shared: Arc<Shared>,
    peer: DeviceId,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let Ok(Ok(Some(bytes))) = tokio::time::timeout(IMAGE_WAIT, read_frame(&mut recv)).await else {
        return;
    };
    let Ok(env) = Envelope::from_cbor(&bytes) else { return };
    if let Some(refused) = image_refusal(&shared, peer) {
        return end_with(&mut send, refused, env.id).await;
    }
    let meta =
        env.expect_body::<AgentImageMeta>(types::AGENT_IMAGE_META).ok().filter(AgentImageMeta::is_valid);
    let Some((meta, length)) = meta.and_then(|m| usize::try_from(m.bytes).ok().map(|length| (m, length)))
    else {
        return end_with(
            &mut send,
            Envelope::error(ErrorCode::BadMessage, "invalid agent.image.meta"),
            env.id,
        )
        .await;
    };
    let Some(_upload) = Upload::begin(&shared, peer) else {
        return end_with(&mut send, Envelope::error(ErrorCode::Busy, "one image at a time"), env.id).await;
    };
    if write_frame(&mut send, &Envelope::empty(types::OK).reply_to(env.id).to_cbor()).await.is_err() {
        return;
    }
    let data = match read_image(&mut recv, length).await {
        Ok(data) => data,
        Err(Some(reply)) => return end_with(&mut send, reply, env.id).await,
        Err(None) => return,
    };
    let request = AgentRequest::ImageAdd { name: clean_image_name(&meta.name), bytes: data };
    let reply = match ask_platform(&shared, peer, request).await {
        Ok(AgentReply::Image(image)) => match image.sanitized() {
            Some(image) => Envelope::new(types::AGENT_IMAGE, &image),
            None => Ok(AgentFailure::Internal.reply("the host's answer was malformed")),
        },
        Ok(_) => Ok(AgentFailure::Internal.reply("the host answered something else")),
        Err(failure) => Ok(failure.reply(failure_message(failure))),
    };
    let reply = reply.unwrap_or_else(|_| AgentFailure::Internal.reply("failed"));
    end_with(&mut send, reply, env.id).await;
}

/// PC: the `agent/thumb` stream (its header already read): a JPEG of one
/// image, at most [`agent::MAX_THUMB_BYTES`] (§5.3).
pub(crate) async fn serve_thumb(
    shared: Arc<Shared>,
    peer: DeviceId,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let Ok(Ok(Some(bytes))) = tokio::time::timeout(IMAGE_WAIT, read_frame(&mut recv)).await else {
        return;
    };
    let Ok(env) = Envelope::from_cbor(&bytes) else { return };
    if let Some(refused) = image_refusal(&shared, peer) {
        return end_with(&mut send, refused, env.id).await;
    }
    let Some(thumb) =
        env.expect_body::<AgentImageThumb>(types::AGENT_IMAGE_THUMB).ok().filter(AgentImageThumb::is_valid)
    else {
        return end_with(
            &mut send,
            Envelope::error(ErrorCode::BadMessage, "invalid agent.image.thumb"),
            env.id,
        )
        .await;
    };
    let max = thumb.max();
    let jpeg = match ask_platform(&shared, peer, AgentRequest::ImageThumb { id: thumb.id, max }).await {
        Ok(AgentReply::Thumb(jpeg))
            if is_jpeg(&jpeg) && (1..=agent::MAX_THUMB_BYTES).contains(&jpeg.len()) =>
        {
            jpeg
        }
        Ok(AgentReply::Thumb(jpeg)) => {
            tracing::warn!(bytes = jpeg.len(), "a thumbnail isn't one that can be sent");
            return end_with(&mut send, AgentFailure::Internal.reply("can't send that picture"), env.id)
                .await;
        }
        Ok(_) => {
            return end_with(
                &mut send,
                AgentFailure::Internal.reply("the host answered something else"),
                env.id,
            )
            .await;
        }
        Err(failure) => return end_with(&mut send, failure.reply(failure_message(failure)), env.id).await,
    };
    let meta = AgentImageThumbMeta { length: jpeg.len() as u64 };
    let Ok(reply) = Envelope::new(types::AGENT_IMAGE_THUMB_META, &meta).map(|e| e.reply_to(env.id)) else {
        return;
    };
    let sent = tokio::time::timeout(IMAGE_SEND, async {
        write_frame(&mut send, &reply.to_cbor()).await.is_ok() && send.write_all(&jpeg).await.is_ok()
    })
    .await;
    if sent == Ok(true) {
        let _ = send.finish();
    }
}

/// Handles `agent.*` messages: requests on a PC, pushes on a phone.
pub(crate) async fn handle(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<bool> {
    let peer = session.peer;
    match env.t.as_str() {
        types::AGENT_THREADS
        | types::AGENT_START
        | types::AGENT_SEND
        | types::AGENT_CANCEL
        | types::AGENT_READ
        | types::AGENT_WATCH
        | types::AGENT_ANSWER
        | types::AGENT_SESSIONS
        | types::AGENT_SESSION_READ
        | types::AGENT_SESSION_WATCH
        | types::AGENT_SESSION_CONTINUE
        | types::AGENT_CONFIGURE
        | types::AGENT_QUEUE
        | types::AGENT_REPLY
        | types::AGENT_IMPLEMENT
        | types::AGENT_FILES
        | types::AGENT_COMMANDS
        | types::AGENT_DRAFT_GET
        | types::AGENT_DRAFT_SET => match answer(shared, peer, env).await {
            PcAnswer::Plain(reply) => rsp.send(reply).await?,
            PcAnswer::Threads(body) => rsp.paged(body).await?,
            PcAnswer::Items(body) => rsp.paged(*body).await?,
            PcAnswer::Sessions(body) => rsp.paged(body).await?,
            PcAnswer::SessionItems(body) => rsp.paged(*body).await?,
            PcAnswer::Files(body) => rsp.paged(body).await?,
            PcAnswer::Commands(body) => rsp.paged(body).await?,
        },
        t if crate::agent_history::is_request(t) => {
            crate::agent_history::handle_request(shared, peer, env, rsp).await?;
        }
        t if crate::agent_history::is_event(t) => crate::agent_history::handle_event(shared, peer, env).await,
        t if crate::agent_presence::is_message(t) => {
            crate::agent_presence::handle(shared, peer, env, rsp).await?;
        }
        t if crate::agent_git::is_request(t) => {
            crate::agent_git::handle_request(shared, peer, env, rsp).await?
        }
        t if crate::agent_git::is_event(t) => crate::agent_git::handle_event(shared, peer, env).await,
        types::AGENT_CATALOG if shared.we_offer(agent::REMOTE) => {
            let Ok(catalog) = env.body::<AgentCatalog>() else { return Ok(true) };
            if !catalog.is_valid() {
                return Ok(true);
            }
            let catalog = catalog.sanitized();
            let catalog = (!catalog.agents.is_empty()).then_some(catalog);
            crate::agent_history::catalog_arrived(shared, peer, catalog.is_some());
            if catalog.is_none() {
                shared.agents.presence.forget_pc(&peer);
            }
            let offers = peer_offers_now(shared, &peer);
            {
                let mut inner = shared.agents.lock();
                match &catalog {
                    Some(c) => inner.peer_catalogs.insert(peer, (c.clone(), offers)),
                    None => inner.peer_catalogs.remove(&peer),
                };
            }
            shared.emit(NodeEvent::AgentCatalog {
                device: peer,
                catalog,
                can_say_why: offers.say_why,
                can_control: offers.control,
            });
        }
        types::AGENT_EVENT if shared.we_offer(agent::REMOTE) => {
            if let Ok(event) = env.body::<AgentEvent>()
                && event.is_valid()
                && let Some(event) = event.sanitized()
            {
                crate::agent_history::event_arrived(shared, peer, &event).await;
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
                    message: asked.message,
                });
            }
        }
        types::AGENT_SESSION if shared.we_offer(agent::FOLLOW) => {
            if let Ok(event) = env.body::<AgentSessionEvent>()
                && event.is_valid()
                && let Some(event) = event.sanitized()
            {
                shared.emit(NodeEvent::AgentSession {
                    device: peer,
                    session: event.session,
                    removed: event.removed,
                });
            }
        }
        types::AGENT_SESSION_ITEM if shared.we_offer(agent::FOLLOW) => {
            if let Ok(item) = env.body::<AgentSessionItem>()
                && item.is_valid()
                && let Some(item) = item.sanitized()
            {
                shared.emit(NodeEvent::AgentSessionItem {
                    device: peer,
                    session: item.session,
                    item: item.item,
                });
            }
        }
        types::AGENT_QUESTION if shared.we_offer(agent::REMOTE) => {
            if let Ok(question) = env.body::<AgentQuestion>()
                && question.is_valid()
                && let Some(question) = question.sanitized()
            {
                shared.emit(NodeEvent::AgentQuestion { device: peer, question: question.for_phone() });
            }
        }
        types::AGENT_QUESTIONED if shared.we_offer(agent::REMOTE) => {
            if let Ok(done) = env.body::<AgentQuestioned>()
                && done.is_valid()
                && let Some(done) = done.sanitized()
            {
                shared.emit(NodeEvent::AgentQuestioned {
                    device: peer,
                    token: done.token,
                    thread: done.thread,
                    outcome: done.outcome,
                    by_name: done.by_name,
                });
            }
        }
        types::AGENT_DRAFT if shared.we_offer(agent::REMOTE) => {
            if let Ok(changed) = env.body::<AgentDraftChanged>()
                && changed.is_valid()
            {
                shared.emit(NodeEvent::AgentDraft { device: peer, key: changed.key, rev: changed.rev });
            }
        }
        types::AGENT_CATALOG
        | types::AGENT_EVENT
        | types::AGENT_ASK
        | types::AGENT_ASKED
        | types::AGENT_SESSION
        | types::AGENT_SESSION_ITEM
        | types::AGENT_QUESTION
        | types::AGENT_QUESTIONED
        | types::AGENT_DRAFT => {}
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
) -> Result<T> {
    request_of(shared, peer, env, agent::HOST).await
}

async fn request_paged<T>(shared: &Arc<Shared>, peer: DeviceId, env: Envelope) -> Result<T>
where
    T: g7link_protocol::lanes::Paged + serde::de::DeserializeOwned + Send + 'static,
{
    request_paged_of(shared, peer, env, agent::HOST).await
}

/// The session with `peer` for a request it must offer `cap` for.
fn session_offering(shared: &Arc<Shared>, peer: &DeviceId, cap: &str) -> Result<Arc<Session>> {
    let session = shared.session(peer).ok_or(Error::Offline)?;
    if !shared.peer_offers_cap(peer, cap) {
        return Err(Error::Unsupported);
    }
    Ok(session)
}

/// A request to `peer`, which must offer `cap`: the body of its `ok`.
pub(crate) async fn request_of<T: serde::de::DeserializeOwned>(
    shared: &Arc<Shared>,
    peer: DeviceId,
    env: Envelope,
    cap: &str,
) -> Result<T> {
    let session = session_offering(shared, &peer, cap)?;
    let reply = session.request(env, AGENT_WAIT).await?;
    reply.expect_body::<T>(types::OK).map_err(reply_error)
}

/// A request to `peer` whose answer is a list that may come in parts.
pub(crate) async fn request_paged_of<T>(
    shared: &Arc<Shared>,
    peer: DeviceId,
    env: Envelope,
    cap: &str,
) -> Result<T>
where
    T: g7link_protocol::lanes::Paged + serde::de::DeserializeOwned + Send + 'static,
{
    let session = session_offering(shared, &peer, cap)?;
    let op = g7link_protocol::lanes::rpc_of(&env.t).ok_or_else(|| Error::Internal("not an RPC".into()))?;
    let answer = session.rpc(op, env, AGENT_WAIT).await?;
    answer.last.expect(types::OK).map_err(reply_error)?;
    crate::rpc::assemble(answer).await
}

async fn request_ok(shared: &Arc<Shared>, peer: DeviceId, env: Envelope) -> Result<()> {
    request::<g7link_protocol::messages::Empty>(shared, peer, env).await.map(|_| ())
}

pub(crate) fn invalid(what: &str) -> Error {
    Error::Protocol(format!("invalid {what}"))
}

pub(crate) async fn threads(shared: &Arc<Shared>, peer: DeviceId) -> Result<Vec<AgentThread>> {
    let env = Envelope::empty(types::AGENT_THREADS);
    let threads = match request_paged::<AgentThreads>(shared, peer, env).await {
        Ok(reply) if reply.is_valid() => Ok(reply.sanitized().threads),
        Ok(_) => Err(invalid("agent.threads reply")),
        Err(e) => Err(e),
    };
    crate::agent_history::threads_arrived(shared, peer, &threads).await;
    threads
}

/// Phone: `Unsupported` without sending anything when `uses` needs §5 and
/// `peer` or this phone doesn't offer `agent.control`.
fn needs_control(shared: &Shared, peer: &DeviceId, uses: bool) -> Result<()> {
    if uses && !can_control(shared, peer) {
        return Err(Error::Unsupported);
    }
    Ok(())
}

pub(crate) async fn start(shared: &Arc<Shared>, peer: DeviceId, start: AgentStart) -> Result<AgentThread> {
    if !start.is_valid() {
        return Err(invalid("agent.start"));
    }
    needs_control(shared, &peer, start.uses_control())?;
    let env = Envelope::new(types::AGENT_START, &start)?;
    let thread: AgentThread = request(shared, peer, env).await?;
    if !thread.is_valid() {
        return Err(invalid("agent.thread reply"));
    }
    let thread = thread.sanitized().ok_or_else(|| invalid("agent.thread reply"))?;
    crate::agent_history::thread_arrived(shared, peer, &thread).await;
    Ok(thread)
}

pub(crate) async fn send(shared: &Arc<Shared>, peer: DeviceId, body: AgentSend) -> Result<()> {
    if !body.is_valid() {
        return Err(invalid("agent.send"));
    }
    needs_control(shared, &peer, body.uses_control())?;
    request_ok(shared, peer, Envelope::new(types::AGENT_SEND, &body)?).await
}

/// Phone: [`send`], answering whether the PC had already taken a message
/// with the body's `key` (and so sent nothing now).
pub(crate) async fn send_keyed(shared: &Arc<Shared>, peer: DeviceId, body: AgentSend) -> Result<bool> {
    if !body.is_valid() {
        return Err(invalid("agent.send"));
    }
    needs_control(shared, &peer, body.uses_control())?;
    let sent: AgentSent = request(shared, peer, Envelope::new(types::AGENT_SEND, &body)?).await?;
    Ok(sent.duplicate)
}

// ---- Phone side of §5 (`agent.control`) ----

/// A request of §5 to `peer`, which must offer `agent.control` (as must this phone).
async fn control_request<T: serde::de::DeserializeOwned>(
    shared: &Arc<Shared>,
    peer: DeviceId,
    env: Envelope,
) -> Result<T> {
    needs_control(shared, &peer, true)?;
    request_of(shared, peer, env, agent::CONTROL).await
}

/// Like [`control_request`], for a list that may come in parts.
async fn control_paged<T>(shared: &Arc<Shared>, peer: DeviceId, env: Envelope) -> Result<T>
where
    T: g7link_protocol::lanes::Paged + serde::de::DeserializeOwned + Send + 'static,
{
    needs_control(shared, &peer, true)?;
    request_paged_of(shared, peer, env, agent::CONTROL).await
}

async fn control_ok(shared: &Arc<Shared>, peer: DeviceId, env: Envelope) -> Result<()> {
    control_request::<g7link_protocol::messages::Empty>(shared, peer, env).await.map(|_| ())
}

pub(crate) async fn configure(
    shared: &Arc<Shared>,
    peer: DeviceId,
    body: AgentConfigure,
) -> Result<AgentThread> {
    if !body.is_valid() {
        return Err(invalid("agent.configure"));
    }
    let env = Envelope::new(types::AGENT_CONFIGURE, &body)?;
    let thread: AgentThread = control_request(shared, peer, env).await?;
    if !thread.is_valid() {
        return Err(invalid("agent.thread reply"));
    }
    let thread = thread.sanitized().ok_or_else(|| invalid("agent.thread reply"))?;
    crate::agent_history::thread_arrived(shared, peer, &thread).await;
    Ok(thread)
}

pub(crate) async fn queue(shared: &Arc<Shared>, peer: DeviceId, body: AgentQueueChange) -> Result<()> {
    if !body.is_valid() {
        return Err(invalid("agent.queue"));
    }
    control_ok(shared, peer, Envelope::new(types::AGENT_QUEUE, &body)?).await
}

pub(crate) async fn reply(shared: &Arc<Shared>, peer: DeviceId, body: AgentQuestionReply) -> Result<()> {
    if !body.is_valid() {
        return Err(invalid("agent.reply"));
    }
    control_ok(shared, peer, Envelope::new(types::AGENT_REPLY, &body)?).await
}

pub(crate) async fn implement(shared: &Arc<Shared>, peer: DeviceId, body: AgentImplement) -> Result<()> {
    if !body.is_valid() {
        return Err(invalid("agent.implement"));
    }
    control_ok(shared, peer, Envelope::new(types::AGENT_IMPLEMENT, &body)?).await
}

pub(crate) async fn files(
    shared: &Arc<Shared>,
    peer: DeviceId,
    body: AgentFilesRequest,
) -> Result<AgentFiles> {
    if !body.is_valid() {
        return Err(invalid("agent.files"));
    }
    let env = Envelope::new(types::AGENT_FILES, &body)?;
    let files: AgentFiles = control_paged(shared, peer, env).await?;
    // A hit outside the folder is left out here too, never shown.
    Ok(files.sanitized())
}

pub(crate) async fn commands(
    shared: &Arc<Shared>,
    peer: DeviceId,
    body: AgentCommandsRequest,
) -> Result<Vec<AgentCommand>> {
    if !body.is_valid() {
        return Err(invalid("agent.commands"));
    }
    let env = Envelope::new(types::AGENT_COMMANDS, &body)?;
    let commands: AgentCommands = control_paged(shared, peer, env).await?;
    Ok(commands.sanitized().commands)
}

pub(crate) async fn draft_get(
    shared: &Arc<Shared>,
    peer: DeviceId,
    key: String,
) -> Result<Option<AgentDraft>> {
    let body = AgentDraftRequest { key };
    if !body.is_valid() {
        return Err(invalid("agent.draft.get"));
    }
    let env = Envelope::new(types::AGENT_DRAFT_GET, &body)?;
    let reply: AgentDraftReply = control_request(shared, peer, env).await?;
    Ok(reply.draft.and_then(AgentDraft::sanitized))
}

pub(crate) async fn draft_set(
    shared: &Arc<Shared>,
    peer: DeviceId,
    body: AgentDraftSet,
) -> Result<AgentDraftSaved> {
    if !body.is_valid() {
        return Err(invalid("agent.draft.set"));
    }
    let env = Envelope::new(types::AGENT_DRAFT_SET, &body)?;
    let saved: AgentDraftSaved = control_request(shared, peer, env).await?;
    Ok(saved.sanitized())
}

/// Phone: sends one image for a message on an `agent/image` stream; its id
/// goes in `agent.start` or `agent.send`. The PC decodes it with its own
/// bounded decoder and keeps a re-encoded copy without its metadata.
pub(crate) async fn image_send(
    shared: &Arc<Shared>,
    peer: DeviceId,
    name: String,
    bytes: Vec<u8>,
) -> Result<AgentImage> {
    if !(1..=agent::MAX_IMAGE_BYTES).contains(&bytes.len()) {
        return Err(invalid("agent image"));
    }
    let meta = AgentImageMeta { name: clean_image_name(&name), bytes: bytes.len() as u64 };
    image_send_unchecked(shared, peer, meta, bytes).await
}

/// [`image_send`] with a meta that may not match the bytes, for tests of
/// what the PC refuses.
pub(crate) async fn image_send_unchecked(
    shared: &Arc<Shared>,
    peer: DeviceId,
    meta: AgentImageMeta,
    bytes: Vec<u8>,
) -> Result<AgentImage> {
    needs_control(shared, &peer, true)?;
    let session = shared.session(&peer).ok_or(Error::Offline)?;
    let (mut send, mut recv) = session.open_bi().await?;
    let header = StreamHeader {
        svc: agent_image::SERVICE.into(),
        op: agent_image::OP_UPLOAD.into(),
        v: agent_image::VERSION,
    };
    write_frame(&mut send, &Envelope::new(types::STREAM, &header)?.to_cbor()).await?;
    write_frame(&mut send, &Envelope::new(types::AGENT_IMAGE_META, &meta)?.with_id(1).to_cbor()).await?;
    let go = read_reply(&mut recv, IMAGE_WAIT).await?;
    go.expect(types::OK).map_err(reply_error)?;
    // A PC that stops reading has said why: its reply follows.
    let _ = tokio::time::timeout(IMAGE_SEND, send.write_all(&bytes)).await;
    let _ = send.finish();
    let reply = read_reply(&mut recv, IMAGE_SEND).await?;
    let image: AgentImage = reply.expect_body(types::AGENT_IMAGE).map_err(reply_error)?;
    if !image.is_valid() {
        return Err(invalid("agent.image reply"));
    }
    image.sanitized().ok_or_else(|| invalid("agent.image reply"))
}

/// The next frame on `recv` as an envelope, waiting at most `wait`.
async fn read_reply(recv: &mut RecvStream, wait: Duration) -> Result<Envelope> {
    match tokio::time::timeout(wait, read_frame(recv)).await {
        Ok(Ok(Some(bytes))) => Ok(Envelope::from_cbor(&bytes)?),
        Ok(Ok(None)) => Err(Error::Offline),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Err(Error::Timeout),
    }
}

/// Phone: a JPEG of image `id` of `peer`, no larger than `max` px a side, on
/// an `agent/thumb` stream; refused before it's read when it's over
/// [`agent::MAX_THUMB_BYTES`].
pub(crate) async fn image_thumb(
    shared: &Arc<Shared>,
    peer: DeviceId,
    id: String,
    max: Option<u32>,
) -> Result<Vec<u8>> {
    let body = AgentImageThumb { id, max };
    if !body.is_valid() {
        return Err(invalid("agent.image.thumb"));
    }
    needs_control(shared, &peer, true)?;
    let session = shared.session(&peer).ok_or(Error::Offline)?;
    let (mut send, mut recv) = session.open_bi().await?;
    let header = StreamHeader {
        svc: agent_image::SERVICE.into(),
        op: agent_image::OP_THUMB.into(),
        v: agent_image::VERSION,
    };
    write_frame(&mut send, &Envelope::new(types::STREAM, &header)?.to_cbor()).await?;
    write_frame(&mut send, &Envelope::new(types::AGENT_IMAGE_THUMB, &body)?.with_id(1).to_cbor()).await?;
    let _ = send.finish();
    let reply = read_reply(&mut recv, IMAGE_WAIT).await?;
    let meta: AgentImageThumbMeta = reply.expect_body(types::AGENT_IMAGE_THUMB_META).map_err(reply_error)?;
    let Some(length) = usize::try_from(meta.length).ok().filter(|_| meta.is_valid()) else {
        return Err(invalid("agent.image.thumb.meta"));
    };
    let mut data = vec![0u8; length];
    tokio::time::timeout(IMAGE_SEND, recv.read_exact(&mut data))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|e| Error::Network(e.to_string()))?;
    if !is_jpeg(&data) {
        return Err(invalid("agent thumbnail"));
    }
    Ok(data)
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
    let opened = body.before.is_none();
    let items: AgentItems = request_paged(shared, peer, Envelope::new(types::AGENT_READ, &body)?).await?;
    if !items.is_valid() {
        return Err(invalid("agent.items reply"));
    }
    let items = items.sanitized().ok_or_else(|| invalid("agent.items reply"))?;
    crate::agent_history::page_arrived(shared, peer, &items, opened).await;
    Ok(items)
}

pub(crate) async fn watch(shared: &Arc<Shared>, peer: DeviceId, thread: Option<String>) -> Result<()> {
    let body = AgentWatch { thread };
    if !body.is_valid() {
        return Err(invalid("agent.watch"));
    }
    request_ok(shared, peer, Envelope::new(types::AGENT_WATCH, &body)?).await
}

/// Phone: answers an ask. A `message` goes with a denial only, and only to
/// a PC that offers `agent.say_why` (`Unsupported` otherwise, and nothing is
/// sent: the request still waits).
pub(crate) async fn answer_ask(
    shared: &Arc<Shared>,
    peer: DeviceId,
    token: String,
    allow: bool,
    message: Option<String>,
) -> Result<()> {
    let decision = if allow { agent::decision::ALLOW } else { agent::decision::DENY };
    let message = match message {
        None => None,
        Some(message) if allow || message.len() > agent::MAX_DENY_MESSAGE_BYTES => {
            return Err(invalid("agent.answer"));
        }
        Some(message) => Some(clean_deny_message(&message).ok_or_else(|| invalid("agent.answer"))?),
    };
    if message.is_some() && !shared.peer_offers(&peer, agent::SAY_WHY) {
        return Err(Error::Unsupported);
    }
    let body = AgentAnswer { token, decision: decision.into(), scope: None, message };
    if !body.is_valid() {
        return Err(invalid("agent.answer"));
    }
    request_ok(shared, peer, Envelope::new(types::AGENT_ANSWER, &body)?).await
}

/// Phone: whether `peer` takes a denial with a message (`agent.say_why`).
pub(crate) fn can_say_why(shared: &Shared, peer: &DeviceId) -> bool {
    shared.peer_offers(peer, agent::SAY_WHY)
}

/// Phone: whether `peer` and this phone both offer `agent.control` (§5).
pub(crate) fn can_control(shared: &Shared, peer: &DeviceId) -> bool {
    shared.we_offer(agent::CONTROL) && shared.peer_offers(peer, agent::CONTROL)
}

/// Phone: what `peer` offers on top of `agent.host`, as it last said.
fn peer_offers_now(shared: &Shared, peer: &DeviceId) -> PeerOffers {
    PeerOffers { say_why: can_say_why(shared, peer), control: can_control(shared, peer) }
}

/// Phone: `peer` said what it offers again. When whether it takes a denial
/// with a message, or §5, changed, its catalog is told again with the news.
pub(crate) fn peer_caps_changed(shared: &Shared, peer: &DeviceId) {
    let offers = peer_offers_now(shared, peer);
    let catalog = {
        let mut inner = shared.agents.lock();
        match inner.peer_catalogs.get_mut(peer) {
            Some((catalog, said)) if *said != offers => {
                *said = offers;
                catalog.clone()
            }
            _ => return,
        }
    };
    shared.emit(NodeEvent::AgentCatalog {
        device: *peer,
        catalog: Some(catalog),
        can_say_why: offers.say_why,
        can_control: offers.control,
    });
}

/// Phone: the sessions started on `peer` outside G7 Link.
pub(crate) async fn sessions(shared: &Arc<Shared>, peer: DeviceId) -> Result<Vec<AgentSession>> {
    let env = Envelope::empty(types::AGENT_SESSIONS);
    let reply: AgentSessions = request_paged_of(shared, peer, env, agent::SESSIONS).await?;
    if !reply.is_valid() {
        return Err(invalid("agent.sessions reply"));
    }
    Ok(reply.sanitized().sessions)
}

pub(crate) async fn session_read(
    shared: &Arc<Shared>,
    peer: DeviceId,
    session: String,
    before: Option<u64>,
    limit: u32,
) -> Result<AgentSessionItems> {
    let body = AgentSessionRead { session, before, limit: Some(limit.clamp(1, agent::MAX_ITEMS as u32)) };
    if !body.is_valid() {
        return Err(invalid("agent.session.read"));
    }
    let env = Envelope::new(types::AGENT_SESSION_READ, &body)?;
    let items: AgentSessionItems = request_paged_of(shared, peer, env, agent::SESSIONS).await?;
    if !items.is_valid() {
        return Err(invalid("agent.session.items reply"));
    }
    items.sanitized().ok_or_else(|| invalid("agent.session.items reply"))
}

pub(crate) async fn session_watch(
    shared: &Arc<Shared>,
    peer: DeviceId,
    session: Option<String>,
) -> Result<()> {
    let body = AgentSessionWatch { session };
    if !body.is_valid() {
        return Err(invalid("agent.session.watch"));
    }
    let env = Envelope::new(types::AGENT_SESSION_WATCH, &body)?;
    request_of::<g7link_protocol::messages::Empty>(shared, peer, env, agent::SESSIONS).await.map(|_| ())
}

pub(crate) async fn session_continue(
    shared: &Arc<Shared>,
    peer: DeviceId,
    session: String,
) -> Result<AgentThread> {
    let body = AgentSessionContinue { session };
    if !body.is_valid() {
        return Err(invalid("agent.session.continue"));
    }
    let env = Envelope::new(types::AGENT_SESSION_CONTINUE, &body)?;
    let thread: AgentThread = request_of(shared, peer, env, agent::SESSIONS).await?;
    if !thread.is_valid() {
        return Err(invalid("agent.thread reply"));
    }
    thread.sanitized().ok_or_else(|| invalid("agent.thread reply"))
}

/// Phone: a thread or a session of `peer` as Markdown, on a stream of its
/// own (`docs/protocol/agents.md` §4). A meta over the limits is refused
/// before the text is read.
pub(crate) async fn export(
    shared: &Arc<Shared>,
    peer: DeviceId,
    request: AgentExportRequest,
) -> Result<AgentExport> {
    if !request.is_valid() {
        return Err(invalid("agent.export"));
    }
    export_unchecked(shared, peer, request).await
}

/// [`export`] without checking the request here, for tests of what the PC refuses.
pub(crate) async fn export_unchecked(
    shared: &Arc<Shared>,
    peer: DeviceId,
    request: AgentExportRequest,
) -> Result<AgentExport> {
    let session = shared.session(&peer).ok_or(Error::Offline)?;
    let cap = if request.is_session() { agent::SESSIONS } else { agent::HOST };
    if !shared.store.get_peer(&peer)?.is_some_and(|p| p.caps.contains(cap)) {
        return Err(Error::Unsupported);
    }
    let (mut send, mut recv) = session.open_bi().await?;
    let header = StreamHeader {
        svc: agent_export::SERVICE.into(),
        op: agent_export::OP.into(),
        v: agent_export::VERSION,
    };
    write_frame(&mut send, &Envelope::new(types::STREAM, &header)?.to_cbor()).await?;
    write_frame(&mut send, &Envelope::new(types::AGENT_EXPORT, &request)?.with_id(1).to_cbor()).await?;
    let _ = send.finish();
    let reply = match tokio::time::timeout(EXPORT_WAIT, read_frame(&mut recv)).await {
        Ok(Ok(Some(bytes))) => Envelope::from_cbor(&bytes)?,
        Ok(Ok(None)) => return Err(Error::Offline),
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err(Error::Timeout),
    };
    let meta: AgentExportMeta = reply.expect_body(types::AGENT_EXPORT_META).map_err(reply_error)?;
    let Some(length) = usize::try_from(meta.length).ok().filter(|_| meta.is_valid()) else {
        return Err(invalid("agent.export.meta"));
    };
    let mut data = vec![0u8; length];
    tokio::time::timeout(EXPORT_SEND, recv.read_exact(&mut data))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|e| Error::Network(e.to_string()))?;
    let text = String::from_utf8(data).map_err(|_| invalid("agent export text"))?;
    let (text, cut) = clean_agent_export(text);
    Ok(AgentExport { text, truncated: meta.truncated || cut, file_name: meta.file_name })
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
    let reply = session.request(Envelope::new(t, body)?, AGENT_WAIT).await?;
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
            model: None,
            effort: None,
            images: Vec::new(),
            custom_model: false,
            key: Some("0123456789abcdef".into()),
        };
        let answer = AgentRequest::Answer {
            token: "0123abcd".into(),
            allow: false,
            message: Some("secret instead".into()),
        };
        let search = AgentRequest::Search { query: "secret".into(), limit: 1, cursor: None, archived: false };
        let rename = AgentRequest::Rename { thread: "abc".into(), title: "secret".into() };
        let patch = AgentRequest::DiffPatch {
            thread: "abc".into(),
            range: TurnRange::Turn(1),
            path: "secret/x.rs".into(),
            offset: None,
        };
        let apply = AgentRequest::RewindApply { token: "0123abcd".into(), refill: true };
        let found = AgentRequest::Found {
            query: Some("secret".into()),
            agent: None,
            show: None,
            before_ms: None,
            limit: 1,
        };
        let shown = format!("{start:?} {answer:?} {search:?} {rename:?} {patch:?} {apply:?} {found:?}");
        assert!(!shown.contains("secret") && !shown.contains("0123abcd"), "{shown}");
        assert!(!shown.contains("0123456789abcdef"), "a message's key is not printed: {shown}");
        let export = AgentExport {
            text: "# secret plan".into(),
            truncated: false,
            file_name: "claude-secret-plan.md".into(),
        };
        assert!(!format!("{export:?}").contains("secret"));
    }

    #[test]
    fn an_export_is_kept_within_its_limits() {
        let export = AgentExport {
            text: format!("a\u{1b}b\u{202E}{}", "x".repeat(agent::MAX_EXPORT_BYTES)),
            truncated: false,
            file_name: "../my notes.md".into(),
        }
        .sanitized();
        assert!(export.truncated && export.text.len() == agent::MAX_EXPORT_BYTES);
        assert!(export.text.starts_with("abx"));
        assert_eq!(export.file_name, "-my-notes.md");
        let fine = AgentExport { text: "# Hi\n".into(), truncated: true, file_name: "a.md".into() };
        assert_eq!(fine.clone().sanitized(), fine);
    }
}
