// SPDX-License-Identifier: MPL-2.0
//! Source control (phase 4 §SC3, §SC4): the thread's folder's git status,
//! its uncommitted diff, commits of the files a person ticks, push and
//! fetch, branches, worktrees, and pull requests through the user's `gh`.
//!
//! Restore points (`host/git`) stay as they are; this module has its own
//! closed list of git commands ([`argv`]) and runs them through
//! [`Git::spawn`](crate::host::git::Git) in two profiles: reads with every
//! program-running setting off, and a person's requests ("Act") with the
//! user's configuration, hooks and signing as they are. Nothing here
//! discards, forces, rewrites or deletes. Nothing here reads threads,
//! settings or connections: who may ask for what is the host's (§SC5.1).

pub mod argv;
pub mod branch;
pub mod clean;
pub mod commit;
pub mod gh;
pub mod patch;
pub mod push;
pub mod run;
pub mod sha256;
pub mod status;
pub mod worktree;

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::api::ErrorCode;
use crate::host::git::Git;

pub use branch::{Branches, SwitchTo, new_name_ok, worktree_branch_name};
pub use commit::{Committed, Picks, Review, ReviewFile};
pub use gh::{Method, Methods, NewPr, Show, split_diff};
pub use patch::Patch;
pub use push::Pushed;
pub use run::{Lines, MAX_JOB_BYTES, MAX_JOB_LINES, Stop};
pub use worktree::{Listed, Unpushed, worktree_size};

use argv::Op;

/// Files kept in a status (§SC7.4).
pub const MAX_STATUS_FILES: usize = 2000;
/// Remotes listed in a status.
pub const MAX_REMOTES: usize = 8;
/// Branches listed.
pub const MAX_BRANCHES: usize = 200;
/// A commit message, in bytes after trimming.
pub const MAX_MESSAGE: usize = 8192;
/// How long a name read from `Refs` or `Remotes` may be taken from a client.
const NAMES_FRESH: Duration = Duration::from_secs(60);
/// Repositories whose names are remembered.
const NAMES_KEPT: usize = 64;

// ---- Types the host returns (in api.rs since H1) ----

pub use crate::api::{
    Branch, Check, CheckCounts, CheckState, Doing, GhState, GhUse, GitChange as Change, GitFile, GitHub,
    GitStatus, LastCommit, Mergeable, PrInfo, PrRow, PrState, Remote, RepoState, ReviewState, Staged,
    WorktreeTag,
};

impl GitStatus {
    /// The status of a folder that isn't a repository G7 works in.
    pub fn without(repo: RepoState) -> GitStatus {
        GitStatus { repo, checked_ms: now_ms(), ..GitStatus::default() }
    }

    /// Files with changes, of those kept and the ones past the cap.
    pub fn changed(&self) -> u32 {
        self.total
    }
}

// ---- Errors ----

/// Why source control didn't do something: a word for clients (§SC3.4,
/// §SC3.5, §SC3.7, §SC3.8, §SC4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    GitBusy,
    Doing,
    Detached,
    Conflicts,
    Nothing,
    Identity,
    Changed,
    Hook,
    Failed,
    Stopped,
    Slow,
    Behind,
    RemoteRefused,
    SignIn,
    Network,
    PickRemote,
    NoRemote,
    NoCommits,
    NoUpstream,
    Dirty,
    WouldOverwrite,
    InWorktree,
    Exists,
    BadName,
    NotFound,
    BadMessage,
    BadRequest,
    Large,
    NotRepo,
    Refused,
    NoGit,
    NotPushed,
    NotGithub,
    NoGh,
    SignedOut,
    OldGh,
    PrExists,
    NotAllowed,
    Unavailable,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::GitBusy => "git_busy",
            Reason::Doing => "doing",
            Reason::Detached => "detached",
            Reason::Conflicts => "conflicts",
            Reason::Nothing => "nothing",
            Reason::Identity => "identity",
            Reason::Changed => "changed",
            Reason::Hook => "hook",
            Reason::Failed => "failed",
            Reason::Stopped => "stopped",
            Reason::Slow => "slow",
            Reason::Behind => "behind",
            Reason::RemoteRefused => "remote_refused",
            Reason::SignIn => "sign_in",
            Reason::Network => "network",
            Reason::PickRemote => "pick_remote",
            Reason::NoRemote => "no_remote",
            Reason::NoCommits => "no_commits",
            Reason::NoUpstream => "no_upstream",
            Reason::Dirty => "dirty",
            Reason::WouldOverwrite => "would_overwrite",
            Reason::InWorktree => "in_worktree",
            Reason::Exists => "exists",
            Reason::BadName => "bad_name",
            Reason::NotFound => "not_found",
            Reason::BadMessage => "bad_message",
            Reason::BadRequest => "bad_request",
            Reason::Large => "large",
            Reason::NotRepo => "not_repo",
            Reason::Refused => "refused",
            Reason::NoGit => "no_git",
            Reason::NotPushed => "not_pushed",
            Reason::NotGithub => "not_github",
            Reason::NoGh => "no_gh",
            Reason::SignedOut => "signed_out",
            Reason::OldGh => "old_gh",
            Reason::PrExists => "pr_exists",
            Reason::NotAllowed => "not_allowed",
            Reason::Unavailable => "unavailable",
        }
    }

    /// The error code a method answers with when it refuses for this reason
    /// (§SC7.2); a job that ran and failed carries the word instead.
    pub fn code(self) -> ErrorCode {
        match self {
            Reason::GitBusy => ErrorCode::Busy,
            Reason::Changed | Reason::PrExists => ErrorCode::Conflict,
            Reason::NoGit
            | Reason::NoGh
            | Reason::SignedOut
            | Reason::OldGh
            | Reason::Network
            | Reason::Slow
            | Reason::Unavailable => ErrorCode::Unavailable,
            Reason::NotFound => ErrorCode::NotFound,
            Reason::Large => ErrorCode::Limit,
            _ => ErrorCode::BadRequest,
        }
    }
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A refusal or a failure, with the lines a client may show (cleaned): a
/// job's output, or the names a client picks from (`pick_remote`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScmError {
    pub reason: Reason,
    pub lines: Vec<String>,
}

impl ScmError {
    pub fn new(reason: Reason) -> ScmError {
        ScmError { reason, lines: Vec::new() }
    }

    pub fn with_lines(reason: Reason, lines: Vec<String>) -> ScmError {
        ScmError { reason, lines }
    }
}

impl From<Reason> for ScmError {
    fn from(reason: Reason) -> ScmError {
        ScmError::new(reason)
    }
}

impl std::fmt::Display for ScmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason.as_str())
    }
}

impl std::error::Error for ScmError {}

/// Why a folder has no [`Spot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoRepo {
    /// Not in a git work tree (or gone).
    None,
    /// Git says it isn't safe (another owner), or it lands outside the
    /// folders G7 works in.
    Refused,
    /// Git can't be run.
    NoGit,
}

impl NoRepo {
    pub fn repo(self) -> RepoState {
        match self {
            NoRepo::None => RepoState::None,
            NoRepo::Refused => RepoState::Refused,
            NoRepo::NoGit => RepoState::NoGit,
        }
    }
}

// ---- Where a thread works ----

/// A thread's working directory in its repository: the folder, the work
/// tree's top level, its git directory and the common one (the same, except
/// in a linked worktree). Cheap to clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spot {
    folder: PathBuf,
    top: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
    /// The folder from the top level: empty, or ending in `/`.
    prefix: Vec<u8>,
}

impl Spot {
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn top(&self) -> &Path {
        &self.top
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    /// A linked worktree (its `.git` is a file).
    pub fn linked(&self) -> bool {
        self.git_dir != self.common_dir
    }

    pub(crate) fn prefix(&self) -> &[u8] {
        &self.prefix
    }
}

// ---- Names read from the repository ----

/// One ref `Refs` listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefRow {
    /// `main` for `refs/heads/main`; `origin/feat` for `refs/remotes/origin/feat`.
    pub name: String,
    /// The remote of a remote-tracking ref.
    pub remote: Option<String>,
    /// The branch name on the remote (`feat` of `origin/feat`).
    pub short: String,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub last_ms: u64,
    pub head: bool,
    pub worktree: Option<PathBuf>,
}

/// What the host read of one repository, for looking up names a client sends.
#[derive(Debug, Clone, Default)]
pub(crate) struct Names {
    refs: Option<(Instant, Vec<RefRow>)>,
    remotes: Option<(Instant, Vec<(String, String)>)>,
}

/// Source control for the host. One per host, built with the same
/// environment as [`Git`]; `gh` is looked for on first use.
pub struct Scm {
    git: Arc<Git>,
    /// The user's home folder, written `~` in lines.
    home: Option<String>,
    /// The user's own `LC_ALL`, for Act calls (reads run in the C locale).
    locale: Option<OsString>,
    names: Mutex<HashMap<PathBuf, Names>>,
    gh: gh::Gh,
}

impl std::fmt::Debug for Scm {
    /// Never the remotes' URLs or the environment.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scm").field("git", &self.git.bin()).field("gh", &self.gh).finish_non_exhaustive()
    }
}

impl Scm {
    /// `env`: the host's environment (the same as [`Git::new`]'s).
    pub fn new(git: Arc<Git>, env: &[(OsString, OsString)]) -> Scm {
        let get = |name: &str| env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        let home =
            get("HOME").filter(|h| Path::new(h).is_absolute()).map(|h| h.to_string_lossy().into_owned());
        Scm {
            git,
            home,
            locale: get("LC_ALL").filter(|v| !v.is_empty()),
            names: Mutex::default(),
            gh: gh::Gh::new(env),
        }
    }

    pub fn git(&self) -> &Arc<Git> {
        &self.git
    }

    /// Where `folder` sits in its repository (`Top`).
    pub async fn spot(&self, folder: &Path) -> Result<Spot, NoRepo> {
        if !folder.is_absolute() {
            return Err(NoRepo::None);
        }
        let out = match self.read(&Op::Top, folder, None).await {
            Ok(out) => out,
            Err(e) if e.reason == Reason::NoGit => return Err(NoRepo::NoGit),
            Err(_) => return Err(NoRepo::None),
        };
        if out.code != Some(0) {
            // Read calls run in the C locale: this is git's own word.
            if out.stderr.contains("dubious ownership") {
                return Err(NoRepo::Refused);
            }
            return Err(NoRepo::None);
        }
        let lines: Vec<&[u8]> = out.stdout.split(|&b| b == b'\n').collect();
        let [top, git_dir, common_dir, prefix, end] = lines.as_slice() else { return Err(NoRepo::None) };
        let path = |raw: &[u8]| PathBuf::from(OsStr::from_bytes(raw));
        let (top, git_dir, common_dir) = (path(top), path(git_dir), path(common_dir));
        let prefix_ok =
            prefix.is_empty() || prefix.strip_suffix(b"/").is_some_and(crate::host::git::path_parts_ok);
        if !end.is_empty() || !prefix_ok || ![&top, &git_dir, &common_dir].iter().all(|p| p.is_absolute()) {
            return Err(NoRepo::None);
        }
        Ok(Spot { folder: folder.to_owned(), top, git_dir, common_dir, prefix: prefix.to_vec() })
    }

    /// [`Scm::spot`], refused unless the repository is the agent folder's
    /// own (the same top level) or a worktree under `worktrees` (§SC3.2).
    pub async fn spot_within(
        &self,
        folder: &Path,
        agent_folder: &Path,
        worktrees: &Path,
    ) -> Result<Spot, NoRepo> {
        let spot = self.spot(folder).await?;
        if spot.top.starts_with(worktrees) && worktrees.is_absolute() {
            return Ok(spot);
        }
        if folder == agent_folder {
            return Ok(spot);
        }
        match self.spot(agent_folder).await {
            Ok(agent) if agent.top == spot.top => Ok(spot),
            Ok(_) | Err(NoRepo::None) | Err(NoRepo::Refused) => Err(NoRepo::Refused),
            Err(NoRepo::NoGit) => Err(NoRepo::NoGit),
        }
    }

    // ---- The names cache ----

    fn with_names<T>(&self, spot: &Spot, f: impl FnOnce(&mut Names) -> T) -> T {
        let mut names = self.names.lock().unwrap_or_else(PoisonError::into_inner);
        if names.len() >= NAMES_KEPT && !names.contains_key(&spot.common_dir) {
            names.retain(|_, n| {
                n.refs.as_ref().is_some_and(|(at, _)| at.elapsed() < NAMES_FRESH)
                    || n.remotes.as_ref().is_some_and(|(at, _)| at.elapsed() < NAMES_FRESH)
            });
            if names.len() >= NAMES_KEPT {
                names.clear();
            }
        }
        f(names.entry(spot.common_dir.clone()).or_default())
    }

    /// The remotes as `(name, url)`, read now; remembered for lookups.
    pub(crate) async fn remotes(&self, spot: &Spot) -> Result<Vec<(String, String)>, ScmError> {
        let out = self.read(&Op::Remotes, &spot.folder, None).await?;
        // Exit 1: no remote configured.
        if out.code != Some(0) && out.code != Some(1) {
            return Err(ScmError::new(Reason::Failed));
        }
        let mut remotes: Vec<(String, String)> = Vec::new();
        for line in out.stdout.split(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(line);
            let Some((key, url)) = line.split_once(' ') else { continue };
            let Some(name) = key.strip_prefix("remote.").and_then(|k| k.strip_suffix(".url")) else {
                continue;
            };
            if name.is_empty() || name.len() > 64 || remotes.iter().any(|(n, _)| n == name) {
                continue;
            }
            remotes.push((name.to_owned(), url.to_owned()));
        }
        let kept = remotes.clone();
        self.with_names(spot, |n| n.remotes = Some((Instant::now(), kept)));
        Ok(remotes)
    }

    /// The refs as [`RefRow`]s, read now; remembered for lookups.
    pub(crate) async fn refs(&self, spot: &Spot) -> Result<Vec<RefRow>, ScmError> {
        let remotes = self.remotes(spot).await?;
        let out = self.read(&Op::Refs, &spot.folder, None).await?;
        if out.code != Some(0) {
            return Err(ScmError::new(Reason::Failed));
        }
        let rows = branch::parse_refs(&out.stdout, &remotes);
        let kept = rows.clone();
        self.with_names(spot, |n| n.refs = Some((Instant::now(), kept)));
        Ok(rows)
    }

    /// The refs read in the last minute, else read now.
    pub(crate) async fn known_refs(&self, spot: &Spot) -> Result<Vec<RefRow>, ScmError> {
        let fresh = self.with_names(spot, |n| {
            n.refs.clone().filter(|(at, _)| at.elapsed() < NAMES_FRESH).map(|(_, r)| r)
        });
        match fresh {
            Some(rows) => Ok(rows),
            None => self.refs(spot).await,
        }
    }

    /// The remotes read in the last minute, else read now.
    pub(crate) async fn known_remotes(&self, spot: &Spot) -> Result<Vec<(String, String)>, ScmError> {
        let fresh = self.with_names(spot, |n| {
            n.remotes.clone().filter(|(at, _)| at.elapsed() < NAMES_FRESH).map(|(_, r)| r)
        });
        match fresh {
            Some(remotes) => Ok(remotes),
            None => self.remotes(spot).await,
        }
    }

    /// Forgets what was read of the repository (after a switch, a push or
    /// a new worktree).
    pub(crate) fn forget_names(&self, spot: &Spot) {
        self.with_names(spot, |n| *n = Names::default());
    }

    pub(crate) fn home(&self) -> Option<&str> {
        self.home.as_deref()
    }
}

/// The remote a push or fetch of this branch goes to (§SC3.5): `asked`
/// when it is a known remote; else the upstream's; else `origin`; else the
/// only one. `PickRemote` carries the names (at most 8).
pub(crate) fn pick_remote(
    remotes: &[(String, String)],
    upstream_remote: Option<&str>,
    asked: Option<&str>,
) -> Result<String, ScmError> {
    if remotes.is_empty() {
        return Err(ScmError::new(Reason::NoRemote));
    }
    let known = |name: &str| remotes.iter().any(|(n, _)| n == name);
    if let Some(asked) = asked {
        return if known(asked) { Ok(asked.to_owned()) } else { Err(ScmError::new(Reason::NotFound)) };
    }
    if let Some(up) = upstream_remote.filter(|r| known(r)) {
        return Ok(up.to_owned());
    }
    if known("origin") {
        return Ok("origin".to_owned());
    }
    if let [(only, _)] = remotes {
        return Ok(only.clone());
    }
    Err(ScmError::with_lines(
        Reason::PickRemote,
        remotes.iter().take(MAX_REMOTES).map(|(n, _)| n.clone()).collect(),
    ))
}

/// The remote of an upstream ref (`refs/remotes/<remote>/…`): the longest
/// remote name that fits.
pub(crate) fn remote_of(upstream_ref: &str, remotes: &[(String, String)]) -> Option<String> {
    let rest = upstream_ref.strip_prefix("refs/remotes/")?;
    remotes
        .iter()
        .map(|(n, _)| n)
        .filter(|n| rest.strip_prefix(n.as_str()).is_some_and(|r| r.starts_with('/')))
        .max_by_key(|n| n.len())
        .cloned()
}

/// A remote's URL as people may see it: host and path, no scheme, user,
/// secret or port; `local` for a folder on this PC.
pub(crate) fn remote_label(url: &str, home: Option<&str>) -> (String, bool) {
    let url = url.trim();
    let local_path = url.strip_prefix("file://").or_else(|| url.starts_with('/').then_some(url));
    if let Some(path) = local_path {
        let line = clean::line(path.as_bytes(), home);
        return (crate::api::cut(&line, 140).to_owned(), true);
    }
    let (host, path) = match url_host_path(url) {
        Some(found) => found,
        None => return (crate::api::cut(&clean::line(url.as_bytes(), home), 140).to_owned(), true),
    };
    let path = path.trim_start_matches('/').trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let label = if path.is_empty() { host } else { format!("{host}/{path}") };
    (crate::api::cut(&clean::text(&label, 140), 140).to_owned(), false)
}

/// The host (lower case, no user or port) and path of a remote URL:
/// `scheme://[user[:secret]@]host[:port]/path` or `[user@]host:path`.
pub(crate) fn url_host_path(url: &str) -> Option<(String, String)> {
    let (authority, path) = if let Some((_, rest)) = url.split_once("://") {
        let end = rest.find('/').unwrap_or(rest.len());
        (&rest[..end], &rest[end..])
    } else {
        let (authority, path) = url.split_once(':')?;
        if authority.contains('/') {
            return None;
        }
        (authority, path)
    };
    let host = authority.rsplit('@').next()?;
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => host.split(':').next()?,
    };
    let ok = !host.is_empty()
        && host.len() <= 255
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'));
    ok.then(|| (host.to_ascii_lowercase(), path.to_owned()))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    crate::host::git::hex(bytes)
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// A path's bytes as UTF-8 for clients (lossy).
pub(crate) fn lossy(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remotes(names: &[&str]) -> Vec<(String, String)> {
        names.iter().map(|n| ((*n).to_owned(), format!("https://example.test/{n}.git"))).collect()
    }

    #[test]
    fn the_remote_is_the_upstreams_then_origin_then_the_only_one() {
        assert_eq!(pick_remote(&[], None, None).unwrap_err().reason, Reason::NoRemote);
        assert_eq!(pick_remote(&remotes(&["up", "origin"]), Some("up"), None).unwrap(), "up");
        assert_eq!(pick_remote(&remotes(&["up", "origin"]), None, None).unwrap(), "origin");
        assert_eq!(pick_remote(&remotes(&["fork"]), None, None).unwrap(), "fork");
        let two = pick_remote(&remotes(&["a", "b"]), None, None).unwrap_err();
        assert_eq!((two.reason, two.lines), (Reason::PickRemote, vec!["a".to_owned(), "b".to_owned()]));
        assert_eq!(pick_remote(&remotes(&["a", "b"]), None, Some("b")).unwrap(), "b");
        assert_eq!(
            pick_remote(&remotes(&["a"]), None, Some("--upload-pack=x")).unwrap_err().reason,
            Reason::NotFound
        );
    }

    #[test]
    fn an_upstreams_remote_is_the_longest_that_fits() {
        let r = remotes(&["o", "o/x"]);
        assert_eq!(remote_of("refs/remotes/o/x/main", &r).as_deref(), Some("o/x"));
        assert_eq!(remote_of("refs/remotes/o/main", &r).as_deref(), Some("o"));
        assert_eq!(remote_of("refs/heads/main", &r), None);
    }

    #[test]
    fn labels_never_hold_a_user_a_secret_or_a_scheme() {
        assert_eq!(
            remote_label("https://kim:pw@GitHub.com/o/r.git", None),
            ("github.com/o/r".to_owned(), false)
        );
        assert_eq!(remote_label("git@github.com:o/r.git", None), ("github.com/o/r".to_owned(), false));
        assert_eq!(
            remote_label("ssh://git@host.test:2222/srv/r", None),
            ("host.test/srv/r".to_owned(), false)
        );
        assert_eq!(remote_label("/home/kim/bare.git", Some("/home/kim")), ("~/bare.git".to_owned(), true));
        assert_eq!(remote_label("file:///srv/x.git", None), ("/srv/x.git".to_owned(), true));
        assert_eq!(url_host_path("https://[::1]:8080/x").map(|(h, _)| h).as_deref(), Some("::1"));
    }
}
