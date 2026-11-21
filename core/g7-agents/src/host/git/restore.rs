// SPDX-License-Identifier: MPL-2.0
//! Restore (§U5.5; reconcile §2-§4): the only code in G7 that changes the
//! user's files.
//!
//! A plan compares the target restore point with a snapshot of the folder
//! taken now, path by path, and changes nothing. Applying it takes the
//! snapshot again and refuses if the folder changed; keeps that snapshot as
//! the thread's undo point; then checks every path itself before anything is
//! written: its parents must be real directories (not links, not files), the
//! path must be what the snapshot holds (hashed, a link by its text), and a
//! path to bring back must still be absent. Only then are files deleted (by
//! this code, one by one, never following a link) and written back (by
//! `git restore --overlay` from the target, to the work tree only).
//!
//! The checks are not optional: `git restore` deletes a directory standing
//! where a file is restored, untracked files in it included, and unlinks a
//! file standing where a parent directory is needed (see `run.rs`).
//!
//! Never: `git clean`, `reset`, `checkout`, `stash`, `--staged`, a force
//! flag, or a write to the user's index, `HEAD` or a branch. Ignored files,
//! files outside the folder, submodules and files left out of either restore
//! point are in no plan.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::io::Errno;

use super::checkpoint::{
    Meta, Recorded, Work, git_dir, hold, metas, parse_pairs, probe, record, set_ref, spec_of, split_z,
};
use super::run::{Call, GitError};
use super::{Place, SNAPSHOT_BUDGET, folder_path, oid_ok, preview_ref, undo_ref};
use crate::api::{CheckpointWhy, MAX_TURN_FILES, RewindAction, RewindBlock, RewindFile, RewindFiles};

/// How long a preview waits for another git command's lock to go.
const BUSY_WAIT: Duration = Duration::from_secs(2);
const BUSY_POLL: Duration = Duration::from_millis(100);
const CHECK_LIMIT: Duration = Duration::from_secs(5);
const APPLY_LIMIT: Duration = Duration::from_secs(30);
/// `rev-list --objects` over the folder of a restore point.
const MAX_OBJECTS_OUT: usize = 32 << 20;
/// Paths per `hash-object` call, and their bytes.
const HASH_BATCH: usize = 500;
const HASH_BATCH_BYTES: usize = 64 << 10;
const GITLINK: &str = "160000";
const NO_MODE: &str = "000000";

/// Why the files part of a rewind can't be planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    NotGit,
    /// Another git command holds the folder's repository, or a rebase,
    /// merge, cherry-pick, revert or bisect is in progress there.
    Busy,
    /// The restore point, or an object it needs, is gone; or the folder is
    /// now in another repository.
    Expired,
    /// The folder as it is now couldn't be recorded: no rewind without a way back.
    Unrecorded(CheckpointWhy),
    /// Git failed.
    Failed,
}

impl Blocked {
    /// The reason word a client shows.
    pub fn block(self) -> RewindBlock {
        match self {
            Blocked::NotGit => RewindBlock::NotGit,
            Blocked::Busy | Blocked::Unrecorded(CheckpointWhy::Busy) => RewindBlock::Busy,
            Blocked::Expired => RewindBlock::Expired,
            Blocked::Unrecorded(_) | Blocked::Failed => RewindBlock::NoCheckpoint,
        }
    }
}

/// A blob in a tree: its mode and id.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Blob {
    mode: String,
    oid: String,
}

/// One path of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanFile {
    /// Relative to the folder.
    pub path: String,
    pub action: RewindAction,
    /// Changed after the agent's last turn: left alone unless asked.
    pub later: bool,
    /// From the top level, as git has it.
    raw: Vec<u8>,
    /// In the snapshot taken for the plan.
    now: Option<Blob>,
    /// In the target.
    then: Option<Blob>,
}

/// What a rewind would do to the files. Held by the host between the
/// preview and the apply; changes nothing by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePlan {
    pub thread: String,
    /// The target restore point (a commit id).
    pub target: String,
    /// The snapshot taken for the plan (a commit id).
    pub snapshot: String,
    /// The folder in the snapshot (a tree id): apply refuses when it changed.
    pub tree: String,
    /// Every path that would change, in git's order.
    pub files: Vec<PlanFile>,
    /// Files left out of the target or the snapshot: never touched.
    pub skipped: u32,
    /// `HEAD` differs from the target's: files go back, the branch stays.
    pub head_moved: bool,
    /// Paths a restore point holds that G7 won't touch (names that leave
    /// the folder or are too long): never listed, never touched.
    pub hidden: u32,
    top: PathBuf,
    prefix: Vec<u8>,
}

impl RestorePlan {
    /// The plan as a client sees it (`RewindPlan.files`).
    pub fn wire(&self) -> RewindFiles {
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        RewindFiles {
            change: self
                .files
                .iter()
                .take(MAX_TURN_FILES)
                .map(|f| RewindFile { path: f.path.clone(), action: f.action, later: f.later })
                .collect(),
            more: count(self.files.len().saturating_sub(MAX_TURN_FILES)),
            later: count(self.files.iter().filter(|f| f.later).count()),
            skipped: self.skipped,
            head_moved: self.head_moved,
            tree: self.tree.clone(),
        }
    }
}

/// A path a rewind left as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Left {
    /// Relative to the folder.
    pub path: String,
    pub why: LeftWhy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftWhy {
    /// Changed after the agent's last turn, and not included.
    Later,
    /// Not what the snapshot holds any more.
    Changed,
    /// A folder stands there now.
    Folder,
    /// Not a regular file or a link (a FIFO, a device), or a parent is not a folder.
    NotFile,
    /// It couldn't be changed.
    Failed,
}

impl LeftWhy {
    /// The reason line (reconcile §11), at most `MAX_LEFT_WHY` bytes.
    pub fn text(self) -> &'static str {
        match self {
            LeftWhy::Later => "changed after the agent finished",
            LeftWhy::Changed => "changed since you looked",
            LeftWhy::Folder => "changed into a folder",
            LeftWhy::NotFile => "not a file",
            LeftWhy::Failed => "couldn't be changed back",
        }
    }
}

/// Plans putting the folder back to `target` (a turn's *pre*; a ref of G7's
/// or a commit id). `last_post`: the thread's newest *post*, against which
/// paths changed since are marked `later`; `skipped`: the target turn's
/// left-out paths (relative to the folder). Takes a snapshot of the folder
/// (the thread's preview ref) and changes nothing else.
pub async fn plan_restore(
    place: &Place,
    thread: &str,
    target: &str,
    last_post: Option<&str>,
    skipped: &[String],
) -> Result<RestorePlan, Blocked> {
    let deadline = Instant::now() + SNAPSHOT_BUDGET + Duration::from_secs(10);
    let snapshot_ref = preview_ref(thread).ok_or(Blocked::Failed)?;
    let work = probe(place, deadline).await.map_err(|e| match e {
        GitError::NotGit => Blocked::NotGit,
        _ => Blocked::Failed,
    })?;
    let mut revs = vec![target];
    revs.extend(last_post);
    let mut found = metas(place, &revs, deadline).await.map_err(|e| match e {
        GitError::Gone => Blocked::Expired,
        _ => Blocked::Failed,
    })?;
    let last = found.pop_if(|_| last_post.is_some()).flatten();
    let then = found.pop().flatten().ok_or(Blocked::Expired)?;
    if !same_place(&then, &work) {
        return Err(Blocked::Expired);
    }
    let waited = Instant::now() + BUSY_WAIT;
    while work.busy() {
        if Instant::now() >= waited {
            return Err(Blocked::Busy);
        }
        tokio::time::sleep(BUSY_POLL).await;
    }
    let _held = hold(place, &work, deadline).await.map_err(|_| Blocked::Busy)?;
    if work.busy() {
        return Err(Blocked::Busy);
    }
    all_present(place, &then, deadline).await?;
    let now = record(place, &work, Some(&snapshot_ref), deadline).await.map_err(|e| {
        Blocked::Unrecorded(match e {
            GitError::Busy => CheckpointWhy::Busy,
            GitError::Slow => CheckpointWhy::Slow,
            GitError::Large => CheckpointWhy::Large,
            _ => CheckpointWhy::Refused,
        })
    })?;
    let pairs =
        changes(place, &then.oid, &now.commit, &work.prefix, deadline).await.map_err(|_| Blocked::Failed)?;
    // Paths changed since the agent's last turn; all of them when that
    // restore point is gone.
    let later: Option<HashSet<Vec<u8>>> = match (last_post, &last) {
        (None, _) => Some(HashSet::new()),
        (Some(_), None) => None,
        (Some(_), Some(last)) => {
            let mut set = changed_names(place, &last.oid, &now.commit, &work.prefix, deadline)
                .await
                .map_err(|_| Blocked::Failed)?;
            set.extend(last.skipped.iter().cloned());
            Some(set)
        }
    };
    Ok(build(thread, &then, &now, pairs, later.as_ref(), skipped, &work))
}

fn same_place(meta: &Meta, work: &Work) -> bool {
    meta.top == work.top.as_os_str().as_bytes() && meta.prefix == work.prefix
}

fn build(
    thread: &str,
    then: &Meta,
    now: &Recorded,
    pairs: Vec<super::checkpoint::RawPair>,
    later: Option<&HashSet<Vec<u8>>>,
    skipped: &[String],
    work: &Work,
) -> RestorePlan {
    let left_out: HashSet<&[u8]> = then.skipped.iter().chain(&now.skipped).map(Vec::as_slice).collect();
    let named: HashSet<&str> = skipped.iter().map(String::as_str).collect();
    let mut unrecorded: HashSet<String> =
        left_out.iter().filter_map(|raw| folder_path(raw, &work.prefix)).collect();
    unrecorded.extend(named.iter().map(|s| (*s).to_owned()));
    let mut files = Vec::new();
    let mut hidden = 0u32;
    for pair in pairs {
        if pair.old_mode == GITLINK || pair.new_mode == GITLINK || left_out.contains(pair.path.as_slice()) {
            continue;
        }
        let Some(path) = folder_path(&pair.path, &work.prefix) else {
            hidden = hidden.saturating_add(1);
            continue;
        };
        if named.contains(path.as_str()) {
            continue;
        }
        let blob = |mode: &str, oid: &str| {
            (mode != NO_MODE && oid_ok(oid)).then(|| Blob { mode: mode.to_owned(), oid: oid.to_owned() })
        };
        let (then_blob, now_blob) =
            (blob(&pair.old_mode, &pair.old_oid), blob(&pair.new_mode, &pair.new_oid));
        let action = match (&then_blob, &now_blob) {
            (None, Some(_)) => RewindAction::Delete,
            (Some(_), None) => RewindAction::BringBack,
            (Some(_), Some(_)) => RewindAction::Restore,
            (None, None) => {
                hidden = hidden.saturating_add(1);
                continue;
            }
        };
        let later = later.is_none_or(|set| set.contains(pair.path.as_slice()));
        files.push(PlanFile { path, action, later, raw: pair.path, now: now_blob, then: then_blob });
    }
    RestorePlan {
        thread: thread.to_owned(),
        target: then.oid.clone(),
        snapshot: now.commit.clone(),
        tree: now.tree.clone(),
        files,
        skipped: u32::try_from(unrecorded.len()).unwrap_or(u32::MAX),
        head_moved: then.head != now.head,
        hidden,
        top: work.top.clone(),
        prefix: work.prefix.clone(),
    }
}

/// Every object of the folder in `meta` is in G7's repository or the user's.
/// Shared objects can go with the user's own `git gc` after a branch is
/// dropped: then the restore point is gone.
async fn all_present(place: &Place, meta: &Meta, deadline: Instant) -> Result<(), Blocked> {
    let call = Call::new(&place.repo, CHECK_LIMIT)
        .arg(git_dir(place))
        .args(["rev-list", "--objects", "--missing=print", &meta.oid, "--"])
        .args(spec_of(&meta.prefix))
        .cap(MAX_OBJECTS_OUT);
    let out = place.git.run(call, deadline).await.map_err(|_| Blocked::Failed)?;
    if out.over {
        return Err(Blocked::Failed);
    }
    if !out.ok() || out.stdout.split(|&b| b == b'\n').any(|line| line.starts_with(b"?")) {
        return Err(Blocked::Expired);
    }
    Ok(())
}

/// The blobs a restore will write are all readable.
async fn blobs_present(place: &Place, oids: &[&str], deadline: Instant) -> Result<(), GitError> {
    if oids.is_empty() {
        return Ok(());
    }
    let input: Vec<u8> = oids.iter().flat_map(|oid| format!("{oid}\n").into_bytes()).collect();
    let call = Call::new(&place.repo, CHECK_LIMIT)
        .arg(git_dir(place))
        .args(["cat-file", "--batch-check=%(objectname) %(objecttype)"])
        .input(input)
        .cap(MAX_OBJECTS_OUT);
    let out = place.git.run(call, deadline).await?.check()?;
    let lines: Vec<&[u8]> = out.stdout.split(|&b| b == b'\n').filter(|l| !l.is_empty()).collect();
    if lines.len() != oids.len() || lines.iter().any(|l| !l.ends_with(b" blob")) {
        return Err(GitError::Gone);
    }
    Ok(())
}

async fn changes(
    place: &Place,
    from: &str,
    to: &str,
    prefix: &[u8],
    deadline: Instant,
) -> Result<Vec<super::checkpoint::RawPair>, GitError> {
    let call = Call::new(&place.repo, CHECK_LIMIT)
        .arg(git_dir(place))
        .args(["diff-tree", "-r", "--no-renames", "--raw", "-z", from, to, "--"])
        .args(spec_of(prefix));
    let out = place.git.run(call, deadline).await?;
    if out.over {
        return Err(GitError::Large);
    }
    let out = out.check()?;
    let (pairs, _, cut) = parse_pairs(&out.stdout);
    if cut { Err(GitError::Failed("unreadable diff".into())) } else { Ok(pairs) }
}

async fn changed_names(
    place: &Place,
    from: &str,
    to: &str,
    prefix: &[u8],
    deadline: Instant,
) -> Result<HashSet<Vec<u8>>, GitError> {
    let call = Call::new(&place.repo, CHECK_LIMIT)
        .arg(git_dir(place))
        .args(["diff-tree", "-r", "--no-renames", "--name-only", "-z", from, to, "--"])
        .args(spec_of(prefix));
    let out = place.git.run(call, deadline).await?;
    if out.over {
        return Err(GitError::Large);
    }
    Ok(split_z(&out.check()?.stdout).map(<[u8]>::to_vec).collect())
}

/// Applies a plan: `later` paths only with `include_later`. Returns the
/// paths left as they were. [`GitError::Changed`]: the folder changed since
/// the plan (nothing done); [`GitError::Busy`]: git is busy there (nothing
/// done); [`GitError::Gone`]: an object needed is gone (nothing done). An
/// error after files began to change leaves the snapshot at the thread's
/// undo ref, which holds everything as it was.
pub async fn apply_restore(
    place: &Place,
    plan: &RestorePlan,
    include_later: bool,
) -> Result<Vec<Left>, GitError> {
    let deadline = Instant::now() + SNAPSHOT_BUDGET + APPLY_LIMIT;
    let (Some(check_ref), Some(undo)) = (preview_ref(&plan.thread), undo_ref(&plan.thread)) else {
        return Err(GitError::Failed("not a thread id".into()));
    };
    let work = probe(place, deadline).await?;
    if work.top != plan.top || work.prefix != plan.prefix {
        return Err(GitError::Moved);
    }
    if work.busy() {
        return Err(GitError::Busy);
    }
    let _held = hold(place, &work, deadline).await?;
    if work.busy() {
        return Err(GitError::Busy);
    }
    let (todo, kept): (Vec<&PlanFile>, Vec<&PlanFile>) =
        plan.files.iter().partition(|f| include_later || !f.later);
    let mut left: Vec<Left> =
        kept.iter().map(|f| Left { path: f.path.clone(), why: LeftWhy::Later }).collect();
    let needed: Vec<&str> = todo.iter().filter_map(|f| f.then.as_ref().map(|b| b.oid.as_str())).collect();
    blobs_present(place, &needed, deadline).await?;
    let now = record(place, &work, Some(&check_ref), deadline).await?;
    if now.tree != plan.tree {
        return Err(GitError::Changed);
    }
    set_ref(place, &undo, &now.commit, deadline).await?;

    // Look at every path before anything changes.
    let paths: Vec<Vec<u8>> = todo.iter().map(|f| f.raw.clone()).collect();
    let seen = look_all(&work, paths.clone()).await?;
    let hashes = hash_seen(place, &work, &paths, &seen, deadline).await?;
    let mut deletes = Vec::new();
    let mut writes = Vec::new();
    for (k, file) in todo.iter().enumerate() {
        let is =
            |want: &Option<Blob>| want.as_ref().is_some_and(|b| hashes[k].as_deref() == Some(b.oid.as_str()));
        let verdict = match (file.action, &seen[k]) {
            (_, Seen::Left(why)) => Err(*why),
            (RewindAction::Delete, Seen::Absent) => continue,
            (RewindAction::Delete, _) if is(&file.now) => Ok(true),
            (RewindAction::Restore, Seen::File | Seen::Link(_)) if is(&file.now) => Ok(false),
            (RewindAction::BringBack, Seen::Absent) => Ok(false),
            _ => Err(LeftWhy::Changed),
        };
        match verdict {
            Ok(true) => deletes.push(*file),
            Ok(false) => writes.push(*file),
            Err(why) => left.push(Left { path: file.path.clone(), why }),
        }
    }

    // Deletes first: a folder that empties may be where a file comes back.
    let prefix_len = work.prefix.len();
    let unlinked = {
        let (top, prefix) = (work.top.clone(), work.prefix.clone());
        let paths: Vec<Vec<u8>> = deletes.iter().map(|f| f.raw[prefix_len..].to_vec()).collect();
        tokio::task::spawn_blocking(move || {
            paths.iter().map(|rel| unlink(&top, &prefix, rel)).collect::<Vec<io::Result<()>>>()
        })
        .await
        .map_err(|e| GitError::Failed(e.to_string()))?
    };
    for (file, done) in deletes.iter().zip(unlinked) {
        if let Err(e) = done {
            tracing::info!("rewind left a file: {e}");
            left.push(Left { path: file.path.clone(), why: LeftWhy::Failed });
        }
    }

    if !writes.is_empty() {
        let mut input = Vec::new();
        for file in &writes {
            input.extend_from_slice(&file.raw);
            input.push(0);
        }
        let call = work
            .call(place, APPLY_LIMIT)
            .env("GIT_LITERAL_PATHSPECS", "1")
            .args(["restore", "--worktree", "--overlay", "--quiet"])
            .arg(format!("--source={}", plan.target))
            .args(["--pathspec-from-file=-", "--pathspec-file-nul"])
            .input(input);
        let out = place.git.run(call, deadline).await?;
        if !out.ok() {
            tracing::info!("git restore: {}", out.why());
        }
        // Each path is checked whatever git said.
        let paths: Vec<Vec<u8>> = writes.iter().map(|f| f.raw.clone()).collect();
        let seen = look_all(&work, paths.clone()).await?;
        let hashes = hash_seen(place, &work, &paths, &seen, deadline).await?;
        for ((file, seen), hash) in writes.iter().zip(&seen).zip(hashes) {
            let back = matches!(seen, Seen::File | Seen::Link(_))
                && file.then.as_ref().is_some_and(|b| hash.as_deref() == Some(b.oid.as_str()));
            if !back {
                left.push(Left { path: file.path.clone(), why: LeftWhy::Failed });
            }
        }
    }
    left.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(left)
}

/// What stands at a path, looked at without following a link.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    File,
    /// A link and its text.
    Link(Vec<u8>),
    /// Nothing (or a parent is missing).
    Absent,
    Left(LeftWhy),
}

async fn look_all(work: &Work, paths: Vec<Vec<u8>>) -> Result<Vec<Seen>, GitError> {
    let (top, prefix) = (work.top.clone(), work.prefix.clone());
    tokio::task::spawn_blocking(move || {
        let folder = open_folder(&top, &prefix);
        paths
            .iter()
            .map(|raw| match (&folder, raw.strip_prefix(prefix.as_slice())) {
                (Ok(folder), Some(rel)) => look(folder, rel),
                _ => Seen::Left(LeftWhy::Failed),
            })
            .collect()
    })
    .await
    .map_err(|e| GitError::Failed(e.to_string()))
}

/// The folder, reached from the top level without following a link.
fn open_folder(top: &Path, prefix: &[u8]) -> io::Result<OwnedFd> {
    let mut dir = rustix::fs::open(top, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())?;
    for part in prefix.split(|&b| b == b'/').filter(|p| !p.is_empty()) {
        dir = open_dir(&dir, part)?;
    }
    Ok(dir)
}

fn open_dir(dir: &OwnedFd, name: &[u8]) -> io::Result<OwnedFd> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    Ok(rustix::fs::openat(dir, OsStr::from_bytes(name), flags, Mode::empty())?)
}

/// What stands at `rel` (relative to the folder): every parent must be a
/// real folder.
fn look(folder: &OwnedFd, rel: &[u8]) -> Seen {
    let parts: Vec<&[u8]> = rel.split(|&b| b == b'/').collect();
    let Some((name, parents)) = parts.split_last() else { return Seen::Left(LeftWhy::Failed) };
    let mut dir: Option<OwnedFd> = None;
    for part in parents {
        let opened = open_dir(dir.as_ref().unwrap_or(folder), part);
        match opened {
            Ok(fd) => dir = Some(fd),
            Err(e) if e.raw_os_error() == Some(Errno::NOENT.raw_os_error()) => return Seen::Absent,
            Err(e)
                if [Errno::NOTDIR, Errno::LOOP]
                    .iter()
                    .any(|n| e.raw_os_error() == Some(n.raw_os_error())) =>
            {
                return Seen::Left(LeftWhy::NotFile);
            }
            Err(_) => return Seen::Left(LeftWhy::Failed),
        }
    }
    let dir = dir.as_ref().unwrap_or(folder);
    let name = OsStr::from_bytes(name);
    match rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Err(Errno::NOENT) => Seen::Absent,
        Err(_) => Seen::Left(LeftWhy::Failed),
        Ok(st) => match FileType::from_raw_mode(st.st_mode) {
            FileType::RegularFile => Seen::File,
            FileType::Symlink => match rustix::fs::readlinkat(dir, name, Vec::new()) {
                Ok(text) => Seen::Link(text.into_bytes()),
                Err(_) => Seen::Left(LeftWhy::Failed),
            },
            FileType::Directory => Seen::Left(LeftWhy::Folder),
            _ => Seen::Left(LeftWhy::NotFile),
        },
    }
}

/// Unlinks `rel` (a regular file or a link: the link itself) under the
/// folder, then the folders it leaves empty, never the folder itself.
fn unlink(top: &Path, prefix: &[u8], rel: &[u8]) -> io::Result<()> {
    let folder = open_folder(top, prefix)?;
    let parts: Vec<&[u8]> = rel.split(|&b| b == b'/').collect();
    let (name, parents) = parts.split_last().ok_or_else(|| io::Error::other("empty path"))?;
    let mut dirs: Vec<OwnedFd> = Vec::with_capacity(parents.len());
    for part in parents {
        let fd = open_dir(dirs.last().unwrap_or(&folder), part)?;
        dirs.push(fd);
    }
    let dir = dirs.last().unwrap_or(&folder);
    let name = OsStr::from_bytes(name);
    let kind = FileType::from_raw_mode(rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)?.st_mode);
    if !matches!(kind, FileType::RegularFile | FileType::Symlink) {
        return Err(io::Error::other("not a file or a link"));
    }
    rustix::fs::unlinkat(dir, name, AtFlags::empty())?;
    // Emptied parents, deepest first; stop at the first that isn't empty.
    for depth in (0..parents.len()).rev() {
        let parent = if depth == 0 { &folder } else { &dirs[depth - 1] };
        if rustix::fs::unlinkat(parent, OsStr::from_bytes(parents[depth]), AtFlags::REMOVEDIR).is_err() {
            break;
        }
    }
    Ok(())
}

/// The blob id of what stands at each of `paths` (from the top level), as
/// `seen` found it: a file's content through the user's filters (as `add`
/// would hash it), a link's text as it is; `None` for anything else, or a
/// file git couldn't read. Files go in batches named on the command line;
/// git stops a batch at the first file it can't read, so the rest of the
/// batch is hashed again after it.
async fn hash_seen(
    place: &Place,
    work: &Work,
    paths: &[Vec<u8>],
    seen: &[Seen],
    deadline: Instant,
) -> Result<Vec<Option<String>>, GitError> {
    let mut hashes = vec![None; seen.len()];
    let files: Vec<usize> = (0..seen.len()).filter(|&k| seen[k] == Seen::File).collect();
    let mut i = 0;
    while i < files.len() {
        let mut j = i;
        let mut bytes = 0;
        while j < files.len()
            && j - i < HASH_BATCH
            && (j == i || bytes + paths[files[j]].len() < HASH_BATCH_BYTES)
        {
            bytes += paths[files[j]].len() + 1;
            j += 1;
        }
        let call = work
            .call(place, CHECK_LIMIT)
            .args(["hash-object", "--"])
            .args(files[i..j].iter().map(|&k| OsStr::from_bytes(&paths[k])));
        let out = place.git.run(call, deadline).await?;
        let ids: Vec<String> = out
            .stdout
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| String::from_utf8_lossy(l).into_owned())
            .take_while(|l| oid_ok(l))
            .take(j - i)
            .collect();
        let done = ids.len();
        for (k, id) in ids.into_iter().enumerate() {
            hashes[files[i + k]] = Some(id);
        }
        i += if out.ok() || done == j - i { j - i } else { done + 1 };
    }
    for (k, look) in seen.iter().enumerate() {
        if let Seen::Link(text) = look {
            let call = Call::new(&place.repo, CHECK_LIMIT)
                .arg(git_dir(place))
                .args(["hash-object", "--stdin", "--no-filters"])
                .input(text.clone());
            let out = place.git.run(call, deadline).await?;
            let id = String::from_utf8_lossy(out.line()).into_owned();
            if out.ok() && oid_ok(&id) {
                hashes[k] = Some(id);
            }
        }
    }
    Ok(hashes)
}
