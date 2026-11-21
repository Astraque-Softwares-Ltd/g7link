// SPDX-License-Identifier: MPL-2.0
//! Source control on a PC's agent threads, from a phone
//! (`docs/protocol/agents.md` §8): the folder's status and uncommitted diff,
//! reviewing files and committing them, push and fetch, branches, the
//! branch's pull request and the lists of them, and "Ask <agent> to write it".
//!
//! PC side: the core checks the toggles and every limit before anything
//! reaches the platform, and enforces what a phone may do. A commit goes
//! only after the PC gave this phone every file of its review whole: the
//! core keeps each phone's reviews (4, 10 minutes each), counts a file read
//! when the last part of its patch was written to the phone, passes the
//! host the review's own paths, never the phone's, and lets a review commit
//! once. Push and opening a pull request need the phone's `agent_push`
//! toggle, read again for each request. A switch that takes changes along,
//! a merge, a worktree and "keep in step" have no message at all. Every
//! answer is made to fit the phone: no URL, no absolute path, no worktree
//! path, no folder on the PC by its path. Phone side: it sends the
//! requests, keeps each thread's last strip for reading offline, and turns
//! what arrives into [`NodeEvent`]s.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

pub use g7link_protocol::messages::{
    AgentBranch, AgentBranches, AgentCheck, AgentCheckCounts, AgentGhState, AgentGit,
    AgentGitBranchesRequest, AgentGitChanged, AgentGitCommitRequest, AgentGitFile, AgentGitHub,
    AgentGitJobRequest, AgentGitLast, AgentGitPatch, AgentGitPatchRequest, AgentGitProgress,
    AgentGitPushRequest, AgentGitRemote, AgentGitReview, AgentGitReviewFile, AgentGitReviewRequest,
    AgentGitSwitchRequest, AgentGitThread, AgentGitWorktree, AgentJob, AgentJobError, AgentJobResult,
    AgentPrBrief, AgentPrCreateRequest, AgentPrInfo, AgentPrListRequest, AgentPrRow, AgentPrRows,
    AgentPrView, AgentPrViewRequest, AgentWriteRequest, AgentWrote, clean_typed_text, git, is_branch_name,
    is_commit_message, is_git_job_id, is_https_url, is_remote_name, is_review_token, pr,
};
use g7link_protocol::{
    DeviceId, Envelope, ErrorCode,
    messages::{AgentThread, agent, cut_text, one_line, types},
};

use crate::{
    Error, Node, Result,
    agent_history::{Cached, with_cache},
    agents::{self, AgentFailure, AgentReply, AgentRequest},
    dispatch::Responder,
    events::NodeEvent,
    node::Shared,
};

/// One page of a file's uncommitted patch as the host gives it
/// (`git.patch`): ask again from `next` for the rest. An untracked folder
/// has no patch but the first names inside it.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct AgentGitPatchPage {
    pub patch: String,
    pub next: Option<u64>,
    pub truncated: bool,
    pub binary: bool,
    pub names: Vec<String>,
    pub more_names: u32,
}

/// Never prints the patch or a name.
impl std::fmt::Debug for AgentGitPatchPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGitPatchPage")
            .field("bytes", &self.patch.len())
            .field("next", &self.next)
            .field("truncated", &self.truncated)
            .field("binary", &self.binary)
            .field("names", &self.names.len())
            .field("more_names", &self.more_names)
            .finish()
    }
}

/// One file of the host's review (`git.review`): its size on disk, and an
/// untracked folder's entries.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct AgentGitReviewedFile {
    pub path: String,
    pub bytes: u64,
    pub folder: bool,
    pub entries: u32,
}

/// Never prints the path.
impl std::fmt::Debug for AgentGitReviewedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGitReviewedFile")
            .field("bytes", &self.bytes)
            .field("folder", &self.folder)
            .field("entries", &self.entries)
            .finish_non_exhaustive()
    }
}

/// The host's review (`git.review`): `review` is its id, passed back as
/// `git.commit`'s `review`; `pc_only` when a folder is too large for anyone
/// but the PC to commit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentGitReviewed {
    pub review: String,
    pub files: Vec<AgentGitReviewedFile>,
    pub pc_only: bool,
}

/// How long the PC keeps a phone's review.
const REVIEW_LIFE: Duration = Duration::from_millis(git::REVIEW_LIFE_MS);
/// Host pages of one patch the core follows at most.
const MAX_PATCH_PAGES: usize = 64;

fn now_ms() -> u64 {
    u64::try_from(crate::now_unix_ms()).unwrap_or(0)
}

/// A fresh id: 16 lowercase hex digits.
fn fresh_id() -> String {
    format!("{:016x}", rand::random::<u64>())
}

// ---- The reviews (PC) ----

/// One file of a phone's review.
struct ReviewFile {
    path: String,
    readable: bool,
    /// The last part of its patch was written to the phone.
    served: bool,
}

/// A review a phone made: the token it was given, the host's id, its files.
struct Review {
    token: String,
    thread: String,
    host: String,
    files: Vec<ReviewFile>,
    made: Instant,
}

impl Review {
    fn fresh(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.made) < REVIEW_LIFE
    }
}

/// PC: the reviews phones made, at most [`git::MAX_REVIEWS`] each, for
/// [`git::REVIEW_LIFE_MS`]. Kept across a reconnect; gone when the phone is
/// unpaired or its `agents` toggle goes off.
#[derive(Default)]
pub(crate) struct Reviews {
    by_phone: Mutex<HashMap<DeviceId, Vec<Review>>>,
}

/// Never prints a path.
impl std::fmt::Debug for Reviews {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reviews").field("phones", &self.lock().len()).finish()
    }
}

fn no_review() -> Envelope {
    Envelope::error(ErrorCode::NotFound, "no such review: read the files again")
}

fn not_all_read() -> Envelope {
    Envelope::error(ErrorCode::Denied, "every file of the review must be read whole on this phone first")
}

impl Reviews {
    fn lock(&self) -> MutexGuard<'_, HashMap<DeviceId, Vec<Review>>> {
        self.by_phone.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Keeps `review` for `peer`, its oldest going past the cap.
    fn keep(&self, peer: DeviceId, review: Review) {
        let now = Instant::now();
        let mut all = self.lock();
        let reviews = all.entry(peer).or_default();
        reviews.retain(|r| r.fresh(now));
        if reviews.len() >= git::MAX_REVIEWS {
            reviews.remove(0);
        }
        reviews.push(review);
    }

    /// Whether `peer` may read `path` for its review `token` of `thread`.
    fn check_patch(&self, peer: &DeviceId, token: &str, thread: &str, path: &str) -> Option<Envelope> {
        let now = Instant::now();
        let all = self.lock();
        let Some(review) = all
            .get(peer)
            .and_then(|r| r.iter().find(|r| r.token == token && r.thread == thread && r.fresh(now)))
        else {
            return Some(no_review());
        };
        (!review.files.iter().any(|f| f.path == path))
            .then(|| Envelope::error(ErrorCode::BadMessage, "that file isn't in the review"))
    }

    /// `path` of `peer`'s review `token` was written to it whole.
    fn served(&self, peer: &DeviceId, token: &str, path: &str) {
        let mut all = self.lock();
        if let Some(file) = all
            .get_mut(peer)
            .and_then(|r| r.iter_mut().find(|r| r.token == token))
            .and_then(|r| r.files.iter_mut().find(|f| f.path == path))
        {
            file.served = true;
        }
    }

    /// Takes `peer`'s review `token` of `thread` for a commit: only when
    /// every file of it can be read and was given to the phone whole.
    fn take(&self, peer: &DeviceId, token: &str, thread: &str) -> std::result::Result<Review, Envelope> {
        let now = Instant::now();
        let mut all = self.lock();
        let reviews = all.get_mut(peer).ok_or_else(no_review)?;
        reviews.retain(|r| r.fresh(now));
        let at = reviews.iter().position(|r| r.token == token && r.thread == thread).ok_or_else(no_review)?;
        if !reviews[at].files.iter().all(|f| f.readable && f.served) {
            return Err(not_all_read());
        }
        Ok(reviews.remove(at))
    }

    /// A review whose commit the host refused before it ran: it may be used again.
    fn put_back(&self, peer: DeviceId, review: Review) {
        if review.fresh(Instant::now()) {
            self.keep(peer, review);
        }
    }

    /// Forgets `peer`'s reviews.
    pub(crate) fn forget(&self, peer: &DeviceId) {
        self.lock().remove(peer);
    }

    /// Test hook: makes every review `by` older.
    fn age(&self, by: Duration) {
        for review in self.lock().values_mut().flatten() {
            review.made = review.made.checked_sub(by).unwrap_or(review.made);
        }
    }
}

// ---- PC side ----

/// Whether `peer` may push and open pull requests from here: its
/// `agent_push` toggle, read for each request.
pub(crate) fn may_push(shared: &Shared, peer: &DeviceId) -> bool {
    shared.toggle_on(peer, git::PUSH_TOGGLE)
}

/// PC: `agent_push` was turned on or off for `peer`: its catalog says so again.
pub(crate) fn push_toggled(shared: &Arc<Shared>, peer: DeviceId) {
    let Some(session) = shared.session(&peer) else { return };
    let task = shared.clone();
    shared.runtime.spawn(async move { agents::send_catalog(&task, &session).await });
}

/// Whether `t` is one of §8's requests.
pub(crate) fn is_request(t: &str) -> bool {
    matches!(
        t,
        types::AGENT_GIT_STATUS
            | types::AGENT_GIT_PATCH
            | types::AGENT_GIT_REVIEW
            | types::AGENT_GIT_COMMIT
            | types::AGENT_GIT_PUSH
            | types::AGENT_GIT_FETCH
            | types::AGENT_GIT_BRANCHES
            | types::AGENT_GIT_SWITCH
            | types::AGENT_GIT_JOB
            | types::AGENT_PR_VIEW
            | types::AGENT_PR_CREATE
            | types::AGENT_PR_LIST
            | types::AGENT_WRITE
    )
}

/// Whether `t` is one of §8's events (PC → phone).
pub(crate) fn is_event(t: &str) -> bool {
    matches!(t, types::AGENT_GIT | types::AGENT_GIT_PROGRESS)
}

/// A PC's answer to one of §8's requests: one envelope, or a list that may
/// go in parts.
enum Answer {
    Plain(Envelope),
    Git(Box<AgentGit>),
    /// A patch; `read` names the review and the file it counts for once
    /// all of it was written, when it was whole.
    Patch {
        body: AgentGitPatch,
        read: Option<(String, String)>,
    },
    Review(AgentGitReview),
    Branches(AgentBranches),
    Prs(AgentPrRows),
}

fn plain(env: Envelope) -> Answer {
    Answer::Plain(env)
}

fn ok_with<T: serde::Serialize>(body: &T) -> Answer {
    plain(Envelope::new(types::OK, body).unwrap_or_else(|_| AgentFailure::Internal.reply("failed")))
}

fn bad() -> Envelope {
    Envelope::error(ErrorCode::BadMessage, "invalid agent request")
}

fn malformed() -> Answer {
    plain(AgentFailure::Internal.reply("the host's answer was malformed"))
}

fn something_else() -> Answer {
    plain(AgentFailure::Internal.reply("the host answered something else"))
}

/// The refusal of a push or a pull request while `agent_push` is off.
fn push_refused() -> Envelope {
    Envelope::error(ErrorCode::Denied, "agent_push is off for this phone")
}

/// PC: answers one of §8's requests from `peer`.
pub(crate) async fn handle_request(
    shared: &Arc<Shared>,
    peer: DeviceId,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<()> {
    match answer(shared, peer, env).await {
        Answer::Plain(reply) => rsp.send(reply).await,
        Answer::Git(body) => rsp.paged(*body).await,
        Answer::Review(body) => rsp.paged(body).await,
        Answer::Branches(body) => rsp.paged(body).await,
        Answer::Prs(body) => rsp.paged(body).await,
        Answer::Patch { body, read } => {
            let whole = !body.truncated;
            let sent = rsp.paged_sent(body).await?;
            if let Some((token, path)) = read
                && sent
                && whole
            {
                shared.agents.git.served(&peer, &token, &path);
            }
            Ok(())
        }
    }
}

/// What the platform must answer a request with (or `Refused`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expects {
    Git,
    Review,
    Job,
    Branches,
    Pr,
    Prs,
    Thread,
}

impl Expects {
    fn of(request: &AgentRequest) -> Self {
        match request {
            AgentRequest::GitStatus { .. } => Self::Git,
            AgentRequest::GitReview { .. } => Self::Review,
            AgentRequest::GitBranches { .. } => Self::Branches,
            AgentRequest::PrView { .. } => Self::Pr,
            AgentRequest::PrList { .. } => Self::Prs,
            AgentRequest::Ask { .. } => Self::Thread,
            _ => Self::Job,
        }
    }
}

/// What a request starting a job is, as its job's `kind`.
fn job_kind(request: &AgentRequest) -> Option<&'static str> {
    Some(match request {
        AgentRequest::GitCommit { .. } => git::kind::COMMIT,
        AgentRequest::GitPush { .. } => git::kind::PUSH,
        AgentRequest::GitFetch { .. } => git::kind::FETCH,
        AgentRequest::GitSwitch { .. } => git::kind::SWITCH,
        AgentRequest::PrCreate { .. } => git::kind::PR_CREATE,
        _ => return None,
    })
}

/// The thread a request is about, if it names one.
fn thread_of(request: &AgentRequest) -> &str {
    match request {
        AgentRequest::GitCommit { thread, .. }
        | AgentRequest::GitPush { thread, .. }
        | AgentRequest::GitFetch { thread }
        | AgentRequest::GitSwitch { thread, .. }
        | AgentRequest::PrCreate { thread, .. } => thread,
        _ => "",
    }
}

/// The request `env` asks for after the checks a phone's request gets
/// before the platform hears of it; the answer to send instead when it's
/// refused. A commit's review is checked in [`answer`].
fn request_of(
    shared: &Shared,
    peer: &DeviceId,
    env: &Envelope,
) -> std::result::Result<AgentRequest, Envelope> {
    let request = match env.t.as_str() {
        types::AGENT_GIT_STATUS => {
            let r = agents::decode::<AgentGitThread>(env, AgentGitThread::is_valid).ok_or_else(bad)?;
            AgentRequest::GitStatus { thread: r.thread }
        }
        types::AGENT_GIT_PATCH => {
            let r = agents::decode::<AgentGitPatchRequest>(env, AgentGitPatchRequest::is_valid)
                .ok_or_else(bad)?;
            AgentRequest::GitPatch { thread: r.thread, path: r.path, offset: None }
        }
        types::AGENT_GIT_REVIEW => {
            let r = agents::decode::<AgentGitReviewRequest>(env, AgentGitReviewRequest::is_valid)
                .ok_or_else(bad)?;
            AgentRequest::GitReview { thread: r.thread, paths: r.paths }
        }
        types::AGENT_GIT_COMMIT => {
            let r = agents::decode::<AgentGitCommitRequest>(env, AgentGitCommitRequest::is_valid)
                .ok_or_else(bad)?;
            let message = clean_typed_text(&r.message).trim().to_owned();
            if !is_commit_message(&message) {
                return Err(bad());
            }
            AgentRequest::GitCommit { thread: r.thread, paths: Vec::new(), message, review: r.review }
        }
        types::AGENT_GIT_PUSH => {
            let r =
                agents::decode::<AgentGitPushRequest>(env, AgentGitPushRequest::is_valid).ok_or_else(bad)?;
            if !may_push(shared, peer) {
                return Err(push_refused());
            }
            AgentRequest::GitPush { thread: r.thread, remote: r.remote }
        }
        types::AGENT_GIT_FETCH => {
            let r = agents::decode::<AgentGitThread>(env, AgentGitThread::is_valid).ok_or_else(bad)?;
            AgentRequest::GitFetch { thread: r.thread }
        }
        types::AGENT_GIT_BRANCHES => {
            let r = agents::decode::<AgentGitBranchesRequest>(env, AgentGitBranchesRequest::is_valid)
                .ok_or_else(bad)?;
            let query = r.query.map(|q| one_line(&q).trim().to_owned()).filter(|q| !q.is_empty());
            AgentRequest::GitBranches { thread: r.thread, query }
        }
        types::AGENT_GIT_SWITCH => {
            let r = agents::decode::<AgentGitSwitchRequest>(env, AgentGitSwitchRequest::is_valid)
                .ok_or_else(bad)?;
            if r.carry {
                return Err(Envelope::error(
                    ErrorCode::Denied,
                    "a switch that takes changes along is done from G7 Link on the PC",
                ));
            }
            AgentRequest::GitSwitch { thread: r.thread, branch: r.branch, create: r.create, track: r.track }
        }
        types::AGENT_GIT_JOB => {
            let r =
                agents::decode::<AgentGitJobRequest>(env, AgentGitJobRequest::is_valid).ok_or_else(bad)?;
            if r.stop { AgentRequest::GitStop { job: r.job } } else { AgentRequest::GitJob { job: r.job } }
        }
        types::AGENT_PR_VIEW => {
            let r =
                agents::decode::<AgentPrViewRequest>(env, AgentPrViewRequest::is_valid).ok_or_else(bad)?;
            AgentRequest::PrView { thread: r.thread, refresh: r.refresh }
        }
        types::AGENT_PR_CREATE => {
            let r = agents::decode::<AgentPrCreateRequest>(env, AgentPrCreateRequest::is_valid)
                .ok_or_else(bad)?;
            if !may_push(shared, peer) {
                return Err(push_refused());
            }
            let title = one_line(&r.title).trim().to_owned();
            if title.is_empty() {
                return Err(bad());
            }
            let body = clean_typed_text(&r.body);
            AgentRequest::PrCreate { thread: r.thread, title, body, base: r.base, draft: r.draft }
        }
        types::AGENT_PR_LIST => {
            let r =
                agents::decode::<AgentPrListRequest>(env, AgentPrListRequest::is_valid).ok_or_else(bad)?;
            AgentRequest::PrList { show: r.show, folder: r.folder, refresh: r.refresh }
        }
        types::AGENT_WRITE => {
            let r = agents::decode::<AgentWriteRequest>(env, AgentWriteRequest::is_valid).ok_or_else(bad)?;
            AgentRequest::Ask { thread: r.thread, purpose: r.purpose }
        }
        _ => return Err(Envelope::error(ErrorCode::Unsupported, "unknown agent request")),
    };
    Ok(request)
}

/// The refusal a request that starts no job gets for a host's reason.
fn refusal_of(error: &AgentJobError) -> Envelope {
    let error = error.clone().sanitized();
    let code = match error.reason.as_str() {
        git::reason::GIT_BUSY | git::reason::THREAD_BUSY | git::reason::SHARED_BUSY => ErrorCode::Busy,
        "not_found" => ErrorCode::NotFound,
        _ => ErrorCode::BadMessage,
    };
    Envelope::error(code, error.reason)
}

/// A job the host refused to start: it failed at once, with its reason.
fn refused_job(thread: &str, kind: &str, error: AgentJobError) -> AgentJob {
    let now = now_ms();
    AgentJob {
        id: fresh_id(),
        thread: thread.to_owned(),
        kind: kind.to_owned(),
        phase: git::phase::FAILED.to_owned(),
        started_ms: now,
        ended_ms: Some(now),
        mine: true,
        error: Some(error.sanitized()),
        ..Default::default()
    }
}

/// `job` for `peer`: within a phone's limits, `mine` when it asked.
fn job_for(job: AgentJob, by: Option<DeviceId>, peer: &DeviceId) -> Option<AgentJob> {
    let mut job = job.sanitized()?;
    job.mine = by.as_ref() == Some(peer);
    Some(job)
}

/// Whether a reviewed file can be read whole on a phone, and why not. An
/// untracked folder never can: a phone sees only the names inside it, never
/// their bytes, so it's committed on the PC.
fn readable(file: &AgentGitReviewedFile) -> Option<&'static str> {
    if file.folder {
        Some(git::unreadable::FOLDER)
    } else if file.bytes > git::MAX_PATCH_BYTES as u64 {
        Some(git::unreadable::LARGE)
    } else {
        None
    }
}

/// PC: the review the host pinned, kept for `peer` under a token of the
/// core's own, as the phone takes it.
fn review_for(
    shared: &Shared,
    peer: DeviceId,
    thread: String,
    asked: &[String],
    host: AgentGitReviewed,
) -> Answer {
    if !is_review_token(&host.review) {
        return malformed();
    }
    let mut files: Vec<ReviewFile> = Vec::with_capacity(host.files.len());
    for file in &host.files {
        if !asked.contains(&file.path) || files.iter().any(|f| f.path == file.path) {
            continue;
        }
        files.push(ReviewFile { path: file.path.clone(), readable: readable(file).is_none(), served: false });
    }
    if files.is_empty() {
        return plain(Envelope::error(ErrorCode::BadMessage, git::reason::NOTHING));
    }
    let token = fresh_id();
    let wire = AgentGitReview {
        review: token.clone(),
        files: host
            .files
            .iter()
            .filter(|f| files.iter().any(|kept| kept.path == f.path))
            .map(|f| {
                let why = readable(f);
                AgentGitReviewFile {
                    path: f.path.clone(),
                    readable: why.is_none(),
                    why: why.map(str::to_owned),
                }
            })
            .collect(),
    };
    shared.agents.git.keep(peer, Review { token, thread, host: host.review, files, made: Instant::now() });
    Answer::Review(wire)
}

async fn answer(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) -> Answer {
    if let Some(refused) = agents::refusal(shared, peer, false) {
        return plain(refused);
    }
    let mut request = match request_of(shared, &peer, env) {
        Ok(request) => request,
        Err(refused) => return plain(refused),
    };
    if let AgentRequest::GitPatch { thread, path, .. } = request {
        let review = agents::decode::<AgentGitPatchRequest>(env, |_| true).and_then(|r| r.review);
        if let Some(token) = &review
            && let Some(refused) = shared.agents.git.check_patch(&peer, token, &thread, &path)
        {
            return plain(refused);
        }
        return patch(shared, peer, thread, path, review).await;
    }
    let mut taken = None;
    if let AgentRequest::GitCommit { thread, paths, review, .. } = &mut request {
        match shared.agents.git.take(&peer, review, thread) {
            Ok(kept) => {
                *paths = kept.files.iter().map(|f| f.path.clone()).collect();
                review.clone_from(&kept.host);
                taken = Some(kept);
            }
            Err(refused) => return plain(refused),
        }
    }
    let asked = match &request {
        AgentRequest::GitReview { thread, paths } => Some((thread.clone(), paths.clone())),
        _ => None,
    };
    let kind = job_kind(&request);
    let expects = Expects::of(&request);
    let thread = thread_of(&request).to_owned();
    let control = shared.control_with(&peer);
    let reply = agents::ask_platform(shared, peer, request).await;
    if let Some(kept) = taken
        && !matches!(reply, Ok(AgentReply::GitJob { .. }))
    {
        shared.agents.git.put_back(peer, kept);
    }
    let reply = match reply {
        Ok(reply) => reply,
        Err(failure) => return plain(failure.reply(agents::failure_message(failure))),
    };
    match (expects, reply) {
        (_, AgentReply::Refused(error)) => match kind {
            Some(kind) => ok_with(&refused_job(&thread, kind, error)),
            None => plain(refusal_of(&error)),
        },
        (Expects::Git, AgentReply::Git(status)) => match status.sanitized() {
            Some(status) => Answer::Git(Box::new(status)),
            None => malformed(),
        },
        (Expects::Review, AgentReply::Review(host)) => match asked {
            Some((thread, paths)) => review_for(shared, peer, thread, &paths, host),
            None => something_else(),
        },
        (Expects::Job, AgentReply::GitJob { job, by }) => match job_for(job, by, &peer) {
            Some(job) => ok_with(&job),
            None => malformed(),
        },
        (Expects::Branches, AgentReply::Branches(branches)) => Answer::Branches(branches.sanitized()),
        (Expects::Pr, AgentReply::Pr(view)) => match view.sanitized() {
            Some(view) => ok_with(&view),
            None => malformed(),
        },
        (Expects::Prs, AgentReply::PrRows(rows)) => match rows.sanitized() {
            Some(rows) => Answer::Prs(rows),
            None => malformed(),
        },
        (Expects::Thread, AgentReply::Thread(thread)) => match thread.sanitized() {
            Some(thread) => ok_with(&agents::thread_for(thread, control)),
            None => malformed(),
        },
        _ => something_else(),
    }
}

/// PC: one file's uncommitted patch, following the host's pages up to
/// [`git::MAX_PATCH_BYTES`]; a later page that fails ends it, cut. With a
/// review, it counts as read only when it came whole.
async fn patch(
    shared: &Arc<Shared>,
    peer: DeviceId,
    thread: String,
    path: String,
    review: Option<String>,
) -> Answer {
    let started = Instant::now();
    let mut whole = AgentGitPatch::default();
    let mut offset = None;
    let mut ended = false;
    for _ in 0..MAX_PATCH_PAGES {
        let request = AgentRequest::GitPatch { thread: thread.clone(), path: path.clone(), offset };
        let page = match agents::ask_platform(shared, peer, request).await {
            Ok(AgentReply::GitPatch(page)) => page,
            Ok(AgentReply::Refused(error)) if offset.is_none() => return plain(refusal_of(&error)),
            Ok(_) if offset.is_none() => return something_else(),
            Err(failure) if offset.is_none() => {
                return plain(failure.reply(agents::failure_message(failure)));
            }
            Ok(_) | Err(_) => break,
        };
        if offset.is_none() {
            whole.names = page.names;
            whole.more_names = page.more_names;
        }
        whole.binary |= page.binary;
        whole.truncated |= page.truncated;
        let room = git::MAX_PATCH_BYTES - whole.patch.len();
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
    let body = whole.sanitized();
    Answer::Patch { read: review.map(|token| (token, path)), body }
}

/// PC: a thread's folder changed; every phone that gets agent events hears
/// its strip (`agent.git`).
pub(crate) fn send_changed(shared: &Arc<Shared>, change: AgentGitChanged) -> Result<()> {
    let change = change.sanitized().ok_or_else(|| Error::Protocol("invalid agent.git".into()))?;
    let env = Envelope::new(types::AGENT_GIT, &change)?;
    for session in shared.agent_recipients() {
        let _ = session.send_now(env.clone());
    }
    Ok(())
}

/// PC: a job's new lines and where it stands, to every phone that gets
/// agent events (`agent.git.progress`); `mine` for the one that asked.
pub(crate) fn send_progress(shared: &Arc<Shared>, job: AgentJob, by: Option<DeviceId>) -> Result<()> {
    let job = job.sanitized().ok_or_else(|| Error::Protocol("invalid agent.git.progress".into()))?;
    for session in shared.agent_recipients() {
        let mut job = job.clone();
        job.mine = by == Some(session.peer);
        let env = Envelope::new(types::AGENT_GIT_PROGRESS, &AgentGitProgress { job })?;
        let _ = session.send_now(env);
    }
    Ok(())
}

// ---- Phone side: what arrives ----

/// Phone: one of §8's events from `peer`.
pub(crate) async fn handle_event(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) {
    if !shared.we_offer(agent::REMOTE) {
        return;
    }
    match env.t.as_str() {
        types::AGENT_GIT => {
            if let Ok(change) = env.body::<AgentGitChanged>()
                && change.is_valid()
                && let Some(change) = change.sanitized()
            {
                let (saved, now) = (change.clone(), now_ms());
                with_cache(shared, move |cache| cache.save_git(&peer, &saved, now)).await;
                shared.emit(NodeEvent::AgentGit { device: peer, change });
            }
        }
        types::AGENT_GIT_PROGRESS => {
            if let Ok(progress) = env.body::<AgentGitProgress>()
                && progress.is_valid()
                && let Some(job) = progress.job.sanitized()
            {
                shared.emit(NodeEvent::AgentGitJob { device: peer, job });
            }
        }
        _ => {}
    }
}

// ---- Phone side: requests ----

fn invalid(what: &str) -> Error {
    agents::invalid(what)
}

async fn paged<T>(shared: &Arc<Shared>, peer: DeviceId, t: &str, body: &impl serde::Serialize) -> Result<T>
where
    T: g7link_protocol::lanes::Paged + serde::de::DeserializeOwned + Send + 'static,
{
    agents::request_paged_of(shared, peer, Envelope::new(t, body)?, agent::HOST).await
}

async fn single<T: serde::de::DeserializeOwned>(
    shared: &Arc<Shared>,
    peer: DeviceId,
    t: &str,
    body: &impl serde::Serialize,
) -> Result<T> {
    agents::request_of(shared, peer, Envelope::new(t, body)?, agent::HOST).await
}

/// A job a PC answered with, checked.
async fn job(
    shared: &Arc<Shared>,
    peer: DeviceId,
    t: &str,
    body: &impl serde::Serialize,
) -> Result<AgentJob> {
    let job: AgentJob = single(shared, peer, t, body).await?;
    job.sanitized().ok_or_else(|| invalid(&format!("{t} reply")))
}

/// The node's §8 methods. PC: what the platform pushes. Phone: requests to
/// a PC (`peer`) and each thread's last strip.
impl Node {
    /// PC: a thread's folder changed (the host's `git.changed`): every phone
    /// with the `agents` toggle on gets its strip (`agent.git`).
    pub async fn agent_git_changed(&self, change: AgentGitChanged) -> Result<()> {
        send_changed(self.shared(), change)
    }

    /// PC: a job's new lines and where it stands (the host's
    /// `git.progress`), at most 4 a second; `by` is the device that asked
    /// for it (`None`: this PC). Each phone gets its newest 40 lines.
    pub async fn agent_git_progress(&self, job: AgentJob, by: Option<DeviceId>) -> Result<()> {
        send_progress(self.shared(), job, by)
    }

    /// Test hook: makes the reviews phones made `by` older.
    #[doc(hidden)]
    pub fn debug_age_agent_reviews(&self, by: Duration) {
        self.shared().agents.git.age(by);
    }

    /// Phone: a thread's folder's status: at most 500 files, relative paths,
    /// no remote's URL. Its strip is kept for reading offline.
    pub async fn agent_git_status(&self, peer: DeviceId, thread: String) -> Result<AgentGit> {
        let body = AgentGitThread { thread };
        if !body.is_valid() {
            return Err(invalid("agent.git.status"));
        }
        let shared = self.shared();
        let reply: AgentGit = paged(shared, peer, types::AGENT_GIT_STATUS, &body).await?;
        let status = reply.sanitized().ok_or_else(|| invalid("agent.git.status reply"))?;
        let (strip, now) = (AgentGitChanged::of(body.thread, &status), now_ms());
        with_cache(shared, move |cache| cache.save_git_status(&peer, &strip, now)).await;
        Ok(status)
    }

    /// Phone: one listed file's uncommitted patch, at most 1 MiB
    /// (`truncated` past that); an untracked folder's first names instead.
    /// With `review`, the PC counts the file read once all of it went.
    pub async fn agent_git_patch(
        &self,
        peer: DeviceId,
        thread: String,
        path: String,
        review: Option<String>,
    ) -> Result<AgentGitPatch> {
        let body = AgentGitPatchRequest { thread, path, review };
        if !body.is_valid() {
            return Err(invalid("agent.git.patch"));
        }
        let reply: AgentGitPatch = paged(self.shared(), peer, types::AGENT_GIT_PATCH, &body).await?;
        Ok(reply.sanitized())
    }

    /// Phone: pins 1 to 200 listed files before reading and committing them.
    pub async fn agent_git_review(
        &self,
        peer: DeviceId,
        thread: String,
        paths: Vec<String>,
    ) -> Result<AgentGitReview> {
        let body = AgentGitReviewRequest { thread, paths };
        if !body.is_valid() {
            return Err(invalid("agent.git.review"));
        }
        let reply: AgentGitReview = paged(self.shared(), peer, types::AGENT_GIT_REVIEW, &body).await?;
        reply.sanitized().ok_or_else(|| invalid("agent.git.review reply"))
    }

    /// Phone: commits the files of `review`, every one read whole on this
    /// phone, with `message` (cleaned here: 1 to 8,192 bytes).
    pub async fn agent_git_commit(
        &self,
        peer: DeviceId,
        thread: String,
        review: String,
        message: String,
    ) -> Result<AgentJob> {
        let message = clean_typed_text(&message).trim().to_owned();
        let body = AgentGitCommitRequest { thread, review, message };
        if !body.is_valid() {
            return Err(invalid("agent.git.commit"));
        }
        job(self.shared(), peer, types::AGENT_GIT_COMMIT, &body).await
    }

    /// Phone: pushes the thread's branch (`remote` after a `pick_remote`).
    /// `Denied` while the PC's `agent_push` is off for this phone.
    pub async fn agent_git_push(
        &self,
        peer: DeviceId,
        thread: String,
        remote: Option<String>,
    ) -> Result<AgentJob> {
        let body = AgentGitPushRequest { thread, remote };
        if !body.is_valid() {
            return Err(invalid("agent.git.push"));
        }
        job(self.shared(), peer, types::AGENT_GIT_PUSH, &body).await
    }

    /// Phone: fetches the thread's branch's remote.
    pub async fn agent_git_fetch(&self, peer: DeviceId, thread: String) -> Result<AgentJob> {
        let body = AgentGitThread { thread };
        if !body.is_valid() {
            return Err(invalid("agent.git.fetch"));
        }
        job(self.shared(), peer, types::AGENT_GIT_FETCH, &body).await
    }

    /// Phone: the branches, local first, matching `query` (one line, 100 bytes).
    pub async fn agent_git_branches(
        &self,
        peer: DeviceId,
        thread: String,
        query: Option<String>,
    ) -> Result<AgentBranches> {
        let query = query.map(|q| one_line(&q).trim().to_owned()).filter(|q| !q.is_empty());
        let body = AgentGitBranchesRequest { thread, query };
        if !body.is_valid() {
            return Err(invalid("agent.git.branches"));
        }
        let reply: AgentBranches = paged(self.shared(), peer, types::AGENT_GIT_BRANCHES, &body).await?;
        Ok(reply.sanitized())
    }

    /// Phone: switches to a listed branch, one only on a remote (`track`),
    /// or a new one (`create`). Only while nothing is changed: the PC
    /// refuses with `dirty` otherwise.
    pub async fn agent_git_switch(
        &self,
        peer: DeviceId,
        thread: String,
        branch: String,
        create: bool,
        track: bool,
    ) -> Result<AgentJob> {
        let body = AgentGitSwitchRequest { thread, branch, create, track, carry: false };
        if !body.is_valid() {
            return Err(invalid("agent.git.switch"));
        }
        job(self.shared(), peer, types::AGENT_GIT_SWITCH, &body).await
    }

    /// Phone: a job as it stands, with its newest 40 lines.
    pub async fn agent_git_job(&self, peer: DeviceId, job_id: String) -> Result<AgentJob> {
        let body = AgentGitJobRequest { job: job_id, stop: false };
        if !body.is_valid() {
            return Err(invalid("agent.git.job"));
        }
        job(self.shared(), peer, types::AGENT_GIT_JOB, &body).await
    }

    /// Phone: stops a job this phone started (a commit's hooks, a push).
    pub async fn agent_git_stop(&self, peer: DeviceId, job_id: String) -> Result<AgentJob> {
        let body = AgentGitJobRequest { job: job_id, stop: true };
        if !body.is_valid() {
            return Err(invalid("agent.git.job"));
        }
        job(self.shared(), peer, types::AGENT_GIT_JOB, &body).await
    }

    /// Phone: the thread's branch's pull request; `refresh` reads GitHub now.
    pub async fn agent_pr_view(&self, peer: DeviceId, thread: String, refresh: bool) -> Result<AgentPrView> {
        let body = AgentPrViewRequest { thread, refresh };
        if !body.is_valid() {
            return Err(invalid("agent.pr.view"));
        }
        let view: AgentPrView = single(self.shared(), peer, types::AGENT_PR_VIEW, &body).await?;
        view.sanitized().ok_or_else(|| invalid("agent.pr.view reply"))
    }

    /// Phone: opens a pull request from the thread's pushed branch.
    /// `Denied` while the PC's `agent_push` is off for this phone.
    pub async fn agent_pr_create(&self, peer: DeviceId, request: AgentPrCreateRequest) -> Result<AgentJob> {
        let mut body = request;
        body.title = one_line(&body.title).trim().to_owned();
        body.body = clean_typed_text(&body.body);
        if !body.is_valid() {
            return Err(invalid("agent.pr.create"));
        }
        job(self.shared(), peer, types::AGENT_PR_CREATE, &body).await
    }

    /// Phone: the open pull requests that are mine, wait for my review, or
    /// are of one folder's repository.
    pub async fn agent_pr_list(&self, peer: DeviceId, request: AgentPrListRequest) -> Result<AgentPrRows> {
        if !request.is_valid() {
            return Err(invalid("agent.pr.list"));
        }
        let rows: AgentPrRows = paged(self.shared(), peer, types::AGENT_PR_LIST, &request).await?;
        rows.sanitized().ok_or_else(|| invalid("agent.pr.list reply"))
    }

    /// Phone: asks the thread's agent to write a commit message or a pull
    /// request (`purpose`, one of [`git::purpose`]); its answer comes as the
    /// thread's `wrote`.
    pub async fn agent_write(&self, peer: DeviceId, thread: String, purpose: String) -> Result<AgentThread> {
        let body = AgentWriteRequest { thread, purpose };
        if !body.is_valid() {
            return Err(invalid("agent.write"));
        }
        let shared = self.shared();
        let thread: AgentThread = single(shared, peer, types::AGENT_WRITE, &body).await?;
        let thread = thread.sanitized().ok_or_else(|| invalid("agent.write reply"))?;
        crate::agent_history::thread_arrived(shared, peer, &thread).await;
        Ok(thread)
    }

    /// Phone: a thread's strip as last seen, at once and offline.
    pub async fn agent_cached_git(
        &self,
        pc: DeviceId,
        thread: String,
    ) -> Result<Cached<Option<AgentGitChanged>>> {
        crate::agent_history::cache_call(self.shared(), move |cache| cache.git(&pc, &thread)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reviewed(path: &str, bytes: u64, folder: bool, entries: u32) -> AgentGitReviewedFile {
        AgentGitReviewedFile { path: path.into(), bytes, folder, entries }
    }

    #[test]
    fn every_request_and_event_is_named() {
        for t in [
            types::AGENT_GIT_STATUS,
            types::AGENT_GIT_PATCH,
            types::AGENT_GIT_REVIEW,
            types::AGENT_GIT_COMMIT,
            types::AGENT_GIT_PUSH,
            types::AGENT_GIT_FETCH,
            types::AGENT_GIT_BRANCHES,
            types::AGENT_GIT_SWITCH,
            types::AGENT_GIT_JOB,
            types::AGENT_PR_VIEW,
            types::AGENT_PR_CREATE,
            types::AGENT_PR_LIST,
            types::AGENT_WRITE,
        ] {
            assert!(is_request(t) && !is_event(t), "{t}");
            assert!(g7link_protocol::lanes::rpc_of(t).is_some(), "{t}");
        }
        for t in [types::AGENT_GIT, types::AGENT_GIT_PROGRESS] {
            assert!(is_event(t) && !is_request(t), "{t}");
        }
        assert!(!is_request(types::AGENT_ASK), "an ask for permission is not \"ask to write it\"");
    }

    #[test]
    fn a_file_too_large_or_an_untracked_folder_is_for_the_pc() {
        assert_eq!(readable(&reviewed("a.rs", 1024, false, 0)), None);
        assert_eq!(readable(&reviewed("big.bin", 2 << 20, false, 0)), Some("large"));
        for entries in [0, 1, 3, 50, 51, 2_000] {
            assert_eq!(
                readable(&reviewed("docs/", 10, true, entries)),
                Some("folder"),
                "a phone never gets a folder's bytes ({entries} entries)"
            );
        }
    }

    #[test]
    fn a_review_commits_once_and_only_when_read_whole() {
        let reviews = Reviews::default();
        let (phone, other) = (DeviceId([1; 32]), DeviceId([2; 32]));
        let files = |paths: &[(&str, bool)]| {
            paths.iter().map(|(p, r)| ReviewFile { path: (*p).into(), readable: *r, served: false }).collect()
        };
        let review = |token: &str, files| Review {
            token: token.into(),
            thread: "t1".into(),
            host: "00000000000000aa".into(),
            files,
            made: Instant::now(),
        };
        reviews.keep(phone, review("r1", files(&[("a.rs", true), ("b.rs", true)])));
        assert!(reviews.check_patch(&phone, "r1", "t1", "a.rs").is_none());
        assert!(reviews.check_patch(&phone, "r1", "t1", "c.rs").is_some(), "not in the review");
        assert!(reviews.check_patch(&other, "r1", "t1", "a.rs").is_some(), "another phone's");
        assert!(reviews.check_patch(&phone, "r1", "t2", "a.rs").is_some(), "another thread's");
        reviews.served(&phone, "r1", "a.rs");
        assert!(reviews.take(&phone, "r1", "t1").is_err(), "b.rs was never read");
        reviews.served(&phone, "r1", "b.rs");
        assert!(reviews.take(&other, "r1", "t1").is_err());
        let taken = reviews.take(&phone, "r1", "t1").expect("read whole");
        assert!(reviews.take(&phone, "r1", "t1").is_err(), "once");
        reviews.put_back(phone, taken);
        assert!(reviews.take(&phone, "r1", "t1").is_ok(), "a refused commit may go again");

        reviews.keep(phone, review("r2", files(&[("big.bin", false)])));
        reviews.served(&phone, "r2", "big.bin");
        assert!(reviews.take(&phone, "r2", "t1").is_err(), "unreadable here");

        for n in 0..5 {
            reviews.keep(phone, review(&format!("x{n}"), files(&[("a.rs", true)])));
        }
        assert!(reviews.check_patch(&phone, "x0", "t1", "a.rs").is_some(), "four kept");
        assert!(reviews.check_patch(&phone, "x4", "t1", "a.rs").is_none());
        reviews.age(REVIEW_LIFE);
        assert!(reviews.check_patch(&phone, "x4", "t1", "a.rs").is_some(), "ten minutes");
        reviews.keep(phone, review("y", files(&[("a.rs", true)])));
        reviews.forget(&phone);
        assert!(reviews.check_patch(&phone, "y", "t1", "a.rs").is_some());
        assert!(!format!("{reviews:?}").contains("a.rs"));
    }

    #[test]
    fn a_refused_job_fails_at_once_with_its_reason() {
        let error = AgentJobError { reason: "thread_busy".into(), message: "busy".into(), names: Vec::new() };
        let job = refused_job("t1", git::kind::COMMIT, error.clone());
        assert!(job.is_valid(), "{job:?}");
        assert_eq!((job.phase.as_str(), job.mine), ("failed", true));
        assert_eq!(job.error.map(|e| e.reason), Some("thread_busy".into()));
        let env = refusal_of(&error);
        let body = env.body::<g7link_protocol::messages::ErrorBody>().unwrap();
        assert_eq!((body.code, body.msg.as_deref()), (ErrorCode::Busy, Some("thread_busy")));
    }
}
