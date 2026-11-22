// SPDX-License-Identifier: MPL-2.0
//! Bodies of source control (`docs/protocol/agents.md` §8): a thread's
//! folder's git status and its uncommitted diff, a review of the files a
//! phone read before it commits them, commits, pushes, fetches, branches,
//! the branch's pull request and the lists of pull requests, and "Ask the
//! agent to write it". Plain protocol v1: no capability, governed by the
//! `agents` toggle; push and opening a pull request also by `agent_push`.
//!
//! A phone never receives a remote's URL, an absolute path, a worktree's
//! path, a remote that is a folder on the PC by its path, or an account
//! name: the PC's core makes every answer fit before it leaves. A phone can
//! never ask for a merge, a worktree, a switch that takes changes along, or
//! "keep in step with the remote": no message says it. A word this build
//! doesn't know reads as absent where a field may be absent.

use std::ops::Not;

use serde::{Deserialize, Serialize};

use super::{
    clean_agent_text, clean_line, cut_text, fits_word, is_agent_word, is_bidi_control, is_lower_hex,
    is_relative_path, is_valid_agent_id, known_word,
};

/// Limits and wire words of §8's git messages.
pub mod git {
    /// Files in a status a phone gets: the first.
    pub const MAX_FILES: usize = 500;
    /// The most of one file's patch a phone gets; the PC stops there and
    /// sets `truncated`.
    pub const MAX_PATCH_BYTES: usize = 1024 * 1024;
    /// Names an untracked folder's patch lists.
    pub const MAX_FOLDER_NAMES: usize = 50;
    /// Paths one review pins.
    pub const MAX_REVIEW_PATHS: usize = 200;
    /// A commit message, in bytes.
    pub const MAX_MESSAGE_BYTES: usize = 8192;
    /// A branch's name (`main`, `origin/feat`).
    pub const MAX_BRANCH_BYTES: usize = 255;
    /// Branches in a `agent.git.branches` answer.
    pub const MAX_BRANCHES: usize = 200;
    /// An `agent.git.branches` query.
    pub const MAX_QUERY_BYTES: usize = 100;
    /// Remotes in a status.
    pub const MAX_REMOTES: usize = 8;
    /// A remote's name (`origin`).
    pub const MAX_REMOTE_NAME_BYTES: usize = 64;
    /// A remote's label: host and path, never a URL.
    pub const MAX_REMOTE_LABEL_BYTES: usize = 140;
    /// The last commit's subject.
    pub const MAX_SUBJECT_BYTES: usize = 200;
    /// A worktree's label.
    pub const MAX_WORKTREE_LABEL_BYTES: usize = 64;
    /// `github.repo` (`owner/name`) and a pull request row's `repo`.
    pub const MAX_REPO_BYTES: usize = 140;
    /// A host's name (`github.com`).
    pub const MAX_HOST_BYTES: usize = 255;
    /// A status's `fingerprint`: lowercase hex digits.
    pub const FINGERPRINT_BYTES: usize = 16;
    /// A short commit id: 7 to 40 lowercase hex digits.
    pub const MIN_OID_BYTES: usize = 7;
    pub const MAX_OID_BYTES: usize = 40;
    /// A job's id, and a review's: lowercase hex digits.
    pub const JOB_ID_BYTES: usize = 16;
    pub const REVIEW_BYTES: usize = 16;
    /// A job's lines a phone gets: the newest, at most this many and
    /// [`MAX_JOB_TEXT_BYTES`] in all.
    pub const MAX_JOB_LINES: usize = 40;
    pub const MAX_JOB_TEXT_BYTES: usize = 8 * 1024;
    /// One line of a job's output, in characters.
    pub const MAX_LINE_CHARS: usize = 512;
    /// A job's or a refusal's `reason`: `[a-z0-9_]`.
    pub const MAX_REASON_BYTES: usize = 24;
    /// A job's error `message`.
    pub const MAX_ERROR_MESSAGE_BYTES: usize = 200;
    /// Remote names a `pick_remote` error offers.
    pub const MAX_PICK_NAMES: usize = 8;
    /// What the agent wrote when asked (`AgentThread.wrote`).
    pub const MAX_WROTE_BYTES: usize = 8192;
    /// Reviews the PC keeps for one phone, and how long each.
    pub const MAX_REVIEWS: usize = 4;
    pub const REVIEW_LIFE_MS: u64 = 10 * 60 * 1000;
    /// The per-device toggle on the PC that lets a phone push and open pull
    /// requests (off by default; counts only while `agents` is on).
    pub const PUSH_TOGGLE: &str = "agent_push";

    /// A status's `repo`.
    pub mod repo {
        pub const OK: &str = "ok";
        /// Not in a git repository.
        pub const NONE: &str = "none";
        /// Git says the folder isn't safe, or it lands outside the folders G7 works in.
        pub const REFUSED: &str = "refused";
        /// Git isn't installed on the PC.
        pub const NO_GIT: &str = "no_git";
        pub const ALL: &[&str] = &[OK, NONE, REFUSED, NO_GIT];
    }

    /// What git is in the middle of (`doing`).
    pub mod doing {
        pub const MERGE: &str = "merge";
        pub const REBASE: &str = "rebase";
        pub const CHERRY_PICK: &str = "cherry_pick";
        pub const REVERT: &str = "revert";
        pub const BISECT: &str = "bisect";
        pub const ALL: &[&str] = &[MERGE, REBASE, CHERRY_PICK, REVERT, BISECT];
    }

    /// A changed file's `status`.
    pub mod file_status {
        pub const ADDED: &str = "added";
        pub const MODIFIED: &str = "modified";
        pub const DELETED: &str = "deleted";
        pub const RENAMED: &str = "renamed";
        /// Its type changed (a file became a link).
        pub const TYPE: &str = "type";
        pub const UNTRACKED: &str = "untracked";
        pub const CONFLICT: &str = "conflict";
        pub const ALL: &[&str] = &[ADDED, MODIFIED, DELETED, RENAMED, TYPE, UNTRACKED, CONFLICT];
    }

    /// How much of a file's change the user staged already (`staged`).
    pub mod staged {
        pub const ALL_OF_IT: &str = "all";
        pub const PART: &str = "part";
        pub const NONE: &str = "none";
        pub const ALL: &[&str] = &[ALL_OF_IT, PART, NONE];
    }

    /// What a job does (`kind`).
    pub mod kind {
        pub const COMMIT: &str = "commit";
        pub const PUSH: &str = "push";
        pub const FETCH: &str = "fetch";
        pub const SWITCH: &str = "switch";
        pub const WORKTREE: &str = "worktree";
        pub const PR_CREATE: &str = "pr_create";
        pub const PR_MERGE: &str = "pr_merge";
        pub const ALL: &[&str] = &[COMMIT, PUSH, FETCH, SWITCH, WORKTREE, PR_CREATE, PR_MERGE];
    }

    /// Where a job stands (`phase`).
    pub mod phase {
        pub const RUNNING: &str = "running";
        pub const DONE: &str = "done";
        pub const FAILED: &str = "failed";
        pub const STOPPED: &str = "stopped";
        pub const ALL: &[&str] = &[RUNNING, DONE, FAILED, STOPPED];
    }

    /// Why a reviewed file can't be committed from a phone (`why`).
    pub mod unreadable {
        /// Its patch would pass [`super::MAX_PATCH_BYTES`].
        pub const LARGE: &str = "large";
        /// An untracked folder: a phone sees only the names inside it, never
        /// their bytes, so it's committed on the PC.
        pub const FOLDER: &str = "folder";
        pub const ALL: &[&str] = &[LARGE, FOLDER];
    }

    /// What "Ask <agent> to write it" is for (`purpose`), and "Ask <agent>
    /// to review it" (`review`, with the pull request's number).
    pub mod purpose {
        pub const COMMIT: &str = "commit";
        pub const PR: &str = "pr";
        /// "Ask <agent> to review it" (§13): needs `pr`; the thread's
        /// `wrote` never holds it.
        pub const REVIEW: &str = "review";
        /// What the agent writes when asked, and `AgentThread.wrote` holds.
        pub const ALL: &[&str] = &[COMMIT, PR];
        /// Every `agent.write`'s `purpose`.
        pub const ASK: &[&str] = &[COMMIT, PR, REVIEW];
    }

    /// The reasons a job fails or a request is refused with that a phone
    /// words itself. A receiver shows a word it doesn't know as `failed`.
    pub mod reason {
        pub const GIT_BUSY: &str = "git_busy";
        pub const DOING: &str = "doing";
        pub const DETACHED: &str = "detached";
        pub const CONFLICTS: &str = "conflicts";
        pub const NOTHING: &str = "nothing";
        pub const IDENTITY: &str = "identity";
        pub const THREAD_BUSY: &str = "thread_busy";
        pub const SHARED_BUSY: &str = "shared_busy";
        pub const CHANGED: &str = "changed";
        pub const HOOK: &str = "hook";
        pub const FAILED: &str = "failed";
        pub const STOPPED: &str = "stopped";
        pub const BEHIND: &str = "behind";
        pub const REMOTE_REFUSED: &str = "remote_refused";
        pub const SIGN_IN: &str = "sign_in";
        pub const NETWORK: &str = "network";
        pub const PICK_REMOTE: &str = "pick_remote";
        pub const NO_REMOTE: &str = "no_remote";
        pub const NO_COMMITS: &str = "no_commits";
        pub const DIRTY: &str = "dirty";
        pub const WOULD_OVERWRITE: &str = "would_overwrite";
        pub const IN_WORKTREE: &str = "in_worktree";
        pub const EXISTS: &str = "exists";
        pub const BAD_NAME: &str = "bad_name";
        pub const NOT_PUSHED: &str = "not_pushed";
        pub const NOT_GITHUB: &str = "not_github";
        pub const NO_GH: &str = "no_gh";
        pub const SIGNED_OUT: &str = "signed_out";
        pub const OLD_GH: &str = "old_gh";
        pub const PR_EXISTS: &str = "pr_exists";
    }
}

/// Limits and wire words of §8's pull-request messages.
pub mod pr {
    /// A pull request's title, and `agent.pr.create`'s.
    pub const MAX_TITLE_BYTES: usize = 256;
    /// `agent.pr.create`'s body (Markdown).
    pub const MAX_BODY_BYTES: usize = 32 * 1024;
    /// Checks of one pull request.
    pub const MAX_CHECKS: usize = 100;
    /// A check's name.
    pub const MAX_CHECK_NAME_BYTES: usize = 120;
    /// A pull request's or a check's `url`: `https` only.
    pub const MAX_URL_BYTES: usize = 512;
    /// Rows in an `agent.pr.list` answer.
    pub const MAX_ROWS: usize = 50;
    /// Hosts the GitHub CLI is signed in to.
    pub const MAX_HOSTS: usize = 8;
    /// The GitHub CLI's version.
    pub const MAX_VERSION_BYTES: usize = 32;

    pub mod state {
        pub const OPEN: &str = "open";
        pub const MERGED: &str = "merged";
        pub const CLOSED: &str = "closed";
        pub const ALL: &[&str] = &[OPEN, MERGED, CLOSED];
    }

    /// The review decision (`review`).
    pub mod review {
        pub const APPROVED: &str = "approved";
        pub const CHANGES_REQUESTED: &str = "changes_requested";
        pub const REVIEW_REQUIRED: &str = "review_required";
        pub const ALL: &[&str] = &[APPROVED, CHANGES_REQUESTED, REVIEW_REQUIRED];
    }

    pub mod mergeable {
        pub const YES: &str = "yes";
        pub const CONFLICTS: &str = "conflicts";
        pub const UNKNOWN: &str = "unknown";
        pub const ALL: &[&str] = &[YES, CONFLICTS, UNKNOWN];
    }

    /// A check's `state`. One this build doesn't know reads as `pending`,
    /// never `pass`.
    pub mod check {
        pub const PASS: &str = "pass";
        pub const FAIL: &str = "fail";
        pub const PENDING: &str = "pending";
        pub const SKIPPED: &str = "skipped";
        pub const CANCELLED: &str = "cancelled";
        pub const ALL: &[&str] = &[PASS, FAIL, PENDING, SKIPPED, CANCELLED];
    }

    /// The GitHub CLI on the PC (`gh`).
    pub mod gh {
        pub const OK: &str = "ok";
        pub const MISSING: &str = "missing";
        pub const SIGNED_OUT: &str = "signed_out";
        pub const OLD: &str = "old";
        pub const ALL: &[&str] = &[OK, MISSING, SIGNED_OUT, OLD];
    }

    /// Which list `agent.pr.list` gives (`show`).
    pub mod show {
        pub const MINE: &str = "mine";
        pub const REVIEW: &str = "review";
        /// The open ones of one folder's repository (`folder`).
        pub const FOLDER: &str = "folder";
        pub const ALL: &[&str] = &[MINE, REVIEW, FOLDER];
    }

    /// Why `agent.pr.view` or `agent.pr.detail` has no pull request to
    /// show, or an old one (`why`).
    pub mod why {
        pub const NOT_GITHUB: &str = "not_github";
        pub const NO_UPSTREAM: &str = "no_upstream";
        pub const DETACHED: &str = "detached";
        /// GitHub couldn't be reached: `pr` is the last one read, if any.
        pub const NETWORK: &str = "network";
        /// The folder isn't a git repository.
        pub const NOT_REPO: &str = "not_repo";
        /// The pull request is too large to show here (§13).
        pub const TOO_LARGE: &str = "too_large";
        /// The thread's branch has no pull request (§13).
        pub const NO_PR: &str = "no_pr";
        pub const ALL: &[&str] = &[NOT_GITHUB, NO_UPSTREAM, DETACHED, NETWORK, NOT_REPO, TOO_LARGE, NO_PR];
    }

    // ---- Pull request detail (§13) ----

    /// Changed files of a pull request a phone gets: the first.
    pub const MAX_FILES: usize = 300;
    /// Comments and reviews a phone gets: the newest.
    pub const MAX_COMMENTS: usize = 50;
    /// One comment's or review's body, in bytes.
    pub const MAX_COMMENT_BYTES: usize = 4096;
    /// An author's login, in bytes.
    pub const MAX_AUTHOR_BYTES: usize = 64;
    /// One repository part (`owner`, `name`, a host), in bytes.
    pub const MAX_REPO_PART_BYTES: usize = 100;

    /// How a pull request changed a file (`change`). One this build doesn't
    /// know reads as `changed`.
    pub mod change {
        pub const ADDED: &str = "added";
        pub const DELETED: &str = "deleted";
        pub const RENAMED: &str = "renamed";
        pub const COPIED: &str = "copied";
        pub const MODIFIED: &str = "modified";
        pub const CHANGED: &str = "changed";
        pub const ALL: &[&str] = &[ADDED, DELETED, RENAMED, COPIED, MODIFIED, CHANGED];
    }

    /// A review's verdict on a comment (`review`); absent: a comment, or a
    /// word this build doesn't know.
    pub mod verdict {
        pub const APPROVED: &str = "approved";
        pub const CHANGES_REQUESTED: &str = "changes_requested";
        pub const COMMENTED: &str = "commented";
        pub const DISMISSED: &str = "dismissed";
        pub const ALL: &[&str] = &[APPROVED, CHANGES_REQUESTED, COMMENTED, DISMISSED];
    }
}

// ---- Shared checks ----

/// Whether `text` has no control or bidi characters.
fn is_one_line(text: &str) -> bool {
    !text.chars().any(|c| c.is_control() || is_bidi_control(c))
}

/// Whether `name` can be a branch's name as git lists it (`main`,
/// `origin/feat`): 1 to 255 bytes, one line, no white space, no `:` or `\`
/// (git refuses both), never an option or an absolute path.
pub fn is_branch_name(name: &str) -> bool {
    (1..=git::MAX_BRANCH_BYTES).contains(&name.len())
        && !name.starts_with(['-', '/'])
        && !name
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || is_bidi_control(c) || c == ':' || c == '\\')
}

/// Whether `name` can be a remote's name (`origin`): 1 to 64 bytes, one
/// line, no white space, never an option, a path or a URL.
pub fn is_remote_name(name: &str) -> bool {
    (1..=git::MAX_REMOTE_NAME_BYTES).contains(&name.len())
        && !name.starts_with(['-', '/', '.', '~'])
        && !name.chars().any(|c| {
            c.is_control() || c.is_whitespace() || is_bidi_control(c) || matches!(c, ':' | '\\' | '@')
        })
}

/// Whether `text` could be a URL, an account or an absolute path: what a
/// label shown on a phone may never be.
fn looks_like_a_place(text: &str) -> bool {
    let bytes = text.as_bytes();
    text.contains("://")
        || text.contains('@')
        || text.starts_with(['/', '\\', '~'])
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

/// A label (a remote's, a worktree's), one line, cut to `max`; `None` when
/// it could be a URL, an account or an absolute path, or is empty.
fn safe_label(label: &str, max: usize) -> Option<String> {
    let clean = clean_agent_text(label, false, max).0;
    (!clean.trim().is_empty() && !looks_like_a_place(&clean)).then_some(clean)
}

fn is_oid(oid: &str) -> bool {
    (git::MIN_OID_BYTES..=git::MAX_OID_BYTES).contains(&oid.len())
        && oid.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Whether `host` is a host's name: 1 to 255 of `[A-Za-z0-9.-]`, and a port.
fn is_host(host: &str) -> bool {
    (1..=git::MAX_HOST_BYTES).contains(&host.len())
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
}

/// Whether `url` is one a phone may open: `https`, a host, at most 512
/// bytes, one line without spaces.
pub fn is_https_url(url: &str) -> bool {
    url.len() <= pr::MAX_URL_BYTES
        && url.strip_prefix("https://").is_some_and(|rest| {
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            is_host(host) && !host.contains('@')
        })
        && !url.chars().any(|c| c.is_control() || c.is_whitespace() || is_bidi_control(c))
}

/// Whether `word` is a reason: `[a-z0-9_]`, at most 24 bytes.
fn is_reason(word: &str) -> bool {
    word.len() <= git::MAX_REASON_BYTES && is_agent_word(word)
}

/// A multi-line text a person typed (a commit message, a pull request's
/// description): control and bidi characters other than newline and tab
/// taken out, `\r\n` as `\n`.
pub fn clean_typed_text(text: &str) -> String {
    clean_agent_text(&text.replace("\r\n", "\n"), true, usize::MAX).0
}

/// Paths, each relative to the folder and none twice.
fn are_distinct_paths(paths: &[String]) -> bool {
    paths.iter().all(|p| is_relative_path(p))
        && paths.iter().enumerate().all(|(i, p)| !paths[..i].contains(p))
}

/// Whether `id` is a job's id: 16 lowercase hex digits.
pub fn is_git_job_id(id: &str) -> bool {
    is_lower_hex(id, git::JOB_ID_BYTES)
}

/// Whether `token` is a review's token: 16 lowercase hex digits.
pub fn is_review_token(token: &str) -> bool {
    is_lower_hex(token, git::REVIEW_BYTES)
}

// ---- What a thread and the catalog gain ----

/// The worktree a thread works in: its label and branch, never its path.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitWorktree {
    pub label: String,
    pub branch: String,
}

impl AgentGitWorktree {
    pub fn is_valid(&self) -> bool {
        self.label.len() <= git::MAX_WORKTREE_LABEL_BYTES
            && !looks_like_a_place(&self.label)
            && is_one_line(&self.label)
            && is_branch_name(&self.branch)
    }

    /// Within its limits, or `None` when its label could be a path or a URL
    /// or its branch isn't a branch's name.
    pub fn sanitized(self) -> Option<Self> {
        let label = safe_label(&self.label, git::MAX_WORKTREE_LABEL_BYTES)?;
        is_branch_name(&self.branch).then_some(Self { label, branch: self.branch })
    }
}

/// What the agent wrote when asked for a commit message or a pull request
/// (`AgentThread.wrote`), until the next commit or pull request, the next
/// ask, or a day.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentWrote {
    /// One of [`git::purpose`].
    pub purpose: String,
    /// The turn that answered.
    pub turn: u32,
    /// Its answer, at most [`git::MAX_WROTE_BYTES`].
    pub text: String,
    /// Unix milliseconds.
    pub at_ms: u64,
}

/// Never prints the text.
impl std::fmt::Debug for AgentWrote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentWrote")
            .field("purpose", &self.purpose)
            .field("turn", &self.turn)
            .field("bytes", &self.text.len())
            .finish_non_exhaustive()
    }
}

impl AgentWrote {
    pub fn is_valid(&self) -> bool {
        git::purpose::ALL.contains(&self.purpose.as_str()) && self.text.len() <= git::MAX_WROTE_BYTES
    }

    /// Within its limits, or `None` for a purpose this build doesn't know.
    pub fn sanitized(mut self) -> Option<Self> {
        if !git::purpose::ALL.contains(&self.purpose.as_str()) {
            return None;
        }
        self.text = clean_agent_text(&self.text, true, git::MAX_WROTE_BYTES).0;
        Some(self)
    }
}

/// Whether the fields §8 added to `thread` are within their limits.
pub(super) fn is_valid_thread_git(thread: &super::AgentThread) -> bool {
    thread.branch.as_deref().is_none_or(is_branch_name)
        && thread.worktree.as_ref().is_none_or(AgentGitWorktree::is_valid)
        && thread.wrote.as_ref().is_none_or(AgentWrote::is_valid)
}

/// The fields §8 added to `thread`, within their limits: what doesn't fit
/// reads as absent.
pub(super) fn clean_thread_git(thread: &mut super::AgentThread) {
    if !thread.branch.as_deref().is_none_or(is_branch_name) {
        thread.branch = None;
    }
    thread.worktree = thread.worktree.take().and_then(AgentGitWorktree::sanitized);
    thread.wrote = thread.wrote.take().and_then(AgentWrote::sanitized);
}

// ---- Status ----

/// Body of `agent.git.status` and `agent.git.fetch`: the thread whose folder it is.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitThread {
    pub thread: String,
}

impl AgentGitThread {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
    }
}

/// A remote, by its name and a label of host and path (never a URL); no
/// label when it's a folder on the PC (`local`).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitRemote {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// A folder on the PC: "a folder on <PC>".
    #[serde(default, skip_serializing_if = "Not::not")]
    pub local: bool,
}

/// Never prints the label.
impl std::fmt::Debug for AgentGitRemote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGitRemote").field("name", &self.name).field("local", &self.local).finish()
    }
}

impl AgentGitRemote {
    fn is_valid(&self) -> bool {
        is_remote_name(&self.name)
            && self.label.as_deref().is_none_or(|l| {
                !self.local
                    && l.len() <= git::MAX_REMOTE_LABEL_BYTES
                    && is_one_line(l)
                    && !looks_like_a_place(l)
            })
    }

    /// Within its limits, or `None` when its name isn't one. A label that
    /// could be a URL or a path, or a local remote's, is left out.
    fn sanitized(self) -> Option<Self> {
        if !is_remote_name(&self.name) {
            return None;
        }
        let label = if self.local {
            None
        } else {
            self.label.as_deref().and_then(|l| safe_label(l, git::MAX_REMOTE_LABEL_BYTES))
        };
        Some(Self { name: self.name, label, local: self.local })
    }
}

/// A changed file of a status, relative to the thread's folder; an untracked
/// folder's path ends in `/`.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitFile {
    pub path: String,
    /// Its earlier path (`renamed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// One of [`git::file_status`]; absent when the word isn't known here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// One of [`git::staged`]; absent when the word isn't known here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<String>,
    /// An untracked folder: one row, committed whole.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub folder: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dels: Option<u32>,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub binary: bool,
}

/// Never prints a path.
impl std::fmt::Debug for AgentGitFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGitFile")
            .field("status", &self.status)
            .field("staged", &self.staged)
            .field("folder", &self.folder)
            .field("adds", &self.adds)
            .field("dels", &self.dels)
            .finish_non_exhaustive()
    }
}

impl AgentGitFile {
    pub fn is_valid(&self) -> bool {
        is_relative_path(&self.path)
            && self.from.as_deref().is_none_or(is_relative_path)
            && fits_word(&self.status)
            && fits_word(&self.staged)
    }

    /// Within its limits, or `None` when its path isn't one that stays in
    /// the folder (never cut: a cut path would name another file).
    pub fn sanitized(mut self) -> Option<Self> {
        if !is_relative_path(&self.path) {
            return None;
        }
        if !self.from.as_deref().is_none_or(is_relative_path) {
            self.from = None;
        }
        known_word(&mut self.status, git::file_status::ALL);
        known_word(&mut self.staged, git::staged::ALL);
        Some(self)
    }
}

/// The last commit (`last`).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitLast {
    pub oid7: String,
    pub subject: String,
    /// Unix milliseconds.
    pub ms: u64,
}

/// Never prints the subject.
impl std::fmt::Debug for AgentGitLast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGitLast").field("oid7", &self.oid7).field("ms", &self.ms).finish_non_exhaustive()
    }
}

/// A folder's repository on GitHub (`github`): `owner/name` and the host.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitHub {
    pub repo: String,
    pub host: String,
}

impl AgentGitHub {
    fn is_valid(&self) -> bool {
        self.repo.len() <= git::MAX_REPO_BYTES
            && !self.repo.is_empty()
            && is_one_line(&self.repo)
            && !looks_like_a_place(&self.repo)
            && is_host(&self.host)
    }
}

/// Body of the `agent.git.status` answer (paged): the thread's folder's
/// status. `files` is split across the parts; the rest repeats.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGit {
    /// One of [`git::repo`].
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Not on a branch: the commit's short id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached: Option<String>,
    /// No commit yet.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub unborn: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// Commits to push, and on the remote, as of the last fetch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<u32>,
    /// The remote a push goes to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remotes: Vec<AgentGitRemote>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    /// On the remote's default branch: a push says so first.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub on_default: bool,
    /// One of [`git::doing`]; absent when the word isn't known here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doing: Option<String>,
    /// Git is busy in the folder (`index.lock`).
    #[serde(default, skip_serializing_if = "Not::not")]
    pub locked: bool,
    #[serde(default)]
    pub files: Vec<AgentGitFile>,
    /// More files changed than are listed: "%1 more files aren't listed."
    #[serde(default, skip_serializing_if = "Not::not")]
    pub more: bool,
    /// Files changed in all, and their lines.
    #[serde(default)]
    pub total: u32,
    #[serde(default)]
    pub adds: u32,
    #[serde(default)]
    pub dels: u32,
    #[serde(default)]
    pub conflicts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<AgentGitLast>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<AgentGitWorktree>,
    /// When the remote was last fetched, Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_ms: Option<u64>,
    /// When this was read, Unix milliseconds.
    #[serde(default)]
    pub checked_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<AgentGitHub>,
    /// Changes when anything listed here changes: 16 hex digits; empty when
    /// the folder isn't a repository.
    #[serde(default)]
    pub fingerprint: String,
}

/// Never prints a path, a branch or a subject.
impl std::fmt::Debug for AgentGit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGit")
            .field("repo", &self.repo)
            .field("files", &self.files.len())
            .field("more", &self.more)
            .field("total", &self.total)
            .field("ahead", &self.ahead)
            .field("behind", &self.behind)
            .field("doing", &self.doing)
            .field("locked", &self.locked)
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl AgentGit {
    pub fn is_valid(&self) -> bool {
        git::repo::ALL.contains(&self.repo.as_str())
            && self.branch.as_deref().is_none_or(is_branch_name)
            && self.detached.as_deref().is_none_or(is_oid)
            && self.upstream.as_deref().is_none_or(is_branch_name)
            && self.remote.as_deref().is_none_or(is_remote_name)
            && self.remotes.len() <= git::MAX_REMOTES
            && self.remotes.iter().all(AgentGitRemote::is_valid)
            && self.default_branch.as_deref().is_none_or(is_branch_name)
            && fits_word(&self.doing)
            && self.files.len() <= git::MAX_FILES
            && self.files.iter().all(AgentGitFile::is_valid)
            && self.last.as_ref().is_none_or(|l| is_oid(&l.oid7) && l.subject.len() <= git::MAX_SUBJECT_BYTES)
            && self.worktree.as_ref().is_none_or(AgentGitWorktree::is_valid)
            && self.github.as_ref().is_none_or(AgentGitHub::is_valid)
            && (self.fingerprint.is_empty() || is_lower_hex(&self.fingerprint, git::FINGERPRINT_BYTES))
    }

    /// The status a phone may have: names that aren't names, labels that
    /// could be a URL or a path, files out of the folder and words this
    /// build doesn't know left out, at most [`git::MAX_FILES`] files
    /// (`more` set past that); `None` for a `repo` this build doesn't know.
    pub fn sanitized(mut self) -> Option<Self> {
        if !git::repo::ALL.contains(&self.repo.as_str()) {
            return None;
        }
        let name = |n: Option<String>| n.filter(|n| is_branch_name(n));
        self.branch = name(self.branch);
        self.upstream = name(self.upstream);
        self.default_branch = name(self.default_branch);
        self.detached = self.detached.filter(|oid| is_oid(oid));
        self.remote = self.remote.filter(|r| is_remote_name(r));
        self.remotes =
            std::mem::take(&mut self.remotes).into_iter().filter_map(AgentGitRemote::sanitized).collect();
        self.remotes.truncate(git::MAX_REMOTES);
        known_word(&mut self.doing, git::doing::ALL);
        let listed = self.files.len();
        self.files =
            std::mem::take(&mut self.files).into_iter().filter_map(AgentGitFile::sanitized).collect();
        if self.files.len() > git::MAX_FILES {
            self.files.truncate(git::MAX_FILES);
            self.more = true;
        }
        self.total = self.total.max(u32::try_from(listed).unwrap_or(u32::MAX));
        self.last = self.last.filter(|l| is_oid(&l.oid7)).map(|mut l| {
            clean_line(&mut l.subject, git::MAX_SUBJECT_BYTES);
            l
        });
        self.worktree = self.worktree.and_then(AgentGitWorktree::sanitized);
        self.github = self.github.filter(AgentGitHub::is_valid);
        if !is_lower_hex(&self.fingerprint, git::FINGERPRINT_BYTES) {
            self.fingerprint = String::new();
        }
        Some(self)
    }

    /// Files with changes, as a thread's strip counts them.
    pub fn changed(&self) -> u32 {
        self.total.max(u32::try_from(self.files.len()).unwrap_or(u32::MAX))
    }
}

impl crate::lanes::Paged for AgentGit {
    type Item = AgentGitFile;

    fn take_items(&mut self) -> Vec<AgentGitFile> {
        std::mem::take(&mut self.files)
    }

    fn set_items(&mut self, items: Vec<AgentGitFile>) {
        self.files = items;
    }

    fn continued(&self, items: Vec<AgentGitFile>) -> Self {
        Self { files: items, ..self.clone_without_files() }
    }

    fn append(&mut self, more: Self) {
        self.files.extend(more.files);
        self.more |= more.more;
    }
}

impl AgentGit {
    fn clone_without_files(&self) -> Self {
        Self {
            repo: self.repo.clone(),
            branch: self.branch.clone(),
            detached: self.detached.clone(),
            unborn: self.unborn,
            upstream: self.upstream.clone(),
            ahead: self.ahead,
            behind: self.behind,
            remote: self.remote.clone(),
            remotes: self.remotes.clone(),
            default_branch: self.default_branch.clone(),
            on_default: self.on_default,
            doing: self.doing.clone(),
            locked: self.locked,
            files: Vec::new(),
            more: self.more,
            total: self.total,
            adds: self.adds,
            dels: self.dels,
            conflicts: self.conflicts,
            last: self.last.clone(),
            worktree: self.worktree.clone(),
            fetched_ms: self.fetched_ms,
            checked_ms: self.checked_ms,
            github: self.github.clone(),
            fingerprint: self.fingerprint.clone(),
        }
    }
}

// ---- The uncommitted diff ----

/// Body of `agent.git.patch`: one listed file's uncommitted change. With
/// `review`, the PC counts it read for that review once all of it went.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitPatchRequest {
    pub thread: String,
    /// As the status listed it; the PC looks it up, never passes it on trust.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
}

/// Never prints the path.
impl std::fmt::Debug for AgentGitPatchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGitPatchRequest")
            .field("thread", &self.thread)
            .field("review", &self.review.is_some())
            .finish_non_exhaustive()
    }
}

impl AgentGitPatchRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
            && is_relative_path(&self.path)
            && self.review.as_deref().is_none_or(is_review_token)
    }
}

/// Body of the `agent.git.patch` answer (paged): the file's whole patch, at
/// most [`git::MAX_PATCH_BYTES`], its parts each carrying a piece of
/// `patch` in order; an untracked folder has no patch but the first names
/// inside it (in the first part).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitPatch {
    #[serde(default)]
    pub patch: String,
    /// The patch was cut: this file can't be committed from a phone.
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub binary: bool,
    /// An untracked folder: the first names inside, relative to the thread's folder.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    /// An untracked folder: names past `names`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more_names: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Never prints the patch or a name.
impl std::fmt::Debug for AgentGitPatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGitPatch")
            .field("bytes", &self.patch.len())
            .field("truncated", &self.truncated)
            .field("binary", &self.binary)
            .field("names", &self.names.len())
            .field("more_names", &self.more_names)
            .finish()
    }
}

impl AgentGitPatch {
    pub fn is_valid(&self) -> bool {
        self.patch.len() <= git::MAX_PATCH_BYTES
            && !self
                .patch
                .chars()
                .any(|c| is_bidi_control(c) || (c.is_control() && !matches!(c, '\n' | '\t')))
            && self.names.len() <= git::MAX_FOLDER_NAMES
            && self.names.iter().all(|n| is_relative_path(n))
    }

    /// Without control characters other than newline and tab and without
    /// bidi controls, cut to [`git::MAX_PATCH_BYTES`] (then `truncated`); a
    /// name out of the folder left out, at most [`git::MAX_FOLDER_NAMES`].
    pub fn sanitized(mut self) -> Self {
        if !self.is_valid() {
            let (patch, cut) = clean_agent_text(&self.patch, true, git::MAX_PATCH_BYTES);
            self.patch = patch;
            self.truncated |= cut;
        }
        let listed = self.names.len();
        self.names.retain(|n| is_relative_path(n));
        self.names.truncate(git::MAX_FOLDER_NAMES);
        let gone = u32::try_from(listed - self.names.len()).unwrap_or(u32::MAX);
        self.more_names = self.more_names.saturating_add(gone);
        self
    }

    /// `patch` in pieces of at most [`super::history::PATCH_PIECE_BYTES`],
    /// each ending at a line end when one is in its second half.
    fn pieces(patch: &str) -> Vec<String> {
        let size = super::history::PATCH_PIECE_BYTES;
        let mut pieces = Vec::with_capacity(patch.len() / size + 1);
        let mut rest = patch;
        while !rest.is_empty() {
            let mut end = cut_text(rest, size).len();
            if end < rest.len()
                && let Some(line) = rest[..end].rfind('\n').filter(|&at| at + 1 > end / 2)
            {
                end = line + 1;
            }
            if end == 0 {
                end = rest.chars().next().map_or(rest.len(), char::len_utf8);
            }
            pieces.push(rest[..end].to_owned());
            rest = &rest[end..];
        }
        pieces
    }
}

impl crate::lanes::Paged for AgentGitPatch {
    type Item = String;

    fn take_items(&mut self) -> Vec<String> {
        let patch = std::mem::take(&mut self.patch);
        Self::pieces(&patch)
    }

    fn set_items(&mut self, items: Vec<String>) {
        self.patch = items.concat();
    }

    /// The flags repeat; the names go in the first part only.
    fn continued(&self, items: Vec<String>) -> Self {
        Self {
            patch: items.concat(),
            truncated: self.truncated,
            binary: self.binary,
            names: Vec::new(),
            more_names: 0,
        }
    }

    fn append(&mut self, more: Self) {
        self.patch.push_str(&more.patch);
        self.truncated |= more.truncated;
        self.binary |= more.binary;
        self.names.extend(more.names);
        self.more_names = self.more_names.max(more.more_names);
    }
}

// ---- Review, then commit ----

/// Body of `agent.git.review`: pins the files a phone is about to read and
/// commit, as they are now.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitReviewRequest {
    pub thread: String,
    /// 1 to [`git::MAX_REVIEW_PATHS`] paths the status listed, none twice.
    pub paths: Vec<String>,
}

/// Never prints a path.
impl std::fmt::Debug for AgentGitReviewRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGitReviewRequest")
            .field("thread", &self.thread)
            .field("paths", &self.paths.len())
            .finish()
    }
}

impl AgentGitReviewRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
            && (1..=git::MAX_REVIEW_PATHS).contains(&self.paths.len())
            && are_distinct_paths(&self.paths)
    }
}

/// One file of a review: whether a phone can read all of it (`why` says
/// why not, one of [`git::unreadable`]).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitReviewFile {
    pub path: String,
    pub readable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// Never prints the path.
impl std::fmt::Debug for AgentGitReviewFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGitReviewFile")
            .field("readable", &self.readable)
            .field("why", &self.why)
            .finish_non_exhaustive()
    }
}

/// Body of the `agent.git.review` answer (paged): the review's token, which
/// `agent.git.patch` and `agent.git.commit` name, and its files.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitReview {
    pub review: String,
    #[serde(default)]
    pub files: Vec<AgentGitReviewFile>,
}

impl AgentGitReview {
    pub fn is_valid(&self) -> bool {
        is_review_token(&self.review)
            && self.files.len() <= git::MAX_REVIEW_PATHS
            && self.files.iter().all(|f| is_relative_path(&f.path) && fits_word(&f.why))
    }

    /// Files out of the folder left out; a `why` this build doesn't know
    /// still leaves the file unreadable.
    pub fn sanitized(mut self) -> Option<Self> {
        if !is_review_token(&self.review) {
            return None;
        }
        self.files.retain(|f| is_relative_path(&f.path));
        self.files.truncate(git::MAX_REVIEW_PATHS);
        for file in &mut self.files {
            if file.readable {
                file.why = None;
            } else {
                known_word(&mut file.why, git::unreadable::ALL);
            }
        }
        Some(self)
    }
}

/// Body of `agent.git.commit`: commits the files of a review this phone
/// read whole, with `message`.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitCommitRequest {
    pub thread: String,
    pub review: String,
    /// 1 to [`git::MAX_MESSAGE_BYTES`] bytes: the subject, a blank line, the body.
    pub message: String,
}

/// Never prints the message.
impl std::fmt::Debug for AgentGitCommitRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGitCommitRequest")
            .field("thread", &self.thread)
            .field("review", &self.review)
            .field("bytes", &self.message.len())
            .finish()
    }
}

impl AgentGitCommitRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread) && is_review_token(&self.review) && is_commit_message(&self.message)
    }
}

/// Whether `message` can be a commit's: 1 to [`git::MAX_MESSAGE_BYTES`]
/// bytes, not blank, no NUL.
pub fn is_commit_message(message: &str) -> bool {
    message.len() <= git::MAX_MESSAGE_BYTES && !message.trim().is_empty() && !message.contains('\0')
}

// ---- Push, fetch, branches, switch ----

/// Body of `agent.git.push`: the current branch to its remote, or to
/// `remote` after a `pick_remote`. Needs `agent_push`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitPushRequest {
    pub thread: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

impl AgentGitPushRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread) && self.remote.as_deref().is_none_or(is_remote_name)
    }
}

/// Body of `agent.git.branches`.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitBranchesRequest {
    pub thread: String,
    /// A part of the name, at most [`git::MAX_QUERY_BYTES`], one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

/// Never prints the query.
impl std::fmt::Debug for AgentGitBranchesRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGitBranchesRequest").field("thread", &self.thread).finish_non_exhaustive()
    }
}

impl AgentGitBranchesRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
            && self.query.as_deref().is_none_or(|q| q.len() <= git::MAX_QUERY_BYTES && is_one_line(q))
    }
}

/// A branch: local, or only on a remote (`remote`, named `origin/feat`).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentBranch {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub current: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<u32>,
    /// Checked out in another worktree: its label ("Open in another worktree (%1)").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    /// Its last commit, Unix milliseconds.
    #[serde(default)]
    pub last_ms: u64,
}

/// Never prints the name.
impl std::fmt::Debug for AgentBranch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentBranch")
            .field("current", &self.current)
            .field("remote", &self.remote.is_some())
            .finish_non_exhaustive()
    }
}

impl AgentBranch {
    fn is_valid(&self) -> bool {
        is_branch_name(&self.name)
            && self.remote.as_deref().is_none_or(is_remote_name)
            && self.upstream.as_deref().is_none_or(is_branch_name)
            && self.worktree.as_deref().is_none_or(|w| {
                w.len() <= git::MAX_WORKTREE_LABEL_BYTES && is_one_line(w) && !looks_like_a_place(w)
            })
    }

    fn sanitized(mut self) -> Option<Self> {
        if !is_branch_name(&self.name) {
            return None;
        }
        self.remote = self.remote.filter(|r| is_remote_name(r));
        self.upstream = self.upstream.filter(|u| is_branch_name(u));
        self.worktree = self.worktree.and_then(|w| safe_label(&w, git::MAX_WORKTREE_LABEL_BYTES));
        Some(self)
    }
}

/// Body of the `agent.git.branches` answer (paged): local branches first,
/// then those only on a remote, each newest first.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentBranches {
    #[serde(default)]
    pub branches: Vec<AgentBranch>,
    #[serde(default)]
    pub more: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
}

impl AgentBranches {
    pub fn is_valid(&self) -> bool {
        self.branches.len() <= git::MAX_BRANCHES
            && self.branches.iter().all(AgentBranch::is_valid)
            && self.current.as_deref().is_none_or(is_branch_name)
    }

    pub fn sanitized(mut self) -> Self {
        self.branches =
            std::mem::take(&mut self.branches).into_iter().filter_map(AgentBranch::sanitized).collect();
        if self.branches.len() > git::MAX_BRANCHES {
            self.branches.truncate(git::MAX_BRANCHES);
            self.more = true;
        }
        self.current = self.current.filter(|c| is_branch_name(c));
        self
    }
}

/// Body of `agent.git.switch`: to a branch `agent.git.branches` listed, one
/// only on a remote (`track`), or a new one (`create`). A phone switches
/// only while nothing is changed: `carry` (take the changes along) is
/// refused from a phone.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitSwitchRequest {
    pub thread: String,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub create: bool,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub track: bool,
    /// Never from a phone: the PC refuses it (`DENIED`).
    #[serde(default, skip_serializing_if = "Not::not")]
    pub carry: bool,
}

/// Never prints the branch.
impl std::fmt::Debug for AgentGitSwitchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGitSwitchRequest")
            .field("thread", &self.thread)
            .field("create", &self.create)
            .field("track", &self.track)
            .field("carry", &self.carry)
            .finish_non_exhaustive()
    }
}

impl AgentGitSwitchRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread) && is_branch_name(&self.branch) && !(self.create && self.track)
    }
}

// ---- Jobs ----

/// Body of `agent.git.job`: a job as it stands, or (`stop`) stops it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitJobRequest {
    pub job: String,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub stop: bool,
}

impl AgentGitJobRequest {
    pub fn is_valid(&self) -> bool {
        is_git_job_id(&self.job)
    }
}

/// What a job that went through made.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentJobResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oid7: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Files committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<u32>,
    /// The branch pushed, or switched to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// A push the remote had already: "Already up to date".
    #[serde(default, skip_serializing_if = "Not::not")]
    pub up_to_date: bool,
    /// A switch made the branch: "Created %1".
    #[serde(default, skip_serializing_if = "Not::not")]
    pub created: bool,
    /// The pull request opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<AgentPrInfo>,
    /// The pull request merged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
}

/// Never prints the subject.
impl std::fmt::Debug for AgentJobResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentJobResult")
            .field("oid7", &self.oid7)
            .field("files", &self.files)
            .field("up_to_date", &self.up_to_date)
            .field("created", &self.created)
            .field("pr", &self.pr.as_ref().map(|p| p.number))
            .finish_non_exhaustive()
    }
}

impl AgentJobResult {
    fn sanitized(mut self) -> Self {
        self.oid7 = self.oid7.filter(|o| is_oid(o));
        if let Some(subject) = &mut self.subject {
            clean_line(subject, git::MAX_SUBJECT_BYTES);
        }
        self.branch = self.branch.filter(|b| is_branch_name(b));
        self.remote = self.remote.filter(|r| is_remote_name(r));
        self.pr = self.pr.and_then(AgentPrInfo::sanitized);
        self
    }
}

/// Why a job didn't go through, or why the PC refused to start it.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentJobError {
    /// A word of [`git::reason`]; one this build doesn't know reads as `failed`.
    pub reason: String,
    /// The PC's own words, at most [`git::MAX_ERROR_MESSAGE_BYTES`]; the
    /// app words the reasons it knows itself.
    #[serde(default)]
    pub message: String,
    /// `pick_remote`: the remotes to choose from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
}

/// Never prints the message.
impl std::fmt::Debug for AgentJobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentJobError").field("reason", &self.reason).field("names", &self.names).finish()
    }
}

impl AgentJobError {
    pub fn sanitized(mut self) -> Self {
        if !is_reason(&self.reason) {
            self.reason = git::reason::FAILED.to_owned();
        }
        clean_line(&mut self.message, git::MAX_ERROR_MESSAGE_BYTES);
        self.names.retain(|n| is_remote_name(n));
        self.names.truncate(git::MAX_PICK_NAMES);
        self
    }
}

/// A source-control job on the PC: a commit, push, fetch, switch or pull
/// request opened. An answer carries its newest lines; `agent.git.progress`
/// only those new since the last.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentJob {
    /// 16 lowercase hex digits.
    pub id: String,
    pub thread: String,
    /// One of [`git::kind`].
    pub kind: String,
    /// One of [`git::phase`].
    pub phase: String,
    /// Unix milliseconds.
    pub started_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<u64>,
    /// The phone this is sent to asked for it (it may stop it).
    #[serde(default, skip_serializing_if = "Not::not")]
    pub mine: bool,
    /// Git's and the hooks' output, oldest first, cleaned: at most
    /// [`git::MAX_JOB_LINES`] and [`git::MAX_JOB_TEXT_BYTES`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    /// The number of `lines[0]`, counting every line the job printed from 0.
    #[serde(default)]
    pub from: u64,
    /// Lines the PC didn't keep: "%n earlier lines not kept".
    #[serde(default)]
    pub dropped: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<AgentJobResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentJobError>,
}

/// Never prints a line.
impl std::fmt::Debug for AgentJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentJob")
            .field("id", &self.id)
            .field("thread", &self.thread)
            .field("kind", &self.kind)
            .field("phase", &self.phase)
            .field("mine", &self.mine)
            .field("lines", &self.lines.len())
            .field("from", &self.from)
            .field("dropped", &self.dropped)
            .field("result", &self.result)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl AgentJob {
    pub fn is_valid(&self) -> bool {
        is_git_job_id(&self.id)
            && is_valid_agent_id(&self.thread)
            && git::kind::ALL.contains(&self.kind.as_str())
            && git::phase::ALL.contains(&self.phase.as_str())
            && self.lines.len() <= git::MAX_JOB_LINES
            && self.lines.iter().map(String::len).sum::<usize>() <= git::MAX_JOB_TEXT_BYTES
            && self.lines.iter().all(|l| l.chars().count() <= git::MAX_LINE_CHARS && is_one_line_or_tab(l))
            && self.error.as_ref().is_none_or(|e| is_reason(&e.reason))
    }

    /// The job as a phone may have it: each line one line of at most
    /// [`git::MAX_LINE_CHARS`] characters, the newest [`git::MAX_JOB_LINES`]
    /// within [`git::MAX_JOB_TEXT_BYTES`] (those left out counted in
    /// `dropped` and `from`); `None` for a bad id or a kind or phase this
    /// build doesn't know.
    pub fn sanitized(mut self) -> Option<Self> {
        if !is_git_job_id(&self.id)
            || !is_valid_agent_id(&self.thread)
            || !git::kind::ALL.contains(&self.kind.as_str())
            || !git::phase::ALL.contains(&self.phase.as_str())
        {
            return None;
        }
        let mut lines: Vec<String> =
            std::mem::take(&mut self.lines).into_iter().map(|l| job_line(&l)).collect();
        let mut keep = 0;
        let mut bytes = 0usize;
        for line in lines.iter().rev() {
            if keep == git::MAX_JOB_LINES || bytes + line.len() > git::MAX_JOB_TEXT_BYTES {
                break;
            }
            keep += 1;
            bytes += line.len();
        }
        let cut = lines.len() - keep;
        lines.drain(..cut);
        self.lines = lines;
        self.from = self.from.saturating_add(cut as u64);
        self.dropped = self.dropped.saturating_add(u32::try_from(cut).unwrap_or(u32::MAX));
        self.result = self.result.map(AgentJobResult::sanitized);
        self.error = self.error.map(AgentJobError::sanitized);
        Some(self)
    }

    /// Whether it has ended.
    pub fn ended(&self) -> bool {
        self.phase != git::phase::RUNNING
    }
}

fn is_one_line_or_tab(line: &str) -> bool {
    !line.chars().any(|c| (c.is_control() && c != '\t') || is_bidi_control(c))
}

/// One line of a job's output: control characters (but tab) and bidi
/// controls taken out, at most [`git::MAX_LINE_CHARS`] characters.
fn job_line(line: &str) -> String {
    line.chars()
        .filter(|&c| !is_bidi_control(c) && (!c.is_control() || c == '\t'))
        .take(git::MAX_LINE_CHARS)
        .collect()
}

// ---- Pull requests ----

/// One check of a pull request.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentCheck {
    pub name: String,
    /// One of [`pr::check`]; one this build doesn't know reads as `pending`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Never prints the name or the link.
impl std::fmt::Debug for AgentCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentCheck").field("state", &self.state).finish_non_exhaustive()
    }
}

impl AgentCheck {
    fn sanitized(mut self) -> Self {
        clean_line(&mut self.name, pr::MAX_CHECK_NAME_BYTES);
        if !pr::check::ALL.contains(&self.state.as_str()) {
            self.state = pr::check::PENDING.to_owned();
        }
        self.url = self.url.filter(|u| is_https_url(u));
        self
    }
}

/// The branch's pull request.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPrInfo {
    pub number: u32,
    pub title: String,
    /// One of [`pr::state`].
    pub state: String,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub draft: bool,
    /// `https` on a host the GitHub CLI is signed in to; empty otherwise.
    #[serde(default)]
    pub url: String,
    /// The branch it goes into ("into %1"), and its own.
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub head: String,
    /// One of [`pr::review`]; absent: none, or a word not known here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    /// One of [`pr::mergeable`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mergeable: Option<String>,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub blocked: bool,
    /// Unix milliseconds.
    #[serde(default)]
    pub updated_ms: u64,
    /// Failing first, at most [`pr::MAX_CHECKS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<AgentCheck>,
    /// Checks passed, failed and still running; skipped and cancelled ones
    /// are in none of them.
    #[serde(default)]
    pub pass: u32,
    #[serde(default)]
    pub fail: u32,
    #[serde(default)]
    pub pending: u32,
    /// When the PC read it from GitHub, Unix milliseconds ("Read %1").
    #[serde(default)]
    pub read_ms: u64,
}

/// Never prints the title or a link.
impl std::fmt::Debug for AgentPrInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPrInfo")
            .field("number", &self.number)
            .field("state", &self.state)
            .field("draft", &self.draft)
            .field("checks", &self.checks.len())
            .field("pass", &self.pass)
            .field("fail", &self.fail)
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

impl AgentPrInfo {
    pub fn is_valid(&self) -> bool {
        self.number >= 1
            && self.title.len() <= pr::MAX_TITLE_BYTES
            && pr::state::ALL.contains(&self.state.as_str())
            && (self.url.is_empty() || is_https_url(&self.url))
            && self.base.len() <= git::MAX_BRANCH_BYTES
            && self.head.len() <= git::MAX_BRANCH_BYTES
            && fits_word(&self.review)
            && fits_word(&self.mergeable)
            && self.checks.len() <= pr::MAX_CHECKS
    }

    /// Within its limits, or `None` for number 0 or a state this build
    /// doesn't know.
    pub fn sanitized(mut self) -> Option<Self> {
        if self.number == 0 || !pr::state::ALL.contains(&self.state.as_str()) {
            return None;
        }
        clean_line(&mut self.title, pr::MAX_TITLE_BYTES);
        if !is_https_url(&self.url) {
            self.url = String::new();
        }
        clean_line(&mut self.base, git::MAX_BRANCH_BYTES);
        clean_line(&mut self.head, git::MAX_BRANCH_BYTES);
        known_word(&mut self.review, pr::review::ALL);
        known_word(&mut self.mergeable, pr::mergeable::ALL);
        self.checks = std::mem::take(&mut self.checks).into_iter().map(AgentCheck::sanitized).collect();
        self.checks.truncate(pr::MAX_CHECKS);
        Some(self)
    }
}

/// A pull request in brief, as a thread's strip shows it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPrBrief {
    pub number: u32,
    /// One of [`pr::state`].
    pub state: String,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub draft: bool,
    #[serde(default)]
    pub pass: u32,
    #[serde(default)]
    pub fail: u32,
    #[serde(default)]
    pub pending: u32,
}

impl AgentPrBrief {
    fn is_valid(&self) -> bool {
        self.number >= 1 && pr::state::ALL.contains(&self.state.as_str())
    }
}

/// The GitHub CLI on the PC.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGhState {
    /// One of [`pr::gh`].
    pub gh: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The hosts it's signed in to; a link opens only on one of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    /// Unix milliseconds.
    #[serde(default)]
    pub checked_ms: u64,
}

impl AgentGhState {
    pub fn is_valid(&self) -> bool {
        pr::gh::ALL.contains(&self.gh.as_str())
            && fits_opt_line(&self.version, pr::MAX_VERSION_BYTES)
            && self.hosts.len() <= pr::MAX_HOSTS
            && self.hosts.iter().all(|h| is_host(h))
    }

    /// Within its limits, or `None` for a word this build doesn't know.
    pub fn sanitized(mut self) -> Option<Self> {
        if !pr::gh::ALL.contains(&self.gh.as_str()) {
            return None;
        }
        if let Some(version) = &mut self.version {
            clean_line(version, pr::MAX_VERSION_BYTES);
        }
        self.hosts.retain(|h| is_host(h));
        self.hosts.truncate(pr::MAX_HOSTS);
        Some(self)
    }
}

fn fits_opt_line(text: &Option<String>, max: usize) -> bool {
    text.as_deref().is_none_or(|t| t.len() <= max && is_one_line(t))
}

/// Body of `agent.pr.view`: the thread's branch's pull request; `refresh`
/// reads it from GitHub now.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPrViewRequest {
    pub thread: String,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub refresh: bool,
}

impl AgentPrViewRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
    }
}

/// Body of the `agent.pr.view` answer.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPrView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<AgentPrInfo>,
    pub gh: AgentGhState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<AgentGitHub>,
    /// One of [`pr::why`]; absent when the word isn't known here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

impl AgentPrView {
    pub fn is_valid(&self) -> bool {
        self.pr.as_ref().is_none_or(AgentPrInfo::is_valid)
            && self.gh.is_valid()
            && self.github.as_ref().is_none_or(AgentGitHub::is_valid)
            && fits_word(&self.why)
    }

    pub fn sanitized(mut self) -> Option<Self> {
        self.gh = self.gh.sanitized()?;
        self.pr = self.pr.and_then(AgentPrInfo::sanitized);
        self.github = self.github.filter(AgentGitHub::is_valid);
        known_word(&mut self.why, pr::why::ALL);
        Some(self)
    }
}

/// Body of `agent.pr.create`: opens a pull request from the thread's
/// branch, already pushed. Needs `agent_push`.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPrCreateRequest {
    pub thread: String,
    /// 1 to [`pr::MAX_TITLE_BYTES`], one line.
    pub title: String,
    /// At most [`pr::MAX_BODY_BYTES`], Markdown.
    #[serde(default)]
    pub body: String,
    /// A remote branch `agent.git.branches` listed; absent: the repository's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub draft: bool,
}

/// Never prints the title or the body.
impl std::fmt::Debug for AgentPrCreateRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPrCreateRequest")
            .field("thread", &self.thread)
            .field("body", &self.body.len())
            .field("draft", &self.draft)
            .finish_non_exhaustive()
    }
}

impl AgentPrCreateRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
            && self.title.len() <= pr::MAX_TITLE_BYTES
            && !self.title.trim().is_empty()
            && is_one_line(&self.title)
            && self.body.len() <= pr::MAX_BODY_BYTES
            && !self.body.contains('\0')
            && self.base.as_deref().is_none_or(is_branch_name)
    }
}

/// Body of `agent.pr.list`: the open pull requests that are this account's
/// (`mine`), waiting for its review (`review`), or of one folder's
/// repository (`folder`, with its id).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPrListRequest {
    /// One of [`pr::show`].
    pub show: String,
    /// With `show: "folder"`: the folder's id from the catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub refresh: bool,
}

impl AgentPrListRequest {
    pub fn is_valid(&self) -> bool {
        pr::show::ALL.contains(&self.show.as_str())
            && self.folder.as_deref().is_none_or(is_valid_agent_id)
            && (self.show == pr::show::FOLDER) == self.folder.is_some()
    }
}

/// Check counts of a pull request row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentCheckCounts {
    #[serde(default)]
    pub pass: u32,
    #[serde(default)]
    pub fail: u32,
    #[serde(default)]
    pub pending: u32,
}

/// One row of the pull requests list.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPrRow {
    pub number: u32,
    pub title: String,
    /// `owner/name`.
    pub repo: String,
    /// One of [`pr::state`].
    pub state: String,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub draft: bool,
    /// Unix milliseconds.
    #[serde(default)]
    pub updated_ms: u64,
    /// `https` on a host the GitHub CLI is signed in to; empty otherwise.
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<AgentCheckCounts>,
    /// A thread on the PC whose folder is that repository and whose branch
    /// is the pull request's: "Open thread".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
}

/// Never prints the title or the link.
impl std::fmt::Debug for AgentPrRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPrRow")
            .field("number", &self.number)
            .field("state", &self.state)
            .field("draft", &self.draft)
            .field("checks", &self.checks)
            .field("thread", &self.thread)
            .finish_non_exhaustive()
    }
}

impl AgentPrRow {
    fn is_valid(&self) -> bool {
        self.number >= 1
            && self.title.len() <= pr::MAX_TITLE_BYTES
            && self.repo.len() <= git::MAX_REPO_BYTES
            && pr::state::ALL.contains(&self.state.as_str())
            && (self.url.is_empty() || is_https_url(&self.url))
            && self.branch.as_deref().is_none_or(is_branch_name)
            && self.thread.as_deref().is_none_or(is_valid_agent_id)
    }

    fn sanitized(mut self) -> Option<Self> {
        if self.number == 0 || !pr::state::ALL.contains(&self.state.as_str()) {
            return None;
        }
        clean_line(&mut self.title, pr::MAX_TITLE_BYTES);
        clean_line(&mut self.repo, git::MAX_REPO_BYTES);
        if !is_https_url(&self.url) {
            self.url = String::new();
        }
        self.branch = self.branch.filter(|b| is_branch_name(b));
        self.thread = self.thread.filter(|t| is_valid_agent_id(t));
        Some(self)
    }
}

/// Body of the `agent.pr.list` answer (paged): at most [`pr::MAX_ROWS`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPrRows {
    #[serde(default)]
    pub prs: Vec<AgentPrRow>,
    #[serde(default)]
    pub more: bool,
    pub gh: AgentGhState,
}

impl AgentPrRows {
    pub fn is_valid(&self) -> bool {
        self.prs.len() <= pr::MAX_ROWS && self.prs.iter().all(AgentPrRow::is_valid) && self.gh.is_valid()
    }

    pub fn sanitized(mut self) -> Option<Self> {
        self.gh = self.gh.sanitized()?;
        self.prs = std::mem::take(&mut self.prs).into_iter().filter_map(AgentPrRow::sanitized).collect();
        if self.prs.len() > pr::MAX_ROWS {
            self.prs.truncate(pr::MAX_ROWS);
            self.more = true;
        }
        Some(self)
    }
}

// ---- Pull request detail (§13) ----

/// Whether `repo` names a repository as `gh --repo` takes it: `owner/name`,
/// or `host/owner/name`; each part 1 to 100 of `[A-Za-z0-9._-]`, not
/// starting with `-` or `.`; at most [`git::MAX_REPO_BYTES`] in all.
pub fn is_pr_repo(repo: &str) -> bool {
    let parts: Vec<&str> = repo.split('/').collect();
    repo.len() <= git::MAX_REPO_BYTES
        && (2..=3).contains(&parts.len())
        && parts.iter().all(|part| {
            (1..=pr::MAX_REPO_PART_BYTES).contains(&part.len())
                && !part.starts_with(['-', '.'])
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}

/// Body of `agent.pr.detail`: the thread's branch's pull request (`thread`),
/// or one `agent.pr.list` listed (`repo` and `number`); `refresh` reads it
/// from GitHub now.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPrDetailRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub refresh: bool,
}

impl AgentPrDetailRequest {
    /// A thread, or a repository and a number (both, the number not 0), or
    /// all three.
    pub fn is_valid(&self) -> bool {
        self.thread.as_deref().is_none_or(is_valid_agent_id)
            && match (&self.repo, self.number) {
                (Some(repo), Some(number)) => is_pr_repo(repo) && number >= 1,
                (None, None) => self.thread.is_some(),
                _ => false,
            }
    }
}

/// A file a pull request changes.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPrFile {
    /// Relative to the repository.
    pub path: String,
    /// One of [`pr::change`]; one this build doesn't know reads as `changed`.
    pub change: String,
    #[serde(default)]
    pub adds: u32,
    #[serde(default)]
    pub dels: u32,
}

/// Never prints the path.
impl std::fmt::Debug for AgentPrFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPrFile")
            .field("change", &self.change)
            .field("adds", &self.adds)
            .field("dels", &self.dels)
            .finish_non_exhaustive()
    }
}

impl AgentPrFile {
    fn is_valid(&self) -> bool {
        is_relative_path(&self.path) && is_agent_word(&self.change)
    }

    /// Within its limits, or `None` when its path isn't one that stays in
    /// the repository (never cut: a cut path would name another file).
    fn sanitized(mut self) -> Option<Self> {
        if !is_relative_path(&self.path) {
            return None;
        }
        if !pr::change::ALL.contains(&self.change.as_str()) {
            self.change = pr::change::CHANGED.to_owned();
        }
        Some(self)
    }
}

/// A comment on a pull request's conversation, or a review's summary
/// (`review` set). Comments made on lines of code aren't among them.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPrComment {
    /// Its author's login.
    pub author: String,
    /// Unix milliseconds.
    #[serde(default)]
    pub at_ms: u64,
    /// Markdown, at most [`pr::MAX_COMMENT_BYTES`].
    #[serde(default)]
    pub body: String,
    /// `body` was cut to fit.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub cut: bool,
    /// One of [`pr::verdict`]: a review's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    /// `https` on a host the GitHub CLI is signed in to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Never prints the author, the body or the link.
impl std::fmt::Debug for AgentPrComment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPrComment")
            .field("at_ms", &self.at_ms)
            .field("bytes", &self.body.len())
            .field("cut", &self.cut)
            .field("review", &self.review)
            .finish_non_exhaustive()
    }
}

impl AgentPrComment {
    fn is_valid(&self) -> bool {
        self.author.len() <= pr::MAX_AUTHOR_BYTES
            && is_one_line(&self.author)
            && self.body.len() <= pr::MAX_COMMENT_BYTES
            && is_typed_text(&self.body)
            && fits_word(&self.review)
            && self.url.as_deref().is_none_or(is_https_url)
    }

    fn sanitized(mut self) -> Self {
        clean_line(&mut self.author, pr::MAX_AUTHOR_BYTES);
        let (body, cut) = clean_agent_text(&self.body.replace("\r\n", "\n"), true, pr::MAX_COMMENT_BYTES);
        self.body = body;
        self.cut |= cut;
        known_word(&mut self.review, pr::verdict::ALL);
        self.url = self.url.filter(|u| is_https_url(u));
        self
    }
}

/// Whether `text` holds no control character but newline and tab, and no
/// bidi control.
fn is_typed_text(text: &str) -> bool {
    !text.chars().any(|c| is_bidi_control(c) || (c.is_control() && !matches!(c, '\n' | '\t')))
}

/// Body of the `agent.pr.detail` answer (paged: `files` and `comments` are
/// split across the parts, the rest repeats): a pull request's description,
/// changed files, checks and conversation. GitHub that can't be reached is
/// `why: "network"` with the last detail read, if any.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPrDetail {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<AgentPrInfo>,
    pub gh: AgentGhState,
    /// One of [`pr::why`]; absent when the word isn't known here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// `owner/name`, or `host/owner/name`; empty with no pull request.
    #[serde(default)]
    pub repo: String,
    /// Who opened it: "opened by %1".
    #[serde(default)]
    pub author: String,
    /// Its description, Markdown, at most [`pr::MAX_BODY_BYTES`].
    #[serde(default)]
    pub body: String,
    /// `body` was cut to fit: "Shortened to fit."
    #[serde(default, skip_serializing_if = "Not::not")]
    pub body_cut: bool,
    /// Lines added and deleted, and files changed, in all.
    #[serde(default)]
    pub adds: u32,
    #[serde(default)]
    pub dels: u32,
    #[serde(default)]
    pub changed: u32,
    /// At most [`pr::MAX_FILES`]; `files_more` past that.
    #[serde(default)]
    pub files: Vec<AgentPrFile>,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub files_more: bool,
    /// Oldest first, the newest [`pr::MAX_COMMENTS`].
    #[serde(default)]
    pub comments: Vec<AgentPrComment>,
    /// Older ones not sent: "%n older comments aren't shown."
    #[serde(default)]
    pub comments_older: u32,
    /// A thread on the PC on that repository and head branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// When the PC read it from GitHub, Unix milliseconds.
    #[serde(default)]
    pub read_ms: u64,
}

/// Never prints the description, an author, a path or a comment.
impl std::fmt::Debug for AgentPrDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPrDetail")
            .field("pr", &self.pr)
            .field("why", &self.why)
            .field("bytes", &self.body.len())
            .field("files", &self.files.len())
            .field("files_more", &self.files_more)
            .field("comments", &self.comments.len())
            .field("comments_older", &self.comments_older)
            .field("thread", &self.thread)
            .finish_non_exhaustive()
    }
}

impl AgentPrDetail {
    pub fn is_valid(&self) -> bool {
        self.pr.as_ref().is_none_or(AgentPrInfo::is_valid)
            && self.gh.is_valid()
            && fits_word(&self.why)
            && (self.repo.is_empty() || is_pr_repo(&self.repo))
            && self.author.len() <= pr::MAX_AUTHOR_BYTES
            && is_one_line(&self.author)
            && self.body.len() <= pr::MAX_BODY_BYTES
            && is_typed_text(&self.body)
            && self.files.len() <= pr::MAX_FILES
            && self.files.iter().all(AgentPrFile::is_valid)
            && self.comments.len() <= pr::MAX_COMMENTS
            && self.comments.iter().all(AgentPrComment::is_valid)
            && self.thread.as_deref().is_none_or(is_valid_agent_id)
    }

    /// The detail a phone may have: text cleaned and cut (then flagged),
    /// files out of the repository left out, at most [`pr::MAX_FILES`]
    /// (`files_more` past that), the newest [`pr::MAX_COMMENTS`] comments
    /// (the rest counted in `comments_older`); `None` for a `gh` this build
    /// doesn't know.
    pub fn sanitized(mut self) -> Option<Self> {
        self.gh = self.gh.sanitized()?;
        self.pr = self.pr.and_then(AgentPrInfo::sanitized);
        known_word(&mut self.why, pr::why::ALL);
        if !is_pr_repo(&self.repo) {
            self.repo = String::new();
        }
        clean_line(&mut self.author, pr::MAX_AUTHOR_BYTES);
        let (body, cut) = clean_agent_text(&self.body.replace("\r\n", "\n"), true, pr::MAX_BODY_BYTES);
        self.body = body;
        self.body_cut |= cut;
        self.files = std::mem::take(&mut self.files).into_iter().filter_map(AgentPrFile::sanitized).collect();
        if self.files.len() > pr::MAX_FILES {
            self.files.truncate(pr::MAX_FILES);
            self.files_more = true;
        }
        let older = self.comments.len().saturating_sub(pr::MAX_COMMENTS);
        self.comments.drain(..older);
        self.comments_older = self.comments_older.saturating_add(u32::try_from(older).unwrap_or(u32::MAX));
        self.comments =
            std::mem::take(&mut self.comments).into_iter().map(AgentPrComment::sanitized).collect();
        self.thread = self.thread.filter(|t| is_valid_agent_id(t));
        Some(self)
    }

    fn clone_without_lists(&self) -> Self {
        Self {
            pr: self.pr.clone(),
            gh: self.gh.clone(),
            why: self.why.clone(),
            repo: self.repo.clone(),
            author: self.author.clone(),
            body: String::new(),
            body_cut: self.body_cut,
            adds: self.adds,
            dels: self.dels,
            changed: self.changed,
            files: Vec::new(),
            files_more: self.files_more,
            comments: Vec::new(),
            comments_older: self.comments_older,
            thread: self.thread.clone(),
            read_ms: self.read_ms,
        }
    }
}

/// One piece of an [`AgentPrDetail`]'s parts: a file or a comment.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum AgentPrDetailItem {
    File(AgentPrFile),
    Comment(AgentPrComment),
}

impl crate::lanes::Paged for AgentPrDetail {
    type Item = AgentPrDetailItem;

    /// The files, then the comments.
    fn take_items(&mut self) -> Vec<AgentPrDetailItem> {
        let files = std::mem::take(&mut self.files).into_iter().map(AgentPrDetailItem::File);
        let comments = std::mem::take(&mut self.comments).into_iter().map(AgentPrDetailItem::Comment);
        files.chain(comments).collect()
    }

    fn set_items(&mut self, items: Vec<AgentPrDetailItem>) {
        self.files.clear();
        self.comments.clear();
        for item in items {
            match item {
                AgentPrDetailItem::File(file) => self.files.push(file),
                AgentPrDetailItem::Comment(comment) => self.comments.push(comment),
            }
        }
    }

    /// Everything but the lists and the description repeats; the
    /// description goes in the first part only.
    fn continued(&self, items: Vec<AgentPrDetailItem>) -> Self {
        let mut part = self.clone_without_lists();
        <Self as crate::lanes::Paged>::set_items(&mut part, items);
        part
    }

    fn append(&mut self, more: Self) {
        self.files.extend(more.files);
        self.comments.extend(more.comments);
        self.files_more |= more.files_more;
        self.body_cut |= more.body_cut;
    }
}

/// Body of `agent.pr.diff`: one file's diff in a pull request
/// `agent.pr.detail` listed. The answer is an [`AgentGitPatch`] of at most
/// [`git::MAX_PATCH_BYTES`] (`truncated` past that), in parts.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPrDiffRequest {
    pub repo: String,
    pub number: u32,
    /// As the detail listed it; the PC looks it up, never passes it on trust.
    pub path: String,
}

/// Never prints the path.
impl std::fmt::Debug for AgentPrDiffRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPrDiffRequest").field("number", &self.number).finish_non_exhaustive()
    }
}

impl AgentPrDiffRequest {
    pub fn is_valid(&self) -> bool {
        is_pr_repo(&self.repo) && self.number >= 1 && is_relative_path(&self.path)
    }
}

// ---- Ask the agent to write it, or to review a pull request ----

/// Body of `agent.write`: asks the thread's agent, in the thread, to write
/// a commit message or a pull request's title and description, or (§13,
/// `review` with `pr`) to review pull request `pr`. The PC sends a fixed
/// message of its own; what the agent writes comes back as the thread's
/// `wrote` (never for a review).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentWriteRequest {
    pub thread: String,
    /// One of [`git::purpose::ASK`].
    pub purpose: String,
    /// The pull request to review: with `review` only, and then needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u32>,
}

impl AgentWriteRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
            && git::purpose::ASK.contains(&self.purpose.as_str())
            && match self.pr {
                Some(number) => self.purpose == git::purpose::REVIEW && number >= 1,
                None => self.purpose != git::purpose::REVIEW,
            }
    }
}

// ---- Events ----

/// Body of `agent.git` (PC → phone): a thread's folder changed, as its
/// strip shows it. Read `agent.git.status` again for the rest.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitChanged {
    pub thread: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Not on a branch.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub detached: bool,
    /// Files with changes.
    #[serde(default)]
    pub changed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<AgentPrBrief>,
}

/// Never prints the branch.
impl std::fmt::Debug for AgentGitChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGitChanged")
            .field("thread", &self.thread)
            .field("detached", &self.detached)
            .field("changed", &self.changed)
            .field("ahead", &self.ahead)
            .field("behind", &self.behind)
            .field("pr", &self.pr)
            .finish_non_exhaustive()
    }
}

impl AgentGitChanged {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
            && self.branch.as_deref().is_none_or(is_branch_name)
            && self.pr.as_ref().is_none_or(AgentPrBrief::is_valid)
    }

    /// Within its limits, or `None` when its thread isn't an id.
    pub fn sanitized(mut self) -> Option<Self> {
        if !is_valid_agent_id(&self.thread) {
            return None;
        }
        self.branch = self.branch.filter(|b| is_branch_name(b));
        self.pr = self.pr.filter(AgentPrBrief::is_valid);
        Some(self)
    }

    /// A status in brief, as `agent.git` would say it.
    pub fn of(thread: String, status: &AgentGit) -> Self {
        Self {
            thread,
            branch: status.branch.clone(),
            detached: status.detached.is_some(),
            changed: status.changed(),
            ahead: status.ahead,
            behind: status.behind,
            pr: None,
        }
    }
}

/// Body of `agent.git.progress` (PC → phone): a job's new lines and where
/// it stands; the last has its final `phase`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGitProgress {
    pub job: AgentJob,
}

impl AgentGitProgress {
    pub fn is_valid(&self) -> bool {
        self.job.is_valid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Envelope,
        lanes::{Lane, Paged, RPC_PART_TARGET, lane_of, rpc_of, svc},
        messages::{AgentCatalog, AgentCatalogFolder, AgentThread, types},
    };

    fn status() -> AgentGit {
        AgentGit {
            repo: git::repo::OK.into(),
            branch: Some("feat/sync-retry".into()),
            upstream: Some("origin/feat/sync-retry".into()),
            ahead: Some(2),
            behind: Some(0),
            remote: Some("origin".into()),
            remotes: vec![
                AgentGitRemote {
                    name: "origin".into(),
                    label: Some("github.com/acme/app".into()),
                    local: false,
                },
                AgentGitRemote {
                    name: "mirror".into(),
                    label: Some("https://me:secret@example.com/app.git".into()),
                    local: false,
                },
                AgentGitRemote { name: "backup".into(), label: Some("/srv/git/app.git".into()), local: true },
                AgentGitRemote { name: "-x".into(), label: None, local: false },
            ],
            files: vec![
                AgentGitFile {
                    path: "src/sync.rs".into(),
                    status: Some("modified".into()),
                    ..Default::default()
                },
                AgentGitFile {
                    path: "docs/".into(),
                    status: Some("untracked".into()),
                    folder: true,
                    ..Default::default()
                },
                AgentGitFile { path: "/home/me/.ssh/id_ed25519".into(), ..Default::default() },
                AgentGitFile { path: "../outside".into(), ..Default::default() },
                AgentGitFile { path: "new.rs".into(), status: Some("someday".into()), ..Default::default() },
            ],
            total: 5,
            worktree: Some(AgentGitWorktree {
                label: "/home/me/.local/share/g7-agents/worktrees/f1/x".into(),
                branch: "g7/x".into(),
            }),
            last: Some(AgentGitLast { oid7: "db48c89".into(), subject: "Fix\nthe retry".into(), ms: 1 }),
            github: Some(AgentGitHub { repo: "acme/app".into(), host: "github.com".into() }),
            fingerprint: "0123456789abcdef".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_status_never_carries_a_url_an_absolute_path_or_a_worktree_path() {
        let clean = status().sanitized().unwrap();
        assert!(clean.is_valid(), "{clean:?}");
        let encoded = Envelope::new(types::OK, &clean).unwrap().to_cbor();
        for secret in ["://", "secret", "/home/me", "/srv/git", "worktrees", ".ssh", "../"] {
            assert!(!encoded.windows(secret.len()).any(|w| w == secret.as_bytes()), "{secret} was sent");
        }
        assert_eq!(
            clean.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["src/sync.rs", "docs/", "new.rs"]
        );
        assert_eq!(clean.files[2].status, None, "a word not known here reads as absent");
        assert_eq!(clean.remotes.len(), 3);
        assert_eq!(clean.remotes[0].label.as_deref(), Some("github.com/acme/app"));
        assert_eq!(clean.remotes[1].label, None, "a URL is never a label");
        assert_eq!((clean.remotes[2].label.as_deref(), clean.remotes[2].local), (None, true));
        assert_eq!(clean.worktree, None, "a worktree labelled with its path is left out");
        assert_eq!(clean.last.as_ref().map(|l| l.subject.as_str()), Some("Fixthe retry"));
        assert_eq!(clean.total, 5);
        assert!(AgentGit { repo: "maybe".into(), ..Default::default() }.sanitized().is_none());
    }

    #[test]
    fn a_status_keeps_500_files_and_says_there_are_more() {
        let files =
            (0..700).map(|i| AgentGitFile { path: format!("f{i}.rs"), ..Default::default() }).collect();
        let big = AgentGit { repo: git::repo::OK.into(), files, total: 700, ..Default::default() }
            .sanitized()
            .unwrap();
        assert_eq!(big.files.len(), git::MAX_FILES);
        assert!(big.more);
        assert_eq!(big.changed(), 700);
        let parts = big.clone().split(RPC_PART_TARGET);
        let mut joined = parts[0].clone();
        for part in parts.into_iter().skip(1) {
            assert_eq!(part.fingerprint, joined.fingerprint);
            joined.append(part);
        }
        assert_eq!(joined, big);
    }

    #[test]
    fn a_patch_goes_in_pieces_and_a_folder_lists_its_names_once() {
        let patch = format!("@@ -1 +1 @@\n{}", "+line\n".repeat(60_000));
        let names: Vec<String> = (0..60).map(|i| format!("docs/{i}.md")).collect();
        let body = AgentGitPatch { patch: patch.clone(), names, ..Default::default() }.sanitized();
        assert_eq!(body.names.len(), git::MAX_FOLDER_NAMES);
        assert_eq!(body.more_names, 10);
        let parts = body.split(RPC_PART_TARGET);
        assert!(parts.len() > 1);
        assert!(parts.iter().skip(1).all(|p| p.names.is_empty()));
        let mut joined = parts[0].clone();
        for part in parts.into_iter().skip(1) {
            joined.append(part);
        }
        assert_eq!(joined.patch, patch);
        assert_eq!(joined.names.len(), git::MAX_FOLDER_NAMES);
        let cut =
            AgentGitPatch { patch: "x".repeat(git::MAX_PATCH_BYTES + 1), ..Default::default() }.sanitized();
        assert!(cut.truncated && cut.patch.len() == git::MAX_PATCH_BYTES);
    }

    #[test]
    fn requests_hold_their_limits() {
        let thread = "abc".to_owned();
        let review = |paths: Vec<String>| AgentGitReviewRequest { thread: thread.clone(), paths };
        assert!(review(vec!["a.rs".into(), "docs/".into()]).is_valid());
        assert!(!review(vec![]).is_valid());
        assert!(!review((0..201).map(|i| format!("f{i}")).collect()).is_valid());
        assert!(!review(vec!["a.rs".into(), "a.rs".into()]).is_valid(), "none twice");
        assert!(!review(vec!["/etc/passwd".into()]).is_valid());
        assert!(!review(vec!["d/".repeat(257)]).is_valid(), "a path over 512 bytes");
        let commit = |message: &str| AgentGitCommitRequest {
            thread: thread.clone(),
            review: "0123456789abcdef".into(),
            message: message.into(),
        };
        assert!(commit("Fix the retry\n\nIt waited forever.").is_valid());
        assert!(!commit("").is_valid() && !commit(" \n ").is_valid());
        assert!(!commit(&"x".repeat(8193)).is_valid());
        assert!(commit(&"x".repeat(8192)).is_valid());
        assert!(!commit("a\0b").is_valid());
        assert!(!AgentGitCommitRequest { review: "nope".into(), ..commit("x") }.is_valid());
        let switch = |branch: &str| AgentGitSwitchRequest {
            thread: thread.clone(),
            branch: branch.into(),
            ..Default::default()
        };
        assert!(switch("feat/x").is_valid() && switch(&"b".repeat(255)).is_valid());
        for bad in ["", "-f", "--discard-changes", "a b", "/abs", "a:b", &"b".repeat(256)] {
            assert!(!switch(bad).is_valid(), "{bad:?}");
        }
        assert!(!AgentGitSwitchRequest { create: true, track: true, ..switch("x") }.is_valid());
        let create = |title: &str, body: usize| AgentPrCreateRequest {
            thread: thread.clone(),
            title: title.into(),
            body: "b".repeat(body),
            ..Default::default()
        };
        assert!(create("Fix it", pr::MAX_BODY_BYTES).is_valid());
        assert!(!create(&"t".repeat(257), 0).is_valid());
        assert!(!create("", 0).is_valid() && !create("two\nlines", 0).is_valid());
        assert!(!create("Fix it", pr::MAX_BODY_BYTES + 1).is_valid());
        assert!(!AgentGitPushRequest { thread: thread.clone(), remote: Some("-o".into()) }.is_valid());
        assert!(!AgentGitPushRequest { thread: thread.clone(), remote: Some("https://x".into()) }.is_valid());
        assert!(
            AgentPrListRequest { show: "folder".into(), folder: Some("f1".into()), refresh: false }
                .is_valid()
        );
        assert!(!AgentPrListRequest { show: "folder".into(), folder: None, refresh: false }.is_valid());
        assert!(!AgentPrListRequest { show: "all".into(), folder: None, refresh: false }.is_valid());
        let write = |purpose: &str, pr: Option<u32>| AgentWriteRequest {
            thread: thread.clone(),
            purpose: purpose.into(),
            pr,
        };
        assert!(write("pr", None).is_valid() && write("commit", None).is_valid());
        assert!(!write("merge", None).is_valid());
        assert!(write("review", Some(212)).is_valid());
        assert!(!write("review", None).is_valid(), "a review names its pull request");
        assert!(!write("review", Some(0)).is_valid());
        assert!(!write("commit", Some(212)).is_valid(), "a number only with a review");
    }

    #[test]
    fn a_job_keeps_its_newest_lines_for_a_phone() {
        let lines: Vec<String> = (0..100).map(|i| format!("line {i}\u{1b}[31m red\u{202E}")).collect();
        let job = AgentJob {
            id: "0123456789abcdef".into(),
            thread: "abc".into(),
            kind: git::kind::COMMIT.into(),
            phase: git::phase::FAILED.into(),
            lines,
            from: 10,
            dropped: 10,
            error: Some(AgentJobError { reason: "Not A Word".into(), message: "x\ny".into(), names: vec![] }),
            ..Default::default()
        }
        .sanitized()
        .unwrap();
        assert!(job.is_valid(), "{job:?}");
        assert_eq!(job.lines.len(), git::MAX_JOB_LINES);
        assert_eq!(job.lines[0], "line 60[31m red");
        assert_eq!((job.from, job.dropped), (70, 70));
        assert_eq!(job.error.as_ref().map(|e| e.reason.as_str()), Some("failed"));
        let long = AgentJob { lines: vec!["é".repeat(600); 30], ..job.clone() }.sanitized().unwrap();
        assert!(long.lines.iter().all(|l| l.chars().count() == git::MAX_LINE_CHARS));
        assert!(long.lines.iter().map(String::len).sum::<usize>() <= git::MAX_JOB_TEXT_BYTES);
        assert!(AgentJob { kind: "merge_local".into(), ..job.clone() }.sanitized().is_none());
        assert!(AgentJob { id: "XYZ".into(), ..job }.sanitized().is_none());
    }

    #[test]
    fn a_pull_request_opens_only_on_https_and_an_unknown_check_is_pending() {
        let info = AgentPrInfo {
            number: 212,
            title: "Fix\tthe retry".into(),
            state: "open".into(),
            url: "http://github.com/acme/app/pull/212".into(),
            review: Some("".into()),
            checks: vec![
                AgentCheck {
                    name: "ci / build".into(),
                    state: "pass".into(),
                    url: Some("https://github.com/x".into()),
                },
                AgentCheck {
                    name: "lint".into(),
                    state: "neutralish".into(),
                    url: Some("javascript:alert(1)".into()),
                },
            ],
            ..Default::default()
        }
        .sanitized()
        .unwrap();
        assert_eq!(info.url, "", "only https opens");
        assert_eq!(info.review, None, "no review reads as absent");
        assert_eq!(info.checks[1].state, "pending", "never pass");
        assert_eq!(info.checks[1].url, None);
        assert!(is_https_url("https://github.com/acme/app/pull/212"));
        assert!(!is_https_url("https://me@github.com/x") && !is_https_url("https:///x"));
        assert!(AgentPrInfo { state: "draft".into(), ..info }.sanitized().is_none());
    }

    #[test]
    fn a_thread_and_the_catalog_carry_what_source_control_adds() {
        let mut thread = AgentThread {
            id: "abc".into(),
            agent: "claude".into(),
            state: "done".into(),
            mode: "supervised".into(),
            branch: Some("g7/fix".into()),
            worktree: Some(AgentGitWorktree { label: "fix-1a2b3c".into(), branch: "g7/fix".into() }),
            changed: Some(4),
            wrote: Some(AgentWrote { purpose: "commit".into(), turn: 3, text: "Fix it".into(), at_ms: 1 }),
            ..Default::default()
        };
        assert!(thread.is_valid());
        let env = Envelope::new(types::AGENT_THREAD, &thread).unwrap();
        assert_eq!(env.body::<AgentThread>().unwrap(), thread);
        thread.branch = Some("bad branch".into());
        thread.worktree = Some(AgentGitWorktree { label: "~/x".into(), branch: "g7/fix".into() });
        thread.wrote = Some(AgentWrote { purpose: "poem".into(), ..Default::default() });
        assert!(!thread.is_valid());
        let clean = thread.sanitized().unwrap();
        assert_eq!((clean.branch, clean.worktree, clean.wrote), (None, None, None));
        assert_eq!(clean.changed, Some(4));

        let catalog = AgentCatalog {
            folders: vec![AgentCatalogFolder {
                id: "f1".into(),
                label: "app".into(),
                checkpoints: true,
                follow_remote: true,
                ..Default::default()
            }],
            push: true,
            ..Default::default()
        };
        let env = Envelope::new(types::AGENT_CATALOG, &catalog).unwrap();
        assert_eq!(env.body::<AgentCatalog>().unwrap(), catalog);
        let old: AgentCatalog =
            Envelope::new(types::AGENT_CATALOG, &AgentCatalog::default()).unwrap().body().unwrap();
        assert!(!old.push, "off unless the PC says so");
    }

    #[test]
    fn every_message_has_its_lane_and_none_travels_on_bluetooth() {
        let rpcs = [
            (types::AGENT_GIT_STATUS, "git.status", true),
            (types::AGENT_GIT_PATCH, "git.patch", true),
            (types::AGENT_GIT_REVIEW, "git.review", true),
            (types::AGENT_GIT_COMMIT, "git.commit", false),
            (types::AGENT_GIT_PUSH, "git.push", false),
            (types::AGENT_GIT_FETCH, "git.fetch", false),
            (types::AGENT_GIT_BRANCHES, "git.branches", true),
            (types::AGENT_GIT_SWITCH, "git.switch", false),
            (types::AGENT_GIT_JOB, "git.job", false),
            (types::AGENT_PR_VIEW, "pr.view", false),
            (types::AGENT_PR_CREATE, "pr.create", false),
            (types::AGENT_PR_LIST, "pr.list", true),
            (types::AGENT_WRITE, "write", false),
            (types::AGENT_PR_DETAIL, "pr.detail", true),
            (types::AGENT_PR_DIFF, "pr.diff", true),
        ];
        for (t, op, paged) in rpcs {
            let rpc = rpc_of(t).unwrap_or_else(|| panic!("{t} is an RPC"));
            assert_eq!((rpc.svc, rpc.op, rpc.paged), (svc::AGENT, op, paged), "{t}");
            assert!(!crate::ble::carries(t), "{t}");
        }
        for t in [types::AGENT_GIT, types::AGENT_GIT_PROGRESS] {
            assert_eq!(lane_of(t), Some(Lane::Service(svc::AGENT)), "{t}");
            assert!(!crate::ble::carries(t), "{t}");
        }
        for never in [
            "agent.git.merge",
            "agent.pr.merge",
            "agent.pr.ready",
            "agent.worktree.remove",
            "agent.folders.set",
            "agent.pr.review",
            "agent.pr.comment",
            "agent.pr.start",
        ] {
            assert_eq!(lane_of(never), None, "{never} has no message");
        }
    }

    fn detail() -> AgentPrDetail {
        AgentPrDetail {
            pr: Some(AgentPrInfo { number: 212, state: "open".into(), ..Default::default() }),
            gh: AgentGhState {
                gh: pr::gh::OK.into(),
                hosts: vec!["github.com".into()],
                ..Default::default()
            },
            repo: "acme/app".into(),
            author: "octo\u{202E}cat\n".into(),
            body: "Fixes the retry.\r\n\u{1b}[31mred\n".into(),
            adds: 12,
            dels: 3,
            changed: 2,
            files: vec![
                AgentPrFile { path: "src/sync.rs".into(), change: "modified".into(), adds: 10, dels: 3 },
                AgentPrFile { path: "docs/new.md".into(), change: "added".into(), adds: 2, dels: 0 },
                AgentPrFile {
                    path: "../../etc/passwd".into(),
                    change: "modified".into(),
                    ..Default::default()
                },
                AgentPrFile { path: "/abs".into(), change: "deleted".into(), ..Default::default() },
                AgentPrFile { path: "x.rs".into(), change: "type_changed".into(), ..Default::default() },
            ],
            comments: vec![
                AgentPrComment {
                    author: "rev".into(),
                    at_ms: 1,
                    body: "Looks fine".into(),
                    review: Some("approved".into()),
                    url: Some("https://github.com/acme/app/pull/212#r1".into()),
                    ..Default::default()
                },
                AgentPrComment {
                    author: "bot".into(),
                    at_ms: 2,
                    body: "x".repeat(5000),
                    review: Some("pending".into()),
                    url: Some("http://example.com".into()),
                    ..Default::default()
                },
            ],
            thread: Some("abc".into()),
            read_ms: 9,
            ..Default::default()
        }
    }

    #[test]
    fn a_pull_requests_detail_fits_a_phone() {
        let clean = detail().sanitized().unwrap();
        assert!(clean.is_valid(), "{clean:?}");
        assert_eq!(clean.author, "octocat");
        assert_eq!(clean.body, "Fixes the retry.\n[31mred\n");
        let paths: Vec<&str> = clean.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["src/sync.rs", "docs/new.md", "x.rs"], "no path out of the repository");
        assert_eq!(clean.files[2].change, "changed", "a word not known here reads as changed");
        assert_eq!(clean.comments[0].review.as_deref(), Some("approved"));
        assert_eq!(clean.comments[1].review, None);
        assert_eq!(clean.comments[1].url, None, "only https opens");
        assert!(clean.comments[1].cut && clean.comments[1].body.len() == pr::MAX_COMMENT_BYTES);

        let many = AgentPrDetail {
            body: "b".repeat(pr::MAX_BODY_BYTES + 1),
            files: (0..400)
                .map(|i| AgentPrFile {
                    path: format!("f{i}.rs"),
                    change: "added".into(),
                    ..Default::default()
                })
                .collect(),
            comments: (0..100)
                .map(|i| AgentPrComment { author: format!("a{i}"), at_ms: i, ..Default::default() })
                .collect(),
            comments_older: 7,
            ..detail()
        }
        .sanitized()
        .unwrap();
        assert!(many.body_cut && many.body.len() == pr::MAX_BODY_BYTES);
        assert_eq!((many.files.len(), many.files_more), (pr::MAX_FILES, true));
        assert_eq!(many.comments.len(), pr::MAX_COMMENTS);
        assert_eq!(many.comments[0].author, "a50", "the newest, oldest first");
        assert_eq!(many.comments_older, 57);
        assert!(
            AgentPrDetail { gh: AgentGhState { gh: "maybe".into(), ..Default::default() }, ..detail() }
                .sanitized()
                .is_none()
        );
        let why = AgentPrDetail { why: Some("too_large".into()), ..detail() }.sanitized().unwrap();
        assert_eq!(why.why.as_deref(), Some(pr::why::TOO_LARGE));
        let later = AgentPrDetail { why: Some("archived".into()), ..detail() }.sanitized().unwrap();
        assert_eq!(later.why, None);
    }

    #[test]
    fn a_detail_goes_in_parts_and_joins() {
        let big = AgentPrDetail {
            body: "d".repeat(30_000),
            files: (0..300)
                .map(|i| AgentPrFile {
                    path: format!("{}/f{i}.rs", "dir".repeat(100)),
                    change: "modified".into(),
                    adds: i,
                    dels: 1,
                })
                .collect(),
            comments: (0..50u32)
                .map(|i| AgentPrComment {
                    author: format!("a{i}"),
                    at_ms: u64::from(i),
                    body: "c".repeat(4096),
                    ..Default::default()
                })
                .collect(),
            ..detail()
        }
        .sanitized()
        .unwrap();
        let parts = big.clone().split(RPC_PART_TARGET);
        assert!(parts.len() > 1, "{} parts", parts.len());
        for part in &parts {
            assert!(Envelope::new(types::PART, part).unwrap().to_cbor().len() <= RPC_PART_TARGET + 8 * 1024);
        }
        assert!(parts.iter().skip(1).all(|p| p.body.is_empty() && p.repo == big.repo));
        let mut joined = parts[0].clone();
        for part in parts.into_iter().skip(1) {
            joined.append(part);
        }
        assert_eq!(joined, big);
    }

    #[test]
    fn pull_request_requests_hold_their_limits() {
        for ok in ["acme/app", "github.example.com/acme/app", "a-b/c.d_e", &format!("{}/x", "o".repeat(100))]
        {
            assert!(is_pr_repo(ok), "{ok}");
        }
        for bad in [
            "",
            "acme",
            "a/b/c/d",
            "-x/y",
            "x/.y",
            "../x",
            "acme/app/",
            "acme//app",
            "https://github.com/acme/app",
            "acme/app name",
            "me@host/acme/app",
            &format!("{}/x", "o".repeat(101)),
            &format!("{}/{}", "o".repeat(70), "n".repeat(70)),
        ] {
            assert!(!is_pr_repo(bad), "{bad:?}");
        }
        let req = |thread: Option<&str>, repo: Option<&str>, number: Option<u32>| AgentPrDetailRequest {
            thread: thread.map(Into::into),
            repo: repo.map(Into::into),
            number,
            refresh: false,
        };
        assert!(req(Some("abc"), None, None).is_valid());
        assert!(req(None, Some("acme/app"), Some(212)).is_valid());
        assert!(req(Some("abc"), Some("acme/app"), Some(212)).is_valid());
        assert!(!req(None, None, None).is_valid());
        assert!(!req(None, Some("acme/app"), None).is_valid());
        assert!(!req(None, None, Some(212)).is_valid());
        assert!(!req(None, Some("acme/app"), Some(0)).is_valid(), "number 0");
        assert!(!req(None, Some("-x/y"), Some(1)).is_valid());
        assert!(!req(Some("/home"), None, None).is_valid());
        let diff = |repo: &str, number: u32, path: &str| AgentPrDiffRequest {
            repo: repo.into(),
            number,
            path: path.into(),
        };
        assert!(diff("acme/app", 212, "src/sync.rs").is_valid());
        assert!(!diff("acme/app", 0, "src/sync.rs").is_valid());
        assert!(!diff("a/b/c/d", 1, "src/sync.rs").is_valid());
        assert!(!diff("acme/app", 1, "../x").is_valid());
        assert!(!diff("acme/app", 1, "/etc/passwd").is_valid());
        assert!(!diff("acme/app", 1, &"d/".repeat(257)).is_valid(), "a path over 512 bytes");
    }

    #[test]
    fn nothing_prints_a_path_a_message_or_a_line() {
        let file = AgentGitFile { path: "secret/plan.md".into(), ..Default::default() };
        let commit = AgentGitCommitRequest { message: "secret plan".into(), ..Default::default() };
        let job = AgentJob { lines: vec!["secret line".into()], ..Default::default() };
        let wrote = AgentWrote { text: "secret text".into(), ..Default::default() };
        let row = AgentPrRow { title: "secret title".into(), ..Default::default() };
        let detail = AgentPrDetail {
            body: "secret body".into(),
            author: "secret".into(),
            files: vec![AgentPrFile { path: "secret.rs".into(), ..Default::default() }],
            comments: vec![AgentPrComment { body: "secret".into(), ..Default::default() }],
            ..Default::default()
        };
        let diff = AgentPrDiffRequest { path: "secret.rs".into(), ..Default::default() };
        for printed in [
            format!("{file:?}"),
            format!("{commit:?}"),
            format!("{job:?}"),
            format!("{wrote:?}"),
            format!("{row:?}"),
            format!("{detail:?}"),
            format!("{diff:?}"),
        ] {
            assert!(!printed.contains("secret"), "{printed}");
        }
    }
}
