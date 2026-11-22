// SPDX-License-Identifier: GPL-3.0-or-later
//! Source control on the Agents page (phase 4, §SC9.2): the open thread's
//! git status and its strip, the commit box, jobs and their output, branches,
//! push, the branch's pull request, merge and Mark ready, the Pull Requests
//! list, worktrees, Setup's source-control section and the quote chip.
//!
//! The host runs every git and `gh` command; this keeps what the page shows
//! and turns the host's answers into what the page needs. The page maps the
//! reason words to copy. Every act (commit, push, switch, create, merge) runs
//! only when the user pressed its button: never on a timer, never when a
//! turn ends.

use std::collections::BTreeSet;

use g7_agents::api::{
    CheckState, FoldersSetParams, GhState, GitBranchesParams, GitBranchesResult, GitChange, GitChangedEvent,
    GitCommitParams, GitFetchParams, GitJobParams, GitPushParams, GitStatus, GitStatusParams,
    GitSwitchParams, Job, JobKind, JobPhase, MergeMethod, PrCreateParams, PrInfo, PrListParams, PrListResult,
    PrMergeParams, PrMethodsResult, PrReadyParams, PrShow, PrViewParams, PrViewResult, Purpose, RepoState,
    Worktree, WorktreeRemoveParams, WorktreeWanted, Wrote,
};

use super::*;

// ---- Bounds (§SC7.4) ----

/// Job lines the panel keeps.
const KEPT_LINES: usize = api::MAX_JOB_LINES;
/// Job lines a phone gets: the last 40, at most 8 KiB.
const PHONE_JOB_LINES: usize = 40;
const PHONE_JOB_BYTES: usize = 8 * 1024;
/// Files in a phone's status.
const PHONE_FILES: usize = 500;
/// Quote chips in one message, and characters in each.
const MAX_QUOTES: usize = 5;
const MAX_QUOTE_CHARS: usize = 2_000;
/// A new branch name, in bytes (§SC3.1).
const MAX_NEW_BRANCH: usize = 100;
/// A subject longer than this is "long" (never refused).
const SUBJECT_LONG: usize = 72;
/// `git.changed` is read again this long after the last one.
const CHANGED_QUIET: Duration = Duration::from_millis(300);
/// A running job is read this often when its events don't come.
const JOB_EVERY: Duration = Duration::from_secs(1);
/// The branch menu's search waits this long after the last key.
const BRANCH_QUIET: Duration = Duration::from_millis(150);

// ---- The quote chip (§SC9.1) ----

/// Lines of a diff quoted into a message: `path:first–last` and the lines.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Quote {
    pub path: String,
    pub first: u32,
    pub last: u32,
    pub text: String,
}

impl Quote {
    /// The text a message carries: the path line, then the lines in a fence
    /// longer than any run of backticks inside them.
    pub fn text_form(&self) -> String {
        let body = self.text.strip_suffix('\n').unwrap_or(&self.text);
        let (mut longest, mut run) = (0, 0);
        for c in body.chars() {
            run = if c == '`' { run + 1 } else { 0 };
            longest = longest.max(run);
        }
        let fence = "`".repeat((longest + 1).max(3));
        format!("{}:{}\u{2013}{}\n{fence}\n{body}\n{fence}", self.path, self.first, self.last)
    }
}

/// The message with each chip's quote before the user's words.
pub(crate) fn with_quotes(quotes: &[Quote], text: &str) -> String {
    if quotes.is_empty() {
        return text.to_owned();
    }
    let mut parts: Vec<String> = quotes.iter().map(Quote::text_form).collect();
    if !text.trim().is_empty() {
        parts.push(text.to_owned());
    }
    parts.join("\n\n")
}

/// One row of a file's diff as a quote needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct QuoteRow {
    /// Old and new line numbers (0: none).
    pub old: u32,
    pub new: u32,
    pub hunk: bool,
    pub text: String,
}

/// The quote of rows `a..=b` (either order); `None` when they hold no
/// line. Hunk headers are left out; a line counts by its new number, a
/// removed one by its old.
pub(crate) fn quote_rows(path: &str, rows: &[QuoteRow], a: usize, b: usize) -> Option<Quote> {
    let (a, b) = (a.min(b), a.max(b).min(rows.len().checked_sub(1)?));
    let picked: Vec<&QuoteRow> = rows.get(a..=b)?.iter().filter(|r| !r.hunk).collect();
    let number = |r: &QuoteRow| if r.new > 0 { r.new } else { r.old };
    let first = number(picked.first()?);
    let last = number(picked.last()?).max(first);
    let joined = picked.iter().map(|r| r.text.as_str()).collect::<Vec<_>>().join("\n");
    Some(Quote { path: path.to_owned(), first, last, text: joined.chars().take(MAX_QUOTE_CHARS).collect() })
}

// ---- Fields the agent can fill (§SC5.4) ----

/// A field "Ask <agent> to write it" may fill: the answer goes in only
/// when the user hasn't typed in it since asking; otherwise it's offered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AskField {
    pub text: String,
    /// Counts texts the app put in: the page takes each once.
    pub set_n: u64,
    /// Asked; the answer hasn't come yet.
    pub asking: bool,
    /// The user typed since asking.
    pub typed: bool,
    /// The answer, when it wasn't put in ("Use <agent>'s message").
    pub offer: Option<String>,
    /// The answer last taken, so a repeated `thread.state` doesn't take it again.
    pub taken: Option<(u32, u64)>,
    /// When it was last asked and not answered: an answer written since
    /// still fills it, even after the turn's end came first.
    pub asked_ms: Option<u64>,
}

impl AskField {
    /// The user changed the text (the page's echo of a put changes nothing).
    pub fn typed(&mut self, text: String) {
        if text != self.text {
            self.text = text;
            if self.asked_ms.is_some() {
                self.typed = true;
            }
        }
    }

    /// The app puts `text` in.
    pub fn put(&mut self, text: String) {
        self.text = text;
        self.set_n += 1;
        self.offer = None;
    }

    pub fn ask(&mut self, now_ms: u64) {
        self.asking = true;
        self.asked_ms = Some(now_ms);
        self.typed = false;
        self.offer = None;
    }

    /// The answer (turn `turn`, written at `at_ms`) arrived: true when it went in.
    pub fn answered(&mut self, turn: u32, at_ms: u64, text: &str) -> bool {
        let asked = self.asking || self.asked_ms.is_some_and(|t| at_ms >= t);
        if !asked || self.taken == Some((turn, at_ms)) {
            return false;
        }
        self.taken = Some((turn, at_ms));
        self.asking = false;
        self.asked_ms = None;
        if self.typed && !self.text.trim().is_empty() {
            self.offer = Some(text.to_owned());
            false
        } else {
            self.put(text.to_owned());
            true
        }
    }

    pub fn gave_up(&mut self) {
        self.asking = false;
    }

    fn json(&self) -> Value {
        json!({ "set": { "n": self.set_n, "text": self.text }, "asking": self.asking, "offer": self.offer.is_some() })
    }
}

/// A pull request's title and description from the agent's answer.
pub(crate) fn split_title(text: &str) -> (String, String) {
    match text.trim().split_once('\n') {
        Some((title, rest)) => (title.trim().to_owned(), rest.trim().to_owned()),
        None => (text.trim().to_owned(), String::new()),
    }
}

/// The subject (first line) is past 72 characters.
pub(crate) fn subject_long(message: &str) -> bool {
    message.lines().next().unwrap_or_default().chars().count() > SUBJECT_LONG
}

// ---- New branch names (§SC3.1; the host checks again, with git) ----

pub(crate) fn branch_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NEW_BRANCH
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
        && !name.starts_with(['-', '/', '.'])
        && !name.contains("..")
        && !name.contains("//")
        && !name.ends_with(['/', '.'])
        && !name.split('/').any(|part| part.starts_with('.') || part.ends_with(".lock"))
        && name != "HEAD"
}

/// A worktree branch from a thread's words: `g7/`, then lowercase ASCII
/// letters and digits, other runs one `-`, at most 40.
pub(crate) fn worktree_branch(words: &str) -> String {
    let mut slug = String::new();
    for c in words.chars() {
        if slug.len() >= 40 {
            break;
        }
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() { "g7/new-thread".to_owned() } else { format!("g7/{slug}") }
}

// ---- What the commit box allows (§SC3.4, §SC9.4) ----

/// Why Commit can't be pressed, as a word the page words (`None`: it can).
pub(crate) fn commit_blocked(
    st: &GitStatus,
    picked: usize,
    thread_running: bool,
    job_running: bool,
) -> Option<&'static str> {
    if st.repo != RepoState::Ok {
        Some("not_git")
    } else if st.locked {
        Some("git_busy")
    } else if st.doing.is_some() {
        Some("doing")
    } else if st.detached.is_some() {
        Some("detached")
    } else if st.conflicts > 0 || st.files.iter().any(|f| f.status == GitChange::Conflict) {
        Some("conflicts")
    } else if thread_running {
        Some("thread_busy")
    } else if job_running {
        Some("job")
    } else if picked == 0 {
        Some("nothing")
    } else {
        None
    }
}

/// The paths a commit takes: the ticked ones, never a conflict. (The host
/// adds a rename's old name itself.)
pub(crate) fn picked_paths(st: &GitStatus, unticked: &BTreeSet<String>) -> Vec<String> {
    st.files
        .iter()
        .filter(|f| f.status != GitChange::Conflict && !unticked.contains(&f.path))
        .map(|f| f.path.clone())
        .collect()
}

// ---- Jobs (§SC5.2) ----

fn running(job: &Job) -> bool {
    job.phase == JobPhase::Running
}

/// A `git.progress` event onto the job shown: its new lines go on when they
/// follow what is shown; true when the job must be read whole instead.
pub(crate) fn take_progress(shown: &mut Job, new: Job) -> bool {
    if new.id != shown.id {
        return false;
    }
    let next = shown.from + shown.lines.len() as u64;
    let gap = new.from > next;
    if !gap {
        let skip = usize::try_from(next - new.from).unwrap_or(usize::MAX);
        shown.lines.extend(new.lines.into_iter().skip(skip));
        let over = shown.lines.len().saturating_sub(KEPT_LINES);
        if over > 0 {
            shown.lines.drain(..over);
            shown.from += over as u64;
            shown.dropped = shown.dropped.saturating_add(u32::try_from(over).unwrap_or(u32::MAX));
        }
    }
    shown.phase = new.phase;
    shown.ended_ms = new.ended_ms.or(shown.ended_ms);
    if new.result.is_some() {
        shown.result = new.result;
    }
    if new.error.is_some() {
        shown.error = new.error;
    }
    gap
}

/// A job's last words, as a word and its values: the page words them.
pub(crate) fn job_words(job: &Job) -> (&'static str, Vec<String>) {
    let reason = job.error.as_ref().map(|e| e.reason.as_str()).unwrap_or_default();
    let result = job.result.clone().unwrap_or_default();
    match (job.kind, job.phase) {
        (JobKind::Commit, JobPhase::Running) => ("running_hooks", vec![]),
        (JobKind::Commit, JobPhase::Done) => {
            ("committed", vec![result.oid7.unwrap_or_default(), result.subject.unwrap_or_default()])
        }
        (JobKind::Commit, JobPhase::Stopped) => ("commit_stopped", vec![]),
        (JobKind::Commit, _) if reason == "hook" => ("commit_hook", vec![]),
        (JobKind::Commit, _) => ("commit_failed", vec![]),
        (JobKind::Push, JobPhase::Running) => ("pushing", vec![result.remote.unwrap_or_default()]),
        (JobKind::Push, JobPhase::Done) if result.up_to_date => ("up_to_date", vec![]),
        (JobKind::Push, JobPhase::Done) => {
            ("pushed", vec![result.branch.unwrap_or_default(), result.remote.unwrap_or_default()])
        }
        (JobKind::Push, JobPhase::Stopped) => ("push_failed", vec![]),
        (JobKind::Push, _) => match reason {
            "behind" => ("push_behind", vec![]),
            "remote_refused" => ("push_remote_refused", vec![]),
            "hook" => ("push_hook", vec![]),
            "sign_in" => ("push_sign_in", vec![]),
            "network" => ("push_network", vec![]),
            _ => ("push_failed", vec![]),
        },
        (JobKind::Switch, JobPhase::Done) if result.created => {
            ("created", vec![result.branch.unwrap_or_default()])
        }
        (JobKind::Switch, JobPhase::Done) => ("switched", vec![result.branch.unwrap_or_default()]),
        (JobKind::Switch, JobPhase::Running) => ("", vec![]),
        (JobKind::Switch, _) if reason == "would_overwrite" => ("would_overwrite", vec![]),
        (JobKind::Switch, _) => ("switch_failed", vec![]),
        (JobKind::PrCreate, JobPhase::Running) => ("opening", vec![]),
        (JobKind::PrCreate, JobPhase::Done) => {
            let number = result.number.or(result.pr.as_ref().map(|p| p.number)).unwrap_or(0);
            ("pr_opened", vec![number.to_string()])
        }
        (JobKind::PrCreate, _) => ("pr_failed", vec![]),
        (JobKind::PrMerge, JobPhase::Running) => ("", vec![]),
        (JobKind::PrMerge, JobPhase::Done) => {
            ("merged", vec![result.number.or(result.pr.as_ref().map(|p| p.number)).unwrap_or(0).to_string()])
        }
        (JobKind::PrMerge, _) => ("merge_failed", vec![]),
        (JobKind::Worktree, JobPhase::Running) => ("creating_worktree", vec![]),
        (JobKind::Worktree, JobPhase::Done) => ("", vec![]),
        (JobKind::Worktree, _) => ("worktree_failed", vec![]),
        (JobKind::Fetch, _) => ("", vec![]),
    }
}

/// The job as the panel shows it.
pub(crate) fn job_json(job: &Job) -> Value {
    let (words, args) = job_words(job);
    let shown = job.lines.len().min(KEPT_LINES);
    let cut = job.lines.len() - shown;
    json!({
        "id": job.id,
        "kind": word(&job.kind),
        "phase": word(&job.phase),
        "running": running(job),
        "lines": job.lines[cut..],
        "dropped": u64::from(job.dropped) + cut as u64,
        "words": words,
        "args": args,
        "reason": job.error.as_ref().map(|e| e.reason.clone()).unwrap_or_default(),
        "stoppable": running(job) && matches!(job.kind, JobKind::Commit | JobKind::Push),
    })
}

// ---- The pull request card (§SC9.1) ----

/// The card's checks line: "passed" (all of `total`), "failed" or "running"
/// (`n` of `total`), or "none". Skipped and cancelled checks are out of the total.
pub(crate) fn checks_line(pr: &PrInfo) -> (&'static str, u32, u32) {
    let total = pr.pass + pr.fail + pr.pending;
    if total == 0 {
        ("none", 0, 0)
    } else if pr.fail > 0 {
        ("failed", pr.fail, total)
    } else if pr.pending > 0 {
        ("running", pr.pending, total)
    } else {
        ("passed", total, total)
    }
}

pub(crate) fn pr_json(pr: &PrInfo) -> Value {
    let (line, n, total) = checks_line(pr);
    let mut checks: Vec<_> = pr.checks.iter().collect();
    // CheckState orders failing first.
    checks.sort_by_key(|c| c.state);
    json!({
        "number": pr.number,
        "title": pr.title,
        "state": word(&pr.state),
        "draft": pr.draft,
        "url": pr.url,
        "base": pr.base,
        "head": pr.head,
        "oid7": pr.head_oid.get(..7).unwrap_or_default(),
        "review": pr.review.as_ref().map(word).unwrap_or_default(),
        "conflicts": pr.mergeable == Some(api::Mergeable::Conflicts),
        "checksLine": line,
        "checksN": n,
        "checksTotal": total,
        "failed": pr.fail,
        "running": pr.pending,
        "checks": checks.iter().filter(|c| c.state != CheckState::Skipped).map(|c| json!({
            "name": c.name, "state": word(&c.state), "url": c.url.clone().unwrap_or_default(),
        })).collect::<Vec<_>>(),
        "readMs": pr.read_ms,
    })
}

/// A link that may open: `https` on a host the GitHub CLI is signed in to.
pub(crate) fn safe_url(url: &str, hosts: &[String]) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !host.is_empty() && !host.contains(['@', ':']) && hosts.iter().any(|h| h.eq_ignore_ascii_case(host))
}

fn gh_json(gh: &GhState) -> Value {
    json!({ "gh": word(&gh.gh), "version": gh.version.clone().unwrap_or_default(), "hosts": gh.hosts, "checkedMs": gh.checked_ms })
}

// ---- What a phone may get (§SC6.1; the core checks again) ----

/// `st` as a phone may see it: at most 500 files, no folder on this PC
/// named, a worktree by its label and branch only.
pub(crate) fn phone_status(st: &GitStatus) -> GitStatus {
    let mut out = st.clone();
    out.files.retain(|f| !f.path.starts_with('/') && !f.from.as_deref().is_some_and(|p| p.starts_with('/')));
    if out.files.len() > PHONE_FILES {
        out.files.truncate(PHONE_FILES);
        out.more = true;
    }
    for r in out.remotes.iter_mut().filter(|r| r.local) {
        r.label = String::new();
    }
    if let Some(w) = out.worktree.as_mut() {
        w.id = String::new();
    }
    out
}

/// A job as a phone may see it: its last 40 lines, at most 8 KiB.
pub(crate) fn phone_job(job: &Job) -> Job {
    let mut out = job.clone();
    let mut bytes = 0;
    let mut kept: Vec<String> = Vec::new();
    for line in job.lines.iter().rev().take(PHONE_JOB_LINES) {
        if bytes + line.len() > PHONE_JOB_BYTES {
            break;
        }
        bytes += line.len();
        kept.push(line.clone());
    }
    kept.reverse();
    let cut = job.lines.len() - kept.len();
    out.from = job.from + cut as u64;
    out.dropped = job.dropped.saturating_add(u32::try_from(cut).unwrap_or(u32::MAX));
    out.lines = kept;
    out
}

// ---- What the page keeps ----

#[derive(Debug, Default)]
pub(crate) struct BranchMenu {
    pub open: bool,
    pub query: String,
    pub seq: u64,
    pub loading: bool,
    pub list: Option<GitBranchesResult>,
}

#[derive(Debug, Default)]
pub(crate) struct Merge {
    pub number: u32,
    /// The head commit the dialog shows; the merge is pinned to it.
    pub head: String,
    pub methods: Option<PrMethodsResult>,
    pub failed: bool,
    pub method: Option<MergeMethod>,
    pub looked: bool,
    /// The Merge button has the keyboard (reached with Tab).
    pub focused: bool,
}

#[derive(Debug, Default)]
pub(crate) struct PrList {
    pub open: bool,
    pub show: Option<PrShow>,
    pub folder_id: String,
    pub seq: u64,
    pub loading: bool,
    pub rows: Option<PrListResult>,
    pub failed: bool,
    pub read_ms: u64,
}

/// What "Push, then open" opens once the push is done.
#[derive(Debug, Clone, Default)]
pub(crate) struct NewPr {
    pub title: String,
    pub body: String,
    pub base: String,
    pub draft: bool,
}

#[derive(Debug, Default)]
pub(crate) struct PrForm {
    pub open: bool,
    pub title: AskField,
    pub body: AskField,
    pub then_open: Option<NewPr>,
}

/// The open thread's source control.
#[derive(Debug, Default)]
pub(crate) struct Panel {
    pub thread_id: String,
    pub seq: u64,
    pub loading: bool,
    pub status: Option<GitStatus>,
    pub failed: bool,
    /// Paths the user unticked: every file starts ticked.
    pub unticked: BTreeSet<String>,
    pub message: AskField,
    /// The files changed while the user looked: Commit waits for Reload.
    pub stale: bool,
    /// A request refused before it ran: its reason word and lines.
    pub refused: Option<(String, Vec<String>)>,
    pub job: Option<Job>,
    pub fetching: bool,
    pub branches: BranchMenu,
    pub pr: Option<PrViewResult>,
    pub pr_loading: bool,
    pub pr_seq: u64,
    pub form: PrForm,
    pub merge: Option<Merge>,
    /// "New branch first…": the push follows the switch.
    pub push_after_switch: bool,
    /// The remotes to choose from before pushing.
    pub pick_remote: Vec<String>,
}

/// `thread.start` with a worktree, while it runs or after it failed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Creating {
    pub running: bool,
    pub lines: Vec<String>,
    pub reason: String,
}

/// Everything source control keeps, in the bridge's state.
#[derive(Debug, Default)]
pub(crate) struct Source {
    /// The host has `git`.
    pub cap: bool,
    /// The panel beside the transcript is open.
    pub shown: bool,
    pub panel: Panel,
    /// The last `git.changed` by thread: ahead, behind and the pull request for the strip.
    pub changed: HashMap<String, GitChangedEvent>,
    /// Quote chips by thread.
    pub quotes: HashMap<String, Vec<Quote>>,
    pub gh: Option<GhState>,
    pub prs: PrList,
    pub worktrees: Option<Vec<Worktree>>,
    /// The merge method last used, by repository.
    pub methods: HashMap<String, MergeMethod>,
    /// Branches a new worktree may start from (and which is checked out), for the folder named.
    pub bases: (String, Vec<(String, bool)>),
    pub creating: Option<Creating>,
}

impl Source {
    /// The open thread changed: the panel starts over for another thread.
    fn follow_thread(&mut self, thread_id: &str) {
        if self.panel.thread_id != thread_id {
            self.panel = Panel { thread_id: thread_id.to_owned(), ..Default::default() };
        }
    }
}

// ---- The page's JSON ----

fn status_json(st: &GitStatus, unticked: &BTreeSet<String>) -> Value {
    let files: Vec<Value> = st
        .files
        .iter()
        .map(|f| {
            let conflict = f.status == GitChange::Conflict;
            json!({
                "path": f.path,
                "from": f.from.clone().unwrap_or_default(),
                "status": word(&f.status),
                "folder": f.folder,
                "adds": f.adds.map_or(-1, i64::from),
                "dels": f.dels.map_or(-1, i64::from),
                "binary": f.binary,
                "ticked": !conflict && !unticked.contains(&f.path),
                "conflict": conflict,
            })
        })
        .collect();
    let total = st.total.max(u32::try_from(st.files.len()).unwrap_or(u32::MAX));
    json!({
        "repo": word(&st.repo),
        "branch": st.branch.clone().unwrap_or_default(),
        "detached": st.detached.clone().unwrap_or_default(),
        "unborn": st.unborn,
        "upstream": st.upstream.clone().unwrap_or_default(),
        "ahead": st.ahead.map_or(-1, i64::from),
        "behind": st.behind.map_or(-1, i64::from),
        "remote": st.remote.clone().unwrap_or_default(),
        "remotes": st.remotes.iter().map(|r| json!({ "name": r.name, "label": r.label })).collect::<Vec<_>>(),
        "defaultBranch": st.default_branch.clone().unwrap_or_default(),
        "onDefault": st.on_default,
        "doing": st.doing.as_ref().map(word).unwrap_or_default(),
        "locked": st.locked,
        "files": files,
        "more": st.more,
        "total": total,
        "unlisted": (total as usize).saturating_sub(st.files.len()),
        "adds": st.adds,
        "dels": st.dels,
        "conflicts": st.conflicts,
        "last": st.last.as_ref().map_or_else(|| json!({}), |l| json!({ "oid7": l.oid7, "subject": l.subject, "ms": l.ms })),
        "worktree": st.worktree.as_ref().map_or_else(|| json!({}), |w| json!({ "id": w.id, "branch": w.branch, "label": w.label, "missing": w.missing })),
        "checkedMs": st.checked_ms,
        "fetchedMs": st.fetched_ms.unwrap_or(0),
        "github": st.github.as_ref().map_or_else(|| json!({}), |g| json!({ "repo": g.repo, "host": g.host })),
    })
}

/// The strip in a thread's header: the status when it was read, else the
/// thread's own summary and its last `git.changed`.
fn strip_json(src: &Source, info: &ThreadInfo) -> Value {
    let brief = |p: &api::PrBrief| json!({ "number": p.number, "state": word(&p.state), "draft": p.draft, "pass": p.pass, "fail": p.fail, "pending": p.pending });
    let pr = src.panel.pr.as_ref().filter(|_| src.panel.thread_id == info.id).and_then(|v| v.pr.as_ref());
    let changed = src.changed.get(&info.id);
    let pr = pr.map(|p| brief(&p.brief())).or_else(|| changed.and_then(|c| c.pr.as_ref()).map(brief));
    if let Some(st) = src.panel.status.as_ref().filter(|_| src.panel.thread_id == info.id) {
        if st.repo != RepoState::Ok {
            return json!({ "repo": word(&st.repo) });
        }
        return json!({
            "repo": word(&st.repo),
            "branch": st.branch.clone().unwrap_or_default(),
            "detached": st.detached.clone().unwrap_or_default(),
            "worktree": st.worktree.is_some(),
            "changed": st.total.max(u32::try_from(st.files.len()).unwrap_or(u32::MAX)),
            "ahead": st.ahead.unwrap_or(0),
            "behind": st.behind.unwrap_or(0),
            "locked": st.locked,
            "pr": pr.unwrap_or_else(|| json!({})),
        });
    }
    let branch = changed.and_then(|c| c.branch.clone()).or_else(|| info.branch.clone());
    let Some(branch) = branch else { return json!({}) };
    json!({
        "repo": "ok",
        "branch": branch,
        "detached": "",
        "worktree": info.worktree.is_some(),
        "changed": changed.map(|c| c.changed).or(info.changed).unwrap_or(0),
        "ahead": changed.and_then(|c| c.ahead).unwrap_or(0),
        "behind": changed.and_then(|c| c.behind).unwrap_or(0),
        "locked": false,
        "pr": pr.unwrap_or_else(|| json!({})),
    })
}

/// The open thread's panel, strip and dialogs (`{}` when no thread is open).
pub(super) fn git_json(s: &BridgeState) -> Value {
    let src = &s.source;
    let Some(open) = s.open.as_ref().filter(|o| o.kind == OpenKind::Thread) else { return json!({}) };
    let Some(info) = s.threads.get(&open.id) else { return json!({}) };
    let p = &src.panel;
    let mine = p.thread_id == info.id;
    let status = p.status.as_ref().filter(|_| mine);
    let busy = info.state.is_running();
    let job = p.job.as_ref().filter(|_| mine);
    let job_running = job.is_some_and(running);
    let picked = status.map_or(0, |st| picked_paths(st, &p.unticked).len());
    let blocked = if p.stale {
        Some("changed")
    } else {
        status.map_or(Some("loading"), |st| commit_blocked(st, picked, busy, job_running))
    };
    let view = p.pr.as_ref().filter(|_| mine);
    let merge = p.merge.as_ref().map_or_else(
        || json!({}),
        |m| {
            let allowed: Vec<&str> = m.methods.map_or_else(Vec::new, |x| {
                [("squash", x.squash), ("merge", x.merge), ("rebase", x.rebase)]
                    .into_iter()
                    .filter_map(|(w, on)| on.then_some(w))
                    .collect()
            });
            json!({
                "number": m.number,
                "oid7": m.head.get(..7).unwrap_or_default(),
                "loading": m.methods.is_none() && !m.failed,
                "failed": m.failed,
                "methods": allowed,
                "method": m.method.as_ref().map(word).unwrap_or_default(),
                "looked": m.looked,
            })
        },
    );
    let branches = &p.branches;
    json!({
        "available": src.cap,
        "shown": src.shown,
        "threadId": info.id,
        "agentName": info.agent.display_name(),
        "title": info.title,
        "running": busy,
        "readonly": info.readonly.is_some(),
        "loading": status.is_none() && (p.loading || !mine),
        "failed": status.is_none() && p.failed,
        "strip": strip_json(src, info),
        "status": status.map_or_else(|| json!({}), |st| status_json(st, &p.unticked)),
        "picked": picked,
        "commit": {
            "blocked": blocked.unwrap_or_default(),
            "message": p.message.json(),
            "subjectLong": subject_long(&p.message.text),
            "all": status.is_some_and(|st| st.more),
        },
        "refused": p.refused.as_ref().filter(|_| mine).map_or_else(|| json!({}), |(w, lines)| json!({ "reason": w, "lines": lines })),
        "job": job.map_or_else(|| json!({}), job_json),
        "fetching": p.fetching,
        "pickRemote": p.pick_remote,
        "branches": {
            "open": branches.open,
            "query": branches.query,
            "loading": branches.loading && branches.list.is_none(),
            "pushAfter": p.push_after_switch,
            "more": branches.list.as_ref().is_some_and(|l| l.more),
            "list": branches.list.as_ref().map_or_else(Vec::new, |l| l.branches.iter().map(|b| json!({
                "name": b.name,
                "remote": b.remote.clone().unwrap_or_default(),
                "current": b.current,
                "worktree": b.worktree.clone().unwrap_or_default(),
                "lastMs": b.last_ms,
            })).collect::<Vec<_>>()),
        },
        "pr": {
            "loading": p.pr_loading && view.is_none(),
            "refreshing": p.pr_loading,
            "pr": view.and_then(|v| v.pr.as_ref()).map_or_else(|| json!({}), pr_json),
            "gh": view.map(|v| &v.gh).or(src.gh.as_ref()).map_or_else(|| json!({}), gh_json),
            "why": view.and_then(|v| v.why.as_ref()).map(word).unwrap_or_default(),
            "github": view.and_then(|v| v.github.as_ref()).map_or_else(|| json!({}), |g| json!({ "repo": g.repo, "host": g.host })),
        },
        "form": { "open": p.form.open, "title": p.form.title.json(), "body": p.form.body.json() },
        "merge": merge,
        // "Changes and comments" on the card (phase 6).
        "prDetail": s.review.cap,
        "quotes": src.quotes.get(&info.id).map_or_else(Vec::new, |q| q.iter().map(|q| json!({
            "path": q.path, "first": q.first, "last": q.last,
        })).collect::<Vec<_>>()),
    })
}

/// The Pull Requests view (`{}` while it's closed).
pub(super) fn prs_json(s: &BridgeState) -> Value {
    let list = &s.source.prs;
    if !list.open {
        return json!({});
    }
    let rows = list.rows.as_ref();
    let gh = rows.map(|r| &r.gh).or(s.source.gh.as_ref());
    let hosts = gh.map(|g| g.hosts.as_slice()).unwrap_or_default();
    json!({
        "show": list.show.as_ref().map(word).unwrap_or_default(),
        "folderId": list.folder_id,
        "loading": list.loading && rows.is_none(),
        "refreshing": list.loading,
        "failed": list.failed && rows.is_none(),
        "readMs": list.read_ms,
        // A row opens the pull request's view (phase 6).
        "detail": s.review.cap,
        "gh": gh.map_or_else(|| json!({}), gh_json),
        "more": rows.is_some_and(|r| r.more),
        "rows": rows.map_or_else(Vec::new, |r| r.prs.iter().map(|p| {
            let row = &p.row;
            json!({
                "number": row.number,
                "title": row.title,
                "repo": row.repo,
                "state": word(&row.state),
                "draft": row.draft,
                "updatedMs": row.updated_ms,
                "url": row.url,
                "canOpen": safe_url(&row.url, hosts),
                "thread": p.thread_id.clone().filter(|t| s.threads.contains_key(t)).unwrap_or_default(),
                "checks": row.checks.as_ref().map_or_else(|| json!({}), |c| json!({ "pass": c.pass, "fail": c.fail, "pending": c.pending })),
            })
        }).collect::<Vec<_>>()),
    })
}

/// Setup's Source control, and the new-thread form's worktree fields.
pub(super) fn setup_json(s: &BridgeState) -> Value {
    let src = &s.source;
    json!({
        "available": src.cap,
        "gh": src.gh.as_ref().map_or_else(|| json!({}), gh_json),
        "worktreesRead": src.worktrees.is_some(),
        "worktrees": src.worktrees.as_deref().unwrap_or_default().iter().map(|w| json!({
            "id": w.id,
            "folder": s.folders.iter().find(|f| f.id == w.folder_id).map(|f| f.label.clone()).unwrap_or_default(),
            "label": w.label,
            "branch": w.branch,
            "thread": w.thread_id.as_ref().and_then(|t| s.threads.get(t)).map(|t| t.title.clone()).unwrap_or_default(),
            "missing": w.missing,
            "bytes": w.bytes.unwrap_or(0),
            "over": w.bytes_over,
            "unpushed": w.unpushed.unwrap_or(0),
        })).collect::<Vec<_>>(),
        "basesFolder": src.bases.0,
        "bases": src.bases.1.iter().map(|(name, current)| json!({ "name": name, "current": current })).collect::<Vec<_>>(),
        "creating": src.creating.as_ref().map_or_else(|| json!({}), |c| json!({
            "running": c.running, "lines": c.lines, "reason": c.reason,
        })),
    })
}

// ---- Talking to the host ----

/// A refusal's reason word (`data.reason`), else one from its code.
pub(super) fn reason_of(e: &ClientError) -> String {
    match e {
        ClientError::Remote(rpc) => {
            let by_code = match rpc.code {
                ErrorCode::Busy => "busy",
                ErrorCode::Denied => "denied",
                ErrorCode::Unavailable => "unavailable",
                ErrorCode::NotFound => "not_found",
                ErrorCode::Conflict => "changed",
                _ => "failed",
            };
            let given = rpc.data.as_ref().and_then(|d| d.get("reason")).and_then(Value::as_str);
            given.unwrap_or(by_code).chars().take(24).collect()
        }
        e if e.is_unreachable() => "unreachable".to_owned(),
        _ => "failed".to_owned(),
    }
}

/// A refusal's lines or names (`data.lines`, `data.names`).
fn data_list(e: &ClientError, key: &str) -> Vec<String> {
    let ClientError::Remote(rpc) = e else { return Vec::new() };
    rpc.data
        .as_ref()
        .and_then(|d| d.get(key))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).take(KEPT_LINES).map(str::to_owned).collect())
        .unwrap_or_default()
}

fn by() -> Option<Origin> {
    Some(Origin::launcher())
}

/// Runs `work` with the connected host.
pub(super) fn with_client<F, Fut>(work: F)
where
    F: FnOnce(Client) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send,
{
    if core_host::try_host().is_none() {
        return;
    }
    core_host::spawn(async move {
        if let Some(client) = connected_client().await {
            work(client).await;
        }
    });
}

/// A refusal that has no place in the panel, as a message.
pub(super) fn tell(reason: &str) {
    let text = match reason {
        "unreachable" => tr!("Agents", "Couldn't reach the agent host"),
        "denied" => tr!("Agents", "That is done from G7 Link on this PC."),
        "unavailable" => outdated_host(),
        "busy" | "thread_busy" | "shared_busy" | "git_busy" | "jobs" => {
            tr!("Agents", "Wait until it finishes.")
        }
        "dirty" => tr!("Agents", "This worktree has uncommitted changes. Commit or delete them first."),
        "bad_name" => tr!("Agents", "That name can't be a branch."),
        "exists" => tr!("Agents", "A branch with that name exists."),
        "limit" => tr!("Agents", "Too many worktrees. Remove one in Setup first."),
        _ => tr!("Agents", "Something went wrong. Try again."),
    };
    crate::bridge::app::show_message(text);
}

/// The panel's thread, while it is the open one.
fn panel_thread() -> Option<String> {
    let s = lock();
    let open = s.open.as_ref().filter(|o| o.kind == OpenKind::Thread)?;
    (s.source.panel.thread_id == open.id).then(|| open.id.clone())
}

// ---- Status ----

/// Reads the open thread's status; `pr`: its pull request too.
pub(crate) fn read_status(pr: bool) {
    let Some((thread_id, seq)) = ({
        let mut s = lock();
        let id = s.open.as_ref().filter(|o| o.kind == OpenKind::Thread).map(|o| o.id.clone());
        let cap = s.source.cap;
        match id {
            Some(id) if cap => {
                s.source.follow_thread(&id);
                let p = &mut s.source.panel;
                p.seq += 1;
                p.loading = true;
                Some((id, p.seq))
            }
            _ => None,
        }
    }) else {
        return;
    };
    changed();
    with_client(move |client| async move {
        let params = GitStatusParams { thread_id: Some(thread_id.clone()), folder_id: None };
        let read = client.git_status(params).await;
        {
            let mut s = lock();
            let p = &mut s.source.panel;
            if p.thread_id != thread_id || p.seq != seq {
                return;
            }
            p.loading = false;
            match read {
                Ok(st) => {
                    // A file that went is forgotten; one that came starts ticked.
                    p.unticked.retain(|path| st.files.iter().any(|f| &f.path == path));
                    p.failed = false;
                    p.status = Some(st);
                }
                Err(e) => {
                    tracing::debug!(error = %e, "git status wasn't read");
                    p.failed = true;
                }
            }
        }
        changed();
        if pr {
            read_pr(false);
        }
    });
}

/// `git.changed`: the strip changes at once; the open thread's status is
/// read 300 ms after the last one.
pub(crate) fn git_changed(event: GitChangedEvent) {
    let seq = {
        let mut s = lock();
        let open = s.open.as_ref().is_some_and(|o| o.kind == OpenKind::Thread && o.id == event.thread_id);
        let thread = event.thread_id.clone();
        s.source.changed.insert(thread.clone(), event);
        if s.source.changed.len() > KNOWN_THREADS {
            let known: HashSet<String> = s.threads.keys().cloned().collect();
            s.source.changed.retain(|k, _| known.contains(k) || *k == thread);
        }
        open.then(|| {
            s.source.panel.seq += 1;
            s.source.panel.seq
        })
    };
    changed();
    let Some(seq) = seq else { return };
    if core_host::try_host().is_none() {
        return;
    }
    core_host::spawn(async move {
        tokio::time::sleep(CHANGED_QUIET).await;
        if lock().source.panel.seq == seq {
            read_status(false);
        }
    });
}

/// `git.progress`: the shown job takes the new lines, or is read whole.
pub(crate) fn git_progress(job: Job) {
    let read = {
        let mut s = lock();
        let p = &mut s.source.panel;
        match p.job.as_mut().filter(|shown| shown.id == job.id) {
            Some(shown) => take_progress(shown, job).then(|| shown.id.clone()),
            None => None,
        }
    };
    changed();
    if let Some(id) = read {
        with_client(move |client| async move {
            if let Ok(job) = client.git_job(id).await {
                let mut s = lock();
                if let Some(shown) = s.source.panel.job.as_mut().filter(|j| j.id == job.id) {
                    *shown = job;
                }
                drop(s);
                changed();
            }
        });
    }
}

/// A thread's state arrived: the answer to an ask (`wrote`) fills its field.
pub(crate) fn thread_state(info: &ThreadInfo) {
    let Some(wrote) = info.wrote.as_ref() else { return };
    if take_wrote(&mut lock().source.panel, &info.id, wrote) {
        changed();
    }
}

/// `wrote` onto the panel's fields; true when anything changed.
pub(crate) fn take_wrote(p: &mut Panel, thread_id: &str, wrote: &Wrote) -> bool {
    if p.thread_id != thread_id {
        return false;
    }
    match wrote.purpose {
        Purpose::Commit => {
            let before = p.message.taken;
            p.message.answered(wrote.turn, wrote.at_ms, &wrote.text);
            before != p.message.taken
        }
        Purpose::Pr => {
            let before = p.form.title.taken;
            if p.form.title.answered(wrote.turn, wrote.at_ms, &wrote.text) {
                let (title, body) = split_title(&wrote.text);
                p.form.title.put(title);
                p.form.body.put(body);
            }
            let took = before != p.form.title.taken;
            if took {
                p.form.body.gave_up();
                p.form.body.asked_ms = None;
            }
            took
        }
        // A review is an ordinary message: nothing fills a field.
        Purpose::Review => false,
    }
}

/// A turn ended: a field stops showing "Asking…". An answer written since
/// the ask still fills it when it comes after the turn's end.
pub(crate) fn turn_ended(thread_id: &str) {
    let mut s = lock();
    let p = &mut s.source.panel;
    if p.thread_id == thread_id && (p.message.asking || p.form.title.asking) {
        p.message.gave_up();
        p.form.title.gave_up();
        p.form.body.gave_up();
        drop(s);
        changed();
    }
}

/// The host connected: whether it has `git`. A thread already open reads its status.
pub(crate) fn connected(cap: bool) {
    lock().source.cap = cap;
    if cap {
        let shown = lock().source.shown;
        read_status(shown);
    }
}

// ---- The panel ----

/// Opens or closes the panel. Opening reads the status, fetches, and reads
/// the pull request: the user asked to look.
pub fn panel(open: bool) {
    if std::mem::replace(&mut lock().source.shown, open) == open {
        return;
    }
    if open {
        // One panel at a time beside the transcript.
        super::scripts::give_way();
    }
    changed();
    if open {
        read_status(true);
        fetch();
    }
}

/// Refresh: fetch, the status, and the pull request past the cache.
pub fn refresh() {
    read_status(false);
    fetch();
    read_pr(true);
}

/// A quiet fetch: it shows as "Checked" moving on.
fn fetch() {
    let Some(thread_id) = panel_thread() else { return };
    {
        let mut s = lock();
        let p = &mut s.source.panel;
        if p.fetching || p.job.as_ref().is_some_and(running) {
            return;
        }
        p.fetching = true;
    }
    changed();
    with_client(move |client| async move {
        if let Ok(mut job) = client.git_fetch(GitFetchParams { thread_id: thread_id.clone(), by: by() }).await
        {
            while running(&job) {
                tokio::time::sleep(JOB_EVERY).await;
                match client.git_job(job.id.clone()).await {
                    Ok(next) => job = next,
                    Err(_) => break,
                }
            }
        }
        {
            let mut s = lock();
            if s.source.panel.thread_id == thread_id {
                s.source.panel.fetching = false;
            }
        }
        read_status(false);
    });
}

pub fn tick(path: String, on: bool) {
    {
        let mut s = lock();
        let p = &mut s.source.panel;
        if on {
            p.unticked.remove(&path);
        } else if p.status.as_ref().is_some_and(|st| st.files.iter().any(|f| f.path == path)) {
            p.unticked.insert(path);
        }
    }
    changed();
}

pub fn tick_all(on: bool) {
    {
        let mut s = lock();
        let p = &mut s.source.panel;
        p.unticked = match (on, p.status.as_ref()) {
            (false, Some(st)) => st.files.iter().map(|f| f.path.clone()).collect(),
            _ => BTreeSet::new(),
        };
    }
    changed();
}

/// The commit message, or the pull request's title or description, changed.
pub fn field_edited(which: &str, text: String) {
    {
        let mut s = lock();
        let p = &mut s.source.panel;
        match which {
            "message" => p.message.typed(text),
            "title" => p.form.title.typed(text),
            "body" => p.form.body.typed(text),
            _ => return,
        }
    }
    changed();
}

/// "Use the thread's title" for the commit message.
pub fn use_title() {
    {
        let mut s = lock();
        let Some(title) = s.open.as_ref().and_then(|o| s.threads.get(&o.id)).map(|t| t.title.clone()) else {
            return;
        };
        s.source.panel.message.put(title);
    }
    changed();
}

/// "Use <agent>'s message": the answer kept aside because the user typed.
pub fn use_offer(which: &str) {
    {
        let mut s = lock();
        let p = &mut s.source.panel;
        if which == "pr" {
            if let Some(text) = p.form.title.offer.take() {
                let (title, body) = split_title(&text);
                p.form.title.put(title);
                p.form.body.put(body);
            }
        } else if let Some(text) = p.message.offer.take() {
            p.message.put(text);
        }
    }
    changed();
}

/// "Ask <agent> to write it": a visible message in the thread (§SC5.4).
pub fn ask(purpose: &str) {
    let purpose = match purpose {
        "commit" => Purpose::Commit,
        "pr" => Purpose::Pr,
        _ => return,
    };
    let Some(thread_id) = panel_thread() else { return };
    {
        let mut s = lock();
        let p = &mut s.source.panel;
        if purpose == Purpose::Commit {
            p.message.ask(now_ms());
        } else {
            p.form.title.ask(now_ms());
            p.form.body.ask(now_ms());
        }
    }
    changed();
    with_client(move |client| async move {
        let params = ThreadSendParams::ask(thread_id, purpose, Origin::launcher());
        if let Err(e) = client.thread_send_with(params).await {
            {
                let mut s = lock();
                let p = &mut s.source.panel;
                p.message.gave_up();
                p.form.title.gave_up();
                p.form.body.gave_up();
            }
            changed();
            tell(&reason_of(&e));
        }
    });
}

// ---- Jobs ----

type Then = Box<dyn FnOnce() + Send>;

/// Shows a job and follows it to its end (events bring its lines; it's read
/// now and then in case one was lost); then the status is read, and the
/// pull request after a push, an open or a merge. `then` runs when it's done.
fn follow(client: Client, job: Job, then: Option<Then>) {
    let id = job.id.clone();
    let thread_id = job.thread_id.clone();
    {
        let mut s = lock();
        if s.source.panel.thread_id == thread_id {
            s.source.panel.job = Some(job);
        }
    }
    changed();
    core_host::spawn(async move {
        let mut last = None;
        loop {
            let shown = lock().source.panel.job.clone().filter(|j| j.id == id);
            if let Some(job) = shown.as_ref().filter(|j| !running(j)) {
                last = Some(job.clone());
                break;
            }
            tokio::time::sleep(JOB_EVERY).await;
            match client.git_job(id.clone()).await {
                Ok(job) => {
                    let done = !running(&job);
                    {
                        let mut s = lock();
                        if s.source.panel.thread_id == job.thread_id {
                            s.source.panel.job = Some(job.clone());
                        }
                    }
                    changed();
                    if done {
                        last = Some(job);
                        break;
                    }
                }
                Err(e) if e.is_unreachable() => {}
                Err(e) => {
                    tracing::debug!(error = %e, "a job wasn't read");
                    break;
                }
            }
        }
        let Some(job) = last else { return };
        if job.kind == JobKind::Commit && job.phase == JobPhase::Done {
            let mut s = lock();
            let p = &mut s.source.panel;
            if p.thread_id == job.thread_id {
                p.message = AskField { set_n: p.message.set_n, ..Default::default() };
                p.message.put(String::new());
                p.unticked.clear();
            }
        }
        if job.kind == JobKind::Push && job.phase != JobPhase::Done {
            // "Push, then open" stops at a push that didn't go through.
            lock().source.panel.form.then_open = None;
        }
        read_status(false);
        if matches!(job.kind, JobKind::Push | JobKind::PrCreate | JobKind::PrMerge) {
            read_pr(true);
        }
        if job.phase == JobPhase::Done
            && let Some(then) = then
        {
            then();
        }
    });
}

/// A refusal before anything ran, in the panel's state line.
fn refused(e: &ClientError) {
    let reason = reason_of(e);
    {
        let mut s = lock();
        let p = &mut s.source.panel;
        match reason.as_str() {
            "changed" => p.stale = true,
            "pick_remote" => p.pick_remote = data_list(e, "names"),
            _ => {}
        }
        p.form.then_open = None;
        p.refused = Some((reason.clone(), data_list(e, "lines")));
    }
    changed();
    if matches!(reason.as_str(), "unreachable" | "denied" | "unavailable") {
        tell(&reason);
    }
}

/// Commits the ticked files (`all`: every file, past 2,000). The status is
/// read first: when the files moved since they were shown, nothing is
/// committed and the panel offers Reload.
pub fn commit(all: bool) {
    let Some(thread_id) = panel_thread() else { return };
    let params = {
        let mut s = lock();
        let busy = s.threads.get(&thread_id).is_some_and(|t| t.state.is_running());
        let p = &mut s.source.panel;
        let Some(st) = p.status.as_ref() else { return };
        let paths = picked_paths(st, &p.unticked);
        if p.stale || commit_blocked(st, paths.len(), busy, p.job.as_ref().is_some_and(running)).is_some() {
            return;
        }
        let message = p.message.text.trim().to_owned();
        if message.is_empty() || message.len() > api::MAX_COMMIT_MESSAGE {
            return;
        }
        p.refused = None;
        let all = all && st.more;
        let params = GitCommitParams {
            thread_id: thread_id.clone(),
            paths: if all { Vec::new() } else { paths },
            all,
            message,
            review: None,
            by: by(),
        };
        (params, st.fingerprint.clone())
    };
    let (params, seen) = params;
    with_client(move |client| async move {
        let now = client.git_status(GitStatusParams { thread_id: Some(thread_id), folder_id: None }).await;
        if now.as_ref().is_ok_and(|st| st.fingerprint != seen) {
            {
                let mut s = lock();
                s.source.panel.stale = true;
                s.source.panel.refused = Some(("changed".into(), Vec::new()));
            }
            changed();
            return;
        }
        match client.git_commit(params).await {
            Ok(job) => follow(client, job, None),
            Err(e) => refused(&e),
        }
    });
}

/// "Reload" after the files changed.
pub fn reload() {
    {
        let mut s = lock();
        s.source.panel.stale = false;
        s.source.panel.refused = None;
    }
    read_status(false);
}

/// Stop, while a commit or a push runs.
pub fn stop() {
    let Some(job) = lock().source.panel.job.as_ref().filter(|j| running(j)).map(|j| j.id.clone()) else {
        return;
    };
    with_client(move |client| async move {
        if let Err(e) = client.git_stop(GitJobParams { job, by: by() }).await {
            tracing::debug!(error = %e, "a job wasn't stopped");
        }
    });
}

/// Push, to `remote` ("" for the branch's own).
pub fn push(remote: String) {
    let Some(thread_id) = panel_thread() else { return };
    {
        let mut s = lock();
        let p = &mut s.source.panel;
        if p.job.as_ref().is_some_and(running) {
            return;
        }
        p.refused = None;
        p.pick_remote.clear();
    }
    changed();
    let params = GitPushParams { thread_id, remote: (!remote.is_empty()).then_some(remote), by: by() };
    let then =
        lock().source.panel.form.then_open.clone().map(|pr| Box::new(move || pr_create_now(pr)) as Then);
    with_client(move |client| async move {
        match client.git_push(params).await {
            Ok(job) => follow(client, job, then),
            Err(e) => refused(&e),
        }
    });
}

// ---- Branches ----

/// The branch menu opens (it reads the branches) or closes.
pub fn branch_menu(open: bool) {
    {
        let mut s = lock();
        let p = &mut s.source.panel;
        p.branches.open = open;
        p.branches.query.clear();
        if !open {
            p.push_after_switch = false;
        }
    }
    changed();
    if open {
        read_branches(Duration::ZERO);
    }
}

pub fn branch_query(query: String) {
    lock().source.panel.branches.query = query.chars().take(api::MAX_BRANCH_QUERY).collect();
    read_branches(BRANCH_QUIET);
}

fn read_branches(wait: Duration) {
    let Some(thread_id) = panel_thread() else { return };
    let (seq, query) = {
        let mut s = lock();
        let b = &mut s.source.panel.branches;
        b.seq += 1;
        b.loading = true;
        (b.seq, b.query.clone())
    };
    changed();
    with_client(move |client| async move {
        tokio::time::sleep(wait).await;
        if lock().source.panel.branches.seq != seq {
            return;
        }
        let params = GitBranchesParams {
            thread_id: Some(thread_id),
            folder_id: None,
            query: (!query.is_empty()).then_some(query),
        };
        let read = client.git_branches(params).await;
        {
            let mut s = lock();
            let b = &mut s.source.panel.branches;
            if b.seq != seq {
                return;
            }
            b.loading = false;
            match read {
                Ok(list) => b.list = Some(list),
                Err(e) => tracing::debug!(error = %e, "branches weren't read"),
            }
        }
        changed();
    });
}

/// Switches to `branch` (`create`: a new one; `carry`: with the changes,
/// after the dialog). After "New branch first…", the push follows.
pub fn switch(branch: String, create: bool, carry: bool) {
    let Some(thread_id) = panel_thread() else { return };
    if create && !branch_name_ok(&branch) {
        return;
    }
    let (track, push_after) = {
        let mut s = lock();
        let p = &mut s.source.panel;
        if p.job.as_ref().is_some_and(running) {
            return;
        }
        p.refused = None;
        p.branches.open = false;
        let track = !create
            && p.branches
                .list
                .as_ref()
                .is_some_and(|l| l.branches.iter().any(|b| b.name == branch && b.remote.is_some()));
        (track, std::mem::take(&mut p.push_after_switch))
    };
    changed();
    let params = GitSwitchParams { thread_id, branch, create, track, carry, by: by() };
    let then = push_after.then(|| Box::new(|| push(String::new())) as Then);
    with_client(move |client| async move {
        match client.git_switch(params).await {
            Ok(job) => follow(client, job, then),
            Err(e) => refused(&e),
        }
    });
}

/// "New branch first…": the branch menu opens on its name field, and the
/// push follows the switch.
pub fn new_branch_then_push() {
    {
        let mut s = lock();
        s.source.panel.push_after_switch = true;
        s.source.panel.branches.open = true;
    }
    changed();
    read_branches(Duration::ZERO);
}

// ---- The pull request ----

/// Reads the branch's pull request (`refresh`: past the host's cache).
pub(crate) fn read_pr(refresh: bool) {
    let Some(thread_id) = panel_thread() else { return };
    let seq = {
        let mut s = lock();
        if !s.source.shown {
            return;
        }
        let p = &mut s.source.panel;
        p.pr_seq += 1;
        p.pr_loading = true;
        p.pr_seq
    };
    changed();
    with_client(move |client| async move {
        let read = client.pr_view(PrViewParams { thread_id: thread_id.clone(), refresh }).await;
        {
            let mut s = lock();
            if s.source.panel.thread_id != thread_id || s.source.panel.pr_seq != seq {
                return;
            }
            s.source.panel.pr_loading = false;
            match read {
                Ok(view) => {
                    s.source.gh = Some(view.gh.clone());
                    s.source.panel.pr = Some(view);
                }
                Err(e) => tracing::debug!(error = %e, "the pull request wasn't read"),
            }
        }
        changed();
    });
}

/// The pull request dialog opens (the thread's title offered) or closes.
pub fn pr_form(open: bool) {
    {
        let mut s = lock();
        let title =
            s.open.as_ref().and_then(|o| s.threads.get(&o.id)).map(|t| t.title.clone()).unwrap_or_default();
        let form = &mut s.source.panel.form;
        form.open = open;
        if open && form.title.text.is_empty() && !form.title.asking {
            form.title.put(title);
        }
    }
    changed();
}

/// Opens a pull request; when the branch isn't pushed, after a push.
pub fn pr_create(title: String, body: String, base: String, draft: bool) {
    let title = title.trim().to_owned();
    if title.is_empty() || title.len() > api::MAX_PR_TITLE || body.len() > api::MAX_PR_BODY {
        return;
    }
    let pr = NewPr { title, body, base, draft };
    let pushed = lock()
        .source
        .panel
        .status
        .as_ref()
        .is_some_and(|st| st.upstream.is_some() && st.ahead.unwrap_or(0) == 0);
    if pushed {
        pr_create_now(pr);
    } else {
        lock().source.panel.form.then_open = Some(pr);
        push(String::new());
    }
}

fn pr_create_now(pr: NewPr) {
    let Some(thread_id) = panel_thread() else { return };
    lock().source.panel.form.then_open = None;
    let params = PrCreateParams {
        thread_id,
        title: pr.title,
        body: pr.body,
        base: (!pr.base.is_empty()).then_some(pr.base),
        draft: pr.draft,
        by: by(),
    };
    let done: Then = Box::new(|| {
        {
            let mut s = lock();
            let form = &mut s.source.panel.form;
            form.open = false;
            form.title.put(String::new());
            form.body.put(String::new());
        }
        changed();
    });
    with_client(move |client| async move {
        match client.pr_create(params).await {
            Ok(job) => follow(client, job, Some(done)),
            Err(e) => refused(&e),
        }
    });
}

fn repo_of(p: &Panel) -> String {
    p.pr.as_ref().and_then(|v| v.github.as_ref()).map(|g| g.repo.clone()).unwrap_or_default()
}

fn method_word(m: MergeMethod) -> &'static str {
    match m {
        MergeMethod::Squash => "squash",
        MergeMethod::Merge => "merge",
        MergeMethod::Rebase => "rebase",
    }
}

fn allows(methods: PrMethodsResult, m: MergeMethod) -> bool {
    match m {
        MergeMethod::Squash => methods.squash,
        MergeMethod::Merge => methods.merge,
        MergeMethod::Rebase => methods.rebase,
    }
}

/// The merge dialog opens: it reads the methods the repository allows.
pub fn merge_open() {
    let Some(thread_id) = panel_thread() else { return };
    let repo = {
        let mut s = lock();
        let p = &mut s.source.panel;
        let Some(pr) = p.pr.as_ref().and_then(|v| v.pr.as_ref()) else { return };
        let merge = Merge { number: pr.number, head: pr.head_oid.clone(), ..Default::default() };
        p.merge = Some(merge);
        repo_of(p)
    };
    changed();
    with_client(move |client| async move {
        let read = client.pr_methods(thread_id).await;
        {
            let mut s = lock();
            let last = s.source.methods.get(&repo).copied();
            let Some(m) = s.source.panel.merge.as_mut() else { return };
            match read {
                Ok(methods) => {
                    let order = [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase];
                    m.method = last
                        .filter(|l| allows(methods, *l))
                        .or_else(|| order.into_iter().find(|x| allows(methods, *x)));
                    m.methods = Some(methods);
                }
                Err(_) => m.failed = true,
            }
        }
        changed();
    });
}

pub fn merge_method(word: &str) {
    let method = match word {
        "squash" => MergeMethod::Squash,
        "merge" => MergeMethod::Merge,
        "rebase" => MergeMethod::Rebase,
        _ => return,
    };
    if let Some(m) = lock().source.panel.merge.as_mut() {
        m.method = Some(method);
    }
    changed();
}

pub fn merge_looked(on: bool) {
    if let Some(m) = lock().source.panel.merge.as_mut() {
        m.looked = on;
    }
    changed();
}

/// The Merge button gained or lost the keyboard.
pub fn merge_focus(focused: bool) {
    if let Some(m) = lock().source.panel.merge.as_mut() {
        m.focused = focused;
    }
}

pub fn merge_close() {
    if lock().source.panel.merge.take().is_some() {
        changed();
    }
}

/// The merge may run: a method the repository allows; a click, or Enter on
/// the button reached with Tab (never a bare Enter); the tick when checks
/// failed or still run; the head the dialog showed.
pub(crate) fn merge_ready(m: &Merge, pr: &PrInfo, how: &str) -> bool {
    let allowed = m.methods.zip(m.method).is_some_and(|(x, method)| allows(x, method));
    let warned = pr.fail > 0 || pr.pending > 0;
    allowed
        && (how == "click" || (how == "key" && m.focused))
        && (!warned || m.looked)
        && pr.head_oid == m.head
}

/// Merges, pinned to the commit the dialog showed.
pub fn merge(how: &str) {
    let Some(thread_id) = panel_thread() else { return };
    let (params, repo, method) = {
        let mut s = lock();
        let p = &mut s.source.panel;
        let pr = p.pr.as_ref().and_then(|v| v.pr.as_ref());
        let (Some(m), Some(pr)) = (p.merge.as_ref(), pr) else { return };
        let Some(method) = m.method.filter(|_| merge_ready(m, pr, how)) else { return };
        let params = PrMergeParams { thread_id, number: m.number, method, head: m.head.clone(), by: by() };
        let repo = repo_of(p);
        p.merge = None;
        (params, repo, method)
    };
    changed();
    if !repo.is_empty() {
        lock().source.methods.insert(repo, method);
        save_prefs();
    }
    with_client(move |client| async move {
        match client.pr_merge(params).await {
            Ok(job) => follow(client, job, None),
            Err(e) => refused(&e),
        }
    });
}

/// Mark ready (a draft).
pub fn ready() {
    let Some(thread_id) = panel_thread() else { return };
    let Some(number) = lock().source.panel.pr.as_ref().and_then(|v| v.pr.as_ref()).map(|p| p.number) else {
        return;
    };
    with_client(move |client| async move {
        match client.pr_ready(PrReadyParams { thread_id, number, by: by() }).await {
            Ok(pr) => {
                if let Some(view) = lock().source.panel.pr.as_mut() {
                    view.pr = pr.or(view.pr.take());
                }
                changed();
                read_pr(true);
            }
            Err(e) => refused(&e),
        }
    });
}

/// Opens a pull request or a check, when it's on a host the GitHub CLI is
/// signed in to.
pub fn open_url(url: String) {
    let hosts = lock().source.gh.as_ref().map(|g| g.hosts.clone()).unwrap_or_default();
    if safe_url(&url, &hosts) {
        open_link(url);
    }
}

// ---- The Pull Requests list ----

/// Shows the list: "mine", "review" or "folder" (`folder_id`, "" for the open thread's).
pub fn prs_open(show: &str, folder_id: String) {
    let show = match show {
        "mine" => PrShow::Mine,
        "review" => PrShow::Review,
        "folder" => PrShow::Folder,
        _ => return,
    };
    {
        let mut s = lock();
        let folder_id = if folder_id.is_empty() {
            let open = s.open.as_ref().and_then(|o| s.threads.get(&o.id)).map(|t| t.folder_id.clone());
            open.unwrap_or_else(|| s.source.prs.folder_id.clone())
        } else {
            folder_id
        };
        let folder_id = if folder_id.is_empty() {
            s.folders.first().map(|f| f.id.clone()).unwrap_or_default()
        } else {
            folder_id
        };
        let list = &mut s.source.prs;
        if list.show != Some(show) || list.folder_id != folder_id {
            list.rows = None;
        }
        list.open = true;
        list.show = Some(show);
        list.folder_id = folder_id;
    }
    read_prs(false);
}

pub fn prs_close() {
    if std::mem::take(&mut lock().source.prs.open) {
        changed();
    }
}

pub fn prs_refresh() {
    read_prs(true);
}

fn read_prs(refresh: bool) {
    let Some((seq, show, folder_id)) = ({
        let mut s = lock();
        let list = &mut s.source.prs;
        match list.show {
            Some(show) if list.open => {
                list.seq += 1;
                list.loading = true;
                Some((list.seq, show, list.folder_id.clone()))
            }
            _ => None,
        }
    }) else {
        return;
    };
    changed();
    with_client(move |client| async move {
        let folder_id = (show == PrShow::Folder && !folder_id.is_empty()).then_some(folder_id);
        let read = client.pr_list(PrListParams { show, folder_id, refresh }).await;
        {
            let mut s = lock();
            if s.source.prs.seq != seq {
                return;
            }
            s.source.prs.loading = false;
            match read {
                Ok(rows) => {
                    s.source.gh = Some(rows.gh.clone());
                    s.source.prs.failed = false;
                    s.source.prs.read_ms = now_ms();
                    s.source.prs.rows = Some(rows);
                }
                Err(e) => {
                    tracing::debug!(error = %e, "pull requests weren't read");
                    s.source.prs.failed = true;
                }
            }
        }
        changed();
    });
}

// ---- Setup: the GitHub CLI, keep in step, worktrees ----

/// Setup opened, or "Check again": the GitHub CLI and the worktrees.
pub fn setup_read(refresh: bool) {
    if !lock().source.cap {
        return;
    }
    with_client(move |client| async move {
        let gh = client.gh_status(refresh).await;
        let worktrees = client.worktree_list(None).await;
        {
            let mut s = lock();
            if let Ok(gh) = gh {
                s.source.gh = Some(gh);
            }
            if let Ok(list) = worktrees {
                s.source.worktrees = Some(list);
            }
        }
        changed();
    });
}

/// A folder's "Keep in step with the remote".
pub fn set_follow_remote(id: String, on: bool) {
    page_request(
        Request::Setting,
        move |client| async move {
            client
                .folders_set_with(FoldersSetParams { id, follow_remote: Some(on), ..Default::default() })
                .await
        },
        |folder: Folder| {
            let mut s = lock();
            if let Some(f) = s.folders.iter_mut().find(|f| f.id == folder.id) {
                *f = folder;
            }
            drop(s);
            changed();
        },
    );
}

/// Removes a worktree's folder (its branch stays), or forgets a missing one.
pub fn worktree_remove(id: String, forget: bool) {
    with_client(move |client| async move {
        match client.worktree_remove(WorktreeRemoveParams { id: id.clone(), forget, by: by() }).await {
            Ok(_) => {
                if let Some(list) = lock().source.worktrees.as_mut() {
                    list.retain(|w| w.id != id);
                }
                changed();
                setup_read(false);
                read_status(false);
            }
            Err(e) => tell(&reason_of(&e)),
        }
    });
}

/// The open thread's worktree, by its id (for Remove in the panel).
pub fn worktree_remove_open() {
    let id = {
        let s = lock();
        s.open
            .as_ref()
            .and_then(|o| s.threads.get(&o.id))
            .and_then(|t| t.worktree.as_ref())
            .map(|w| w.id.clone())
    };
    if let Some(id) = id {
        worktree_remove(id, false);
    }
}

/// The new-thread form's folder: the branches a worktree may start from.
pub fn read_bases(folder_id: String) {
    if !lock().source.cap || folder_id.is_empty() {
        return;
    }
    with_client(move |client| async move {
        let params = GitBranchesParams { thread_id: None, folder_id: Some(folder_id.clone()), query: None };
        let bases = client
            .git_branches(params)
            .await
            .map(|b| {
                b.branches.into_iter().filter(|b| b.remote.is_none()).map(|b| (b.name, b.current)).collect()
            })
            .unwrap_or_default();
        lock().source.bases = (folder_id, bases);
        changed();
    });
}

/// Starts a thread in a new worktree on `branch`, from `base` ("": the
/// folder's current commit). The host makes the worktree first.
pub(crate) fn start_in_worktree(mut params: ThreadStartParams, branch: String, base: String) {
    if !branch_name_ok(&branch) {
        tell("bad_name");
        return;
    }
    params.worktree = Some(WorktreeWanted { branch: Some(branch), base: (!base.is_empty()).then_some(base) });
    lock().source.creating = Some(Creating { running: true, ..Default::default() });
    changed();
    with_client(move |client| async move {
        let started = client.thread_start_with(params).await;
        let creating = match started {
            Ok(result) => {
                bridge().remember(&result.thread);
                lists_changed();
                open_thread(result.thread.id);
                None
            }
            Err(e) => Some(Creating { running: false, lines: data_list(&e, "lines"), reason: reason_of(&e) }),
        };
        lock().source.creating = creating;
        changed();
    });
}

/// The new-thread form started over: the worktree's last failure goes.
pub fn creating_clear() {
    if lock().source.creating.take().is_some() {
        changed();
    }
}

// ---- Quote chips ----

/// A chip for rows `a..=b` of the diff's file, in the open thread's composer.
pub fn quote_add(a: usize, b: usize) {
    let full = {
        let mut s = lock();
        let Some(thread_id) = s.open.as_ref().filter(|o| o.kind == OpenKind::Thread).map(|o| o.id.clone())
        else {
            return;
        };
        let Some(quote) = s.diff.as_ref().and_then(|d| quote_rows(&d.patch.path, &d.patch.rows, a, b)) else {
            return;
        };
        let quotes = s.source.quotes.entry(thread_id).or_default();
        if quotes.len() >= MAX_QUOTES {
            true
        } else {
            if !quotes.contains(&quote) {
                quotes.push(quote);
            }
            false
        }
    };
    if full {
        crate::bridge::app::show_message(tr!("Agents", "Up to 5 quotes in a message."));
    }
    changed();
}

pub fn quote_remove(index: usize) {
    {
        let mut s = lock();
        let Some(id) = s.open.as_ref().map(|o| o.id.clone()) else { return };
        if let Some(quotes) = s.source.quotes.get_mut(&id)
            && index < quotes.len()
        {
            quotes.remove(index);
        }
    }
    changed();
}

/// The message to send: the thread's chips, then `text`. The chips go.
pub(super) fn take_quotes(s: &mut BridgeState, thread_id: &str, text: &str) -> String {
    s.source.quotes.remove(thread_id).map_or_else(|| text.to_owned(), |q| with_quotes(&q, text))
}

// ---- Saved choices ----

const PREFS_FILE: &str = "agents-source.json";
/// Repositories whose merge method is remembered.
const SAVED_METHODS: usize = 200;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    methods: HashMap<String, String>,
}

/// Reads the merge methods last used, at start.
pub(crate) fn load_prefs() {
    core_host::spawn(async {
        let read = tokio::task::spawn_blocking(|| {
            core_host::try_data_dir().map(|dir| crate::atomic_file::read_json::<Saved>(&dir.join(PREFS_FILE)))
        });
        if let Ok(Some(saved)) = read.await {
            let mut s = lock();
            for (repo, word) in saved.methods.into_iter().take(SAVED_METHODS) {
                let method = match word.as_str() {
                    "squash" => MergeMethod::Squash,
                    "merge" => MergeMethod::Merge,
                    "rebase" => MergeMethod::Rebase,
                    _ => continue,
                };
                s.source.methods.entry(repo).or_insert(method);
            }
        }
    });
}

fn save_prefs() {
    let Some(dir) = core_host::try_data_dir() else { return };
    core_host::spawn(async move {
        static SAVING: Mutex<()> = Mutex::new(());
        let written = tokio::task::spawn_blocking(move || {
            let _one = SAVING.lock().unwrap_or_else(|e| e.into_inner());
            let methods = lock()
                .source
                .methods
                .iter()
                .take(SAVED_METHODS)
                .map(|(repo, m)| (repo.clone(), method_word(*m).to_owned()))
                .collect();
            crate::atomic_file::write_json(&dir.join(PREFS_FILE), &Saved { methods })
        })
        .await;
        if let Ok(Err(error)) = written {
            tracing::warn!(%error, "the merge method wasn't saved");
        }
    });
}
