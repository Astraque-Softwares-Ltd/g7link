// SPDX-License-Identifier: MPL-2.0
//! Source control on a PC's agent threads (`docs/protocol/agents.md` §8):
//! the folder's status and uncommitted diff, reviewing and committing
//! files, push and fetch, branches, the branch's pull request and the lists
//! of them, and "Ask <agent> to write it"; and (§13) a pull request's
//! detail and diff, read-only, and "Ask <agent> to review it".
//!
//! Plain protocol v1: no capability. Everything follows the PC's `agents`
//! toggle (`Denied` when off); push and opening a pull request also its
//! `agent_push` toggle for this phone (`AgentCatalog.push`; `Denied` when
//! off). A commit goes only after `agent_git_review` and an
//! `agent_git_patch` with that review for every one of its files, read to
//! the end: the PC counts a file read when all of it was sent here. A phone
//! never merges, never switches taking changes along, and never touches a
//! worktree. No `Debug` here prints a path, a branch, a message or a line.

use g7link_core as core;

use crate::{AgentThread, G7LinkError, G7LinkNode, Result, ms, parse_id};

/// `Debug` for a record that prints only the fields named.
macro_rules! quiet_debug {
    ($ty:ident { $($field:ident),* }) => {
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($ty))$(.field(stringify!($field), &self.$field))*.finish_non_exhaustive()
            }
        }
    };
}

fn malformed(what: &str) -> G7LinkError {
    G7LinkError::Internal { reason: format!("invalid {what} reply") }
}

// ---- Words ----

/// Whether the thread's folder is a repository G7 works in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentRepo {
    Ok,
    /// "This folder isn't a git repository."
    None,
    /// "Git won't work in this folder. It says the folder isn't safe."
    Refused,
    /// "Git isn't installed on %1."
    NoGit,
}

impl AgentRepo {
    fn from_wire(word: &str) -> Option<Self> {
        use core::git::repo;
        Some(match word {
            repo::OK => Self::Ok,
            repo::NONE => Self::None,
            repo::REFUSED => Self::Refused,
            repo::NO_GIT => Self::NoGit,
            _ => return Option::None,
        })
    }
}

/// What git is in the middle of: "A %1 is in progress here. Finish it in a terminal."
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentGitDoing {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Bisect,
}

impl AgentGitDoing {
    fn from_wire(word: &str) -> Option<Self> {
        use core::git::doing;
        Some(match word {
            doing::MERGE => Self::Merge,
            doing::REBASE => Self::Rebase,
            doing::CHERRY_PICK => Self::CherryPick,
            doing::REVERT => Self::Revert,
            doing::BISECT => Self::Bisect,
            _ => return None,
        })
    }
}

/// A changed file's state (the letters M, A, D, R, U, C of the diff view).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentGitFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    /// Its type changed (a file became a link).
    Type,
    /// New, not tracked yet ("New", or "New folder").
    Untracked,
    Conflict,
}

impl AgentGitFileStatus {
    fn from_wire(word: &str) -> Option<Self> {
        use core::git::file_status;
        Some(match word {
            file_status::ADDED => Self::Added,
            file_status::MODIFIED => Self::Modified,
            file_status::DELETED => Self::Deleted,
            file_status::RENAMED => Self::Renamed,
            file_status::TYPE => Self::Type,
            file_status::UNTRACKED => Self::Untracked,
            file_status::CONFLICT => Self::Conflict,
            _ => return None,
        })
    }
}

/// How much of a file's change the user staged already.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentGitStaged {
    Staged,
    PartlyStaged,
    NotStaged,
}

impl AgentGitStaged {
    fn from_wire(word: &str) -> Option<Self> {
        use core::git::staged;
        Some(match word {
            staged::ALL_OF_IT => Self::Staged,
            staged::PART => Self::PartlyStaged,
            staged::NONE => Self::NotStaged,
            _ => return None,
        })
    }
}

/// What a job does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentJobKind {
    Commit,
    Push,
    Fetch,
    Switch,
    /// A new worktree, made on the PC.
    Worktree,
    PrCreate,
    /// A merge, made on the PC.
    PrMerge,
}

impl AgentJobKind {
    fn from_wire(word: &str) -> Option<Self> {
        use core::git::kind;
        Some(match word {
            kind::COMMIT => Self::Commit,
            kind::PUSH => Self::Push,
            kind::FETCH => Self::Fetch,
            kind::SWITCH => Self::Switch,
            kind::WORKTREE => Self::Worktree,
            kind::PR_CREATE => Self::PrCreate,
            kind::PR_MERGE => Self::PrMerge,
            _ => return None,
        })
    }
}

/// Where a job stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentJobPhase {
    Running,
    Done,
    /// `error` says why.
    Failed,
    Stopped,
}

impl AgentJobPhase {
    fn from_wire(word: &str) -> Option<Self> {
        use core::git::phase;
        Some(match word {
            phase::RUNNING => Self::Running,
            phase::DONE => Self::Done,
            phase::FAILED => Self::Failed,
            phase::STOPPED => Self::Stopped,
            _ => return None,
        })
    }
}

/// Why a reviewed file can't be committed from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentUnreadable {
    /// "Too large to read here. Commit it on %1."
    Large,
    /// An untracked folder: this phone sees only its names, never its files'
    /// bytes, so it's committed on the PC.
    Folder,
}

/// What "Ask <agent> to write it" is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentWritePurpose {
    Commit,
    Pr,
}

impl AgentWritePurpose {
    fn wire(self) -> &'static str {
        use core::git::purpose;
        match self {
            Self::Commit => purpose::COMMIT,
            Self::Pr => purpose::PR,
        }
    }

    fn from_wire(word: &str) -> Option<Self> {
        use core::git::purpose;
        Some(match word {
            purpose::COMMIT => Self::Commit,
            purpose::PR => Self::Pr,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentPrState {
    Open,
    Merged,
    Closed,
}

impl AgentPrState {
    fn from_wire(word: &str) -> Option<Self> {
        use core::pr::state;
        Some(match word {
            state::OPEN => Self::Open,
            state::MERGED => Self::Merged,
            state::CLOSED => Self::Closed,
            _ => return None,
        })
    }
}

/// A pull request's review decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentPrReview {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentMergeable {
    Yes,
    /// "Has conflicts with %1".
    Conflicts,
    Unknown,
}

/// A check's state; one the PC didn't know is `Pending`, never `Pass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentCheckState {
    Pass,
    Fail,
    Pending,
    /// Counted in neither passed, failed nor running, nor in "N checks".
    Skipped,
    Cancelled,
}

impl AgentCheckState {
    fn from_wire(word: &str) -> Self {
        use core::pr::check;
        match word {
            check::PASS => Self::Pass,
            check::FAIL => Self::Fail,
            check::SKIPPED => Self::Skipped,
            check::CANCELLED => Self::Cancelled,
            _ => Self::Pending,
        }
    }
}

/// The GitHub CLI on the PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentGhUse {
    Ok,
    /// "Pull requests need the GitHub CLI."
    Missing,
    /// "You're signed out of the GitHub CLI."
    SignedOut,
    /// "Your GitHub CLI is too old for this."
    Old,
}

/// Which pull requests `agent_pr_list` lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentPrShow {
    /// "Mine".
    Mine,
    /// "Review requested".
    Review,
    /// "This folder": with `agent_pr_list`'s `folder`, a folder's id from the catalog.
    Folder,
}

/// Why the branch has no pull request to show, or an old one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentPrWhy {
    /// "This folder's remote isn't on GitHub."
    NotGithub,
    /// "Push this branch to open a pull request."
    NoUpstream,
    /// "You're not on a branch."
    Detached,
    /// "GitHub couldn't be reached. Showing what was read at %1." (`pr` is that).
    Network,
    /// "This folder isn't a git repository."
    NotRepo,
    /// "This pull request is too large to show here."
    TooLarge,
    /// The thread's branch has no pull request.
    NoPr,
}

impl AgentPrWhy {
    fn from_wire(word: &str) -> Option<Self> {
        use core::pr::why;
        Some(match word {
            why::NOT_GITHUB => Self::NotGithub,
            why::NO_UPSTREAM => Self::NoUpstream,
            why::DETACHED => Self::Detached,
            why::NETWORK => Self::Network,
            why::NOT_REPO => Self::NotRepo,
            why::TOO_LARGE => Self::TooLarge,
            why::NO_PR => Self::NoPr,
            _ => return None,
        })
    }
}

// ---- What a thread gains ----

/// The worktree a thread works in: its label and branch.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentGitWorktree {
    pub label: String,
    pub branch: String,
}

quiet_debug!(AgentGitWorktree {});

impl From<core::AgentGitWorktree> for AgentGitWorktree {
    fn from(w: core::AgentGitWorktree) -> Self {
        Self { label: w.label, branch: w.branch }
    }
}

/// What the agent wrote when asked: fill the open field with `text` if the
/// user hasn't typed in it since asking, else offer "Use %1's message".
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentWrote {
    pub purpose: AgentWritePurpose,
    /// The turn that answered.
    pub turn: u32,
    pub text: String,
    pub at_ms: i64,
}

quiet_debug!(AgentWrote { purpose, turn });

impl AgentWrote {
    pub(crate) fn from_core(w: core::AgentWrote) -> Option<Self> {
        Some(Self {
            purpose: AgentWritePurpose::from_wire(&w.purpose)?,
            turn: w.turn,
            text: w.text,
            at_ms: ms(w.at_ms),
        })
    }
}

// ---- Status ----

/// A remote by its name and label (host and path); no label for a folder
/// on the PC (`local`: "a folder on %1").
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentGitRemote {
    pub name: String,
    pub label: Option<String>,
    pub local: bool,
}

quiet_debug!(AgentGitRemote { name, local });

/// A changed file, relative to the thread's folder; an untracked folder's
/// path ends in `/` (`folder`).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentGitFile {
    pub path: String,
    /// Its earlier path (`Renamed`).
    pub from: Option<String>,
    /// Null for a word this build doesn't know: show it as changed.
    pub status: Option<AgentGitFileStatus>,
    pub staged: Option<AgentGitStaged>,
    pub folder: bool,
    pub adds: Option<u32>,
    pub dels: Option<u32>,
    pub binary: bool,
}

quiet_debug!(AgentGitFile { status, staged, folder, adds, dels, binary });

impl From<core::AgentGitFile> for AgentGitFile {
    fn from(f: core::AgentGitFile) -> Self {
        Self {
            status: f.status.as_deref().and_then(AgentGitFileStatus::from_wire),
            staged: f.staged.as_deref().and_then(AgentGitStaged::from_wire),
            path: f.path,
            from: f.from,
            folder: f.folder,
            adds: f.adds,
            dels: f.dels,
            binary: f.binary,
        }
    }
}

/// The last commit: "Last commit: %1".
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentGitLast {
    pub oid7: String,
    pub subject: String,
    pub ms: i64,
}

quiet_debug!(AgentGitLast { oid7, ms });

/// The folder's repository on GitHub: `owner/name` and its host.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentGitHub {
    pub repo: String,
    pub host: String,
}

/// A thread's folder's git status: at most 500 files (`more` past that;
/// `total` counts them all).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentGit {
    pub repo: AgentRepo,
    pub branch: Option<String>,
    /// Not on a branch: "Detached at %1".
    pub detached: Option<String>,
    /// "No commits yet."
    pub unborn: bool,
    pub upstream: Option<String>,
    /// Commits to push ("↑2"), and on the remote as of the last fetch ("↓1").
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub remote: Option<String>,
    pub remotes: Vec<AgentGitRemote>,
    pub default_branch: Option<String>,
    /// On the remote's default branch: "This pushes to %1, the default branch."
    pub on_default: bool,
    pub doing: Option<AgentGitDoing>,
    /// "Git is busy in this folder. Try again in a moment."
    pub locked: bool,
    pub files: Vec<AgentGitFile>,
    pub more: bool,
    pub total: u32,
    pub adds: u32,
    pub dels: u32,
    pub conflicts: u32,
    pub last: Option<AgentGitLast>,
    pub worktree: Option<AgentGitWorktree>,
    pub fetched_ms: Option<i64>,
    /// "Checked %1".
    pub checked_ms: i64,
    pub github: Option<AgentGitHub>,
    /// Changes when anything listed changes; empty when not a repository.
    pub fingerprint: String,
}

quiet_debug!(AgentGit { repo, total, more, ahead, behind, doing, locked, fingerprint });

impl AgentGit {
    fn from_core(g: core::AgentGit) -> Option<Self> {
        Some(Self {
            repo: AgentRepo::from_wire(&g.repo)?,
            branch: g.branch,
            detached: g.detached,
            unborn: g.unborn,
            upstream: g.upstream,
            ahead: g.ahead,
            behind: g.behind,
            remote: g.remote,
            remotes: g
                .remotes
                .into_iter()
                .map(|r| AgentGitRemote { name: r.name, label: r.label, local: r.local })
                .collect(),
            default_branch: g.default_branch,
            on_default: g.on_default,
            doing: g.doing.as_deref().and_then(AgentGitDoing::from_wire),
            locked: g.locked,
            files: g.files.into_iter().map(Into::into).collect(),
            more: g.more,
            total: g.total,
            adds: g.adds,
            dels: g.dels,
            conflicts: g.conflicts,
            last: g.last.map(|l| AgentGitLast { oid7: l.oid7, subject: l.subject, ms: ms(l.ms) }),
            worktree: g.worktree.map(Into::into),
            fetched_ms: g.fetched_ms.map(ms),
            checked_ms: ms(g.checked_ms),
            github: g.github.map(|h| AgentGitHub { repo: h.repo, host: h.host }),
            fingerprint: g.fingerprint,
        })
    }
}

/// One file's uncommitted patch, at most 1 MiB (`truncated` past that:
/// it can't be committed from here); an untracked folder's first 50 names
/// instead (`more_names` past them).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentGitPatch {
    pub patch: String,
    pub truncated: bool,
    pub binary: bool,
    pub names: Vec<String>,
    pub more_names: u32,
}

quiet_debug!(AgentGitPatch { truncated, binary, more_names });

/// One file of a review.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentGitReviewFile {
    pub path: String,
    /// False: unticked and disabled, `why` says why.
    pub readable: bool,
    pub why: Option<AgentUnreadable>,
}

quiet_debug!(AgentGitReviewFile { readable, why });

/// A review: pass `review` to `agent_git_patch` for each file, then to
/// `agent_git_commit`. The PC keeps 4 a phone, 10 minutes each; it commits once.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentGitReview {
    pub review: String,
    pub files: Vec<AgentGitReviewFile>,
}

impl From<core::AgentGitReview> for AgentGitReview {
    fn from(r: core::AgentGitReview) -> Self {
        Self {
            review: r.review,
            files: r
                .files
                .into_iter()
                .map(|f| AgentGitReviewFile {
                    readable: f.readable,
                    why: (!f.readable).then_some(match f.why.as_deref() {
                        Some(core::git::unreadable::FOLDER) => AgentUnreadable::Folder,
                        _ => AgentUnreadable::Large,
                    }),
                    path: f.path,
                })
                .collect(),
        }
    }
}

// ---- Jobs ----

/// What a job that went through made.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentJobResult {
    pub oid7: Option<String>,
    pub subject: Option<String>,
    /// Files committed.
    pub files: Option<u32>,
    /// The branch pushed, or switched to.
    pub branch: Option<String>,
    pub remote: Option<String>,
    /// "Already up to date".
    pub up_to_date: bool,
    /// "Created %1".
    pub created: bool,
    /// The pull request opened: "Opened pull request #%1".
    pub pr: Option<AgentPrInfo>,
    /// The pull request merged.
    pub number: Option<u32>,
}

quiet_debug!(AgentJobResult { oid7, files, up_to_date, created, number });

/// Why a job didn't go through. `reason` is a word of
/// `docs/protocol/agents.md` §8 (`hook`, `behind`, `changed`, `dirty`, …);
/// show one this build doesn't know as `failed`.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentJobError {
    pub reason: String,
    /// The PC's own words, for a reason this build doesn't word itself.
    pub message: String,
    /// `pick_remote`: "Choose a remote", then `agent_git_push` with one.
    pub names: Vec<String>,
}

quiet_debug!(AgentJobError { reason, names });

/// A source-control job on the PC. Its later lines and its end come as
/// `Event.AgentGitJob`; `agent_git_job` reads it again (10 minutes after it
/// ends it's gone).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentJob {
    pub id: String,
    pub thread: String,
    pub kind: AgentJobKind,
    pub phase: AgentJobPhase,
    pub started_ms: i64,
    pub ended_ms: Option<i64>,
    /// This phone asked for it: it may Stop it (`agent_git_stop`).
    pub mine: bool,
    /// The newest lines, at most 40 and 8 KiB, oldest first.
    pub lines: Vec<String>,
    /// The number of `lines[0]`, counting from the job's first line.
    pub from: u64,
    /// "%n earlier lines not kept".
    pub dropped: u32,
    pub result: Option<AgentJobResult>,
    pub error: Option<AgentJobError>,
}

quiet_debug!(AgentJob { id, thread, kind, phase, mine, from, dropped, error });

impl AgentJob {
    pub(crate) fn from_core(j: core::AgentJob) -> Option<Self> {
        Some(Self {
            kind: AgentJobKind::from_wire(&j.kind)?,
            phase: AgentJobPhase::from_wire(&j.phase)?,
            id: j.id,
            thread: j.thread,
            started_ms: ms(j.started_ms),
            ended_ms: j.ended_ms.map(ms),
            mine: j.mine,
            lines: j.lines,
            from: j.from,
            dropped: j.dropped,
            result: j.result.map(|r| AgentJobResult {
                oid7: r.oid7,
                subject: r.subject,
                files: r.files,
                branch: r.branch,
                remote: r.remote,
                up_to_date: r.up_to_date,
                created: r.created,
                pr: r.pr.and_then(AgentPrInfo::from_core),
                number: r.number,
            }),
            error: j.error.map(|e| AgentJobError { reason: e.reason, message: e.message, names: e.names }),
        })
    }
}

fn job_or_internal(job: core::AgentJob) -> Result<AgentJob> {
    AgentJob::from_core(job).ok_or_else(|| malformed("agent.git job"))
}

// ---- Branches ----

/// A branch: local, or only on a remote (`remote`; "On %1").
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentBranch {
    pub name: String,
    pub remote: Option<String>,
    pub current: bool,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    /// Checked out in another worktree: "Open in another worktree (%1)".
    pub worktree: Option<String>,
    pub last_ms: i64,
}

quiet_debug!(AgentBranch { current, ahead, behind });

/// The branches: local first, then those only on a remote, each newest first.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentBranches {
    pub branches: Vec<AgentBranch>,
    pub more: bool,
    pub current: Option<String>,
}

impl From<core::AgentBranches> for AgentBranches {
    fn from(b: core::AgentBranches) -> Self {
        Self {
            branches: b
                .branches
                .into_iter()
                .map(|b| AgentBranch {
                    name: b.name,
                    remote: b.remote,
                    current: b.current,
                    upstream: b.upstream,
                    ahead: b.ahead,
                    behind: b.behind,
                    worktree: b.worktree,
                    last_ms: ms(b.last_ms),
                })
                .collect(),
            more: b.more,
            current: b.current,
        }
    }
}

// ---- Pull requests ----

/// One check of a pull request.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentCheck {
    pub name: String,
    pub state: AgentCheckState,
    /// `https`; open it only on a host of `AgentGhState.hosts`.
    pub url: Option<String>,
}

quiet_debug!(AgentCheck { state });

/// The branch's pull request.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPrInfo {
    pub number: u32,
    pub title: String,
    pub state: AgentPrState,
    pub draft: bool,
    /// `https` ("Open on GitHub"), empty when the PC had none to give.
    pub url: String,
    /// "into %1".
    pub base: String,
    pub head: String,
    pub review: Option<AgentPrReview>,
    pub mergeable: Option<AgentMergeable>,
    pub blocked: bool,
    pub updated_ms: i64,
    /// Failing first, at most 100.
    pub checks: Vec<AgentCheck>,
    /// "7 checks passed", "2 of 7 failed", "3 of 7 running"; skipped and
    /// cancelled ones are in none of these.
    pub pass: u32,
    pub fail: u32,
    pub pending: u32,
    /// "Read %1".
    pub read_ms: i64,
}

quiet_debug!(AgentPrInfo { number, state, draft, pass, fail, pending });

impl AgentPrInfo {
    fn from_core(p: core::AgentPrInfo) -> Option<Self> {
        use core::pr::{mergeable, review};
        Some(Self {
            state: AgentPrState::from_wire(&p.state)?,
            number: p.number,
            title: p.title,
            draft: p.draft,
            url: p.url,
            base: p.base,
            head: p.head,
            review: p.review.as_deref().and_then(|w| match w {
                review::APPROVED => Some(AgentPrReview::Approved),
                review::CHANGES_REQUESTED => Some(AgentPrReview::ChangesRequested),
                review::REVIEW_REQUIRED => Some(AgentPrReview::ReviewRequired),
                _ => None,
            }),
            mergeable: p.mergeable.as_deref().and_then(|w| match w {
                mergeable::YES => Some(AgentMergeable::Yes),
                mergeable::CONFLICTS => Some(AgentMergeable::Conflicts),
                mergeable::UNKNOWN => Some(AgentMergeable::Unknown),
                _ => None,
            }),
            blocked: p.blocked,
            updated_ms: ms(p.updated_ms),
            checks: p
                .checks
                .into_iter()
                .map(|c| AgentCheck { state: AgentCheckState::from_wire(&c.state), name: c.name, url: c.url })
                .collect(),
            pass: p.pass,
            fail: p.fail,
            pending: p.pending,
            read_ms: ms(p.read_ms),
        })
    }
}

/// A pull request in brief, as a thread's strip shows it ("#212" and its checks).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPrBrief {
    pub number: u32,
    pub state: AgentPrState,
    pub draft: bool,
    pub pass: u32,
    pub fail: u32,
    pub pending: u32,
}

impl AgentPrBrief {
    fn from_core(p: core::AgentPrBrief) -> Option<Self> {
        Some(Self {
            state: AgentPrState::from_wire(&p.state)?,
            number: p.number,
            draft: p.draft,
            pass: p.pass,
            fail: p.fail,
            pending: p.pending,
        })
    }
}

/// The GitHub CLI on the PC: open a link only on one of `hosts`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentGhState {
    pub gh: AgentGhUse,
    /// "GitHub CLI %1, signed in".
    pub version: Option<String>,
    pub hosts: Vec<String>,
    pub checked_ms: i64,
}

impl AgentGhState {
    fn from_core(g: core::AgentGhState) -> Option<Self> {
        use core::pr::gh;
        Some(Self {
            gh: match g.gh.as_str() {
                gh::OK => AgentGhUse::Ok,
                gh::MISSING => AgentGhUse::Missing,
                gh::SIGNED_OUT => AgentGhUse::SignedOut,
                gh::OLD => AgentGhUse::Old,
                _ => return None,
            },
            version: g.version,
            hosts: g.hosts,
            checked_ms: ms(g.checked_ms),
        })
    }
}

/// The thread's branch's pull request, or why there's none.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPrView {
    pub pr: Option<AgentPrInfo>,
    pub gh: AgentGhState,
    pub github: Option<AgentGitHub>,
    pub why: Option<AgentPrWhy>,
}

impl AgentPrView {
    fn from_core(v: core::AgentPrView) -> Option<Self> {
        Some(Self {
            gh: AgentGhState::from_core(v.gh)?,
            pr: v.pr.and_then(AgentPrInfo::from_core),
            github: v.github.map(|h| AgentGitHub { repo: h.repo, host: h.host }),
            why: v.why.as_deref().and_then(AgentPrWhy::from_wire),
        })
    }
}

/// Check counts of a pull request row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AgentCheckCounts {
    pub pass: u32,
    pub fail: u32,
    pub pending: u32,
}

/// One row of the pull requests list.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPrRow {
    pub number: u32,
    pub title: String,
    /// `owner/name`.
    pub repo: String,
    pub state: AgentPrState,
    pub draft: bool,
    pub updated_ms: i64,
    /// `https`, empty when the PC had none to give.
    pub url: String,
    pub branch: Option<String>,
    pub checks: Option<AgentCheckCounts>,
    /// "Open thread": a thread on the PC on that branch.
    pub thread: Option<String>,
}

quiet_debug!(AgentPrRow { number, state, draft, checks, thread });

/// The pull requests list: at most 50.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPrRows {
    pub prs: Vec<AgentPrRow>,
    pub more: bool,
    pub gh: AgentGhState,
}

impl AgentPrRows {
    fn from_core(r: core::AgentPrRows) -> Option<Self> {
        Some(Self {
            gh: AgentGhState::from_core(r.gh)?,
            prs: r
                .prs
                .into_iter()
                .filter_map(|p| {
                    Some(AgentPrRow {
                        state: AgentPrState::from_wire(&p.state)?,
                        number: p.number,
                        title: p.title,
                        repo: p.repo,
                        draft: p.draft,
                        updated_ms: ms(p.updated_ms),
                        url: p.url,
                        branch: p.branch,
                        checks: p.checks.map(|c| AgentCheckCounts {
                            pass: c.pass,
                            fail: c.fail,
                            pending: c.pending,
                        }),
                        thread: p.thread,
                    })
                })
                .collect(),
            more: r.more,
        })
    }
}

// ---- Pull request detail (§13) ----

/// A file a pull request changes: its letter from `change` (`added`,
/// `deleted`, `renamed`, `copied`, `modified`, or `changed` for anything
/// else), its path relative to the repository, "+%1 −%2".
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPrFile {
    pub path: String,
    pub change: String,
    pub adds: u32,
    pub dels: u32,
}

quiet_debug!(AgentPrFile { change, adds, dels });

/// A comment on the pull request's conversation, or a review's summary
/// (`review`: `approved`, `changes_requested`, `commented` or `dismissed`).
/// Render `body` as Markdown under the render crate's bounds.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPrComment {
    pub author: String,
    pub at_ms: i64,
    pub body: String,
    /// "Shortened to fit."
    pub cut: bool,
    pub review: Option<String>,
    /// `https`; open it only on a host of `AgentGhState.hosts`.
    pub url: Option<String>,
}

quiet_debug!(AgentPrComment { at_ms, cut, review });

/// A pull request's description, changed files, checks and conversation.
/// Read-only: nothing here posts to GitHub. Below the comments, always:
/// "Comments made on lines of code aren't shown here. Read them on GitHub."
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPrDetail {
    /// The card on top (state, title, "into", checks, review line).
    pub pr: Option<AgentPrInfo>,
    pub gh: AgentGhState,
    /// `Network`: "GitHub couldn't be reached", the rest is what was read
    /// last; `TooLarge`: "This pull request is too large to show here."
    pub why: Option<AgentPrWhy>,
    /// `owner/name`: pass it to `agent_pr_diff`.
    pub repo: String,
    /// "opened by %1".
    pub author: String,
    /// The description, Markdown ("No description." when empty).
    pub body: String,
    /// "Shortened to fit."
    pub body_cut: bool,
    pub adds: u32,
    pub dels: u32,
    /// "%n files changed".
    pub changed: u32,
    /// At most 300.
    pub files: Vec<AgentPrFile>,
    /// "%1 more files aren't listed." (`changed` minus `files`).
    pub files_more: bool,
    /// Oldest first, the newest 50.
    pub comments: Vec<AgentPrComment>,
    /// "%n older comments aren't shown."
    pub comments_older: u32,
    /// A thread on the PC on that repository and branch: "Ask %1 to review
    /// it" goes there; null: "To ask an agent, open a thread in this
    /// repository, or ask on %1."
    pub thread: Option<String>,
    /// "Read %1".
    pub read_ms: i64,
}

quiet_debug!(AgentPrDetail { why, files_more, comments_older, thread });

impl AgentPrDetail {
    fn from_core(d: core::AgentPrDetail) -> Option<Self> {
        use core::pr::change;
        Some(Self {
            gh: AgentGhState::from_core(d.gh)?,
            pr: d.pr.and_then(AgentPrInfo::from_core),
            why: d.why.as_deref().and_then(AgentPrWhy::from_wire),
            repo: d.repo,
            author: d.author,
            body: d.body,
            body_cut: d.body_cut,
            adds: d.adds,
            dels: d.dels,
            changed: d.changed,
            files: d
                .files
                .into_iter()
                .map(|f| AgentPrFile {
                    change: if change::ALL.contains(&f.change.as_str()) {
                        f.change
                    } else {
                        change::CHANGED.into()
                    },
                    path: f.path,
                    adds: f.adds,
                    dels: f.dels,
                })
                .collect(),
            files_more: d.files_more,
            comments: d
                .comments
                .into_iter()
                .map(|c| AgentPrComment {
                    author: c.author,
                    at_ms: ms(c.at_ms),
                    body: c.body,
                    cut: c.cut,
                    review: c.review,
                    url: c.url,
                })
                .collect(),
            comments_older: d.comments_older,
            thread: d.thread,
            read_ms: ms(d.read_ms),
        })
    }
}

// ---- The strip ----

/// A thread's strip: "feat/sync-retry · 4 changed · ↑2", the pull request.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentGitStrip {
    pub branch: Option<String>,
    pub detached: bool,
    pub changed: u32,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub pr: Option<AgentPrBrief>,
}

quiet_debug!(AgentGitStrip { detached, changed, ahead, behind, pr });

impl From<core::AgentGitChanged> for AgentGitStrip {
    fn from(c: core::AgentGitChanged) -> Self {
        Self {
            branch: c.branch,
            detached: c.detached,
            changed: c.changed,
            ahead: c.ahead,
            behind: c.behind,
            pr: c.pr.and_then(AgentPrBrief::from_core),
        }
    }
}

/// A thread's strip as saved on this phone: "as of %1 ago".
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentCachedGit {
    pub strip: Option<AgentGitStrip>,
    pub saved_ms: Option<i64>,
}

// Errors as the other agent methods: `Denied` when the PC's `agents` toggle
// is off for this phone, and for a push or a pull request while its
// `agent_push` is off, or a commit of a review not read whole here;
// `NotFound` an unknown thread or job, or a review the PC no longer keeps
// (read the files again); `Internal` for the rest (a reason starting `BUSY`
// while git or the thread is busy).
#[uniffi::export]
impl G7LinkNode {
    /// The thread's folder's git status.
    pub async fn agent_git_status(&self, id: String, thread: String) -> Result<AgentGit> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let status = self.run(async move { node.agent_git_status(id, thread).await }).await?;
        AgentGit::from_core(status).ok_or_else(|| malformed("agent.git.status"))
    }

    /// One listed file's uncommitted patch. With `review`, the PC counts the
    /// file read once all of it reached this phone (show it to its end).
    #[uniffi::method(default(review = None))]
    pub async fn agent_git_patch(
        &self,
        id: String,
        thread: String,
        path: String,
        review: Option<String>,
    ) -> Result<AgentGitPatch> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let p = self.run(async move { node.agent_git_patch(id, thread, path, review).await }).await?;
        Ok(AgentGitPatch {
            patch: p.patch,
            truncated: p.truncated,
            binary: p.binary,
            names: p.names,
            more_names: p.more_names,
        })
    }

    /// Pins 1 to 200 listed files as they are now, before they're read and committed.
    pub async fn agent_git_review(
        &self,
        id: String,
        thread: String,
        paths: Vec<String>,
    ) -> Result<AgentGitReview> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        Ok(self.run(async move { node.agent_git_review(id, thread, paths).await }).await?.into())
    }

    /// Commits the files of `review` with `message` (1 to 8,192 bytes). A
    /// job that couldn't start comes back already `Failed`, with its reason.
    pub async fn agent_git_commit(
        &self,
        id: String,
        thread: String,
        review: String,
        message: String,
    ) -> Result<AgentJob> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        job_or_internal(
            self.run(async move { node.agent_git_commit(id, thread, review, message).await }).await?,
        )
    }

    /// Pushes the thread's branch; `remote` after a `pick_remote`.
    #[uniffi::method(default(remote = None))]
    pub async fn agent_git_push(
        &self,
        id: String,
        thread: String,
        remote: Option<String>,
    ) -> Result<AgentJob> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        job_or_internal(self.run(async move { node.agent_git_push(id, thread, remote).await }).await?)
    }

    /// Fetches the branch's remote (Refresh).
    pub async fn agent_git_fetch(&self, id: String, thread: String) -> Result<AgentJob> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        job_or_internal(self.run(async move { node.agent_git_fetch(id, thread).await }).await?)
    }

    /// The branches whose names hold `query`.
    #[uniffi::method(default(query = None))]
    pub async fn agent_git_branches(
        &self,
        id: String,
        thread: String,
        query: Option<String>,
    ) -> Result<AgentBranches> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        Ok(self.run(async move { node.agent_git_branches(id, thread, query).await }).await?.into())
    }

    /// Switches to a listed branch, one only on a remote (`track`), or a new
    /// one (`create`); only while nothing is changed (else `dirty`).
    #[uniffi::method(default(create = false, track = false))]
    pub async fn agent_git_switch(
        &self,
        id: String,
        thread: String,
        branch: String,
        create: bool,
        track: bool,
    ) -> Result<AgentJob> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        job_or_internal(
            self.run(async move { node.agent_git_switch(id, thread, branch, create, track).await }).await?,
        )
    }

    /// A job as it stands now.
    pub async fn agent_git_job(&self, id: String, job: String) -> Result<AgentJob> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        job_or_internal(self.run(async move { node.agent_git_job(id, job).await }).await?)
    }

    /// Stops a job this phone started (`mine`).
    pub async fn agent_git_stop(&self, id: String, job: String) -> Result<AgentJob> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        job_or_internal(self.run(async move { node.agent_git_stop(id, job).await }).await?)
    }

    /// The thread's branch's pull request; `refresh` reads GitHub now.
    #[uniffi::method(default(refresh = false))]
    pub async fn agent_pr_view(&self, id: String, thread: String, refresh: bool) -> Result<AgentPrView> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let view = self.run(async move { node.agent_pr_view(id, thread, refresh).await }).await?;
        AgentPrView::from_core(view).ok_or_else(|| malformed("agent.pr.view"))
    }

    /// Opens a pull request from the thread's pushed branch into `base` (null:
    /// the repository's default).
    #[uniffi::method(default(base = None, draft = false))]
    pub async fn agent_pr_create(
        &self,
        id: String,
        thread: String,
        title: String,
        body: String,
        base: Option<String>,
        draft: bool,
    ) -> Result<AgentJob> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let request = core::AgentPrCreateRequest { thread, title, body, base, draft };
        job_or_internal(self.run(async move { node.agent_pr_create(id, request).await }).await?)
    }

    /// The open pull requests: mine, waiting for my review, or a folder's
    /// (`Folder` with `folder`, its id from the catalog; only then).
    #[uniffi::method(default(folder = None, refresh = false))]
    pub async fn agent_pr_list(
        &self,
        id: String,
        show: AgentPrShow,
        folder: Option<String>,
        refresh: bool,
    ) -> Result<AgentPrRows> {
        use core::pr::show as words;
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let show = match show {
            AgentPrShow::Mine => words::MINE,
            AgentPrShow::Review => words::REVIEW,
            AgentPrShow::Folder => words::FOLDER,
        };
        let request = core::AgentPrListRequest { show: show.to_owned(), folder, refresh };
        let rows = self.run(async move { node.agent_pr_list(id, request).await }).await?;
        AgentPrRows::from_core(rows).ok_or_else(|| malformed("agent.pr.list"))
    }

    /// "Ask <agent> to write it": the PC sends its fixed message in the
    /// thread; the answer comes as the thread's `wrote`.
    pub async fn agent_ask(
        &self,
        id: String,
        thread: String,
        purpose: AgentWritePurpose,
    ) -> Result<AgentThread> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let purpose = purpose.wire().to_owned();
        let thread = self.run(async move { node.agent_write(id, thread, purpose).await }).await?;
        AgentThread::from_core(thread).ok_or_else(|| malformed("agent.write"))
    }

    /// A pull request's detail: the thread's branch's (`thread`), or one
    /// `agent_pr_list` listed (`repo` and `number`, both or neither). Read
    /// only; GitHub that can't be reached is `why` `Network` with the last
    /// detail read, never an error.
    #[uniffi::method(default(thread = None, repo = None, number = None, refresh = false))]
    pub async fn agent_pr_detail(
        &self,
        id: String,
        thread: Option<String>,
        repo: Option<String>,
        number: Option<u32>,
        refresh: bool,
    ) -> Result<AgentPrDetail> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let request = core::AgentPrDetailRequest { thread, repo, number, refresh };
        let detail = self.run(async move { node.agent_pr_detail(id, request).await }).await?;
        AgentPrDetail::from_core(detail).ok_or_else(|| malformed("agent.pr.detail"))
    }

    /// One listed file's diff in a pull request ("Pull request" scope of the
    /// diff view), at most 1 MiB (`truncated`: "Only the first part of this
    /// diff is shown.").
    pub async fn agent_pr_diff(
        &self,
        id: String,
        repo: String,
        number: u32,
        path: String,
    ) -> Result<AgentGitPatch> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let p = self.run(async move { node.agent_pr_diff(id, repo, number, path).await }).await?;
        Ok(AgentGitPatch {
            patch: p.patch,
            truncated: p.truncated,
            binary: p.binary,
            names: p.names,
            more_names: p.more_names,
        })
    }

    /// "Ask <agent> to review it", after the user confirmed: the PC sends
    /// one fixed message in the thread naming pull request `number` by
    /// number and link. Nothing is posted to GitHub.
    pub async fn agent_pr_ask(&self, id: String, thread: String, number: u32) -> Result<AgentThread> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let thread = self.run(async move { node.agent_ask_review(id, thread, number).await }).await?;
        AgentThread::from_core(thread).ok_or_else(|| malformed("agent.write"))
    }

    /// A thread's strip as saved on this phone, at once and offline.
    pub async fn agent_cached_git(&self, id: String, thread: String) -> Result<AgentCachedGit> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let cached = self.run(async move { node.agent_cached_git(id, thread).await }).await?;
        Ok(AgentCachedGit { strip: cached.value.map(Into::into), saved_ms: cached.saved_ms.map(ms) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_and_a_status_cross_with_their_words() {
        let job = core::AgentJob {
            id: "0123456789abcdef".into(),
            thread: "t1".into(),
            kind: "pr_create".into(),
            phase: "failed".into(),
            mine: true,
            error: Some(core::AgentJobError { reason: "not_pushed".into(), ..Default::default() }),
            ..Default::default()
        };
        let job = AgentJob::from_core(job).unwrap();
        assert_eq!((job.kind, job.phase, job.mine), (AgentJobKind::PrCreate, AgentJobPhase::Failed, true));
        assert_eq!(job.error.map(|e| e.reason), Some("not_pushed".into()));
        let status = core::AgentGit {
            repo: "none".into(),
            files: vec![core::AgentGitFile {
                path: "a.rs".into(),
                status: Some("type".into()),
                staged: Some("part".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let status = AgentGit::from_core(status).unwrap();
        assert_eq!(status.repo, AgentRepo::None);
        assert_eq!(status.files[0].status, Some(AgentGitFileStatus::Type));
        assert_eq!(status.files[0].staged, Some(AgentGitStaged::PartlyStaged));
        assert!(!format!("{status:?}").contains("a.rs"));
        assert_eq!(AgentCheckState::from_wire("neutral"), AgentCheckState::Pending, "never pass");
    }

    #[test]
    fn a_pull_requests_detail_crosses_with_its_words() {
        let detail = core::AgentPrDetail {
            gh: core::AgentGhState { gh: "ok".into(), ..Default::default() },
            why: Some("too_large".into()),
            repo: "acme/app".into(),
            author: "secret".into(),
            files: vec![core::AgentPrFile {
                path: "secret.rs".into(),
                change: "type_changed".into(),
                adds: 1,
                dels: 2,
            }],
            comments: vec![core::AgentPrComment {
                body: "secret".into(),
                review: Some("approved".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let detail = AgentPrDetail::from_core(detail).unwrap();
        assert_eq!(detail.why, Some(AgentPrWhy::TooLarge));
        assert_eq!(detail.files[0].change, "changed");
        assert_eq!(detail.comments[0].review.as_deref(), Some("approved"));
        assert!(!format!("{detail:?}").contains("secret"));
        assert_eq!(AgentPrWhy::from_wire("no_pr"), Some(AgentPrWhy::NoPr));
        assert_eq!(AgentPrWhy::from_wire("archived"), None);
        let gone = core::AgentPrDetail {
            gh: core::AgentGhState { gh: "maybe".into(), ..Default::default() },
            ..Default::default()
        };
        assert!(AgentPrDetail::from_core(gone).is_none());
    }
}
