// SPDX-License-Identifier: MPL-2.0
//! Source control in the host (phase 4, §SC5): who may ask for what
//! (§SC5.1), jobs (§SC5.2), what a job leaves in the thread (§SC5.3), "Ask
//! <agent> to write it" (§SC5.4), G7's worktree records (§SC3.8), the
//! summary each thread carries and `git.changed` (§SC3.2), and the daily
//! sweep of G7's own restore-point repositories (§SC1.2).
//!
//! Git and `gh` themselves are `scm`'s. Nothing here runs a command that
//! changes anything unless a person asked for it on a connection allowed
//! to: no timer, no turn's end, no agent, no bridge reaches an Act, except
//! the fetch of a folder whose "Keep in step with the remote" is on.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::*;
use crate::host::notify::{GitDid, Note};
use crate::host::scm::{
    self, Lines, Method, NewPr, NoRepo, Picks, Reason, Scm, ScmError, Show, Spot, Stop, SwitchTo,
};
use crate::host::store::{atomic_write, now_ms};
use crate::host::threads::{emit_item, emit_state, err, notify, random_hex, thread_dir, title_of};
use crate::host::undo::folders_overlap;
use crate::host::{Host, git};

/// The message of every row §SC5.1 refuses.
pub const DONE_ON_LINK: &str = "That is done from G7 Link on this PC.";
/// The host's own refusal words, beside `scm`'s.
pub const THREAD_BUSY: &str = "thread_busy";
pub const SHARED_BUSY: &str = "shared_busy";
pub const JOBS: &str = "jobs";
pub const LIMIT: &str = "limit";

/// How often the folders of the threads a client shows are looked at.
pub const POLL: Duration = Duration::from_secs(2);
const PROGRESS_EVERY: Duration = Duration::from_millis(1000 / JOB_EVENTS_PER_SECOND as u64);
const CHANGED_EVERY: Duration = Duration::from_millis(GIT_CHANGED_MS);
const JOB_KEPT: Duration = Duration::from_millis(JOB_KEPT_MS);
/// At shutdown, how long running jobs' kills may take to be reaped.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
const SWEEP_EVERY: Duration = Duration::from_secs(86_400);
const SWEEP_BUDGET: Duration = Duration::from_secs(60);
const FOLLOW_FETCH: Duration = Duration::from_millis(FOLLOW_FETCH_MS);
const FOLLOW_CHECKS: Duration = Duration::from_millis(FOLLOW_CHECKS_MS);
const FOLLOW_CHECKS_FOR: Duration = Duration::from_millis(FOLLOW_CHECKS_FOR_MS);
const RECORDS_FILE: &str = "worktrees.json";
const RECORDS_VERSION: u32 = 1;
const SWEPT_FILE: &str = ".swept";

/// The methods of §SC7.2 that are this module's.
pub const METHODS: [&str; 19] = [
    method::GIT_STATUS,
    method::GIT_PATCH,
    method::GIT_REVIEW,
    method::GIT_COMMIT,
    method::GIT_PUSH,
    method::GIT_FETCH,
    method::GIT_BRANCHES,
    method::GIT_SWITCH,
    method::GIT_JOB,
    method::GIT_STOP,
    method::WORKTREE_LIST,
    method::WORKTREE_REMOVE,
    method::GH_STATUS,
    method::PR_VIEW,
    method::PR_CREATE,
    method::PR_MERGE,
    method::PR_READY,
    method::PR_METHODS,
    method::PR_LIST,
];

// ---- Who may ask (§SC5.1) ----

/// A row of §SC5.1's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ask {
    /// Status, patch, review, branches, a job, `gh.status`, `pr.view`, `pr.list`.
    Read,
    Fetch,
    Commit,
    /// A push, or a pull request opened (a phone's needs its `agent_push`, which the PC's core checks).
    Push,
    /// Create a branch, or switch a clean folder.
    Branch,
    /// Switch with changes.
    Carry,
    /// Start a thread in a new worktree.
    NewWorktree,
    ListWorktrees,
    RemoveWorktree,
    /// Merge, mark ready, the merge methods.
    Merge,
    FollowRemote,
}

impl Ask {
    pub const ALL: [Ask; 11] = [
        Ask::Read,
        Ask::Fetch,
        Ask::Commit,
        Ask::Push,
        Ask::Branch,
        Ask::Carry,
        Ask::NewWorktree,
        Ask::ListWorktrees,
        Ask::RemoveWorktree,
        Ask::Merge,
        Ask::FollowRemote,
    ];
}

/// Whether a connection of `kind`, asking for `by`, may do `ask`. A phone
/// (`by` a phone) reaches the host only through G7 Link. An Act is a
/// person's request: never the bridge's, the CLI's or a test connection's.
pub fn may(kind: ClientKind, by: Option<&Origin>, ask: Ask) -> bool {
    let phone = by.is_some_and(|b| b.kind == OriginKind::Phone);
    if kind == ClientKind::Bridge || (phone && kind != ClientKind::Link) {
        return false;
    }
    let pc = matches!(kind, ClientKind::Link | ClientKind::Launcher);
    match ask {
        Ask::Read => true,
        Ask::Fetch | Ask::Commit | Ask::Push | Ask::Branch => pc,
        Ask::NewWorktree | Ask::ListWorktrees | Ask::FollowRemote => pc && !phone,
        Ask::Carry | Ask::RemoveWorktree | Ask::Merge => kind == ClientKind::Link && !phone,
    }
}

/// [`may`], as the error a refused row answers.
pub(crate) fn check_may(kind: ClientKind, by: Option<&Origin>, ask: Ask) -> Result<(), RpcError> {
    if by.is_some_and(|b| !b.is_valid()) {
        return Err(err(ErrorCode::BadRequest, "Bad origin."));
    }
    if may(kind, by, ask) { Ok(()) } else { Err(err(ErrorCode::Denied, DONE_ON_LINK)) }
}

// ---- Refusals ----

/// A refusal before anything ran: `data.reason` is the word.
pub(crate) fn refusal(code: ErrorCode, reason: &str) -> RpcError {
    RpcError { data: Some(json!({ "reason": reason })), ..RpcError::new(code, words(reason)) }
}

fn scm_refusal(e: ScmError) -> RpcError {
    let mut refused = refusal(e.reason.code(), e.reason.as_str());
    if e.reason == Reason::PickRemote {
        refused.data = Some(json!({ "reason": e.reason.as_str(), "names": e.lines }));
    }
    refused
}

fn repo_refusal(no: NoRepo) -> RpcError {
    scm_refusal(ScmError::new(no_repo_reason(no)))
}

fn no_repo_reason(no: NoRepo) -> Reason {
    match no {
        NoRepo::None => Reason::NotRepo,
        NoRepo::Refused => Reason::Refused,
        NoRepo::NoGit => Reason::NoGit,
    }
}

/// The reason word of a refusal, if it is one.
pub(crate) fn reason_of(e: &RpcError) -> Option<&str> {
    e.data.as_ref()?.get("reason")?.as_str()
}

/// What a client says for a reason word (§SC9.4's wording, short).
pub fn words(reason: &str) -> &'static str {
    match reason {
        "git_busy" => "Git is busy in this folder. Try again in a moment.",
        "doing" => "Git is in the middle of something here. Finish it in a terminal.",
        "detached" => "You're not on a branch. Create one to keep new commits.",
        "conflicts" => "Some files have conflicts. Resolve them first.",
        "nothing" => "Nothing to commit.",
        "identity" => "Git doesn't know your name and email. Set them in a terminal.",
        "changed" => "The files changed while you were looking.",
        THREAD_BUSY => "Wait until the agent finishes.",
        SHARED_BUSY => "Another thread is working in this folder.",
        JOBS => "Four source-control jobs are running. Try again when one ends.",
        LIMIT => "There are too many worktrees. Remove one in Setup.",
        "hook" => "A hook stopped it.",
        "stopped" => "It was stopped. Some files may be left staged.",
        "slow" => "It took too long.",
        "behind" => "The remote has commits you don't have.",
        "remote_refused" => "The remote refused the push.",
        "sign_in" => "Git couldn't sign in to the remote. Push once from a terminal to set it up.",
        "network" => "The remote couldn't be reached.",
        "pick_remote" => "Choose a remote.",
        "no_remote" => "This repository has no remote.",
        "no_commits" => "No commits yet.",
        "no_upstream" | "not_pushed" => "Push this branch to open a pull request.",
        "dirty" => "This folder has changes.",
        "would_overwrite" => "Git didn't switch: these files would be overwritten. Commit them first.",
        "in_worktree" => "This thread works in its own worktree.",
        "exists" => "A branch with that name exists.",
        "bad_name" => "That name can't be a branch.",
        "not_found" => "That isn't there any more.",
        "bad_message" => "The message must be 1 to 8192 bytes.",
        "large" => "That is too large.",
        "not_repo" => "This folder isn't a git repository.",
        "refused" => "Git won't work in this folder. It says the folder isn't safe.",
        "no_git" => "Git isn't installed on this PC.",
        "not_github" => "This folder's remote isn't on GitHub.",
        "no_gh" => "Pull requests need the GitHub CLI.",
        "signed_out" => "You're signed out of the GitHub CLI.",
        "old_gh" => "Your GitHub CLI is too old for this. Update it.",
        "pr_exists" => "This branch already has a pull request.",
        "not_allowed" => "This repository doesn't allow that way of merging.",
        "unavailable" => "Source control isn't available right now.",
        "bad_request" => "That request isn't valid.",
        _ => "It didn't go through.",
    }
}

// ---- Text ----

/// The agent's answer as the commit or pull-request field takes it
/// (§SC5.4): the first fenced block's content when it has one, a leading
/// "Subject:" taken off, cleaned and cut to [`MAX_WROTE`] bytes.
pub fn wrote_text(answer: &str) -> String {
    let fenced = first_fence(answer);
    let text = fenced.as_deref().unwrap_or(answer).trim();
    let text = match text.get(..8) {
        Some(head) if head.eq_ignore_ascii_case("subject:") => text[8..].trim_start(),
        _ => text,
    };
    clean_cut(text.trim(), MAX_WROTE).0
}

/// The lines between the first fence and the next fence of the same kind.
fn first_fence(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let fence_char = |line: &str| {
        let t = line.trim_start();
        ["```", "~~~"].iter().find(|f| t.starts_with(**f)).and_then(|f| f.chars().next())
    };
    let open = lines.iter().position(|l| fence_char(l).is_some())?;
    let mark = fence_char(lines[open])?;
    let rest = lines.get(open + 1..)?;
    let close = rest.iter().position(|l| {
        let t = l.trim();
        t.len() >= 3 && t.chars().all(|c| c == mark)
    })?;
    Some(rest[..close].join("\n"))
}

/// A pull request's base as `scm` looks it up: `origin/main` as it came,
/// `main` (how the launcher names it) as the push remote's branch.
pub fn base_ref(base: &str, push_remote: &str, remotes: &[&str]) -> String {
    let named = remotes
        .iter()
        .any(|r| base.strip_prefix(r).and_then(|rest| rest.strip_prefix('/')).is_some_and(|b| !b.is_empty()));
    if named || push_remote.is_empty() { base.to_owned() } else { format!("{push_remote}/{base}") }
}

/// The message "Ask <agent> to write it" sends (§SC5.4).
pub fn ask_text(purpose: Purpose, branch: &str, base: Option<&str>) -> String {
    match purpose {
        Purpose::Commit => ASK_COMMIT_TEXT.to_owned(),
        Purpose::Pr => ASK_PR_TEXT.replace("%1", branch).replace("%2", base.unwrap_or(ASK_PR_DEFAULT_BASE)),
    }
}

/// The notice a job that went through leaves in its thread (§SC5.3).
pub fn notice_text(job: &Job) -> Option<(String, NoticeKind)> {
    let result = job.result.as_ref().filter(|_| job.phase == JobPhase::Done)?;
    let (text, kind) = match job.kind {
        JobKind::Commit => {
            let files = result.files.unwrap_or(0);
            let noun = if files == 1 { "file" } else { "files" };
            let subject = result.subject.as_deref().unwrap_or_default();
            (format!("Committed {files} {noun}: {subject}"), NoticeKind::Committed)
        }
        JobKind::Push if !result.up_to_date => {
            let branch = result.branch.as_deref().unwrap_or_default();
            let remote = result.remote.as_deref().unwrap_or_default();
            (format!("Pushed {branch} to {remote}"), NoticeKind::Pushed)
        }
        JobKind::Switch => {
            let branch = result.branch.as_deref().unwrap_or_default();
            let text =
                if result.created { format!("Created {branch}") } else { format!("Switched to {branch}") };
            (text, NoticeKind::Switched)
        }
        JobKind::PrCreate => {
            let number = result.pr.as_ref().map(|p| p.number).or(result.number)?;
            (format!("Opened pull request #{number}"), NoticeKind::PrOpened)
        }
        JobKind::PrMerge => (format!("Merged pull request #{}", result.number?), NoticeKind::PrMerged),
        JobKind::Push | JobKind::Fetch | JobKind::Worktree => return None,
    };
    let text = match phone_name(&job.by) {
        Some(device) => format!("{text} · from {device}"),
        None => text,
    };
    Some((clean_cut(&text, MAX_ITEM_TEXT).0, kind))
}

fn phone_name(by: &Origin) -> Option<&str> {
    (by.kind == OriginKind::Phone).then(|| by.device_name.as_deref().unwrap_or("a phone"))
}

// ---- Worktree records ----

/// A G7 worktree as `worktrees.json` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub folder_id: String,
    /// The work tree's top level, under G7's worktrees folder.
    pub path: String,
    /// Where its thread works: `path`, and the agent folder's sub-folder of
    /// its repository when the agent folder is one.
    pub dir: String,
    pub branch: String,
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

impl Record {
    pub fn tag(&self) -> WorktreeTag {
        WorktreeTag {
            id: self.id.clone(),
            branch: self.branch.clone(),
            label: label_of(Path::new(&self.path)),
            missing: !Path::new(&self.dir).is_dir(),
        }
    }
}

fn label_of(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    clean_cut(&name, MAX_LABEL).0
}

#[derive(Serialize, Deserialize)]
struct RecordsFile {
    version: u32,
    worktrees: Vec<Record>,
}

/// The records in `data_dir`, those that make sense, at most [`MAX_WORKTREES`].
fn load_records(data_dir: &Path) -> Vec<Record> {
    let root = data_dir.join("worktrees");
    let Ok(bytes) = std::fs::read(data_dir.join(RECORDS_FILE)) else { return Vec::new() };
    let Ok(file) = serde_json::from_slice::<RecordsFile>(&bytes) else {
        tracing::warn!("couldn't read the worktree list; it is kept aside");
        let _ = std::fs::rename(data_dir.join(RECORDS_FILE), data_dir.join("worktrees.json.unreadable"));
        return Vec::new();
    };
    let mut seen = HashSet::new();
    file.worktrees
        .into_iter()
        .filter(|r| {
            is_valid_worktree_id(&r.id)
                && is_valid_folder_id(&r.folder_id)
                && Path::new(&r.path).starts_with(&root)
                && Path::new(&r.dir).starts_with(&r.path)
                && r.branch.len() <= MAX_BRANCH_NAME
                && r.thread_id.as_deref().is_none_or(is_valid_thread_id)
                && seen.insert(r.id.clone())
        })
        .take(MAX_WORKTREES)
        .collect()
}

fn save_records(data_dir: &Path, records: &[Record]) -> std::io::Result<()> {
    let file = RecordsFile { version: RECORDS_VERSION, worktrees: records.to_vec() };
    let bytes = serde_json::to_vec(&file).map_err(std::io::Error::other)?;
    atomic_write(&data_dir.join(RECORDS_FILE), &bytes)
}

// ---- State ----

/// What the host last read of a thread's folder.
#[derive(Debug, Clone, Default)]
struct Known {
    spot: Option<Spot>,
    status: GitStatus,
    /// `git.changed` as last sent, and when.
    shown: Option<GitChangedEvent>,
    shown_at: Option<Instant>,
    /// A `git.changed` waits for the second to pass.
    waiting: bool,
    marks: Marks,
    pr: Option<PrBrief>,
    /// "Keep in step": the last fetch and check read.
    fetched: Option<Instant>,
    checks_read: Option<Instant>,
    pending_since: Option<Instant>,
}

/// `HEAD`'s and the index's modification times.
type Marks = (Option<SystemTime>, Option<SystemTime>);

fn marks_of(spot: &Spot) -> Marks {
    let at = |name: &str| spot.git_dir().join(name).metadata().and_then(|m| m.modified()).ok();
    (at("HEAD"), at("index"))
}

struct Slot {
    /// Without its lines: those are in `lines`.
    job: Job,
    lines: Lines,
    stop: Stop,
    /// The work tree's top level: one job each.
    tree: PathBuf,
    ended: Option<Instant>,
}

impl Slot {
    fn running(&self) -> bool {
        self.job.phase == JobPhase::Running
    }

    /// The job with its lines numbered `from` and later.
    fn snapshot(&self, from: u64) -> Job {
        let (lines, next) = self.lines.since(from);
        let first = next.saturating_sub(lines.len() as u64);
        Job { lines, from: first, dropped: self.lines.dropped(), ..self.job.clone() }
    }
}

/// Source control's state in the host.
#[derive(Default)]
pub struct Source {
    data_dir: PathBuf,
    jobs: Mutex<HashMap<String, Slot>>,
    known: Mutex<HashMap<String, Known>>,
    records: Arc<Mutex<Vec<Record>>>,
    /// Serializes writes of `worktrees.json`; each writes the list as it is then.
    records_write: Arc<Mutex<()>>,
    /// The poll runs one at a time.
    polling: tokio::sync::Mutex<()>,
    sweeping: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source").field("jobs", &self.lock_jobs().len()).finish_non_exhaustive()
    }
}

impl Source {
    /// The worktree records under `data_dir`.
    pub fn load(data_dir: &Path) -> Source {
        Source {
            data_dir: data_dir.to_owned(),
            records: Arc::new(Mutex::new(load_records(data_dir))),
            ..Source::default()
        }
    }

    /// G7's worktrees folder (`$XDG_DATA_HOME/g7-agents/worktrees`).
    pub fn root(&self) -> PathBuf {
        self.data_dir.join("worktrees")
    }

    fn lock_jobs(&self) -> MutexGuard<'_, HashMap<String, Slot>> {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_known(&self) -> MutexGuard<'_, HashMap<String, Known>> {
        self.known.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn lock_records(&self) -> MutexGuard<'_, Vec<Record>> {
        self.records.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn record(&self, id: &str) -> Option<Record> {
        self.lock_records().iter().find(|r| r.id == id).cloned()
    }

    /// A thread is gone: what was read of it goes, and its worktree is no
    /// longer anyone's (the worktree itself stays).
    pub(crate) fn forget_thread(&self, thread_id: &str) -> bool {
        self.lock_known().remove(thread_id);
        let mut records = self.lock_records();
        let mut changed = false;
        for record in records.iter_mut().filter(|r| r.thread_id.as_deref() == Some(thread_id)) {
            record.thread_id = None;
            changed = true;
        }
        changed
    }

    /// Writes `worktrees.json` as the list is when the write runs (off the async threads).
    fn save(&self) -> tokio::task::JoinHandle<std::io::Result<()>> {
        let (records, serial, data_dir) =
            (self.records.clone(), self.records_write.clone(), self.data_dir.clone());
        tokio::task::spawn_blocking(move || {
            let _serial = serial.lock().unwrap_or_else(PoisonError::into_inner);
            let list = records.lock().unwrap_or_else(PoisonError::into_inner).clone();
            save_records(&data_dir, &list)
        })
    }

    /// [`Source::save`], not waited for.
    pub(crate) fn save_later(&self) {
        let saving = self.save();
        tokio::spawn(async move {
            if !matches!(saving.await, Ok(Ok(()))) {
                tracing::warn!("couldn't save the worktree list");
            }
        });
    }

    /// Jobs ended more than [`JOB_KEPT_MS`] ago are forgotten.
    fn sweep_jobs(jobs: &mut HashMap<String, Slot>) {
        jobs.retain(|_, s| s.ended.is_none_or(|at| at.elapsed() < JOB_KEPT));
    }

    pub(crate) fn running_jobs(&self) -> usize {
        self.lock_jobs().values().filter(|s| s.running()).count()
    }
}

/// A thread as source control sees it.
#[derive(Debug, Clone)]
struct Where {
    cwd: Option<PathBuf>,
    agent_folder: Option<PathBuf>,
    worktree: Option<WorktreeTag>,
    readonly: bool,
    running: bool,
    queued: bool,
    branch: Option<String>,
}

type Work = Pin<Box<dyn Future<Output = Result<JobResult, ScmError>> + Send>>;

/// A worktree's size (bytes, and whether the count stopped at its bound)
/// and its commits on no remote, as `worktree.list` reads them.
type Measured = (Option<(u64, bool)>, Option<u32>);

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params).map_err(|e| err(ErrorCode::BadRequest, e.to_string()))
}

fn to_value<T: Serialize>(value: &T) -> Result<Value, RpcError> {
    serde_json::to_value(value).map_err(|e| err(ErrorCode::Internal, e.to_string()))
}

fn no_thread() -> RpcError {
    err(ErrorCode::NotFound, "No such thread.")
}

fn read_only() -> RpcError {
    err(ErrorCode::Denied, "This thread can be read, not continued.")
}

/// A name a client sends for a branch or remote: one line, no control
/// character, within `max` bytes. `scm` looks it up before it is used.
fn name_ok(name: &str, max: usize) -> bool {
    !name.is_empty() && name.len() <= max && !name.chars().any(|c| c.is_control() || is_stripped_char(c))
}

fn merge_method(m: MergeMethod) -> Method {
    match m {
        MergeMethod::Squash => Method::Squash,
        MergeMethod::Merge => Method::Merge,
        MergeMethod::Rebase => Method::Rebase,
    }
}

fn show_of(s: PrShow) -> Show {
    match s {
        PrShow::Mine => Show::Mine,
        PrShow::Review => Show::Review,
        PrShow::Folder => Show::Folder,
    }
}

fn changed_event(thread_id: &str, status: &GitStatus, pr: Option<PrBrief>) -> GitChangedEvent {
    let ok = status.repo == RepoState::Ok;
    GitChangedEvent {
        thread_id: thread_id.to_owned(),
        branch: status.branch.clone().filter(|_| ok),
        detached: ok && status.detached.is_some(),
        changed: if ok { status.total } else { 0 },
        ahead: status.ahead.filter(|_| ok),
        behind: status.behind.filter(|_| ok),
        pr: pr.filter(|_| ok),
    }
}

impl Host {
    fn scm(&self) -> Result<&Arc<Scm>, RpcError> {
        self.scm.as_ref().ok_or_else(|| refusal(ErrorCode::Unavailable, "no_git"))
    }

    // ---- Where a thread works ----

    /// At start: each worktree thread works in its worktree, as its record
    /// says; one whose worktree is gone is read-only and says so.
    pub(crate) fn attach_worktrees(&self) {
        let records = self.source.lock_records().clone();
        let mut reg = self.lock_reg();
        for entry in reg.threads.values_mut() {
            let Some(tag) = entry.info.worktree.as_mut() else { continue };
            let record = records.iter().find(|r| r.id == tag.id);
            entry.worktree_dir = record.map(|r| PathBuf::from(&r.dir));
            tag.missing = entry.worktree_dir.as_deref().is_none_or(|d| !d.is_dir());
            entry.cwd = match &entry.worktree_dir {
                Some(dir) if !tag.missing => thread_dir(dir, entry.info.subdir.as_deref()),
                _ => None,
            };
            if tag.missing && entry.info.readonly.is_none() {
                entry.info.readonly = Some(Readonly::FolderMissing);
            }
        }
    }

    fn git_where(&self, thread_id: &str) -> Result<Where, RpcError> {
        if !is_valid_thread_id(thread_id) {
            return Err(no_thread());
        }
        let reg = self.lock_reg();
        let entry = reg.threads.get(thread_id).ok_or_else(no_thread)?;
        let folder = reg.folders.iter().find(|f| f.id == entry.info.folder_id);
        let cwd = if entry.info.worktree.is_some() {
            reg.work_dir(entry)
        } else {
            reg.work_dir(entry).or_else(|| entry.cwd.clone())
        };
        Ok(Where {
            cwd,
            agent_folder: folder.map(|f| PathBuf::from(&f.path)),
            worktree: entry.info.worktree.clone(),
            readonly: entry.info.readonly.is_some(),
            running: entry.info.state.is_running(),
            queued: !entry.info.queued.is_empty(),
            branch: entry.info.branch.clone(),
        })
    }

    async fn git_spot(&self, w: &Where) -> Result<Spot, NoRepo> {
        let Some(scm) = &self.scm else { return Err(NoRepo::NoGit) };
        let (Some(cwd), Some(agent_folder)) = (&w.cwd, &w.agent_folder) else { return Err(NoRepo::None) };
        if !cwd.is_dir() {
            return Err(NoRepo::None);
        }
        let spot = scm.spot_within(cwd, agent_folder, &self.source.root()).await?;
        // A worktree thread works in its worktree, nowhere else.
        if w.worktree.is_some() && !spot.top().starts_with(self.source.root()) {
            return Err(NoRepo::Refused);
        }
        Ok(spot)
    }

    async fn folder_spot(&self, folder_id: &str) -> Result<Spot, RpcError> {
        let path = {
            let reg = self.lock_reg();
            if !is_valid_folder_id(folder_id) {
                return Err(err(ErrorCode::NotFound, "No such folder."));
            }
            reg.folders
                .iter()
                .find(|f| f.id == folder_id)
                .map(|f| PathBuf::from(&f.path))
                .ok_or_else(|| err(ErrorCode::NotFound, "No such folder."))?
        };
        let scm = self.scm()?;
        if !path.is_dir() {
            return Err(repo_refusal(NoRepo::None));
        }
        scm.spot_within(&path, &path, &self.source.root()).await.map_err(repo_refusal)
    }

    /// The thread's folder's status, read now and kept: the thread's
    /// summary fields follow, and `git.changed` when the summary changed.
    pub(crate) async fn git_refresh(
        self: &Arc<Self>,
        thread_id: &str,
    ) -> Result<(Option<Spot>, GitStatus), RpcError> {
        let w = self.git_where(thread_id)?;
        let (spot, mut status) = match self.git_spot(&w).await {
            Ok(spot) => {
                let status = self.scm()?.status(&spot).await.map_err(scm_refusal)?;
                (Some(spot), status)
            }
            Err(no) => (None, GitStatus::without(no.repo())),
        };
        status.worktree = w.worktree.clone();
        self.git_keep(thread_id, spot.clone(), &status);
        Ok((spot, status))
    }

    /// The thread's last status, else one read now; with a spot.
    async fn git_known(self: &Arc<Self>, thread_id: &str) -> Result<(Spot, GitStatus), RpcError> {
        let kept =
            self.source.lock_known().get(thread_id).and_then(|k| Some((k.spot.clone()?, k.status.clone())));
        let (spot, status) = match kept {
            Some(kept) => (Some(kept.0), kept.1),
            None => self.git_refresh(thread_id).await?,
        };
        match spot {
            Some(spot) => Ok((spot, status)),
            None => Err(repo_refusal(match status.repo {
                RepoState::Refused => NoRepo::Refused,
                RepoState::NoGit => NoRepo::NoGit,
                _ => NoRepo::None,
            })),
        }
    }

    /// The thread's folder now, refused unless it is a repository.
    async fn git_fresh(self: &Arc<Self>, thread_id: &str) -> Result<(Spot, GitStatus), RpcError> {
        match self.git_refresh(thread_id).await? {
            (Some(spot), status) => Ok((spot, status)),
            (None, status) => Err(repo_refusal(match status.repo {
                RepoState::Refused => NoRepo::Refused,
                RepoState::NoGit => NoRepo::NoGit,
                _ => NoRepo::None,
            })),
        }
    }

    fn git_keep(self: &Arc<Self>, thread_id: &str, spot: Option<Spot>, status: &GitStatus) {
        let pr = match (&spot, &status.branch, &self.scm) {
            (Some(spot), Some(branch), Some(scm)) => scm.cached_pr(spot, branch).map(|p| p.brief()),
            _ => None,
        };
        let ok = status.repo == RepoState::Ok;
        {
            let mut reg = self.lock_reg();
            let Some(entry) = reg.threads.get_mut(thread_id) else { return };
            let branch = status.branch.clone().filter(|_| ok);
            let changed = ok.then_some(status.total);
            if entry.info.branch != branch || entry.info.changed != changed {
                entry.info.branch = branch;
                entry.info.changed = changed;
                emit_state(self, entry);
            }
        }
        let marks = spot.as_ref().map(marks_of).unwrap_or_default();
        {
            let mut known = self.source.lock_known();
            let kept = known.entry(thread_id.to_owned()).or_default();
            kept.spot = spot;
            kept.status = status.clone();
            kept.marks = marks;
            kept.pr = pr;
        }
        self.git_changed(thread_id);
    }

    /// Sends `git.changed` when the thread's summary differs from what was
    /// last sent, at most once a second.
    fn git_changed(self: &Arc<Self>, thread_id: &str) {
        let send = {
            let mut known = self.source.lock_known();
            let Some(kept) = known.get_mut(thread_id) else { return };
            let event = changed_event(thread_id, &kept.status, kept.pr);
            // Before anything was sent, "no repository, nothing changed" is what clients assume.
            let unsent = changed_event(thread_id, &GitStatus::default(), None);
            let shown = kept.shown.as_ref().unwrap_or(&unsent);
            if *shown == event {
                kept.shown = Some(event);
                None
            } else if kept.shown_at.is_none_or(|at| at.elapsed() >= CHANGED_EVERY) {
                kept.shown = Some(event.clone());
                kept.shown_at = Some(Instant::now());
                Some(Ok(event))
            } else if !kept.waiting {
                kept.waiting = true;
                Some(Err(
                    CHANGED_EVERY.saturating_sub(kept.shown_at.map_or(Duration::ZERO, |at| at.elapsed()))
                ))
            } else {
                None
            }
        };
        match send {
            Some(Ok(event)) => self.hub.broadcast(&Event::GitChanged(event)),
            Some(Err(wait)) => {
                let host = Arc::downgrade(self);
                let thread = thread_id.to_owned();
                tokio::spawn(async move {
                    tokio::time::sleep(wait).await;
                    let Some(host) = host.upgrade() else { return };
                    if let Some(kept) = host.source.lock_known().get_mut(&thread) {
                        kept.waiting = false;
                    }
                    host.git_changed(&thread);
                });
            }
            None => {}
        }
    }

    fn git_set_pr(self: &Arc<Self>, thread_id: &str, pr: Option<&PrInfo>) {
        if let Some(kept) = self.source.lock_known().get_mut(thread_id) {
            kept.pr = pr.map(PrInfo::brief);
            kept.checks_read = Some(Instant::now());
            match pr.filter(|p| p.pending > 0) {
                Some(_) => {
                    kept.pending_since.get_or_insert_with(Instant::now);
                }
                None => kept.pending_since = None,
            }
        }
        self.git_changed(thread_id);
    }

    // ---- Checks the host makes before an Act (§SC3.4, §SC3.7) ----

    /// Another of G7's threads, or a followed session, works in the same work tree.
    async fn check_shared(self: &Arc<Self>, thread_id: &str, spot: &Spot) -> Result<(), RpcError> {
        let top = spot.top().to_owned();
        let threads = {
            let reg = self.lock_reg();
            reg.threads.iter().filter(|(id, _)| id.as_str() != thread_id).any(|(_, e)| {
                e.info.state.is_running() && e.cwd.as_deref().is_some_and(|cwd| folders_overlap(cwd, &top))
            })
        };
        let sessions = !threads
            && self
                .sessions_list(MAX_SESSIONS)
                .await
                .iter()
                .any(|s| s.state == SessionState::Working && folders_overlap(Path::new(&s.cwd), &top));
        if threads || sessions { Err(refusal(ErrorCode::Busy, SHARED_BUSY)) } else { Ok(()) }
    }

    // ---- Jobs (§SC5.2) ----

    /// A new job, unless one runs in the same work tree or four run.
    fn open_job(
        &self,
        thread_id: &str,
        kind: JobKind,
        by: Origin,
        tree: &Path,
    ) -> Result<(String, Lines, Stop), RpcError> {
        let mut jobs = self.source.lock_jobs();
        Source::sweep_jobs(&mut jobs);
        if let Some(same) = jobs.values().find(|s| s.running() && s.tree == tree) {
            let kind = serde_json::to_value(same.job.kind).ok();
            let word = kind.as_ref().and_then(Value::as_str).unwrap_or("job");
            let mut busy = refusal(ErrorCode::Busy, "git_busy");
            busy.message = format!("A {} is running in this folder.", word.replace('_', " "));
            return Err(busy);
        }
        if jobs.values().filter(|s| s.running()).count() >= MAX_JOBS_RUNNING {
            return Err(refusal(ErrorCode::Busy, JOBS));
        }
        let mut id = random_hex(GIT_JOB_LEN / 2);
        while jobs.contains_key(&id) {
            id = random_hex(GIT_JOB_LEN / 2);
        }
        let (lines, stop) = (Lines::new(), Stop::new());
        let job = Job {
            id: id.clone(),
            thread_id: thread_id.to_owned(),
            kind,
            phase: JobPhase::Running,
            started_ms: now_ms(),
            ended_ms: None,
            by,
            lines: Vec::new(),
            from: 0,
            dropped: 0,
            result: None,
            error: None,
        };
        let slot = Slot { job, lines: lines.clone(), stop: stop.clone(), tree: tree.to_owned(), ended: None };
        jobs.insert(id.clone(), slot);
        Ok((id, lines, stop))
    }

    /// Runs `work` as job `id`: its lines go out as `git.progress` at most
    /// four times a second, the last with the final phase. The job as it
    /// starts, and its task.
    fn run_job(self: &Arc<Self>, id: String, work: Work) -> (Job, tokio::task::JoinHandle<()>) {
        let (job, lines) = {
            let jobs = self.source.lock_jobs();
            let slot = jobs.get(&id);
            (slot.map(|s| s.snapshot(0)), slot.map(|s| s.lines.clone()).unwrap_or_default())
        };
        let host = self.clone();
        let task_id = id.clone();
        let task = tokio::spawn(async move {
            let mut work = work;
            let mut tick = tokio::time::interval(PROGRESS_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            let mut sent = 0u64;
            let outcome = loop {
                tokio::select! {
                    outcome = &mut work => break outcome,
                    _ = tick.tick() => {
                        if lines.next() > sent {
                            sent = host.job_progress(&task_id, sent);
                        }
                    }
                }
            };
            host.end_job(&task_id, outcome, sent);
        });
        let job = job.unwrap_or_else(|| unreachable_job(&id));
        (job, task)
    }

    fn job_progress(&self, id: &str, from: u64) -> u64 {
        let (job, next) = {
            let jobs = self.source.lock_jobs();
            let Some(slot) = jobs.get(id) else { return from };
            (slot.snapshot(from), slot.lines.next())
        };
        if !job.lines.is_empty() {
            self.hub.broadcast(&Event::GitProgress(GitProgressEvent { job }));
        }
        next
    }

    fn end_job(self: &Arc<Self>, id: &str, outcome: Result<JobResult, ScmError>, sent: u64) {
        let job = {
            let mut jobs = self.source.lock_jobs();
            let Some(slot) = jobs.get_mut(id) else { return };
            slot.ended = Some(Instant::now());
            slot.job.ended_ms = Some(now_ms());
            match outcome {
                Ok(result) => {
                    slot.job.phase = JobPhase::Done;
                    slot.job.result = Some(result);
                }
                Err(e) => {
                    slot.job.phase =
                        if e.reason == Reason::Stopped { JobPhase::Stopped } else { JobPhase::Failed };
                    let names = if e.reason == Reason::PickRemote { e.lines } else { Vec::new() };
                    slot.job.error = Some(JobError {
                        reason: e.reason.as_str().to_owned(),
                        message: words(e.reason.as_str()).to_owned(),
                        names,
                    });
                }
            }
            slot.snapshot(sent)
        };
        self.hub.broadcast(&Event::GitProgress(GitProgressEvent { job: job.clone() }));
        self.after_job(&job);
    }

    /// What a job leaves (§SC5.3): a notice when it went through, the PC's
    /// notification for a phone's commit, push or pull request, and the
    /// folder read again after every Act.
    fn after_job(self: &Arc<Self>, job: &Job) {
        let thread_id = job.thread_id.clone();
        if job.phase == JobPhase::Done {
            if let Some((text, kind)) = notice_text(job) {
                let mut reg = self.lock_reg();
                if let Some(entry) = reg.threads.get_mut(&thread_id) {
                    let item = entry.marked_notice(&text, kind);
                    if matches!(job.kind, JobKind::Commit | JobKind::PrCreate) {
                        entry.info.wrote = None;
                    }
                    emit_state(self, entry);
                    emit_item(self, &thread_id, &item);
                    if let Some(device) = phone_name(&job.by) {
                        let did = match (job.kind, job.result.as_ref()) {
                            (JobKind::Commit, Some(r)) => {
                                Some(GitDid::Committed { files: r.files.unwrap_or(0) })
                            }
                            (JobKind::Push, Some(r)) => {
                                Some(GitDid::Pushed { branch: r.branch.clone().unwrap_or_default() })
                            }
                            (JobKind::PrCreate, Some(r)) => {
                                r.pr.as_ref().map(|p| GitDid::OpenedPr { number: p.number })
                            }
                            _ => None,
                        };
                        if let Some(did) = did {
                            notify(
                                self,
                                Note::PhoneGit { thread: entry.info.clone(), device: device.to_owned(), did },
                            );
                        }
                    }
                }
            }
            if let Some(pr) = job.result.as_ref().and_then(|r| r.pr.as_ref()) {
                self.git_set_pr(&thread_id, Some(pr));
            }
        }
        if job.kind != JobKind::Worktree {
            let host = self.clone();
            tokio::spawn(async move {
                let _ = host.git_refresh(&thread_id).await;
            });
        }
    }

    fn job_of(&self, id: &str) -> Result<Job, RpcError> {
        if !is_valid_job_id(id) {
            return Err(err(ErrorCode::NotFound, "That job isn't kept any more."));
        }
        let mut jobs = self.source.lock_jobs();
        Source::sweep_jobs(&mut jobs);
        jobs.get(id)
            .map(|s| s.snapshot(0))
            .ok_or_else(|| err(ErrorCode::NotFound, "That job isn't kept any more."))
    }

    /// `git.stop`: by the job's own device, or by G7 Link on this PC.
    fn job_stop(&self, kind: ClientKind, p: GitJobParams) -> Result<Job, RpcError> {
        if p.by.as_ref().is_some_and(|b| !b.is_valid()) {
            return Err(err(ErrorCode::BadRequest, "Bad origin."));
        }
        let jobs = self.source.lock_jobs();
        let slot =
            jobs.get(&p.job).ok_or_else(|| err(ErrorCode::NotFound, "That job isn't kept any more."))?;
        let asker_phone = p.by.as_ref().filter(|b| b.kind == OriginKind::Phone);
        let allowed = match (kind, asker_phone) {
            (ClientKind::Link, None) => true,
            (ClientKind::Link, Some(phone)) => {
                phone.device_id.is_some()
                    && slot.job.by.kind == OriginKind::Phone
                    && phone.device_id == slot.job.by.device_id
            }
            (ClientKind::Launcher, None) => slot.job.by.kind != OriginKind::Phone,
            _ => false,
        };
        if !allowed {
            return Err(err(ErrorCode::Denied, "Only who started it, or G7 Link on this PC, can stop it."));
        }
        slot.stop.stop();
        Ok(slot.snapshot(0))
    }

    /// At shutdown: running jobs are stopped, and their kills waited for
    /// up to five seconds.
    pub(crate) async fn stop_jobs(&self) {
        for slot in self.source.lock_jobs().values().filter(|s| s.running()) {
            slot.stop.stop();
        }
        let until = Instant::now() + SHUTDOWN_WAIT;
        while self.source.running_jobs() > 0 && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    // ---- The methods (§SC7.2) ----

    /// One of [`METHODS`], for a connection of `kind`.
    pub(crate) async fn git_call(
        self: &Arc<Self>,
        kind: ClientKind,
        method_name: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        match method_name {
            method::GIT_STATUS => {
                check_may(kind, None, Ask::Read)?;
                let p: GitStatusParams = parse(params)?;
                let status = match (p.thread_id, p.folder_id) {
                    (Some(thread), None) => self.git_refresh(&thread).await?.1,
                    (None, Some(folder)) => self.folder_status(&folder).await?,
                    _ => return Err(err(ErrorCode::BadRequest, "Name a thread or a folder.")),
                };
                to_value(&GitStatusResult { status })
            }
            method::GIT_PATCH => {
                check_may(kind, None, Ask::Read)?;
                to_value(&self.git_patch(parse(params)?).await?)
            }
            method::GIT_REVIEW => {
                check_may(kind, None, Ask::Read)?;
                to_value(&self.git_review(parse(params)?).await?)
            }
            method::GIT_COMMIT => {
                to_value(&GitJobResult { job: self.git_commit(kind, parse(params)?).await? })
            }
            method::GIT_PUSH => to_value(&GitJobResult { job: self.git_push(kind, parse(params)?).await? }),
            method::GIT_FETCH => to_value(&GitJobResult { job: self.git_fetch(kind, parse(params)?).await? }),
            method::GIT_BRANCHES => {
                check_may(kind, None, Ask::Read)?;
                to_value(&self.git_branches(parse(params)?).await?)
            }
            method::GIT_SWITCH => {
                to_value(&GitJobResult { job: self.git_switch(kind, parse(params)?).await? })
            }
            method::GIT_JOB => {
                check_may(kind, None, Ask::Read)?;
                let p: GitJobParams = parse(params)?;
                to_value(&GitJobResult { job: self.job_of(&p.job)? })
            }
            method::GIT_STOP => {
                let p: GitJobParams = parse(params)?;
                to_value(&GitJobResult { job: self.job_stop(kind, p)? })
            }
            method::WORKTREE_LIST => {
                check_may(kind, None, Ask::ListWorktrees)?;
                let p: WorktreeListParams = parse(params)?;
                to_value(&WorktreeListResult { worktrees: self.worktree_list(p.folder_id.as_deref()).await? })
            }
            method::WORKTREE_REMOVE => {
                let p: WorktreeRemoveParams = parse(params)?;
                check_may(kind, p.by.as_ref(), Ask::RemoveWorktree)?;
                to_value(&self.worktree_remove(&p.id, p.forget).await?)
            }
            method::GH_STATUS => {
                check_may(kind, None, Ask::Read)?;
                let p: GhStatusParams = parse(params)?;
                to_value(&self.gh_status(p.refresh).await)
            }
            method::PR_VIEW => {
                check_may(kind, None, Ask::Read)?;
                to_value(&self.pr_view(parse(params)?).await?)
            }
            method::PR_CREATE => to_value(&GitJobResult { job: self.pr_create(kind, parse(params)?).await? }),
            method::PR_MERGE => to_value(&GitJobResult { job: self.pr_merge(kind, parse(params)?).await? }),
            method::PR_READY => to_value(&self.pr_ready(kind, parse(params)?).await?),
            method::PR_METHODS => {
                check_may(kind, None, Ask::Merge)?;
                let p: ThreadIdParams = parse(params)?;
                let (spot, _) = self.git_known(&p.thread_id).await?;
                let methods = self.scm()?.pr_methods(&spot).await.map_err(scm_refusal)?;
                to_value(&PrMethodsResult {
                    squash: methods.squash,
                    merge: methods.merge,
                    rebase: methods.rebase,
                })
            }
            method::PR_LIST => {
                check_may(kind, None, Ask::Read)?;
                to_value(&self.pr_list(parse(params)?).await?)
            }
            _ => Err(err(ErrorCode::BadRequest, UNKNOWN_METHOD)),
        }
    }

    async fn folder_status(&self, folder_id: &str) -> Result<GitStatus, RpcError> {
        if self.scm.is_none() {
            return Ok(GitStatus::without(RepoState::NoGit));
        }
        match self.folder_spot(folder_id).await {
            Ok(spot) => self.scm()?.status(&spot).await.map_err(scm_refusal),
            Err(e) => match reason_of(&e) {
                Some("not_repo") => Ok(GitStatus::without(RepoState::None)),
                Some("refused") => Ok(GitStatus::without(RepoState::Refused)),
                Some("no_git") => Ok(GitStatus::without(RepoState::NoGit)),
                _ => Err(e),
            },
        }
    }

    async fn git_patch(self: &Arc<Self>, p: GitPatchParams) -> Result<GitPatchResult, RpcError> {
        if p.path.is_empty() || p.path.len() > MAX_GIT_PATH {
            return Err(refusal(ErrorCode::BadRequest, "bad_request"));
        }
        let (spot, known) = self.git_known(&p.thread_id).await?;
        let patch = self.scm()?.patch(&spot, &known, &p.path, p.offset.unwrap_or(0)).await.map_err(|e| {
            if e.reason == Reason::NotFound {
                let mut refused = refusal(ErrorCode::BadRequest, "not_found");
                refused.message = "That file isn't in the last status.".to_owned();
                refused
            } else {
                scm_refusal(e)
            }
        })?;
        Ok(GitPatchResult {
            patch: patch.text,
            next: patch.next,
            truncated: patch.truncated,
            binary: patch.binary,
            names: patch.names,
            more_names: patch.more_names,
        })
    }

    fn picks(paths: Vec<String>, all: bool) -> Result<Picks, RpcError> {
        if all {
            return Ok(Picks::All);
        }
        if paths.is_empty() {
            return Err(refusal(ErrorCode::BadRequest, "nothing"));
        }
        if paths.len() > MAX_COMMIT_PATHS || paths.iter().any(|p| p.is_empty() || p.len() > MAX_GIT_PATH) {
            return Err(refusal(ErrorCode::BadRequest, "bad_request"));
        }
        Ok(Picks::Paths(paths))
    }

    async fn git_review(self: &Arc<Self>, p: GitReviewParams) -> Result<GitReviewResult, RpcError> {
        let picks = Self::picks(p.paths, p.all)?;
        let (spot, known) = self.git_known(&p.thread_id).await?;
        let review = self.scm()?.review(&spot, &known, &picks).await.map_err(scm_refusal)?;
        Ok(GitReviewResult {
            review: review.id,
            fingerprint: review.fingerprint,
            files: review
                .files
                .into_iter()
                .map(|f| ReviewedFile { path: f.path, bytes: f.bytes, folder: f.folder, entries: f.entries })
                .collect(),
            pc_only: review.pc_only,
        })
    }

    /// The thread may change its folder now: it exists, can be continued,
    /// doesn't work or wait, and no other thread or session works in the
    /// same work tree.
    async fn may_change(self: &Arc<Self>, thread_id: &str, w: &Where, spot: &Spot) -> Result<(), RpcError> {
        if w.readonly {
            return Err(read_only());
        }
        if w.running {
            return Err(refusal(ErrorCode::Busy, THREAD_BUSY));
        }
        self.check_shared(thread_id, spot).await
    }

    async fn git_commit(self: &Arc<Self>, kind: ClientKind, p: GitCommitParams) -> Result<Job, RpcError> {
        check_may(kind, p.by.as_ref(), Ask::Commit)?;
        let by = p.by.clone().unwrap_or_else(Origin::launcher);
        if by.kind == OriginKind::Phone && p.review.is_none() {
            return Err(err(ErrorCode::Denied, "A phone commits what it has read: review the files first."));
        }
        let message = p.message.trim().to_owned();
        if message.is_empty() || message.len() > MAX_COMMIT_MESSAGE || message.contains('\0') {
            return Err(refusal(ErrorCode::BadRequest, "bad_message"));
        }
        if p.review.as_deref().is_some_and(|r| r.len() != 16 || !is_valid_id(r)) {
            return Err(refusal(ErrorCode::BadRequest, "bad_request"));
        }
        let picks = Self::picks(p.paths, p.all)?;
        let w = self.git_where(&p.thread_id)?;
        if w.readonly {
            return Err(read_only());
        }
        if w.running {
            return Err(refusal(ErrorCode::Busy, THREAD_BUSY));
        }
        let (spot, status) = self.git_fresh(&p.thread_id).await?;
        self.may_change(&p.thread_id, &w, &spot).await?;
        let refused = if status.locked {
            Some(Reason::GitBusy)
        } else if status.doing.is_some() {
            Some(Reason::Doing)
        } else if status.detached.is_some() {
            Some(Reason::Detached)
        } else if status.conflicts > 0 {
            Some(Reason::Conflicts)
        } else if status.total == 0 {
            Some(Reason::Nothing)
        } else {
            None
        };
        if let Some(reason) = refused {
            return Err(scm_refusal(ScmError::new(reason)));
        }
        let (id, lines, stop) = self.open_job(&p.thread_id, JobKind::Commit, by, spot.top())?;
        let scm = self.scm()?.clone();
        let branch = status.branch.clone();
        let review = p.review;
        let work: Work = Box::pin(async move {
            let made = scm.commit(&spot, &status, &picks, &message, review.as_deref(), &lines, &stop).await?;
            Ok(JobResult {
                oid7: Some(made.oid7),
                subject: Some(clean_cut(&made.subject, 200).0),
                files: Some(made.files),
                branch,
                ..JobResult::default()
            })
        });
        Ok(self.run_job(id, work).0)
    }

    async fn git_push(self: &Arc<Self>, kind: ClientKind, p: GitPushParams) -> Result<Job, RpcError> {
        check_may(kind, p.by.as_ref(), Ask::Push)?;
        let by = p.by.clone().unwrap_or_else(Origin::launcher);
        if p.remote.as_deref().is_some_and(|r| !name_ok(r, MAX_REMOTE_NAME)) {
            return Err(refusal(ErrorCode::BadRequest, "not_found"));
        }
        // A push changes no file: it may go while a thread works.
        let (spot, status) = self.git_fresh(&p.thread_id).await?;
        let refused = if status.locked {
            Some(ScmError::new(Reason::GitBusy))
        } else if status.unborn {
            Some(ScmError::new(Reason::NoCommits))
        } else if status.detached.is_some() || status.branch.is_none() {
            Some(ScmError::new(Reason::Detached))
        } else if status.remotes.is_empty() {
            Some(ScmError::new(Reason::NoRemote))
        } else if let Some(asked) = &p.remote {
            (!status.remotes.iter().any(|r| &r.name == asked)).then(|| ScmError::new(Reason::NotFound))
        } else if status.remote.is_none() {
            let names = status.remotes.iter().map(|r| r.name.clone()).collect();
            Some(ScmError::with_lines(Reason::PickRemote, names))
        } else {
            None
        };
        if let Some(e) = refused {
            return Err(scm_refusal(e));
        }
        let (id, lines, stop) = self.open_job(&p.thread_id, JobKind::Push, by, spot.top())?;
        let scm = self.scm()?.clone();
        let remote = p.remote;
        let work: Work = Box::pin(async move {
            let pushed = scm.push(&spot, remote.as_deref(), &lines, &stop).await?;
            Ok(JobResult {
                branch: Some(pushed.branch),
                remote: Some(pushed.remote),
                up_to_date: pushed.up_to_date,
                ..JobResult::default()
            })
        });
        Ok(self.run_job(id, work).0)
    }

    async fn git_fetch(self: &Arc<Self>, kind: ClientKind, p: GitFetchParams) -> Result<Job, RpcError> {
        check_may(kind, p.by.as_ref(), Ask::Fetch)?;
        let by = p.by.clone().unwrap_or_else(Origin::launcher);
        let (spot, status) = self.git_fresh(&p.thread_id).await?;
        if status.remotes.is_empty() {
            return Err(scm_refusal(ScmError::new(Reason::NoRemote)));
        }
        if status.remote.is_none() {
            let names = status.remotes.iter().map(|r| r.name.clone()).collect();
            return Err(scm_refusal(ScmError::with_lines(Reason::PickRemote, names)));
        }
        let (id, lines, _stop) = self.open_job(&p.thread_id, JobKind::Fetch, by, spot.top())?;
        let scm = self.scm()?.clone();
        let remote = status.remote.clone();
        let work: Work = Box::pin(async move {
            scm.fetch(&spot, &lines).await?;
            Ok(JobResult { remote, ..JobResult::default() })
        });
        Ok(self.run_job(id, work).0)
    }

    async fn git_branches(self: &Arc<Self>, p: GitBranchesParams) -> Result<GitBranchesResult, RpcError> {
        if p.query.as_ref().is_some_and(|q| q.len() > MAX_BRANCH_QUERY) {
            return Err(refusal(ErrorCode::BadRequest, "bad_request"));
        }
        let spot = match (p.thread_id, p.folder_id) {
            (Some(thread), None) => self.git_known(&thread).await?.0,
            (None, Some(folder)) => self.folder_spot(&folder).await?,
            _ => return Err(err(ErrorCode::BadRequest, "Name a thread or a folder.")),
        };
        let found = self.scm()?.branches(&spot, p.query.as_deref()).await.map_err(scm_refusal)?;
        Ok(GitBranchesResult { branches: found.branches, more: found.more, current: found.current })
    }

    async fn git_switch(self: &Arc<Self>, kind: ClientKind, p: GitSwitchParams) -> Result<Job, RpcError> {
        check_may(kind, p.by.as_ref(), Ask::Branch)?;
        if p.carry {
            check_may(kind, p.by.as_ref(), Ask::Carry)?;
        }
        let by = p.by.clone().unwrap_or_else(Origin::launcher);
        if p.create && p.track {
            return Err(refusal(ErrorCode::BadRequest, "bad_request"));
        }
        if !name_ok(&p.branch, MAX_BRANCH_NAME) {
            return Err(refusal(ErrorCode::BadRequest, if p.create { "bad_name" } else { "not_found" }));
        }
        let w = self.git_where(&p.thread_id)?;
        if w.worktree.is_some() {
            return Err(scm_refusal(ScmError::new(Reason::InWorktree)));
        }
        if w.readonly {
            return Err(read_only());
        }
        if w.running {
            return Err(refusal(ErrorCode::Busy, THREAD_BUSY));
        }
        let (spot, status) = self.git_fresh(&p.thread_id).await?;
        self.may_change(&p.thread_id, &w, &spot).await?;
        let refused = if status.locked {
            Some(Reason::GitBusy)
        } else if status.doing.is_some() {
            Some(Reason::Doing)
        } else if p.create && !scm::new_name_ok(&p.branch) {
            Some(Reason::BadName)
        } else if status.total > 0 && !p.carry {
            Some(Reason::Dirty)
        } else {
            None
        };
        if let Some(reason) = refused {
            return Err(scm_refusal(ScmError::new(reason)));
        }
        let to = if p.create {
            SwitchTo::New(p.branch.clone())
        } else if p.track {
            SwitchTo::Track(p.branch.clone())
        } else {
            SwitchTo::Local(p.branch.clone())
        };
        let (id, lines, _stop) = self.open_job(&p.thread_id, JobKind::Switch, by, spot.top())?;
        let scm = self.scm()?.clone();
        let (carry, created) = (p.carry, p.create);
        let work: Work = Box::pin(async move {
            let branch = scm.switch(&spot, &to, carry, &lines).await?;
            Ok(JobResult { branch: Some(branch), created, ..JobResult::default() })
        });
        Ok(self.run_job(id, work).0)
    }

    pub(crate) async fn gh_status(&self, refresh: bool) -> GhState {
        match &self.scm {
            Some(scm) => scm.gh_state(refresh).await,
            None => GhState { checked_ms: now_ms(), ..GhState::default() },
        }
    }

    async fn pr_view(self: &Arc<Self>, p: PrViewParams) -> Result<PrViewResult, RpcError> {
        // The CLI's hosts first: whether the folder is on GitHub depends on them.
        let gh = self.gh_status(false).await;
        let (spot, status) = self.git_refresh(&p.thread_id).await?;
        let github = status.github.clone();
        let why = if status.repo != RepoState::Ok {
            Some(PrWhy::NotRepo)
        } else if status.detached.is_some() || status.branch.is_none() {
            Some(PrWhy::Detached)
        } else if github.is_none() {
            Some(PrWhy::NotGithub)
        } else if status.upstream.is_none() {
            Some(PrWhy::NoUpstream)
        } else {
            None
        };
        let (Some(spot), None, GhUse::Ok) = (spot, why, gh.gh) else {
            return Ok(PrViewResult { pr: None, gh, github, why });
        };
        let scm = self.scm()?;
        match scm.pr_for_branch(&spot, &status, p.refresh).await {
            Ok(pr) => {
                self.git_set_pr(&p.thread_id, pr.as_ref());
                Ok(PrViewResult { pr, gh, github, why: None })
            }
            Err(e) if matches!(e.reason, Reason::NoGh | Reason::SignedOut | Reason::OldGh) => {
                Ok(PrViewResult { pr: None, gh: self.gh_status(false).await, github, why: None })
            }
            Err(_) => {
                let pr = status.branch.as_deref().and_then(|b| scm.cached_pr(&spot, b));
                Ok(PrViewResult { pr, gh, github, why: Some(PrWhy::Network) })
            }
        }
    }

    async fn gh_usable(&self) -> Result<(), RpcError> {
        match self.gh_status(false).await.gh {
            GhUse::Ok => Ok(()),
            GhUse::Missing => Err(refusal(ErrorCode::Unavailable, "no_gh")),
            GhUse::SignedOut => Err(refusal(ErrorCode::Unavailable, "signed_out")),
            GhUse::Old => Err(refusal(ErrorCode::Unavailable, "old_gh")),
        }
    }

    async fn pr_create(self: &Arc<Self>, kind: ClientKind, p: PrCreateParams) -> Result<Job, RpcError> {
        check_may(kind, p.by.as_ref(), Ask::Push)?;
        let by = p.by.clone().unwrap_or_else(Origin::launcher);
        let title = p.title.trim().to_owned();
        if title.is_empty() || title.len() > MAX_PR_TITLE || title.contains(['\0', '\n', '\r']) {
            return Err(refusal(ErrorCode::BadRequest, "bad_request"));
        }
        if p.body.len() > MAX_PR_BODY || p.body.contains('\0') {
            return Err(refusal(ErrorCode::BadRequest, "bad_request"));
        }
        if p.base.as_deref().is_some_and(|b| !name_ok(b, MAX_BRANCH_NAME)) {
            return Err(refusal(ErrorCode::BadRequest, "not_found"));
        }
        self.gh_usable().await?;
        let (spot, status) = self.git_fresh(&p.thread_id).await?;
        let refused = if status.detached.is_some() || status.branch.is_none() {
            Some(Reason::Detached)
        } else if status.github.is_none() {
            Some(Reason::NotGithub)
        } else if status.upstream.is_none() || status.ahead != Some(0) {
            Some(Reason::NotPushed)
        } else {
            None
        };
        if let Some(reason) = refused {
            return Err(scm_refusal(ScmError::new(reason)));
        }
        let (id, lines, _stop) = self.open_job(&p.thread_id, JobKind::PrCreate, by, spot.top())?;
        let scm = self.scm()?.clone();
        // The launcher names the base as `main`: the push remote's branch of that name.
        let push_remote = status.remote.clone().unwrap_or_default();
        let remotes: Vec<&str> = status.remotes.iter().map(|r| r.name.as_str()).collect();
        let base = p.base.as_deref().map(|b| base_ref(b, &push_remote, &remotes));
        let new = NewPr { title, body: p.body, base, draft: p.draft };
        let work: Work = Box::pin(async move {
            let pr = scm.pr_create(&spot, &status, &new, &lines).await?;
            Ok(JobResult {
                branch: Some(pr.head.clone()),
                number: Some(pr.number),
                pr: Some(pr),
                ..JobResult::default()
            })
        });
        Ok(self.run_job(id, work).0)
    }

    async fn pr_merge(self: &Arc<Self>, kind: ClientKind, p: PrMergeParams) -> Result<Job, RpcError> {
        check_may(kind, p.by.as_ref(), Ask::Merge)?;
        let by = p.by.clone().unwrap_or_else(Origin::launcher);
        if p.number == 0 || p.head.len() != 40 || !is_valid_id(&p.head) {
            return Err(refusal(ErrorCode::BadRequest, "bad_request"));
        }
        self.gh_usable().await?;
        let (spot, _) = self.git_known(&p.thread_id).await?;
        let (id, lines, _stop) = self.open_job(&p.thread_id, JobKind::PrMerge, by, spot.top())?;
        let scm = self.scm()?.clone();
        let (number, method, head) = (p.number, merge_method(p.method), p.head);
        let work: Work = Box::pin(async move {
            scm.pr_merge(&spot, number, method, &head, &lines).await?;
            Ok(JobResult { number: Some(number), ..JobResult::default() })
        });
        Ok(self.run_job(id, work).0)
    }

    async fn pr_ready(
        self: &Arc<Self>,
        kind: ClientKind,
        p: PrReadyParams,
    ) -> Result<PrReadyResult, RpcError> {
        check_may(kind, p.by.as_ref(), Ask::Merge)?;
        if p.number == 0 {
            return Err(refusal(ErrorCode::BadRequest, "bad_request"));
        }
        self.gh_usable().await?;
        let (spot, _) = self.git_known(&p.thread_id).await?;
        let scm = self.scm()?.clone();
        scm.pr_ready(&spot, p.number).await.map_err(scm_refusal)?;
        let (spot, status) = self.git_fresh(&p.thread_id).await.unwrap_or((spot, GitStatus::default()));
        let pr = scm.pr_for_branch(&spot, &status, true).await.ok().flatten();
        self.git_set_pr(&p.thread_id, pr.as_ref());
        Ok(PrReadyResult { pr })
    }

    async fn pr_list(self: &Arc<Self>, p: PrListParams) -> Result<PrListResult, RpcError> {
        let spot = match (p.show, &p.folder_id) {
            (PrShow::Folder, Some(folder)) => Some(self.folder_spot(folder).await?),
            (PrShow::Folder, None) => return Err(err(ErrorCode::BadRequest, "Name the folder.")),
            (_, _) => None,
        };
        let gh = self.gh_status(false).await;
        if gh.gh != GhUse::Ok {
            return Ok(PrListResult { prs: Vec::new(), more: false, gh });
        }
        let rows = match self.scm()?.pr_list(show_of(p.show), spot.as_ref(), p.refresh).await {
            Ok(rows) => rows,
            Err(e) if matches!(e.reason, Reason::NoGh | Reason::SignedOut | Reason::OldGh) => {
                return Ok(PrListResult { prs: Vec::new(), more: false, gh: self.gh_status(false).await });
            }
            Err(e) => return Err(scm_refusal(e)),
        };
        // A thread whose folder is on that repository and whose branch is the head.
        let heads: Vec<(String, String, String)> = self
            .source
            .lock_known()
            .iter()
            .filter_map(|(id, k)| {
                Some((id.clone(), k.status.github.as_ref()?.repo.clone(), k.status.branch.clone()?))
            })
            .collect();
        let more = rows.len() >= MAX_PR_ROWS;
        let prs = rows
            .into_iter()
            .take(MAX_PR_ROWS)
            .map(|row| {
                let thread_id = row.branch.as_ref().and_then(|branch| {
                    heads
                        .iter()
                        .find(|(_, repo, head)| *repo == row.repo && head == branch)
                        .map(|(id, ..)| id.clone())
                });
                PrListRow { row, thread_id }
            })
            .collect();
        Ok(PrListResult { prs, more, gh })
    }

    // ---- Worktrees (§SC3.8) ----

    async fn worktree_list(&self, folder_id: Option<&str>) -> Result<Vec<Worktree>, RpcError> {
        if folder_id.is_some_and(|f| !is_valid_folder_id(f)) {
            return Err(err(ErrorCode::NotFound, "No such folder."));
        }
        let records: Vec<Record> = self
            .source
            .lock_records()
            .iter()
            .filter(|r| folder_id.is_none_or(|f| r.folder_id == f))
            .cloned()
            .collect();
        let mut tasks = tokio::task::JoinSet::new();
        for (n, record) in records.iter().enumerate() {
            let dir = PathBuf::from(&record.path);
            let scm = self.scm.clone();
            tasks.spawn(async move {
                if !dir.is_dir() {
                    return (n, None, None);
                }
                let size = scm::worktree_size(dir.clone()).await;
                let unpushed = match scm {
                    Some(scm) => scm.unpushed(&dir).await.ok().map(|u| u.commits),
                    None => None,
                };
                (n, Some(size), unpushed)
            });
        }
        let mut found: HashMap<usize, Measured> = HashMap::new();
        while let Some(Ok((n, size, unpushed))) = tasks.join_next().await {
            found.insert(n, (size, unpushed));
        }
        let mut list: Vec<Worktree> = records
            .into_iter()
            .enumerate()
            .map(|(n, r)| {
                let (size, unpushed) = found.remove(&n).unwrap_or((None, None));
                let tag = r.tag();
                Worktree {
                    id: r.id,
                    folder_id: r.folder_id,
                    label: tag.label,
                    branch: r.branch,
                    thread_id: r.thread_id,
                    created_ms: r.created_ms,
                    missing: !Path::new(&r.path).is_dir(),
                    bytes: size.map(|s| s.0),
                    bytes_over: size.is_some_and(|s| s.1),
                    unpushed,
                    path: r.path,
                }
            })
            .collect();
        list.sort_by_key(|w| std::cmp::Reverse(w.created_ms));
        list.truncate(MAX_WORKTREES);
        Ok(list)
    }

    async fn save_worktrees(&self) -> Result<(), RpcError> {
        match self.source.save().await {
            Ok(Ok(())) => Ok(()),
            _ => Err(err(ErrorCode::Internal, "Couldn't save the worktree list.")),
        }
    }

    /// `worktree.remove`: its folder goes when nothing in it is uncommitted;
    /// the branch stays; its thread stays, read-only. `forget`: a missing
    /// one's record goes.
    pub(crate) async fn worktree_remove(
        self: &Arc<Self>,
        id: &str,
        forget: bool,
    ) -> Result<WorktreeRemoveResult, RpcError> {
        if !is_valid_worktree_id(id) {
            return Err(refusal(ErrorCode::NotFound, "not_found"));
        }
        let record = self.source.record(id).ok_or_else(|| refusal(ErrorCode::NotFound, "not_found"))?;
        let path = PathBuf::from(&record.path);
        if forget {
            if path.is_dir() {
                return Err(err(ErrorCode::BadRequest, "It is still there: remove it instead."));
            }
            self.source.lock_records().retain(|r| r.id != id);
            self.save_worktrees().await?;
            return Ok(WorktreeRemoveResult::default());
        }
        if !path.is_dir() {
            return Err(refusal(ErrorCode::NotFound, "not_found"));
        }
        if let Some(thread) = &record.thread_id {
            let running = self.lock_reg().threads.get(thread).is_some_and(|e| e.info.state.is_running());
            if running {
                return Err(refusal(ErrorCode::Busy, THREAD_BUSY));
            }
        }
        if self.source.lock_jobs().values().any(|s| s.running() && s.tree == path) {
            return Err(refusal(ErrorCode::Busy, "git_busy"));
        }
        let spot = self.folder_spot(&record.folder_id).await?;
        let unpushed = self.scm()?.worktree_remove(&spot, &path).await.map_err(scm_refusal)?;
        self.source.lock_records().retain(|r| r.id != id);
        let saved = self.save_worktrees().await;
        if let Some(thread) = &record.thread_id {
            self.source.lock_known().remove(thread);
            let mut reg = self.lock_reg();
            if let Some(entry) = reg.threads.get_mut(thread) {
                if let Some(tag) = entry.info.worktree.as_mut() {
                    tag.missing = true;
                }
                entry.info.readonly = Some(Readonly::FolderMissing);
                entry.info.branch = None;
                entry.info.changed = None;
                entry.cwd = None;
                emit_state(self, entry);
            }
        }
        saved?;
        Ok(WorktreeRemoveResult { unpushed: Some(unpushed.commits) })
    }

    /// `thread.start` in a new worktree (§SC3.8): the worktree first, as a
    /// job the reply carries; the thread only once it is made; the
    /// worktree removed again when the thread can't then be started.
    pub(crate) async fn thread_start_worktree(
        self: &Arc<Self>,
        kind: ClientKind,
        mut p: ThreadStartParams,
        owner: &str,
    ) -> Result<ThreadStartResult, RpcError> {
        let wanted = p.worktree.take().unwrap_or_default();
        check_may(kind, Some(&p.origin), Ask::NewWorktree)?;
        if p.prompt.is_empty() || p.prompt.len() > MAX_PROMPT {
            return Err(err(ErrorCode::BadRequest, "The prompt must be 1 to 16384 bytes."));
        }
        if let Some(key) = &p.key {
            if !is_valid_send_key(key) {
                return Err(err(ErrorCode::BadRequest, "Bad message key."));
            }
            if let Some(info) = self.lock_reg().started_by_key(key, now_ms()) {
                return Ok(ThreadStartResult { thread: info, duplicate: true, job: None });
            }
        }
        if !is_valid_folder_id(&p.folder_id) {
            return Err(err(ErrorCode::NotFound, "No such folder."));
        }
        let spot = self.folder_spot(&p.folder_id).await?;
        {
            let records = self.source.lock_records();
            let here = records.iter().filter(|r| r.folder_id == p.folder_id).count();
            if here >= MAX_WORKTREES_PER_FOLDER || records.len() >= MAX_WORKTREES {
                return Err(refusal(ErrorCode::Limit, LIMIT));
            }
        }
        let branch = match wanted.branch {
            Some(branch) if branch.len() > MAX_BRANCH_NAME || !scm::new_name_ok(&branch) => {
                return Err(refusal(ErrorCode::BadRequest, "bad_name"));
            }
            Some(branch) => branch,
            None => scm::worktree_branch_name(&title_of(&p.prompt), &random_hex(3)),
        };
        if wanted.base.as_deref().is_some_and(|b| !name_ok(b, MAX_BRANCH_NAME)) {
            return Err(refusal(ErrorCode::BadRequest, "not_found"));
        }
        let thread_id = {
            let reg = self.lock_reg();
            let mut id = random_hex(16);
            while reg.threads.contains_key(&id) {
                id = random_hex(16);
            }
            id
        };
        let (job_id, lines, _stop) =
            self.open_job(&thread_id, JobKind::Worktree, p.origin.clone(), spot.top())?;
        let scm = self.scm()?.clone();
        let made: Arc<Mutex<Option<PathBuf>>> = Arc::default();
        let work: Work = {
            let (scm, spot, made, branch) = (scm.clone(), spot.clone(), made.clone(), branch.clone());
            let root = self.source.root().join(&p.folder_id);
            let base = wanted.base;
            Box::pin(async move {
                let path = scm.worktree_add(&spot, &root, &branch, base.as_deref(), &lines).await?;
                *made.lock().unwrap_or_else(PoisonError::into_inner) = Some(path);
                Ok(JobResult { branch: Some(branch), ..JobResult::default() })
            })
        };
        let (_, task) = self.run_job(job_id.clone(), work);
        let _ = task.await;
        let job = self.job_of(&job_id)?;
        let path = made.lock().unwrap_or_else(PoisonError::into_inner).take();
        let Some(path) = path.filter(|_| job.phase == JobPhase::Done) else {
            let reason = job.error.as_ref().map_or("failed", |e| e.reason.as_str());
            let mut refused = refusal(ErrorCode::Unavailable, reason);
            refused.message = "The worktree couldn't be created.".to_owned();
            refused.data = Some(json!({ "reason": reason, "job": job }));
            return Err(refused);
        };
        let prefix = String::from_utf8_lossy(spot.prefix()).into_owned();
        let dir = Some(path.join(&prefix))
            .filter(|d| !prefix.is_empty() && d.is_dir())
            .unwrap_or_else(|| path.clone());
        let record = Record {
            id: random_hex(WORKTREE_ID_LEN / 2),
            folder_id: p.folder_id.clone(),
            path: path.to_string_lossy().into_owned(),
            dir: dir.to_string_lossy().into_owned(),
            branch,
            created_ms: now_ms(),
            thread_id: Some(thread_id.clone()),
        };
        let prepared = crate::host::threads::Prepared { id: thread_id, dir, worktree: record.tag() };
        let key = p.key.clone();
        match self.thread_start_as(p, owner, Some(prepared)).await {
            Ok(info) => {
                self.source.lock_records().push(record);
                if self.save_worktrees().await.is_err() {
                    tracing::warn!("couldn't save the worktree list");
                }
                if let Some(key) = key {
                    self.lock_reg().keep_start_key(key, &info.id, now_ms());
                }
                Ok(ThreadStartResult { thread: info, duplicate: false, job: Some(job) })
            }
            Err(e) => {
                // Clean by construction: nothing was written in it yet.
                if scm.worktree_remove(&spot, &path).await.is_err() {
                    tracing::warn!("couldn't remove a worktree whose thread didn't start");
                }
                Err(e)
            }
        }
    }

    /// `thread.delete`, and its worktree when the box was ticked (G7 Link
    /// on this PC only). A worktree that can't go stays listed in Setup.
    pub(crate) async fn thread_delete_git(
        self: &Arc<Self>,
        kind: ClientKind,
        p: ThreadDeleteParams,
    ) -> Result<ThreadDeleteResult, RpcError> {
        if p.remove_worktree {
            check_may(kind, None, Ask::RemoveWorktree)?;
        }
        let tag = self.lock_reg().threads.get(&p.thread_id).and_then(|e| e.info.worktree.clone());
        self.thread_delete(&p.thread_id)?;
        let Some(tag) = tag.filter(|_| p.remove_worktree) else { return Ok(ThreadDeleteResult::default()) };
        let left = match self.worktree_remove(&tag.id, false).await {
            Ok(_) => None,
            Err(e) => Some(reason_of(&e).unwrap_or("failed").to_owned()),
        };
        Ok(ThreadDeleteResult { worktree_left: left })
    }

    // ---- "Ask <agent> to write it" (§SC5.4) ----

    /// `thread.send { purpose }` made into the message it sends: refused
    /// unless the thread is idle with nothing queued. The last answer goes.
    pub(crate) fn ask_params(&self, mut p: ThreadSendParams) -> Result<ThreadSendParams, RpcError> {
        let Some(purpose) = p.purpose else { return Ok(p) };
        if !p.text.is_empty() || !p.images.is_empty() {
            return Err(err(ErrorCode::BadRequest, "Ask without text: G7 sends its own."));
        }
        if p.base.as_deref().is_some_and(|b| !name_ok(b, MAX_BRANCH_NAME)) {
            return Err(refusal(ErrorCode::BadRequest, "not_found"));
        }
        let w = self.git_where(&p.thread_id)?;
        if w.readonly {
            return Err(read_only());
        }
        if w.running || w.queued {
            return Err(refusal(ErrorCode::Busy, THREAD_BUSY));
        }
        let branch = w.branch.unwrap_or_else(|| "this branch".to_owned());
        // `main` (the launcher's) or `origin/main` (a remote's, as listed): the branch's own name.
        let remotes: Vec<String> = self
            .source
            .lock_known()
            .get(&p.thread_id)
            .map(|k| k.status.remotes.iter().map(|r| r.name.clone()).collect())
            .unwrap_or_default();
        let base = p.base.as_deref().map(|b| {
            remotes
                .iter()
                .find_map(|r| b.strip_prefix(r.as_str()).and_then(|rest| rest.strip_prefix('/')))
                .filter(|rest| !rest.is_empty())
                .unwrap_or(b)
        });
        p.text = ask_text(purpose, &branch, base);
        let mut reg = self.lock_reg();
        if let Some(entry) = reg.threads.get_mut(&p.thread_id)
            && entry.info.wrote.take().is_some()
        {
            emit_state(self, entry);
        }
        Ok(p)
    }

    // ---- The host's own rounds ----

    /// After a turn: the folder read again (only then for a thread nobody views).
    pub(crate) fn git_turn_ended(self: &Arc<Self>, thread_id: &str) {
        if self.scm.is_none() {
            return;
        }
        let host = self.clone();
        let thread = thread_id.to_owned();
        tokio::spawn(async move {
            let _ = host.git_refresh(&thread).await;
        });
    }

    /// Every [`POLL`]: the threads a client shows are read again when their
    /// `HEAD` or index changed (first: read once), and, for a folder that
    /// keeps in step with its remote, fetched every five minutes and its
    /// pending checks read every thirty seconds.
    pub(crate) async fn git_poll(self: &Arc<Self>) {
        let Some(scm) = self.scm.clone() else { return };
        let Ok(_one) = self.source.polling.try_lock() else { return };
        let viewed = self.hub.viewed_threads(Instant::now());
        for thread in viewed {
            let kept = self.source.lock_known().get(&thread).map(|k| (k.spot.clone(), k.marks));
            let spot = match kept {
                None => match self.git_refresh(&thread).await {
                    Ok((spot, _)) => spot,
                    Err(_) => continue,
                },
                Some((Some(spot), marks)) if marks_of(&spot) != marks => {
                    match self.git_refresh(&thread).await {
                        Ok((spot, _)) => spot,
                        Err(_) => continue,
                    }
                }
                Some((spot, _)) => spot,
            };
            let Some(spot) = spot else { continue };
            self.follow_remote(&scm, &thread, &spot).await;
        }
    }

    /// "Keep in step with the remote" for a viewed thread (§SC0.6).
    async fn follow_remote(self: &Arc<Self>, scm: &Arc<Scm>, thread_id: &str, spot: &Spot) {
        let on = {
            let reg = self.lock_reg();
            reg.threads
                .get(thread_id)
                .and_then(|e| reg.folders.iter().find(|f| f.id == e.info.folder_id))
                .is_some_and(|f| f.follow_remote)
        };
        if !on {
            return;
        }
        let (fetch, checks, status) = {
            let mut known = self.source.lock_known();
            let Some(kept) = known.get_mut(thread_id) else { return };
            let fetch =
                kept.status.remote.is_some() && kept.fetched.is_none_or(|at| at.elapsed() >= FOLLOW_FETCH);
            if fetch {
                kept.fetched = Some(Instant::now());
            }
            let checks = kept.pr.is_some_and(|p| p.pending > 0)
                && kept.pending_since.is_some_and(|at| at.elapsed() < FOLLOW_CHECKS_FOR)
                && kept.checks_read.is_none_or(|at| at.elapsed() >= FOLLOW_CHECKS);
            (fetch, checks, kept.status.clone())
        };
        let busy = self.source.lock_jobs().values().any(|s| s.running() && s.tree == spot.top());
        if fetch && !busy && scm.fetch(spot, &Lines::new()).await.is_ok() {
            let _ = self.git_refresh(thread_id).await;
        }
        if checks && let Ok(pr) = scm.pr_for_branch(spot, &status, true).await {
            self.git_set_pr(thread_id, pr.as_ref());
        }
    }

    /// Every few seconds: old jobs forgotten, old `wrote`s cleared, and
    /// once a day, while nothing runs, the sweep.
    pub(crate) fn git_tick(self: &Arc<Self>) {
        Source::sweep_jobs(&mut self.source.lock_jobs());
        let now = now_ms();
        {
            let mut reg = self.lock_reg();
            for entry in reg.threads.values_mut() {
                if entry.info.wrote.as_ref().is_some_and(|w| now.saturating_sub(w.at_ms) >= WROTE_MS) {
                    entry.info.wrote = None;
                    emit_state(self, entry);
                }
            }
        }
        if self.any_running() || self.source.running_jobs() > 0 {
            return;
        }
        let checkpoints = git::checkpoints_dir(&self.config.data_dir);
        let stamp = checkpoints.join(SWEPT_FILE);
        let due = stamp
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_none_or(|age| age >= SWEEP_EVERY);
        if due && !self.source.sweeping.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let host = self.clone();
            tokio::spawn(async move {
                let _ = tokio::time::timeout(SWEEP_BUDGET, host.sweep_checkpoints()).await;
                if checkpoints.is_dir() {
                    let _ = std::fs::write(&stamp, b"");
                }
                host.source.sweeping.store(false, std::sync::atomic::Ordering::SeqCst);
            });
        }
    }

    /// The daily sweep (§SC1.2): `gc` in each of G7's restore-point
    /// repositories, and those whose folder is no longer listed removed.
    /// Only G7's own repositories, never the user's.
    pub(crate) async fn sweep_checkpoints(&self) {
        let Some(git_bin) = self.git.clone() else { return };
        let checkpoints = git::checkpoints_dir(&self.config.data_dir);
        let Ok(entries) = std::fs::read_dir(&checkpoints) else { return };
        let listed: HashSet<String> = self.lock_reg().folders.iter().map(|f| f.id.clone()).collect();
        let ids: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".git").map(str::to_owned))
            .filter(|id| is_valid_folder_id(id))
            .collect();
        for id in ids {
            let Some(place) = git::Place::new(git_bin.clone(), &checkpoints, &id, &checkpoints) else {
                continue;
            };
            let done =
                if listed.contains(&id) { git::gc(&place).await } else { git::remove_repo(&place).await };
            if done.is_err() {
                tracing::debug!("the sweep left one of G7's restore-point repositories as it was");
            }
        }
    }
}

fn unreachable_job(id: &str) -> Job {
    Job {
        id: id.to_owned(),
        thread_id: String::new(),
        kind: JobKind::Commit,
        phase: JobPhase::Failed,
        started_ms: now_ms(),
        ended_ms: Some(now_ms()),
        by: Origin::launcher(),
        lines: Vec::new(),
        from: 0,
        dropped: 0,
        result: None,
        error: Some(JobError { reason: "failed".into(), message: words("failed").into(), names: Vec::new() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone() -> Origin {
        Origin::phone("dev1", "Pixel 8 Pro")
    }

    /// §SC5.1's table, row by row: every kind, a phone `by` and not.
    #[test]
    fn who_may_ask_is_the_table() {
        use ClientKind::*;
        let pc = Origin::launcher();
        let rows: [(Ask, [bool; 5], bool); 11] = [
            //                       Launcher Link  Bridge Cli    Test   phone via Link
            (Ask::Read, [true, true, false, true, true], true),
            (Ask::Fetch, [true, true, false, false, false], true),
            (Ask::Commit, [true, true, false, false, false], true),
            (Ask::Push, [true, true, false, false, false], true),
            (Ask::Branch, [true, true, false, false, false], true),
            (Ask::Carry, [false, true, false, false, false], false),
            (Ask::NewWorktree, [true, true, false, false, false], false),
            (Ask::ListWorktrees, [true, true, false, false, false], false),
            (Ask::RemoveWorktree, [false, true, false, false, false], false),
            (Ask::Merge, [false, true, false, false, false], false),
            (Ask::FollowRemote, [true, true, false, false, false], false),
        ];
        assert_eq!(rows.len(), Ask::ALL.len());
        for (ask, kinds, phone_ok) in rows {
            for (kind, want) in [Launcher, Link, Bridge, Cli, Test].into_iter().zip(kinds) {
                assert_eq!(may(kind, None, ask), want, "{ask:?} {kind:?}");
                assert_eq!(may(kind, Some(&pc), ask), want, "{ask:?} {kind:?} by this PC");
                let by_phone = may(kind, Some(&phone()), ask);
                assert_eq!(by_phone, phone_ok && kind == Link, "{ask:?} {kind:?} by a phone");
            }
        }
        let refused = check_may(Launcher, None, Ask::Merge).unwrap_err();
        assert_eq!((refused.code, refused.message.as_str()), (ErrorCode::Denied, DONE_ON_LINK));
        let long = Origin::phone("d".repeat(MAX_DEVICE_ID + 1), "P");
        assert_eq!(check_may(Link, Some(&long), Ask::Read).unwrap_err().code, ErrorCode::BadRequest);
    }

    #[test]
    fn the_agents_answer_loses_its_fence_and_subject() {
        assert_eq!(wrote_text("Fix the retry\n\nIt waited forever."), "Fix the retry\n\nIt waited forever.");
        assert_eq!(wrote_text("```\nFix the retry\n\nBody line\n```"), "Fix the retry\n\nBody line");
        assert_eq!(wrote_text("Here it is:\n\n```text\nSubject: Fix it\n```\nDone."), "Fix it");
        assert_eq!(wrote_text("SUBJECT:  Add a test\n\nwhy"), "Add a test\n\nwhy");
        assert_eq!(wrote_text("~~~~\nTilde fenced\n~~~~"), "Tilde fenced");
        assert_eq!(wrote_text("```\nno closing fence"), "```\nno closing fence");
        assert_eq!(wrote_text("  \n "), "");
        assert_eq!(wrote_text(&"é".repeat(MAX_WROTE)).len(), MAX_WROTE);
        assert_eq!(wrote_text("a\u{202e}b"), "ab", "cleaned");
    }

    #[test]
    fn a_base_named_without_its_remote_is_the_push_remotes() {
        let remotes = ["origin", "upstream"];
        assert_eq!(base_ref("main", "origin", &remotes), "origin/main", "the launcher's form");
        assert_eq!(base_ref("release/2", "origin", &remotes), "origin/release/2");
        assert_eq!(base_ref("origin/main", "origin", &remotes), "origin/main", "G7 Link's form");
        assert_eq!(base_ref("upstream/main", "origin", &remotes), "upstream/main", "another remote's, named");
        assert_eq!(base_ref("origin/", "origin", &remotes), "origin/origin/");
        assert_eq!(base_ref("main", "", &[]), "main", "no remote: as it came, refused by lookup");
    }

    #[test]
    fn the_ask_is_one_of_two_fixed_texts() {
        assert_eq!(ask_text(Purpose::Commit, "x", None), ASK_COMMIT_TEXT);
        assert_eq!(
            ask_text(Purpose::Pr, "feat/sync", None),
            "Write a title and description for a pull request from feat/sync into the default branch. \
             Reply with only that: the title on the first line, a blank line, then the description in \
             Markdown. Don't open the pull request."
        );
        assert!(ask_text(Purpose::Pr, "a", Some("main")).contains("from a into main."));
    }

    fn done(kind: JobKind, by: Origin, result: JobResult) -> Job {
        Job { phase: JobPhase::Done, result: Some(result), by, kind, ..unreachable_job("0123456789abcdef") }
    }

    #[test]
    fn a_job_that_went_through_leaves_its_notice() {
        let commit = JobResult {
            files: Some(4),
            subject: Some("Fix the reconnect retry".into()),
            ..Default::default()
        };
        assert_eq!(
            notice_text(&done(JobKind::Commit, Origin::launcher(), commit.clone())),
            Some(("Committed 4 files: Fix the reconnect retry".into(), NoticeKind::Committed))
        );
        assert_eq!(
            notice_text(&done(JobKind::Commit, phone(), commit)).unwrap().0,
            "Committed 4 files: Fix the reconnect retry · from Pixel 8 Pro"
        );
        let one = JobResult { files: Some(1), subject: Some("x".into()), ..Default::default() };
        assert_eq!(
            notice_text(&done(JobKind::Commit, Origin::launcher(), one)).unwrap().0,
            "Committed 1 file: x"
        );
        let pushed = JobResult {
            branch: Some("feat/sync-retry".into()),
            remote: Some("origin".into()),
            ..Default::default()
        };
        assert_eq!(
            notice_text(&done(JobKind::Push, Origin::launcher(), pushed.clone())),
            Some(("Pushed feat/sync-retry to origin".into(), NoticeKind::Pushed))
        );
        let same = JobResult { up_to_date: true, ..pushed };
        assert_eq!(notice_text(&done(JobKind::Push, Origin::launcher(), same)), None, "nothing was pushed");
        let made = JobResult { branch: Some("feat/x".into()), created: true, ..Default::default() };
        assert_eq!(
            notice_text(&done(JobKind::Switch, Origin::launcher(), made)).unwrap().0,
            "Created feat/x"
        );
        let to = JobResult { branch: Some("main".into()), ..Default::default() };
        assert_eq!(
            notice_text(&done(JobKind::Switch, Origin::launcher(), to)).unwrap().0,
            "Switched to main"
        );
        let merged = JobResult { number: Some(212), ..Default::default() };
        assert_eq!(
            notice_text(&done(JobKind::PrMerge, Origin::launcher(), merged.clone())),
            Some(("Merged pull request #212".into(), NoticeKind::PrMerged))
        );
        assert_eq!(
            notice_text(&done(JobKind::PrCreate, Origin::launcher(), merged)).unwrap().0,
            "Opened pull request #212"
        );
        assert_eq!(notice_text(&done(JobKind::Fetch, Origin::launcher(), JobResult::default())), None);
        let failed = Job {
            phase: JobPhase::Failed,
            ..done(JobKind::Commit, Origin::launcher(), JobResult::default())
        };
        assert_eq!(notice_text(&failed), None, "a failed job leaves no notice");
    }

    #[test]
    fn refusals_carry_their_word() {
        let busy = refusal(ErrorCode::Busy, THREAD_BUSY);
        assert_eq!(reason_of(&busy), Some(THREAD_BUSY));
        assert_eq!(busy.message, words(THREAD_BUSY));
        let pick = scm_refusal(ScmError::with_lines(Reason::PickRemote, vec!["a".into(), "b".into()]));
        assert_eq!(pick.data, Some(json!({"reason": "pick_remote", "names": ["a", "b"]})));
        assert_eq!(repo_refusal(NoRepo::NoGit).code, ErrorCode::Unavailable);
        assert_eq!(reason_of(&repo_refusal(NoRepo::None)), Some("not_repo"));
    }

    fn record(id: &str, root: &Path) -> Record {
        let path = root.join("worktrees/f0123456789ab/g7-x-abcdef");
        Record {
            id: id.into(),
            folder_id: "f0123456789ab".into(),
            path: path.to_string_lossy().into_owned(),
            dir: path.join("sub").to_string_lossy().into_owned(),
            branch: "g7/x".into(),
            created_ms: 1,
            thread_id: Some("a".repeat(32)),
        }
    }

    #[test]
    fn worktree_records_keep_and_load_only_what_makes_sense() {
        let dir = tempfile::tempdir().unwrap();
        let good = record("0123456789ab", dir.path());
        let outside = Record {
            path: "/elsewhere/x".into(),
            dir: "/elsewhere/x".into(),
            ..record("0123456789ac", dir.path())
        };
        let bad_id = record("../x", dir.path());
        save_records(dir.path(), &[good.clone(), outside, bad_id, good.clone()]).unwrap();
        assert_eq!(load_records(dir.path()), vec![good.clone()]);
        let tag = good.tag();
        assert_eq!((tag.label.as_str(), tag.missing), ("g7-x-abcdef", true));
        std::fs::write(dir.path().join(RECORDS_FILE), b"{not json").unwrap();
        assert!(load_records(dir.path()).is_empty());
        assert!(dir.path().join("worktrees.json.unreadable").exists(), "an unreadable list is kept aside");
    }

    fn by_link() -> Origin {
        Origin::launcher()
    }

    #[tokio::test]
    async fn jobs_are_one_per_work_tree_four_in_the_host_and_kept_ten_minutes() {
        let dir = tempfile::tempdir().unwrap();
        let host = crate::host::test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let thread = "a".repeat(32);
        let (first, ..) = host.open_job(&thread, JobKind::Commit, by_link(), Path::new("/r/one")).unwrap();
        let same = host.open_job(&thread, JobKind::Push, by_link(), Path::new("/r/one")).unwrap_err();
        assert_eq!((same.code, reason_of(&same)), (ErrorCode::Busy, Some("git_busy")));
        assert_eq!(same.message, "A commit is running in this folder.");
        for tree in ["/r/two", "/r/three", "/r/four"] {
            host.open_job(&thread, JobKind::Fetch, by_link(), Path::new(tree)).unwrap();
        }
        let fifth = host.open_job(&thread, JobKind::Fetch, by_link(), Path::new("/r/five")).unwrap_err();
        assert_eq!(reason_of(&fifth), Some(JOBS));

        // It ends; it is read for ten minutes, then forgotten.
        host.end_job(&first, Ok(JobResult::default()), 0);
        assert_eq!(host.job_of(&first).unwrap().phase, JobPhase::Done);
        host.open_job(&thread, JobKind::Commit, by_link(), Path::new("/r/one")).unwrap();
        if let Some(slot) = host.source.lock_jobs().get_mut(&first) {
            slot.ended = Instant::now().checked_sub(JOB_KEPT);
        }
        assert_eq!(host.job_of(&first).unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(host.job_of("not-an-id").unwrap_err().code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn a_jobs_lines_are_capped_and_new_ones_numbered() {
        let dir = tempfile::tempdir().unwrap();
        let host = crate::host::test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let (id, lines, _) =
            host.open_job(&"b".repeat(32), JobKind::Commit, by_link(), Path::new("/r")).unwrap();
        for n in 0..(MAX_JOB_LINES + 50) {
            lines.push(&format!("line {n} {}", "x".repeat(600)));
        }
        let job = host.job_of(&id).unwrap();
        assert_eq!(job.lines.len(), MAX_JOB_BYTES / MAX_JOB_LINE, "the bytes bound: lines of 512");
        assert!(job.lines.iter().all(|l| l.chars().count() <= MAX_JOB_LINE));
        assert_eq!(u64::from(job.dropped) + job.lines.len() as u64, (MAX_JOB_LINES + 50) as u64);
        assert_eq!(job.from, u64::from(job.dropped), "the first kept line's number");
        let next = host.job_progress(&id, job.from + job.lines.len() as u64 - 2);
        assert_eq!(next, (MAX_JOB_LINES + 50) as u64);
    }

    #[tokio::test]
    async fn the_sweep_removes_only_orphaned_restore_point_repositories() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let host = crate::host::test_host_env(dir.path(), vec![("PATH".into(), path)]);
        if host.git.is_none() {
            assert!(std::env::var_os("CI").is_none(), "CI has git");
            eprintln!("skipped: no git on PATH");
            return;
        }
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        host.test_folder(&work);
        let checkpoints = git::checkpoints_dir(&host.config.data_dir);
        for name in ["f000000000001.git", "f00000000000f.git"] {
            std::fs::create_dir_all(checkpoints.join(name)).unwrap();
            std::fs::write(checkpoints.join(name).join("x"), b"x").unwrap();
        }
        std::fs::write(checkpoints.join("notes"), b"not a repository").unwrap();
        host.sweep_checkpoints().await;
        assert!(checkpoints.join("f000000000001.git/x").exists(), "a listed folder's stays");
        assert!(!checkpoints.join("f00000000000f.git").exists(), "an orphan goes");
        assert!(checkpoints.join("notes").exists(), "nothing else is touched");
    }

    #[tokio::test]
    async fn only_the_jobs_device_or_g7_link_stops_it() {
        let dir = tempfile::tempdir().unwrap();
        let host = crate::host::test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let thread = "c".repeat(32);
        let (id, _, stop) = host.open_job(&thread, JobKind::Push, phone(), Path::new("/r")).unwrap();
        let ask = |by: Option<Origin>| GitJobParams { job: id.clone(), by };
        let other = Origin::phone("dev2", "Other phone");
        assert_eq!(host.job_stop(ClientKind::Link, ask(Some(other))).unwrap_err().code, ErrorCode::Denied);
        assert_eq!(host.job_stop(ClientKind::Launcher, ask(None)).unwrap_err().code, ErrorCode::Denied);
        assert_eq!(host.job_stop(ClientKind::Test, ask(None)).unwrap_err().code, ErrorCode::Denied);
        assert!(!stop.is_stopped());
        host.job_stop(ClientKind::Link, ask(Some(phone()))).unwrap();
        assert!(stop.is_stopped(), "the phone that started it");
        let (pc_job, _, pc_stop) =
            host.open_job(&thread, JobKind::Commit, by_link(), Path::new("/s")).unwrap();
        let by_launcher = GitJobParams { job: pc_job.clone(), by: Some(Origin::launcher()) };
        host.job_stop(ClientKind::Launcher, by_launcher).unwrap();
        assert!(pc_stop.is_stopped(), "this PC's own job, from the launcher");
        let (link_job, _, link_stop) =
            host.open_job(&thread, JobKind::Commit, phone(), Path::new("/t")).unwrap();
        host.job_stop(ClientKind::Link, GitJobParams { job: link_job, by: None }).unwrap();
        assert!(link_stop.is_stopped(), "G7 Link on this PC stops any");
    }
}
