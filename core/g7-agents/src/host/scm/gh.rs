// SPDX-License-Identifier: MPL-2.0
//! GitHub through the user's own `gh` (§SC4): the only place in `host/scm`
//! that names a `gh` command. G7 runs `gh` with argv only, reads its JSON,
//! and never asks it for a credential, never reads its files, never sets or
//! reads the token variables (they pass through as the user has them), and
//! never calls GitHub itself. One `gh` process at a time.
//!
//! A pull request's detail and diff (phase 6 §X6.1) are two more reads. G7
//! posts nothing from them: GitHub's write commands are not on the list.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::ops::Range;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

use super::patch::{Patch, shape};
use super::run::{Program, Ran, run_program};
use super::{
    Check, CheckCounts, CheckState, GhState, GhUse, GitHub, GitStatus, Lines, Mergeable, PrInfo, PrRow,
    PrState, Reason, ReviewState, Scm, ScmError, Spot, clean, now_ms, remote_of, url_host_path,
};
use crate::api::{
    MAX_DIFF_PATH, MAX_PR_AUTHOR, MAX_PR_COMMENT, MAX_PR_COMMENTS, MAX_PR_DIFF, MAX_PR_FILES, PrComment,
    PrDetail, PrFile, clean_cut, is_stripped_char,
};
use crate::host::git::run::LineSink;

/// Set for every call, so `gh` never waits on a person or a pager.
const GH_SET: [(&str, &str); 6] = [
    ("GH_PROMPT_DISABLED", "1"),
    ("GH_NO_UPDATE_NOTIFIER", "1"),
    ("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1"),
    ("GH_SPINNER_DISABLED", "1"),
    ("GH_PAGER", "cat"),
    ("NO_COLOR", "1"),
];
/// Taken out: debug output, a forced terminal, another repository or host,
/// and variables that would point `gh`'s own git calls elsewhere.
const GH_UNSET: [&str; 8] = [
    "GH_DEBUG",
    "GH_FORCE_TTY",
    "GH_REPO",
    "GH_HOST",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
];

/// Fields read of a pull request (§SC4.1).
const PR_FIELDS: &str = "number,title,state,isDraft,url,reviewDecision,mergeable,mergeStateStatus,baseRefName,headRefName,headRefOid,updatedAt,statusCheckRollup";
const SEARCH_FIELDS: &str = "number,title,repository,state,isDraft,updatedAt,url";
const LIST_FIELDS: &str = "number,title,state,isDraft,updatedAt,url,headRefName,statusCheckRollup";
const REPO_FIELDS: &str =
    "nameWithOwner,mergeCommitAllowed,squashMergeAllowed,rebaseMergeAllowed,defaultBranchRef";
/// Fields read of one pull request's detail (§X6.1): [`PR_FIELDS`] and the
/// description, files and conversation.
const DETAIL_FIELDS: &str = "number,title,state,isDraft,url,reviewDecision,mergeable,mergeStateStatus,baseRefName,headRefName,headRefOid,updatedAt,statusCheckRollup,author,body,additions,deletions,changedFiles,files,comments,reviews";

const CALL_LIMIT: Duration = Duration::from_secs(20);
const WRITE_LIMIT: Duration = Duration::from_secs(60);
const DIFF_LIMIT: Duration = Duration::from_secs(30);
const MAX_GH_OUT: usize = 1 << 20;
const STATE_FOR: Duration = Duration::from_secs(300);
const PR_FOR: Duration = Duration::from_secs(60);
const LIST_FOR: Duration = Duration::from_secs(30);
const MAX_HOSTS: usize = 8;
const MAX_CHECKS: usize = 100;
const MAX_ROWS: usize = 50;
const MAX_PR_TITLE: usize = 256;
const MAX_PR_BODY: usize = 32_768;
const MAX_URL: usize = 512;
const KEPT: usize = 256;
/// A pull request's detail is kept a minute, for at most 8 pull requests.
const DETAIL_FOR: Duration = Duration::from_secs(60);
const DETAILS_KEPT: usize = 8;
/// A pull request's diff is kept whole 10 minutes, for at most 2.
const DIFF_FOR: Duration = Duration::from_secs(600);
const DIFFS_KEPT: usize = 2;
/// One part of a repository's name: `owner`, `name` or the host.
const MAX_REPO_PART: usize = 100;

/// How a pull request is merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Squash,
    Merge,
    Rebase,
}

/// The merge methods a repository allows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Methods {
    pub squash: bool,
    pub merge: bool,
    pub rebase: bool,
}

impl Methods {
    pub fn allows(self, method: Method) -> bool {
        match method {
            Method::Squash => self.squash,
            Method::Merge => self.merge,
            Method::Rebase => self.rebase,
        }
    }
}

/// Which Pull Requests list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Show {
    Mine,
    Review,
    Folder,
}

/// A pull request to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPr {
    pub title: String,
    pub body: String,
    /// A remote branch's name, from `Refs`; `None`: the repository's default.
    pub base: Option<String>,
    pub draft: bool,
}

/// One `gh` command of the closed list (§SC4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ask {
    Version,
    Auth,
    OfBranch {
        branch: String,
    },
    Create {
        branch: String,
        base: Option<String>,
        title: String,
        draft: bool,
    },
    SearchMine,
    SearchReview,
    OpenHere,
    RepoView,
    Ready {
        number: u32,
    },
    /// Pinned to the head commit the person was shown; never `--auto`,
    /// `--admin` or a branch deletion.
    Merge {
        number: u32,
        method: Method,
        head: String,
    },
    /// One pull request with its description, files and conversation; `repo`
    /// is [`repo_ok`].
    View {
        repo: String,
        number: u32,
    },
    /// One pull request's unified diff, without colour.
    Diff {
        repo: String,
        number: u32,
    },
}

/// The arguments after the binary.
pub(crate) fn gh_argv(ask: &Ask) -> Vec<OsString> {
    let words = |w: &[&str]| w.iter().map(OsString::from).collect::<Vec<_>>();
    match ask {
        Ask::Version => words(&["--version"]),
        Ask::Auth => words(&["auth", "status", "--json", "hosts"]),
        Ask::OfBranch { branch } => {
            let mut v = words(&["pr", "list", "--head"]);
            v.push(branch.into());
            v.extend(words(&["--state", "all", "--limit", "5", "--json", PR_FIELDS]));
            v
        }
        Ask::Create { branch, base, title, draft } => {
            let mut v = words(&["pr", "create", "--head"]);
            v.push(branch.into());
            if let Some(base) = base {
                v.push("--base".into());
                v.push(base.into());
            }
            v.push(format!("--title={title}").into());
            v.extend(words(&["--body-file", "-"]));
            if *draft {
                v.push("--draft".into());
            }
            v
        }
        Ask::SearchMine => words(&[
            "search",
            "prs",
            "--author=@me",
            "--state=open",
            "--limit",
            "50",
            "--json",
            SEARCH_FIELDS,
        ]),
        Ask::SearchReview => words(&[
            "search",
            "prs",
            "--review-requested=@me",
            "--state=open",
            "--limit",
            "50",
            "--json",
            SEARCH_FIELDS,
        ]),
        Ask::OpenHere => words(&["pr", "list", "--state", "open", "--limit", "50", "--json", LIST_FIELDS]),
        Ask::RepoView => words(&["repo", "view", "--json", REPO_FIELDS]),
        Ask::Ready { number } => {
            let mut v = words(&["pr", "ready"]);
            v.push(number.to_string().into());
            v
        }
        Ask::Merge { number, method, head } => {
            let mut v = words(&["pr", "merge"]);
            v.push(number.to_string().into());
            v.push(
                match method {
                    Method::Squash => "--squash",
                    Method::Merge => "--merge",
                    Method::Rebase => "--rebase",
                }
                .into(),
            );
            v.push("--match-head-commit".into());
            v.push(head.into());
            v
        }
        Ask::View { repo, number } => {
            let mut v = words(&["pr", "view"]);
            v.push(number.to_string().into());
            v.push("--repo".into());
            v.push(repo.into());
            v.extend(words(&["--json", DETAIL_FIELDS]));
            v
        }
        Ask::Diff { repo, number } => {
            let mut v = words(&["pr", "diff"]);
            v.push(number.to_string().into());
            v.push("--repo".into());
            v.push(repo.into());
            v.extend(words(&["--color", "never"]));
            v
        }
    }
}

impl Ask {
    fn limit(&self) -> Duration {
        match self {
            Ask::Create { .. } | Ask::Merge { .. } => WRITE_LIMIT,
            Ask::Diff { .. } => DIFF_LIMIT,
            _ => CALL_LIMIT,
        }
    }

    /// Bytes of standard output kept; past them the process group is killed.
    fn cap(&self) -> usize {
        match self {
            Ask::Diff { .. } => MAX_PR_DIFF,
            _ => MAX_GH_OUT,
        }
    }

    /// Names in argv that must not read as an option.
    fn sound(&self) -> bool {
        let name = |s: &str| !s.is_empty() && !s.starts_with('-') && !s.contains('\0') && s.len() <= 255;
        match self {
            Ask::OfBranch { branch } => name(branch),
            Ask::Create { branch, base, title, .. } => {
                name(branch) && base.as_deref().is_none_or(name) && !title.contains('\0')
            }
            Ask::Ready { number } => *number > 0,
            Ask::Merge { number, head, .. } => *number > 0 && crate::host::git::oid_ok(head),
            Ask::View { repo, number } | Ask::Diff { repo, number } => *number > 0 && repo_ok(repo),
            _ => true,
        }
    }
}

/// The branch's pull request per (common git directory, branch), and when it was read.
type PrCache = HashMap<(PathBuf, String), (Instant, Option<PrInfo>)>;
/// A Pull Requests list per (list, the folder's common git directory), and when it was read.
type ListCache = HashMap<(Show, Option<PathBuf>), (Instant, Vec<PrRow>)>;

/// One pull request's diff as `gh` printed it, kept whole.
#[derive(Debug)]
struct KeptDiff {
    repo: String,
    number: u32,
    head: String,
    at: Instant,
    bytes: Vec<u8>,
    /// Standard output passed [`MAX_PR_DIFF`]: `bytes` is its start.
    cut: bool,
    sections: Vec<(String, Range<usize>)>,
}

#[derive(Debug, Default)]
struct Cached {
    state: Option<(Instant, GhState)>,
    bin: Option<PathBuf>,
}

/// The `gh` side of [`Scm`]: its environment, its state and caches.
pub(crate) struct Gh {
    env: Vec<(OsString, OsString)>,
    path: Option<OsString>,
    home: PathBuf,
    cached: Mutex<Cached>,
    prs: Mutex<PrCache>,
    lists: Mutex<ListCache>,
    /// Details read, oldest first; past a minute still the last good one.
    details: Mutex<Vec<(Instant, PrDetail)>>,
    diffs: Mutex<Vec<Arc<KeptDiff>>>,
    one: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for Gh {
    /// Never the environment: it holds the user's own variables.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gh")
            .field("bin", &self.cached.lock().map(|c| c.bin.clone()).ok())
            .finish_non_exhaustive()
    }
}

impl Gh {
    pub(crate) fn new(env: &[(OsString, OsString)]) -> Gh {
        let mut kept: Vec<(OsString, OsString)> = env
            .iter()
            .filter(|(k, _)| !GH_UNSET.iter().any(|u| k == u) && !GH_SET.iter().any(|(s, _)| k == s))
            .cloned()
            .collect();
        kept.extend(GH_SET.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))));
        let get = |name: &str| env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        Gh {
            env: kept,
            path: get("PATH"),
            home: get("HOME")
                .map(PathBuf::from)
                .filter(|h| h.is_absolute())
                .unwrap_or_else(|| PathBuf::from("/")),
            cached: Mutex::default(),
            prs: Mutex::default(),
            lists: Mutex::default(),
            details: Mutex::default(),
            diffs: Mutex::default(),
            one: tokio::sync::Mutex::new(()),
        }
    }

    fn find(&self) -> Option<PathBuf> {
        let path = self.path.as_ref()?;
        std::env::split_paths(path).filter(|dir| dir.is_absolute()).map(|dir| dir.join("gh")).find(|bin| {
            std::fs::metadata(bin).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    }

    fn hosts(&self) -> Vec<String> {
        let cached = self.cached.lock().unwrap_or_else(PoisonError::into_inner);
        cached.state.as_ref().map(|(_, s)| s.hosts.clone()).unwrap_or_default()
    }

    /// The folder's GitHub repository when its push remote's host is one
    /// `gh` is signed in to (from the last state read; never runs `gh`).
    pub(crate) fn github_of(&self, url: &str) -> Option<GitHub> {
        github_repo(url, &self.hosts())
    }

    /// Forgets the branch's pull request (after a push, a create, a merge).
    pub(crate) fn forget_pr(&self, spot: &Spot, branch: &str) {
        let mut prs = self.prs.lock().unwrap_or_else(PoisonError::into_inner);
        prs.remove(&(spot.common_dir().to_owned(), branch.to_owned()));
        self.lists.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }

    /// Forgets every pull request read for the repository (after a merge or ready).
    pub(crate) fn forget_repo(&self, spot: &Spot) {
        let mut prs = self.prs.lock().unwrap_or_else(PoisonError::into_inner);
        prs.retain(|(dir, _), _| dir != spot.common_dir());
        drop(prs);
        self.lists.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }

    /// Where a call that names its repository runs: `cwd` when it is a
    /// folder, else the home folder.
    fn cwd_or_home(&self, cwd: &Path) -> PathBuf {
        if cwd.is_absolute() && cwd.is_dir() { cwd.to_owned() } else { self.home.clone() }
    }

    fn kept_detail(&self, repo: &str, number: u32) -> Option<(Instant, PrDetail)> {
        let details = self.details.lock().unwrap_or_else(PoisonError::into_inner);
        details.iter().find(|(_, d)| d.repo == repo && d.pr.number == number).cloned()
    }

    fn keep_detail(&self, detail: PrDetail) {
        let mut details = self.details.lock().unwrap_or_else(PoisonError::into_inner);
        details.retain(|(_, d)| !(d.repo == detail.repo && d.pr.number == detail.pr.number));
        if details.len() >= DETAILS_KEPT {
            details.remove(0);
        }
        details.push((Instant::now(), detail));
    }

    fn kept_diff(&self, repo: &str, number: u32, head: &str) -> Option<Arc<KeptDiff>> {
        let mut diffs = self.diffs.lock().unwrap_or_else(PoisonError::into_inner);
        diffs.retain(|d| d.at.elapsed() < DIFF_FOR);
        diffs.iter().find(|d| d.repo == repo && d.number == number && d.head == head).cloned()
    }

    fn keep_diff(&self, diff: Arc<KeptDiff>) {
        let mut diffs = self.diffs.lock().unwrap_or_else(PoisonError::into_inner);
        diffs.retain(|d| !(d.repo == diff.repo && d.number == diff.number && d.head == diff.head));
        if diffs.len() >= DIFFS_KEPT {
            diffs.remove(0);
        }
        diffs.push(diff);
    }

    fn forget_diffs(&self, repo: &str, number: u32) {
        let mut diffs = self.diffs.lock().unwrap_or_else(PoisonError::into_inner);
        diffs.retain(|d| !(d.repo == repo && d.number == number));
    }

    fn set_use(&self, gh: GhUse) {
        let mut cached = self.cached.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, state)) = cached.state.as_mut() {
            state.gh = gh;
            if gh != GhUse::Ok {
                state.hosts.clear();
            }
        }
    }
}

/// What one `gh` call printed.
#[derive(Debug)]
struct Said {
    code: Option<i32>,
    stdout: Vec<u8>,
    /// A diff passed its cap: `stdout` is its start (other calls fail instead).
    over: bool,
    /// Its error output says GitHub found the diff too large to send.
    too_large: bool,
}

impl Scm {
    /// Whether `gh` can be used (§SC4.2): read on first use, kept 5
    /// minutes, read again on `refresh` and after a signed-out answer.
    pub async fn gh_state(&self, refresh: bool) -> GhState {
        {
            let cached = self.gh.cached.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((at, state)) = &cached.state
                && !refresh
                && at.elapsed() < STATE_FOR
                && state.gh != GhUse::SignedOut
            {
                return state.clone();
            }
        }
        let state = self.read_gh_state().await;
        let mut cached = self.gh.cached.lock().unwrap_or_else(PoisonError::into_inner);
        cached.state = Some((Instant::now(), state.clone()));
        state
    }

    async fn read_gh_state(&self) -> GhState {
        let mut state = GhState { checked_ms: now_ms(), ..GhState::default() };
        let Some(bin) = self.gh.find() else {
            self.gh.cached.lock().unwrap_or_else(PoisonError::into_inner).bin = None;
            return state;
        };
        self.gh.cached.lock().unwrap_or_else(PoisonError::into_inner).bin = Some(bin);
        let cwd = self.gh.home.clone();
        match self.gh_raw(&Ask::Version, &cwd, None, None).await {
            Ok(said) if said.code == Some(0) => state.version = version(&said.stdout),
            Ok(_) | Err(_) => {
                state.gh = GhUse::Old;
                return state;
            }
        }
        state.gh = match self.gh_raw(&Ask::Auth, &cwd, None, None).await {
            Ok(said) => {
                state.hosts = signed_in_hosts(&said.stdout);
                if state.hosts.is_empty() { GhUse::SignedOut } else { GhUse::Ok }
            }
            Err(e) if e.reason == Reason::OldGh => GhUse::Old,
            Err(_) => GhUse::SignedOut,
        };
        state
    }

    /// The branch's pull request (§SC4.3): the newest open one, else the
    /// newest; `None` when it has none. Kept 60 seconds.
    pub async fn pr_for_branch(
        &self,
        spot: &Spot,
        status: &GitStatus,
        refresh: bool,
    ) -> Result<Option<PrInfo>, ScmError> {
        let branch = status.branch.clone().ok_or(ScmError::new(Reason::Detached))?;
        self.on_github(spot, status).await?;
        let key = (spot.common_dir().to_owned(), branch.clone());
        if !refresh {
            let prs = self.gh.prs.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((at, pr)) = prs.get(&key)
                && at.elapsed() < PR_FOR
            {
                return Ok(pr.clone());
            }
        }
        let said = self.gh_call(&Ask::OfBranch { branch }, spot.folder(), None, None).await?;
        let hosts = self.gh.hosts();
        let prs: Vec<GhPr> =
            serde_json::from_slice(&said.stdout).map_err(|_| ScmError::new(Reason::Failed))?;
        let pr = choose(prs.into_iter().filter_map(|p| pr_info(p, &hosts)).collect());
        let mut cache = self.gh.prs.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.len() >= KEPT {
            cache.retain(|_, (at, _)| at.elapsed() < PR_FOR);
        }
        cache.insert(key, (Instant::now(), pr.clone()));
        Ok(pr)
    }

    /// The last pull request read for the branch, however old (shown with
    /// `why: "network"` when GitHub can't be reached).
    pub fn cached_pr(&self, spot: &Spot, branch: &str) -> Option<PrInfo> {
        let prs = self.gh.prs.lock().unwrap_or_else(PoisonError::into_inner);
        prs.get(&(spot.common_dir().to_owned(), branch.to_owned())).and_then(|(_, pr)| pr.clone())
    }

    /// Opens a pull request for the pushed branch (§SC4.3). `gh`'s lines go
    /// to `out`.
    pub async fn pr_create(
        &self,
        spot: &Spot,
        status: &GitStatus,
        new: &NewPr,
        out: &Lines,
    ) -> Result<PrInfo, ScmError> {
        let title = new.title.trim();
        if title.is_empty() || title.len() > MAX_PR_TITLE || title.contains(['\0', '\n', '\r']) {
            return Err(ScmError::new(Reason::BadRequest));
        }
        if new.body.len() > MAX_PR_BODY || new.body.contains('\0') {
            return Err(ScmError::new(Reason::BadRequest));
        }
        let branch = status.branch.clone().ok_or(ScmError::new(Reason::Detached))?;
        if status.upstream.is_none() || status.ahead != Some(0) {
            return Err(ScmError::new(Reason::NotPushed));
        }
        self.on_github(spot, status).await?;
        let base = match &new.base {
            None => None,
            Some(base) => {
                let rows = self.known_refs(spot).await?;
                let row = rows
                    .iter()
                    .find(|r| r.remote.is_some() && (&r.name == base || &r.short == base))
                    .ok_or(ScmError::new(Reason::NotFound))?;
                Some(row.short.clone())
            }
        };
        if let Some(open) = self.pr_for_branch(spot, status, true).await?
            && open.state == PrState::Open
        {
            return Err(ScmError::new(Reason::PrExists));
        }
        let ask = Ask::Create { branch: branch.clone(), base, title: title.to_owned(), draft: new.draft };
        let said = self.gh_call(&ask, spot.folder(), Some(new.body.clone().into_bytes()), Some(out)).await;
        self.gh.forget_pr(spot, &branch);
        let said = said?;
        let hosts = self.gh.hosts();
        let number = String::from_utf8_lossy(&said.stdout)
            .lines()
            .rev()
            .find(|l| https_on(l.trim(), &hosts))
            .and_then(|l| l.trim().rsplit('/').next().and_then(|n| n.parse::<u32>().ok()));
        let said = self.gh_call(&Ask::OfBranch { branch }, spot.folder(), None, None).await?;
        let prs: Vec<GhPr> =
            serde_json::from_slice(&said.stdout).map_err(|_| ScmError::new(Reason::Failed))?;
        let prs: Vec<PrInfo> = prs.into_iter().filter_map(|p| pr_info(p, &hosts)).collect();
        let found = match number {
            Some(n) => prs.into_iter().find(|p| p.number == n),
            None => choose(prs).filter(|p| p.state == PrState::Open),
        };
        found.ok_or(ScmError::new(Reason::Failed))
    }

    /// The merge methods the repository allows (read when the merge dialog opens).
    pub async fn pr_methods(&self, spot: &Spot) -> Result<Methods, ScmError> {
        self.usable().await?;
        let said = self.gh_call(&Ask::RepoView, spot.folder(), None, None).await?;
        let repo: GhRepo = serde_json::from_slice(&said.stdout).map_err(|_| ScmError::new(Reason::Failed))?;
        Ok(Methods {
            squash: repo.squash_merge_allowed,
            merge: repo.merge_commit_allowed,
            rebase: repo.rebase_merge_allowed,
        })
    }

    /// Merges pull request `number` with `method`, pinned to `head`, the
    /// commit the person confirmed (§SC4.3): a push in between fails the
    /// merge instead of merging something unseen. The method must be one the
    /// repository allows. `gh`'s lines go to `out`; the host asks first.
    pub async fn pr_merge(
        &self,
        spot: &Spot,
        number: u32,
        method: Method,
        head: &str,
        out: &Lines,
    ) -> Result<(), ScmError> {
        if number == 0 || !crate::host::git::oid_ok(head) {
            return Err(ScmError::new(Reason::BadRequest));
        }
        if !self.pr_methods(spot).await?.allows(method) {
            return Err(ScmError::new(Reason::NotAllowed));
        }
        let ask = Ask::Merge { number, method, head: head.to_owned() };
        let said = self.gh_call(&ask, spot.folder(), None, Some(out)).await;
        self.gh.forget_repo(spot);
        said.map(drop)
    }

    /// Marks a draft pull request ready for review.
    pub async fn pr_ready(&self, spot: &Spot, number: u32) -> Result<(), ScmError> {
        if number == 0 {
            return Err(ScmError::new(Reason::BadRequest));
        }
        self.usable().await?;
        let said = self.gh_call(&Ask::Ready { number }, spot.folder(), None, None).await;
        self.gh.forget_repo(spot);
        said.map(drop)
    }

    /// The Pull Requests list (§SC4.4): at most 50 rows, kept 30 seconds.
    /// `Folder` needs the folder's `spot`.
    pub async fn pr_list(
        &self,
        show: Show,
        spot: Option<&Spot>,
        refresh: bool,
    ) -> Result<Vec<PrRow>, ScmError> {
        self.usable().await?;
        let key = (show, spot.map(|s| s.common_dir().to_owned()).filter(|_| show == Show::Folder));
        if !refresh {
            let lists = self.gh.lists.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((at, rows)) = lists.get(&key)
                && at.elapsed() < LIST_FOR
            {
                return Ok(rows.clone());
            }
        }
        let hosts = self.gh.hosts();
        let rows = match show {
            Show::Mine | Show::Review => {
                let ask = if show == Show::Mine { Ask::SearchMine } else { Ask::SearchReview };
                let cwd = spot.map_or_else(|| self.gh.home.clone(), |s| s.folder().to_owned());
                let said = self.gh_call(&ask, &cwd, None, None).await?;
                let found: Vec<GhFound> =
                    serde_json::from_slice(&said.stdout).map_err(|_| ScmError::new(Reason::Failed))?;
                found.into_iter().filter_map(|f| found_row(f, &hosts)).take(MAX_ROWS).collect::<Vec<_>>()
            }
            Show::Folder => {
                let spot = spot.ok_or(ScmError::new(Reason::BadRequest))?;
                let remotes = self.known_remotes(spot).await?;
                let repo = remotes.iter().find_map(|(_, url)| github_repo(url, &hosts)).map(|g| g.repo);
                let said = self.gh_call(&Ask::OpenHere, spot.folder(), None, None).await?;
                let prs: Vec<GhPr> =
                    serde_json::from_slice(&said.stdout).map_err(|_| ScmError::new(Reason::Failed))?;
                prs.into_iter()
                    .filter_map(|p| here_row(p, repo.as_deref().unwrap_or(""), &hosts))
                    .take(MAX_ROWS)
                    .collect()
            }
        };
        let mut lists = self.gh.lists.lock().unwrap_or_else(PoisonError::into_inner);
        if lists.len() >= KEPT {
            lists.clear();
        }
        lists.insert(key, (Instant::now(), rows.clone()));
        Ok(rows)
    }

    /// Refuses unless `gh` is there and signed in.
    async fn usable(&self) -> Result<GhState, ScmError> {
        let state = self.gh_state(false).await;
        match state.gh {
            GhUse::Ok => Ok(state),
            GhUse::Missing => Err(ScmError::new(Reason::NoGh)),
            GhUse::SignedOut => Err(ScmError::new(Reason::SignedOut)),
            GhUse::Old => Err(ScmError::new(Reason::OldGh)),
        }
    }

    /// Refuses unless `gh` can be used and the push remote is on a host it
    /// is signed in to.
    async fn on_github(&self, spot: &Spot, status: &GitStatus) -> Result<GitHub, ScmError> {
        let state = self.usable().await?;
        let remotes = self.known_remotes(spot).await?;
        let upstream_ref = status.upstream.as_ref().map(|u| format!("refs/remotes/{u}"));
        let upstream_remote = upstream_ref.as_deref().and_then(|u| remote_of(u, &remotes));
        let remote = super::pick_remote(&remotes, upstream_remote.as_deref(), None)
            .map_err(|_| ScmError::new(Reason::NotGithub))?;
        let url = remotes.iter().find(|(n, _)| *n == remote).map(|(_, u)| u.as_str()).unwrap_or("");
        github_repo(url, &state.hosts).ok_or(ScmError::new(Reason::NotGithub))
    }

    /// Runs one `gh` call; a non-zero exit is an error: exit 4 is signed
    /// out (and flips the state), an unknown flag or command is an old `gh`,
    /// anything else is GitHub not reached.
    async fn gh_call(
        &self,
        ask: &Ask,
        cwd: &Path,
        input: Option<Vec<u8>>,
        out: Option<&Lines>,
    ) -> Result<Said, ScmError> {
        let said = self.gh_raw(ask, cwd, input, out).await?;
        match said.code {
            Some(0) => Ok(said),
            _ => Err(ScmError::new(if out.is_some() { Reason::Failed } else { Reason::Network })),
        }
    }

    async fn gh_raw(
        &self,
        ask: &Ask,
        cwd: &Path,
        input: Option<Vec<u8>>,
        out: Option<&Lines>,
    ) -> Result<Said, ScmError> {
        if !ask.sound() {
            return Err(ScmError::new(Reason::BadRequest));
        }
        let bin = self.gh.cached.lock().unwrap_or_else(PoisonError::into_inner).bin.clone();
        let Some(bin) = bin.or_else(|| self.gh.find()) else { return Err(ScmError::new(Reason::NoGh)) };
        let _one = self.gh.one.lock().await;
        let home = self.home().map(str::to_owned);
        let mut on_line = |raw: &[u8]| {
            if let Some(out) = out {
                out.push(&clean::line(raw, home.as_deref()));
            }
        };
        let program = Program {
            bin: &bin,
            args: gh_argv(ask),
            env: &self.gh.env,
            cwd: cwd.to_owned(),
            input,
            limit: ask.limit(),
            cap: ask.cap(),
        };
        let sink: LineSink<'_> = &mut on_line;
        let ran = run_program(program, out.is_some().then_some(sink))
            .await
            .map_err(|_| ScmError::new(Reason::NoGh))?;
        let (code, stdout, over, stderr) = match ran {
            Ran::Exited { code, stdout, over, stderr } => (code, stdout, over, stderr),
            Ran::Late => {
                tracing::debug!("gh took too long");
                return Err(ScmError::new(if out.is_some() { Reason::Failed } else { Reason::Network }));
            }
        };
        let stderr = String::from_utf8_lossy(&stderr);
        if *ask != Ask::Auth {
            let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            tracing::debug!(code = ?code, "gh: {}", clean::line(first.as_bytes(), home.as_deref()));
        }
        if code == Some(4) {
            self.gh.set_use(GhUse::SignedOut);
            return Err(ScmError::new(Reason::SignedOut));
        }
        if code == Some(1) && (stderr.contains("unknown flag") || stderr.contains("unknown command")) {
            self.gh.set_use(GhUse::Old);
            return Err(ScmError::new(Reason::OldGh));
        }
        let diff = matches!(ask, Ask::Diff { .. });
        if over && !diff {
            return Err(ScmError::new(Reason::Failed));
        }
        let too_large = diff && code != Some(0) && says_too_large(&stderr);
        Ok(Said { code, stdout, over, too_large })
    }

    // ---- One pull request, read-only (§X6.1) ----

    /// Pull request `number` of `repo` (`owner/name`, or `host/owner/name` off
    /// `github.com`) with its description, changed files and conversation.
    /// `cwd`: the thread's folder, else the home folder is used. Kept a
    /// minute; `refresh` reads it again and drops its kept diff. The host
    /// checks first that the pair is the thread's own or one it listed.
    pub async fn pr_detail(
        &self,
        cwd: &Path,
        repo: &str,
        number: u32,
        refresh: bool,
    ) -> Result<PrDetail, ScmError> {
        let repo = self.named_repo(repo, number).await?;
        if refresh {
            self.gh.forget_diffs(&repo, number);
        } else if let Some((at, detail)) = self.gh.kept_detail(&repo, number)
            && at.elapsed() < DETAIL_FOR
        {
            return Ok(detail);
        }
        let ask = Ask::View { repo: repo.clone(), number };
        let said = self.gh_call(&ask, &self.gh.cwd_or_home(cwd), None, None).await?;
        let pr: GhPr = serde_json::from_slice(&said.stdout).map_err(|_| ScmError::new(Reason::Failed))?;
        let more: GhDetail =
            serde_json::from_slice(&said.stdout).map_err(|_| ScmError::new(Reason::Failed))?;
        let detail =
            detail_of(pr, more, &repo, number, &self.gh.hosts()).ok_or(ScmError::new(Reason::Failed))?;
        self.gh.keep_detail(detail.clone());
        Ok(detail)
    }

    /// The last detail read of the pull request, however old (shown with
    /// `why: "network"` when GitHub can't be reached).
    pub fn cached_pr_detail(&self, repo: &str, number: u32) -> Option<PrDetail> {
        let repo = canonical_repo(repo)?;
        self.gh.kept_detail(&repo, number).map(|(_, detail)| detail)
    }

    /// The page at `offset` of `path`'s part of pull request `number`'s diff,
    /// with the paging and caps of `git.patch`. The diff is read once per
    /// head commit (`head_oid`, from the detail) and kept 10 minutes. A diff
    /// past [`MAX_PR_DIFF`] is cut there: a file past the cut is `truncated`.
    /// GitHub refusing a diff as too large is [`Reason::Large`]. The host
    /// checks first that `path` is one of the detail's files.
    pub async fn pr_diff(
        &self,
        cwd: &Path,
        repo: &str,
        number: u32,
        head_oid: &str,
        path: &str,
        offset: u64,
    ) -> Result<Patch, ScmError> {
        if !crate::host::git::oid_ok(head_oid) || !offered(path.as_bytes()) {
            return Err(ScmError::new(Reason::BadRequest));
        }
        let repo = canonical_repo(repo).ok_or(ScmError::new(Reason::BadRequest))?;
        let kept = match self.gh.kept_diff(&repo, number, head_oid) {
            Some(kept) => kept,
            None => {
                let repo = self.named_repo(&repo, number).await?;
                let ask = Ask::Diff { repo: repo.clone(), number };
                let said = self.gh_raw(&ask, &self.gh.cwd_or_home(cwd), None, None).await?;
                if !said.over && said.code != Some(0) {
                    return Err(ScmError::new(if said.too_large { Reason::Large } else { Reason::Network }));
                }
                let sections = split_diff(&said.stdout);
                let kept = Arc::new(KeptDiff {
                    repo,
                    number,
                    head: head_oid.to_owned(),
                    at: Instant::now(),
                    bytes: said.stdout,
                    cut: said.over,
                    sections,
                });
                self.gh.keep_diff(kept.clone());
                kept
            }
        };
        let Some((_, range)) = kept.sections.iter().find(|(p, _)| p == path) else {
            return if kept.cut {
                Ok(Patch { truncated: true, ..Patch::default() })
            } else {
                Err(ScmError::new(Reason::NotFound))
            };
        };
        let at_cut = kept.cut && range.end == kept.bytes.len();
        let whole = shape(&kept.bytes[range.clone()], at_cut);
        let (text, next) = whole.page(offset).ok_or(ScmError::new(Reason::BadRequest))?;
        Ok(Patch {
            text: text.to_owned(),
            next,
            truncated: whole.truncated,
            binary: whole.binary,
            ..Patch::default()
        })
    }

    /// `repo` as `gh` is given it, refused unless it is [`repo_ok`], `number`
    /// is not 0, `gh` is signed in, and the repository's host is one of its.
    async fn named_repo(&self, repo: &str, number: u32) -> Result<String, ScmError> {
        let repo = canonical_repo(repo).filter(|_| number > 0).ok_or(ScmError::new(Reason::BadRequest))?;
        let state = self.usable().await?;
        if !state.hosts.iter().any(|h| *h == repo_host(&repo)) {
            return Err(ScmError::new(Reason::NotGithub));
        }
        Ok(repo)
    }
}

/// GitHub's refusal to send a diff, by the words its error carries.
fn says_too_large(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    ["too large", "too_large", "exceeded the maximum"].iter().any(|w| lower.contains(w))
}

/// `owner/name`, or `host/owner/name`: each part 1 to 100 of
/// `[A-Za-z0-9._-]`, not starting with `-` or `.`.
pub(crate) fn repo_ok(repo: &str) -> bool {
    let part = |p: &str| {
        (1..=MAX_REPO_PART).contains(&p.len())
            && !p.starts_with(['-', '.'])
            && p.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    let parts = repo.split('/').count();
    (2..=3).contains(&parts) && repo.split('/').all(part)
}

/// A [`repo_ok`] repository as it is kept and passed: the host lower case,
/// and `github.com/` left out.
fn canonical_repo(repo: &str) -> Option<String> {
    if !repo_ok(repo) {
        return None;
    }
    match repo.split_once('/') {
        Some((host, rest)) if rest.contains('/') => {
            let host = host.to_ascii_lowercase();
            Some(if host == "github.com" { rest.to_owned() } else { format!("{host}/{rest}") })
        }
        _ => Some(repo.to_owned()),
    }
}

/// The host of a [`canonical_repo`].
fn repo_host(repo: &str) -> &str {
    match repo.split_once('/') {
        Some((host, rest)) if rest.contains('/') => host,
        _ => "github.com",
    }
}

// ---- What gh prints ----

#[derive(Debug, Deserialize)]
struct GhAuth {
    #[serde(default)]
    hosts: BTreeMap<String, Vec<GhAuthEntry>>,
}

/// Only what G7 reads of an account entry: anything else in it is never parsed.
#[derive(Debug, Deserialize)]
struct GhAuthEntry {
    #[serde(default)]
    state: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct GhPr {
    number: u32,
    title: String,
    state: String,
    is_draft: bool,
    url: String,
    review_decision: Option<String>,
    mergeable: Option<String>,
    merge_state_status: Option<String>,
    base_ref_name: String,
    head_ref_name: String,
    head_ref_oid: String,
    updated_at: String,
    status_check_rollup: Option<Vec<Value>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct GhFound {
    number: u32,
    title: String,
    repository: GhRepoName,
    state: String,
    is_draft: bool,
    updated_at: String,
    url: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct GhRepoName {
    name_with_owner: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct GhRepo {
    merge_commit_allowed: bool,
    squash_merge_allowed: bool,
    rebase_merge_allowed: bool,
}

/// What a detail adds to [`GhPr`], read from the same answer. Every field
/// may be missing or null; anything else in the answer is never parsed.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct GhDetail {
    author: Option<GhAuthor>,
    body: Option<String>,
    additions: Option<u64>,
    deletions: Option<u64>,
    changed_files: Option<u64>,
    files: Option<Vec<GhFile>>,
    comments: Option<Vec<GhComment>>,
    reviews: Option<Vec<GhReview>>,
}

/// Only the login of an author.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GhAuthor {
    login: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct GhFile {
    path: Option<String>,
    additions: Option<u64>,
    deletions: Option<u64>,
    change_type: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct GhComment {
    author: Option<GhAuthor>,
    body: Option<String>,
    created_at: Option<String>,
    url: Option<String>,
    /// GitHub hides it (off-topic, spam, outdated...).
    is_minimized: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct GhReview {
    author: Option<GhAuthor>,
    body: Option<String>,
    state: Option<String>,
    submitted_at: Option<String>,
}

fn version(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    let first = text.lines().next()?;
    let v = first.strip_prefix("gh version ")?.split_whitespace().next()?;
    let ok = !v.is_empty()
        && v.len() <= 32
        && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'));
    ok.then(|| v.to_owned())
}

/// Hosts with an entry whose `state` is `success` (§SC12.1 row 4).
fn signed_in_hosts(stdout: &[u8]) -> Vec<String> {
    let Ok(auth) = serde_json::from_slice::<GhAuth>(stdout) else { return Vec::new() };
    auth.hosts
        .into_iter()
        .filter(|(host, entries)| host_ok(host) && entries.iter().any(|e| e.state == "success"))
        .map(|(host, _)| host.to_ascii_lowercase())
        .take(MAX_HOSTS)
        .collect()
}

fn host_ok(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
}

/// An `https` URL on a host `gh` is signed in to.
fn https_on(url: &str, hosts: &[String]) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let authority = rest.split('/').next().unwrap_or("");
    url.len() <= MAX_URL
        && !authority.contains(['@', ':'])
        && !url.contains(char::is_whitespace)
        && hosts.iter().any(|h| h.eq_ignore_ascii_case(authority))
}

/// `owner/name` of a remote URL on a signed-in host.
fn github_repo(url: &str, hosts: &[String]) -> Option<GitHub> {
    let (host, path) = url_host_path(url)?;
    if !hosts.contains(&host) {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    let part = |s: &str| {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    };
    let repo = format!("{owner}/{name}");
    (part(owner) && part(name) && repo.len() <= 140).then_some(GitHub { repo, host })
}

fn pr_state(word: &str) -> Option<PrState> {
    match word.to_ascii_lowercase().as_str() {
        "open" => Some(PrState::Open),
        "merged" => Some(PrState::Merged),
        "closed" => Some(PrState::Closed),
        _ => None,
    }
}

fn pr_info(pr: GhPr, hosts: &[String]) -> Option<PrInfo> {
    let state = pr_state(&pr.state)?;
    if pr.number == 0 {
        return None;
    }
    let (checks, counts) = checks(pr.status_check_rollup.as_deref().unwrap_or(&[]), hosts);
    let review = match pr.review_decision.as_deref() {
        Some("APPROVED") => Some(ReviewState::Approved),
        Some("CHANGES_REQUESTED") => Some(ReviewState::ChangesRequested),
        Some("REVIEW_REQUIRED") => Some(ReviewState::ReviewRequired),
        _ => None,
    };
    let mergeable = pr.mergeable.as_deref().map(|m| match m {
        "MERGEABLE" => Mergeable::Yes,
        "CONFLICTING" => Mergeable::Conflicts,
        _ => Mergeable::Unknown,
    });
    Some(PrInfo {
        number: pr.number,
        title: clean::text(&pr.title, MAX_PR_TITLE),
        state,
        draft: pr.is_draft,
        url: if https_on(&pr.url, hosts) { pr.url } else { String::new() },
        base: clean::text(&pr.base_ref_name, 255),
        head: clean::text(&pr.head_ref_name, 255),
        head_oid: if crate::host::git::oid_ok(&pr.head_ref_oid) { pr.head_ref_oid } else { String::new() },
        review,
        mergeable,
        blocked: pr.merge_state_status.as_deref() == Some("BLOCKED"),
        updated_ms: iso_ms(&pr.updated_at).unwrap_or(0),
        checks,
        pass: counts.pass,
        fail: counts.fail,
        pending: counts.pending,
        read_ms: now_ms(),
    })
}

/// The newest open pull request, else the newest.
fn choose(mut prs: Vec<PrInfo>) -> Option<PrInfo> {
    prs.sort_by_key(|p| std::cmp::Reverse((p.state == PrState::Open, p.updated_ms, p.number)));
    prs.into_iter().next()
}

fn found_row(found: GhFound, hosts: &[String]) -> Option<PrRow> {
    let state = pr_state(&found.state)?;
    (found.number > 0).then(|| PrRow {
        number: found.number,
        title: clean::text(&found.title, MAX_PR_TITLE),
        repo: clean::text(&found.repository.name_with_owner, 140),
        state,
        draft: found.is_draft,
        updated_ms: iso_ms(&found.updated_at).unwrap_or(0),
        url: if https_on(&found.url, hosts) { found.url } else { String::new() },
        branch: None,
        checks: None,
    })
}

fn here_row(pr: GhPr, repo: &str, hosts: &[String]) -> Option<PrRow> {
    let state = pr_state(&pr.state)?;
    let (_, counts) = checks(pr.status_check_rollup.as_deref().unwrap_or(&[]), hosts);
    (pr.number > 0).then(|| PrRow {
        number: pr.number,
        title: clean::text(&pr.title, MAX_PR_TITLE),
        repo: clean::text(repo, 140),
        state,
        draft: pr.is_draft,
        updated_ms: iso_ms(&pr.updated_at).unwrap_or(0),
        url: if https_on(&pr.url, hosts) { pr.url } else { String::new() },
        branch: Some(clean::text(&pr.head_ref_name, 255)).filter(|b| !b.is_empty()),
        checks: Some(counts),
    })
}

/// `statusCheckRollup` mapped by §SC4.1's table: at most 100 kept, failing
/// first; the counts are over every entry. Nothing unrecognised is a pass.
fn checks(rollup: &[Value], hosts: &[String]) -> (Vec<Check>, CheckCounts) {
    let mut all: Vec<Check> = rollup.iter().map(|entry| check(entry, hosts)).collect();
    let mut counts = CheckCounts::default();
    for c in &all {
        match c.state {
            CheckState::Pass => counts.pass += 1,
            CheckState::Fail => counts.fail += 1,
            CheckState::Pending => counts.pending += 1,
            CheckState::Skipped | CheckState::Cancelled => {}
        }
    }
    all.sort_by_key(|c| c.state);
    all.truncate(MAX_CHECKS);
    (all, counts)
}

fn check(entry: &Value, hosts: &[String]) -> Check {
    let field = |k: &str| entry.get(k).and_then(Value::as_str).unwrap_or("");
    let (name, state, url) = match field("__typename") {
        "CheckRun" => {
            let state = if field("status") == "COMPLETED" {
                match field("conclusion") {
                    "SUCCESS" | "NEUTRAL" => CheckState::Pass,
                    "FAILURE" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => CheckState::Fail,
                    "SKIPPED" => CheckState::Skipped,
                    "CANCELLED" => CheckState::Cancelled,
                    _ => CheckState::Pending,
                }
            } else {
                CheckState::Pending
            };
            let name = clean::text(field("name"), 120);
            let workflow = clean::text(field("workflowName"), 120);
            let both = format!("{workflow} / {name}");
            let shown = if !workflow.is_empty() && both.len() <= 120 { both } else { name };
            (shown, state, field("detailsUrl"))
        }
        "StatusContext" => {
            let state = match field("state") {
                "SUCCESS" => CheckState::Pass,
                "FAILURE" | "ERROR" => CheckState::Fail,
                _ => CheckState::Pending,
            };
            (clean::text(field("context"), 120), state, field("targetUrl"))
        }
        _ => (clean::text(field("name"), 120), CheckState::Pending, ""),
    };
    let name = if name.is_empty() { "check".to_owned() } else { name };
    Check { name, state, url: https_on(url, hosts).then(|| url.to_owned()) }
}

/// A pull request's detail (§X6.1): `None` when `gh` answered with no pull
/// request or another one.
fn detail_of(pr: GhPr, more: GhDetail, repo: &str, number: u32, hosts: &[String]) -> Option<PrDetail> {
    let info = pr_info(pr, hosts).filter(|p| p.number == number)?;
    let count = |n: Option<u64>| n.map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
    let changed = count(more.changed_files);
    let raw_files = more.files.unwrap_or_default();
    let listed = raw_files.len();
    let files: Vec<PrFile> = raw_files.into_iter().filter_map(pr_file).take(MAX_PR_FILES).collect();
    let files_more = listed > MAX_PR_FILES || usize::try_from(changed).is_ok_and(|c| c > files.len());
    let (body, body_cut) = clean_cut(more.body.as_deref().unwrap_or(""), MAX_PR_BODY);
    let mut comments: Vec<PrComment> = more
        .comments
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| comment(c, hosts))
        .chain(more.reviews.unwrap_or_default().into_iter().filter_map(review))
        .collect();
    comments.sort_by_key(|c| c.at_ms);
    let older = comments.len().saturating_sub(MAX_PR_COMMENTS);
    let comments = comments.split_off(older);
    Some(PrDetail {
        pr: info,
        repo: repo.to_owned(),
        author: login(more.author).unwrap_or_default(),
        body,
        body_cut,
        adds: count(more.additions),
        dels: count(more.deletions),
        changed,
        files,
        files_more,
        comments,
        comments_older: u32::try_from(older).unwrap_or(u32::MAX),
        thread_id: None,
        folder_id: None,
        read_ms: now_ms(),
    })
}

/// An author's login, cleaned and cut; `None` when there is none.
fn login(author: Option<GhAuthor>) -> Option<String> {
    let login = clean::text(&author?.login?, MAX_PR_AUTHOR);
    (!login.is_empty()).then_some(login)
}

fn pr_file(file: GhFile) -> Option<PrFile> {
    let path = file.path.filter(|p| offered(p.as_bytes()))?;
    let change = match file.change_type.as_deref() {
        Some("ADDED") => "added",
        Some("DELETED") => "deleted",
        Some("RENAMED") => "renamed",
        Some("COPIED") => "copied",
        Some("MODIFIED") => "modified",
        _ => "changed",
    };
    let count = |n: Option<u64>| n.map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
    Some(PrFile { path, change: change.to_owned(), adds: count(file.additions), dels: count(file.deletions) })
}

/// A conversation comment: left out without an author or a body, and when
/// GitHub hides it.
fn comment(c: GhComment, hosts: &[String]) -> Option<PrComment> {
    if c.is_minimized == Some(true) {
        return None;
    }
    let (body, cut) = clean_cut(c.body?.trim_end(), MAX_PR_COMMENT);
    if body.trim().is_empty() {
        return None;
    }
    Some(PrComment {
        author: login(c.author)?,
        at_ms: c.created_at.as_deref().and_then(iso_ms).unwrap_or(0),
        body,
        cut,
        review: None,
        url: c.url.filter(|u| https_on(u, hosts)),
    })
}

/// A review's summary: left out without an author or a body, and when it
/// is only a comment with nothing written (its line comments aren't read).
fn review(r: GhReview) -> Option<PrComment> {
    let (body, cut) = clean_cut(r.body?.trim_end(), MAX_PR_COMMENT);
    let word = match r.state.as_deref() {
        Some("APPROVED") => Some("approved"),
        Some("CHANGES_REQUESTED") => Some("changes_requested"),
        Some("COMMENTED") => Some("commented"),
        Some("DISMISSED") => Some("dismissed"),
        _ => None,
    };
    if body.trim().is_empty() && matches!(word, Some("commented") | None) {
        return None;
    }
    Some(PrComment {
        author: login(r.author)?,
        at_ms: r.submitted_at.as_deref().and_then(iso_ms).unwrap_or(0),
        body,
        cut,
        review: word.map(str::to_owned),
        url: None,
    })
}

// ---- A pull request's diff, by file ----

/// The files of a unified diff, in order, with the byte range of each one's
/// part (from its `diff --git ` line to the next). A part's path is its
/// `+++ b/` line's, else (deleted) its `--- a/` line's, else (binary, pure
/// rename or copy, mode change) its `rename to` or `copy to` line's, else
/// the header's when both halves name the same path. Quoted paths are
/// unquoted. A path that isn't relative, has an empty, `.` or `..` part, a
/// control character, or passes 512 bytes is not offered.
pub fn split_diff(diff: &[u8]) -> Vec<(String, Range<usize>)> {
    let mut starts = Vec::new();
    let mut at = 0;
    while at < diff.len() {
        let end = diff[at..].iter().position(|&b| b == b'\n').map_or(diff.len(), |n| at + n + 1);
        if diff[at..end].starts_with(b"diff --git ") {
            starts.push(at);
        }
        at = end;
    }
    starts
        .iter()
        .enumerate()
        .filter_map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(diff.len());
            let path = part_path(&diff[start..end])?;
            Some((path, start..end))
        })
        .collect()
}

/// The path of one part of a diff (see [`split_diff`]).
fn part_path(part: &[u8]) -> Option<String> {
    let mut lines = part.split(|&b| b == b'\n');
    let header = lines.next()?.strip_prefix(b"diff --git ")?;
    let (mut minus, mut plus, mut moved) = (None, None, None);
    for line in lines {
        if line.starts_with(b"@@") {
            break;
        }
        if let Some(rest) = line.strip_prefix(b"--- ") {
            minus = side(rest, b"a/");
        } else if let Some(rest) = line.strip_prefix(b"+++ ") {
            plus = side(rest, b"b/");
        } else if let Some(rest) = line.strip_prefix(b"rename to ").or_else(|| line.strip_prefix(b"copy to "))
        {
            moved = side(rest, b"");
        }
    }
    let raw = plus.or(minus).or(moved).or_else(|| same_halves(header))?;
    String::from_utf8(raw).ok().filter(|p| offered(p.as_bytes()))
}

/// A path of a `---`, `+++`, `rename to` or `copy to` line, without
/// `prefix`: the tab git writes after a name holding a space dropped, a
/// quoted name unquoted. `/dev/null` and a name without `prefix` are `None`.
fn side(raw: &[u8], prefix: &[u8]) -> Option<Vec<u8>> {
    let raw = raw.strip_suffix(b"\t").unwrap_or(raw);
    let name = if raw.starts_with(b"\"") {
        let (name, rest) = unquote(raw)?;
        if !rest.is_empty() {
            return None;
        }
        name
    } else {
        raw.to_vec()
    };
    let path = name.strip_prefix(prefix)?;
    (!path.is_empty()).then(|| path.to_vec())
}

/// The path of a `diff --git a/<p> b/<p>` header whose halves are equal;
/// a path may itself hold ` b/`.
fn same_halves(header: &[u8]) -> Option<Vec<u8>> {
    if header.starts_with(b"\"") {
        let (a, rest) = unquote(header)?;
        let rest = rest.strip_prefix(b" ")?;
        let b = if rest.starts_with(b"\"") {
            let (b, tail) = unquote(rest)?;
            if !tail.is_empty() {
                return None;
            }
            b
        } else {
            rest.to_vec()
        };
        let (a, b) = (a.strip_prefix(b"a/")?, b.strip_prefix(b"b/")?);
        return (a == b).then(|| a.to_vec());
    }
    let len = header.len().checked_sub(5)?;
    if len == 0 || !len.is_multiple_of(2) {
        return None;
    }
    let half = len / 2;
    let (a, b) = (&header[2..2 + half], &header[5 + half..]);
    (header.starts_with(b"a/") && &header[2 + half..5 + half] == b" b/" && a == b).then(|| a.to_vec())
}

/// A C-quoted name at the start of `raw` (git's quoting: `\\`, `\"`,
/// `\a \b \t \n \v \f \r` and three octal digits), and what follows its
/// closing quote.
fn unquote(raw: &[u8]) -> Option<(Vec<u8>, &[u8])> {
    let mut rest = raw.strip_prefix(b"\"")?;
    let mut out = Vec::new();
    loop {
        let (&b, tail) = rest.split_first()?;
        rest = tail;
        match b {
            b'"' => return Some((out, rest)),
            b'\\' => {
                let (&e, tail) = rest.split_first()?;
                rest = tail;
                let byte = match e {
                    b'a' => 7,
                    b'b' => 8,
                    b't' => b'\t',
                    b'n' => b'\n',
                    b'v' => 11,
                    b'f' => 12,
                    b'r' => b'\r',
                    b'"' | b'\\' => e,
                    b'0'..=b'3' => {
                        let [d1, d2] = [*rest.first()?, *rest.get(1)?];
                        if !(b'0'..=b'7').contains(&d1) || !(b'0'..=b'7').contains(&d2) {
                            return None;
                        }
                        rest = &rest[2..];
                        (e - b'0') * 64 + (d1 - b'0') * 8 + (d2 - b'0')
                    }
                    _ => return None,
                };
                out.push(byte);
            }
            _ => out.push(b),
        }
    }
}

/// A path G7 offers from a pull request: relative, without empty, `.` or
/// `..` parts, no control character, at most 512 bytes.
fn offered(path: &[u8]) -> bool {
    path.len() <= MAX_DIFF_PATH
        && crate::host::git::path_parts_ok(path)
        && std::str::from_utf8(path).is_ok_and(|p| !p.chars().any(is_stripped_char))
}

/// `YYYY-MM-DDTHH:MM:SS[.fff]Z` as milliseconds since the epoch.
fn iso_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    if !s.ends_with('Z') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss;
    u64::try_from(secs).ok().map(|s| s * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hosts() -> Vec<String> {
        vec!["github.com".to_owned()]
    }

    #[test]
    fn dates_read_as_milliseconds() {
        assert_eq!(iso_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_ms("2026-11-21T10:30:15Z"), Some(1_795_257_015_000));
        assert_eq!(iso_ms("2000-02-29T00:00:00.123Z"), Some(951_782_400_000));
        assert_eq!(iso_ms("2026-13-01T00:00:00Z"), None);
        assert_eq!(iso_ms("yesterday"), None);
    }

    #[test]
    fn checks_map_by_the_table_and_nothing_unknown_passes() {
        let run = |status: &str, conclusion: Value| {
            json!({"__typename": "CheckRun", "name": "build", "status": status, "conclusion": conclusion,
                   "workflowName": "CI", "detailsUrl": "https://github.com/o/r/actions/runs/1"})
        };
        let cases = [
            (run("COMPLETED", json!("SUCCESS")), CheckState::Pass),
            (run("COMPLETED", json!("NEUTRAL")), CheckState::Pass),
            (run("COMPLETED", json!("FAILURE")), CheckState::Fail),
            (run("COMPLETED", json!("TIMED_OUT")), CheckState::Fail),
            (run("COMPLETED", json!("ACTION_REQUIRED")), CheckState::Fail),
            (run("COMPLETED", json!("STARTUP_FAILURE")), CheckState::Fail),
            (run("COMPLETED", json!("SKIPPED")), CheckState::Skipped),
            (run("COMPLETED", json!("CANCELLED")), CheckState::Cancelled),
            (run("COMPLETED", json!("STALE")), CheckState::Pending),
            (run("COMPLETED", Value::Null), CheckState::Pending),
            (run("IN_PROGRESS", json!("SUCCESS")), CheckState::Pending),
            (run("QUEUED", Value::Null), CheckState::Pending),
            (json!({"__typename": "StatusContext", "context": "ci/x", "state": "SUCCESS"}), CheckState::Pass),
            (json!({"__typename": "StatusContext", "context": "ci/x", "state": "ERROR"}), CheckState::Fail),
            (
                json!({"__typename": "StatusContext", "context": "ci/x", "state": "EXPECTED"}),
                CheckState::Pending,
            ),
            (json!({"__typename": "Other", "name": "n"}), CheckState::Pending),
            (json!({}), CheckState::Pending),
        ];
        for (entry, want) in &cases {
            assert_eq!(check(entry, &hosts()).state, *want, "{entry}");
        }
        let first = check(&cases[0].0, &hosts());
        assert_eq!(first.name, "CI / build");
        assert_eq!(first.url.as_deref(), Some("https://github.com/o/r/actions/runs/1"));
        assert_eq!(check(&json!({}), &hosts()).name, "check");
        let off_host = json!({"__typename": "CheckRun", "name": "b", "status": "COMPLETED", "conclusion": "SUCCESS",
                              "detailsUrl": "https://elsewhere.test/x"});
        assert_eq!(check(&off_host, &hosts()).url, None);
    }

    #[test]
    fn urls_must_be_https_on_a_signed_in_host() {
        assert!(https_on("https://github.com/o/r/pull/3", &hosts()));
        assert!(https_on("https://GitHub.com/o/r/pull/3", &hosts()));
        assert!(!https_on("http://github.com/o/r/pull/3", &hosts()));
        assert!(!https_on("https://github.com.evil.test/x", &hosts()));
        assert!(!https_on("https://user@github.com/x", &hosts()));
        assert!(!https_on("https://github.com:8443/x", &hosts()));
        assert_eq!(github_repo("git@github.com:o/r.git", &hosts()).map(|g| g.repo).as_deref(), Some("o/r"));
        assert_eq!(github_repo("https://elsewhere.test/o/r.git", &hosts()), None);
        assert_eq!(github_repo("https://github.com/o", &hosts()), None);
    }

    #[test]
    fn the_newest_open_pull_request_wins() {
        let pr = |number: u32, state: &str, at: &str| GhPr {
            number,
            state: state.to_owned(),
            updated_at: at.to_owned(),
            head_ref_oid: "a".repeat(40),
            ..GhPr::default()
        };
        let prs = vec![
            pr(1, "MERGED", "2026-11-21T10:00:00Z"),
            pr(2, "OPEN", "2026-11-01T10:00:00Z"),
            pr(3, "CLOSED", "2026-11-20T10:00:00Z"),
        ];
        let chosen = choose(prs.into_iter().filter_map(|p| pr_info(p, &hosts())).collect()).unwrap();
        assert_eq!(chosen.number, 2);
        let only_merged =
            vec![pr(1, "MERGED", "2026-11-21T10:00:00Z"), pr(4, "MERGED", "2026-11-22T10:00:00Z")];
        assert_eq!(
            choose(only_merged.into_iter().filter_map(|p| pr_info(p, &hosts())).collect()).unwrap().number,
            4
        );
        assert!(choose(Vec::new()).is_none());
    }

    #[test]
    fn versions_and_hosts() {
        assert_eq!(
            version(b"gh version 2.102.0 (2026-09-30)\nhttps://github.com/cli/cli/releases/tag/v2.102.0\n")
                .as_deref(),
            Some("2.102.0")
        );
        assert_eq!(version(b"something else"), None);
        let auth = br#"{"hosts":{"github.com":[{"state":"success","active":true,"login":"someone"}],"ghe.test":[{"state":"error"}],"bad host":[{"state":"success"}]}}"#;
        assert_eq!(signed_in_hosts(auth), ["github.com"]);
    }

    #[test]
    fn every_ask_is_a_row_of_the_list() {
        let asks = [
            Ask::Version,
            Ask::Auth,
            Ask::OfBranch { branch: "feat/x".into() },
            Ask::Create {
                branch: "feat/x".into(),
                base: Some("main".into()),
                title: "-T".into(),
                draft: true,
            },
            Ask::SearchMine,
            Ask::SearchReview,
            Ask::OpenHere,
            Ask::RepoView,
            Ask::Ready { number: 3 },
            Ask::Merge { number: 3, method: Method::Rebase, head: "a".repeat(40) },
            Ask::View { repo: "o/r".into(), number: 7 },
            Ask::Diff { repo: "ghe.example.test/o/r".into(), number: 7 },
        ];
        for ask in &asks {
            assert!(ask.sound());
            let args: Vec<String> = gh_argv(ask).iter().map(|a| a.to_string_lossy().into_owned()).collect();
            for never in
                ["api", "--admin", "--auto", "--delete-branch", "--fill", "--web", "--editor", "--show-token"]
            {
                assert!(!args.iter().any(|a| a == never), "{ask:?}");
            }
        }
        let create: Vec<String> =
            gh_argv(&asks[3]).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            create,
            [
                "pr",
                "create",
                "--head",
                "feat/x",
                "--base",
                "main",
                "--title=-T",
                "--body-file",
                "-",
                "--draft"
            ]
        );
        assert!(!Ask::OfBranch { branch: "--web".into() }.sound());
        assert!(!Ask::Ready { number: 0 }.sound());
        assert!(!Ask::Merge { number: 3, method: Method::Squash, head: "--admin".into() }.sound());
        let merge: Vec<String> = gh_argv(&asks[9]).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let head = "a".repeat(40);
        assert_eq!(merge, ["pr", "merge", "3", "--rebase", "--match-head-commit", head.as_str()]);
        let view: Vec<String> = gh_argv(&asks[10]).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(view, ["pr", "view", "7", "--repo", "o/r", "--json", DETAIL_FIELDS]);
        let diff: Vec<String> = gh_argv(&asks[11]).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(diff, ["pr", "diff", "7", "--repo", "ghe.example.test/o/r", "--color", "never"]);
        assert_eq!((asks[10].limit(), asks[10].cap()), (CALL_LIMIT, MAX_GH_OUT));
        assert_eq!((asks[11].limit(), asks[11].cap()), (Duration::from_secs(30), 4 << 20));
    }

    /// `SCM_NEVER` as `tests/forbidden_strings.rs` lists it, unquoted.
    fn scm_never() -> Vec<String> {
        const HEAD: &str = "const SCM_NEVER: [&str; ";
        let text = include_str!("../../../tests/forbidden_strings.rs");
        let block = &text[text.find(HEAD).unwrap()..];
        let declared: usize = block[HEAD.len()..].split(']').next().unwrap().parse().unwrap();
        let block = &block[..block.find("];").unwrap()];
        let words: Vec<String> = block
            .lines()
            .skip(1)
            .filter_map(|l| l.trim().strip_prefix("\"\\\"")?.strip_suffix("\\\"\",").map(str::to_owned))
            .collect();
        assert_eq!(words.len(), declared, "every word of SCM_NEVER was read");
        words
    }

    #[test]
    fn a_read_names_its_repository() {
        let never = scm_never();
        assert!(never.len() >= 83 && never.iter().any(|w| w == "-R") && never.iter().any(|w| w == "api"));
        for (ask, verb) in [
            (Ask::View { repo: "fictional/repo".into(), number: 212 }, "view"),
            (Ask::Diff { repo: "fictional/repo".into(), number: 212 }, "diff"),
        ] {
            assert!(ask.sound());
            let args: Vec<String> = gh_argv(&ask).iter().map(|a| a.to_string_lossy().into_owned()).collect();
            assert_eq!(args[..3], ["pr", verb, "212"]);
            assert_eq!(args[3..5], ["--repo", "fictional/repo"]);
            for word in &never {
                assert!(!args.iter().any(|a| a == word), "{word} in {args:?}");
            }
        }
        assert!(DETAIL_FIELDS.starts_with(PR_FIELDS), "the card's fields, then the detail's");
        for repo in ["-x/y", "a/b/c/d", "../x", "x/..", ".x/y", "o/", "/r", "o//r", "o", "", "o/r b", "o/r\0"]
        {
            assert!(!repo_ok(repo), "{repo}");
            assert!(!Ask::View { repo: repo.into(), number: 1 }.sound(), "{repo}");
            assert!(!Ask::Diff { repo: repo.into(), number: 1 }.sound(), "{repo}");
        }
        assert!(!repo_ok(&format!("o/{}", "r".repeat(101))));
        assert!(repo_ok(&format!("o/{}", "r".repeat(100))));
        assert!(!Ask::View { repo: "o/r".into(), number: 0 }.sound());
        assert!(!Ask::Diff { repo: "o/r".into(), number: 0 }.sound());
        assert_eq!(canonical_repo("GitHub.com/o/r").as_deref(), Some("o/r"));
        assert_eq!(canonical_repo("GHE.Example.test/o/r").as_deref(), Some("ghe.example.test/o/r"));
        assert_eq!(canonical_repo("o.x/r_y-z").as_deref(), Some("o.x/r_y-z"));
        assert_eq!((repo_host("o/r"), repo_host("ghe.test/o/r")), ("github.com", "ghe.test"));
    }

    #[test]
    fn quoted_names_unquote_and_bad_ones_are_refused() {
        assert_eq!(
            unquote(br#""a\tb\"c\\d\303\251" rest"#),
            Some((b"a\tb\"c\\d\xc3\xa9".to_vec(), &b" rest"[..]))
        );
        assert_eq!(unquote(br#""no end"#), None);
        assert_eq!(unquote(br#""bad \q""#), None);
        assert_eq!(unquote(br#""bad \39x""#), None);
        assert_eq!(unquote(b"plain"), None);
        assert_eq!(side(b"b/x y\t", b"b/"), Some(b"x y".to_vec()));
        assert_eq!(side(b"/dev/null", b"b/"), None);
        assert_eq!(side(b"b/", b"b/"), None);
        assert_eq!(same_halves(b"a/x b/y b/x b/y"), Some(b"x b/y".to_vec()));
        assert_eq!(same_halves(b"a/x b/z"), None);
        assert_eq!(same_halves(br#""a/\001" "b/\001""#), Some(vec![1]));
        assert!(!offered("\u{1}".as_bytes()) && !offered(b"a/../b") && !offered(b"/abs") && !offered(b""));
        assert!(!offered("x".repeat(513).as_bytes()) && offered("x".repeat(512).as_bytes()));
        assert!(offered("dir/caf\u{e9} b/x.rs".as_bytes()));
    }

    #[test]
    fn what_a_comment_or_a_review_keeps() {
        let author = || Some(GhAuthor { login: Some("someone".into()) });
        let review = |state: &str, body: &str, at: &str| GhReview {
            author: author(),
            body: Some(body.into()),
            state: Some(state.into()),
            submitted_at: Some(at.into()),
        };
        assert!(super::review(review("COMMENTED", "  ", "2026-11-21T10:00:00Z")).is_none());
        assert!(super::review(review("PENDING", "", "2026-11-21T10:00:00Z")).is_none());
        let approved = super::review(review("APPROVED", "", "2026-11-21T10:00:00Z")).unwrap();
        assert_eq!((approved.review.as_deref(), approved.body.as_str()), (Some("approved"), ""));
        let other = super::review(review("SOMETHING_NEW", "text", "2026-11-21T10:00:00Z")).unwrap();
        assert_eq!(other.review, None);
        let nobody = GhReview { author: None, ..review("APPROVED", "ok", "2026-11-21T10:00:00Z") };
        assert!(super::review(nobody).is_none());
        let no_body = GhReview { body: None, ..review("APPROVED", "", "2026-11-21T10:00:00Z") };
        assert!(super::review(no_body).is_none());
        let c = GhComment {
            author: author(),
            body: Some("hi".into()),
            created_at: Some("bad date".into()),
            url: Some("https://elsewhere.test/x".into()),
            is_minimized: Some(false),
        };
        let c = comment(c, &hosts()).unwrap();
        assert_eq!((c.at_ms, c.url, c.review), (0, None, None));
        let hidden = GhComment {
            author: author(),
            body: Some("off topic".into()),
            is_minimized: Some(true),
            ..GhComment::default()
        };
        assert!(comment(hidden, &hosts()).is_none(), "GitHub hides it, so G7 does too");
        let read: GhComment = serde_json::from_str(
            r#"{"author":{"login":"someone"},"body":"spam","isMinimized":true,"minimizedReason":"spam"}"#,
        )
        .unwrap();
        assert!(comment(read, &hosts()).is_none(), "the flag is read from gh's own field name");
    }
}
