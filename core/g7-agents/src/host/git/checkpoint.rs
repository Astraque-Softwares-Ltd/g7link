// SPDX-License-Identifier: MPL-2.0
//! Capture (§U5.3; reconcile §1, §2, §4), the numbers of a turn, and keeping
//! and pruning restore points (§U5.6).
//!
//! A capture writes the thread's folder into a private index kept in G7's
//! repository (one per top level of the user's repositories, seeded from a
//! copy of the user's index when it is first made or older than the
//! user's), turns it into a commit on no branch and points a ref of G7's at
//! it. The user's index, `HEAD`, branches, stash and work tree are only read.
//! The commit's message records what later steps need: the top level, the
//! folder within it, `HEAD` as it was, and the files left out.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::OwnedMutexGuard;

use super::run::{Call, GitError, Out};
use super::{
    MAX_CANDIDATES, MAX_CAPTURE_BYTES, MAX_FILE_BYTES, MAX_LEFT_OUT, Place, folder_path, hex, oid_ok,
    path_parts_ok, ref_ok, rev_ok, unhex,
};
use crate::api::{CheckpointWhy, FileStatus, MAX_TURN_FILES, TurnFile};

/// G7's repository borrows the user's objects (an alternate) and its private
/// index starts from a copy of the user's. Off: G7's repository holds every
/// object itself and each private index starts empty (reconcile §1's
/// fallback; its background first capture is not built).
const SHARE_OBJECTS: bool = true;

const PROBE_LIMIT: Duration = Duration::from_secs(2);
const LIST_LIMIT: Duration = Duration::from_secs(5);
const DIFF_LIMIT: Duration = Duration::from_secs(5);
const REF_LIMIT: Duration = Duration::from_secs(5);
const STATS_BUDGET: Duration = Duration::from_secs(10);
const PRUNE_BUDGET: Duration = Duration::from_secs(15);
const GC_BUDGET: Duration = Duration::from_secs(60);
/// Alternates kept in G7's repository (one per user repository its folder's
/// threads have worked in); the oldest goes first.
const MAX_ALTERNATES: usize = 16;
/// The user's `info/exclude` copied, and the user's index seeded from.
const MAX_EXCLUDE: u64 = 1 << 20;
const MAX_INDEX: u64 = 64 << 20;
/// `for-each-ref` over G7's refs: two refs per turn, 100 turns per thread.
const MAX_REFS_OUT: usize = 32 << 20;
/// Two commit messages with up to [`MAX_LEFT_OUT`] paths each.
const MAX_META_OUT: usize = 8 << 20;

/// The tree with nothing in it.
pub(crate) const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const MESSAGE: &str = "G7 Agents restore point";
const WHO: &str = "G7 Agents";
const EMAIL: &str = "g7-agents@localhost";

/// What a capture made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capture {
    Ready {
        /// The commit (40 hex digits), now at the ref asked for.
        oid: String,
        /// The thread's folder in it (a tree id): equal trees, equal files.
        tree: String,
        /// `HEAD` of the user's repository: `"<symbolic ref or detached> <oid or unborn>"`.
        head: String,
        /// Files left out (too large), relative to the folder: `turns.skipped`.
        skipped: Vec<String>,
    },
    Missing(CheckpointWhy),
    /// The folder is not in a git work tree.
    NotGit,
}

/// What a turn changed, between its two restore points, inside the folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnStats {
    /// Files changed (those left out of either restore point not counted).
    pub files: u32,
    pub adds: u32,
    pub dels: u32,
    /// At most [`MAX_TURN_FILES`]: changed files, then the ones left out
    /// (status `unrecorded`).
    pub rows: Vec<TurnFile>,
    /// Changed files that aren't in `rows`.
    pub more: bool,
    /// Files left out of either restore point.
    pub skipped: u32,
}

/// `checkpoints/` in the host's data folder: G7's repositories.
pub fn checkpoints_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("checkpoints")
}

/// Records the thread's folder and points `to_ref` (a ref under `refs/g7/`,
/// see [`super::pre_ref`]) at it, within `budget`. Never fails: what went
/// wrong is the reason of [`Capture::Missing`].
pub async fn capture(place: &Place, to_ref: &str, budget: Duration) -> Capture {
    let deadline = Instant::now() + budget;
    if !ref_ok(to_ref) {
        return Capture::Missing(CheckpointWhy::Refused);
    }
    if place.git.recently_not_git(&place.folder) {
        return Capture::NotGit;
    }
    let work = match probe(place, deadline).await {
        Ok(work) => work,
        Err(e) => return missing(place, e),
    };
    if work.in_progress() {
        return Capture::Missing(CheckpointWhy::Busy);
    }
    let held = match hold(place, &work, deadline).await {
        Ok(held) => held,
        Err(e) => return missing(place, e),
    };
    let made = record(place, &work, Some(to_ref), deadline).await;
    drop(held);
    match made {
        Ok(made) => {
            let skipped = made.skipped.iter().filter_map(|raw| folder_path(raw, &work.prefix)).collect();
            Capture::Ready { oid: made.commit, tree: made.tree, head: made.head, skipped }
        }
        Err(e) => missing(place, e),
    }
}

fn missing(place: &Place, e: GitError) -> Capture {
    match e {
        GitError::NotGit => {
            place.git.set_not_git(&place.folder);
            Capture::NotGit
        }
        GitError::Busy => Capture::Missing(CheckpointWhy::Busy),
        GitError::Slow => Capture::Missing(CheckpointWhy::Slow),
        GitError::Large => Capture::Missing(CheckpointWhy::Large),
        e => {
            if place.folder.symlink_metadata().is_err() {
                return Capture::NotGit;
            }
            tracing::info!("restore point not recorded: {e}");
            Capture::Missing(CheckpointWhy::Refused)
        }
    }
}

// ---- The user's repository ----

/// Where the thread's folder sits in the user's repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Work {
    pub top: PathBuf,
    /// The folder from the top level: empty, or ending in `/`.
    pub prefix: Vec<u8>,
    user_index: PathBuf,
    objects: PathBuf,
    exclude: PathBuf,
    index_lock: PathBuf,
    /// Present while a rebase, merge, cherry-pick, revert or bisect is in progress.
    states: Vec<PathBuf>,
}

const STATES: [&str; 6] =
    ["rebase-merge", "rebase-apply", "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "BISECT_LOG"];

impl Work {
    /// Another git command holds the user's index (a commit waiting on its
    /// editor, say).
    pub fn locked(&self) -> bool {
        self.index_lock.symlink_metadata().is_ok()
    }

    pub fn in_progress(&self) -> bool {
        self.states.iter().any(|p| p.symlink_metadata().is_ok())
    }

    /// Either of the above.
    pub fn busy(&self) -> bool {
        self.locked() || self.in_progress()
    }

    /// The pathspec that limits a command to the folder; none at the top level.
    pub fn spec(&self) -> Vec<OsString> {
        match self.prefix.strip_suffix(b"/") {
            Some(folder) => vec![literal(b":(literal)", folder)],
            None => Vec::new(),
        }
    }

    /// The folder as one pathspec read from a file (`.` at the top level).
    pub fn spec_line(&self) -> Vec<u8> {
        let mut line = b":(literal)".to_vec();
        match self.prefix.strip_suffix(b"/") {
            Some(folder) => line.extend_from_slice(folder),
            None => line.push(b'.'),
        }
        line
    }

    /// `--git-dir` and `--work-tree` for G7's repository on this work tree.
    pub fn dirs(&self, place: &Place) -> [OsString; 2] {
        [
            literal(b"--git-dir=", place.repo.as_os_str().as_bytes()),
            literal(b"--work-tree=", self.top.as_os_str().as_bytes()),
        ]
    }

    /// The private index for this top level.
    pub fn index(&self, place: &Place) -> PathBuf {
        place.repo.join(format!("index-{:016x}", fnv1a(self.top.as_os_str().as_bytes())))
    }

    /// A call in G7's repository with this work tree and the private index.
    pub fn call(&self, place: &Place, limit: Duration) -> Call {
        Call::new(&self.top, limit).env("GIT_INDEX_FILE", self.index(place)).args(self.dirs(place))
    }
}

fn literal(head: &[u8], tail: &[u8]) -> OsString {
    let mut bytes = head.to_vec();
    bytes.extend_from_slice(tail);
    OsStr::from_bytes(&bytes).to_owned()
}

/// A stable 64-bit hash (FNV-1a) for file names.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// Finds the folder's repository: [`GitError::NotGit`] when it isn't in a
/// work tree (git refuses it for its owner, too), [`GitError::Failed`] for a
/// repository G7 can't keep restore points for (SHA-256).
pub(crate) async fn probe(place: &Place, deadline: Instant) -> Result<Work, GitError> {
    let mut args: Vec<&str> = vec![
        "rev-parse",
        "--path-format=absolute",
        "--show-toplevel",
        "--show-prefix",
        "--show-object-format",
    ];
    for path in ["index", "objects", "info/exclude", "index.lock"].iter().chain(STATES.iter()) {
        args.extend(["--git-path", path]);
    }
    let out = match place.git.run(Call::new(&place.folder, PROBE_LIMIT).args(args), deadline).await {
        Ok(out) => out,
        Err(GitError::Io(_)) => return Err(GitError::NotGit),
        Err(e) => return Err(e),
    };
    if !out.ok() {
        return Err(GitError::NotGit);
    }
    let lines: Vec<&[u8]> = out.stdout.split(|&b| b == b'\n').collect();
    let [top, prefix, format, index, objects, exclude, lock, states @ .., end] = lines.as_slice() else {
        return Err(GitError::NotGit);
    };
    if !end.is_empty() || states.len() != STATES.len() {
        return Err(GitError::NotGit);
    }
    if *format != b"sha1" {
        return Err(GitError::Failed("not a SHA-1 repository".into()));
    }
    let path = |raw: &[u8]| PathBuf::from(OsStr::from_bytes(raw));
    let top = path(top);
    let prefix_ok = prefix.is_empty() || prefix.strip_suffix(b"/").is_some_and(path_parts_ok);
    if !top.is_absolute() || !prefix_ok {
        return Err(GitError::NotGit);
    }
    Ok(Work {
        top,
        prefix: prefix.to_vec(),
        user_index: path(index),
        objects: path(objects),
        exclude: path(exclude),
        index_lock: path(lock),
        states: states.iter().map(|raw| path(raw)).collect(),
    })
}

/// The two locks an operation on a folder holds: G7's repository, then the
/// user's top level.
pub(crate) async fn hold(
    place: &Place,
    work: &Work,
    deadline: Instant,
) -> Result<(OwnedMutexGuard<()>, OwnedMutexGuard<()>), GitError> {
    let repo = place.git.lock(&place.repo, deadline).await?;
    let top = place.git.lock(&work.top, deadline).await?;
    Ok((repo, top))
}

// ---- Recording ----

/// A restore point just made.
#[derive(Debug, Clone)]
pub(crate) struct Recorded {
    pub commit: String,
    /// The folder's subtree.
    pub tree: String,
    pub head: String,
    /// Left out, from the top level.
    pub skipped: Vec<Vec<u8>>,
}

/// Removes what a killed git may leave behind, on every way out.
struct Leftovers(Vec<PathBuf>);

impl Drop for Leftovers {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

/// Records the folder (the caller holds [`hold`]); with `to_ref`, points it
/// there.
pub(crate) async fn record(
    place: &Place,
    work: &Work,
    to_ref: Option<&str>,
    deadline: Instant,
) -> Result<Recorded, GitError> {
    let index = work.index(place);
    let index_lock = lock_of(&index);
    let mut leftovers = vec![index_lock.clone()];
    if let Some(name) = to_ref {
        leftovers.push(lock_of(&place.repo.join(name)));
    }
    let _leftovers = Leftovers(leftovers);
    // Only this host writes G7's repository, one operation at a time: a lock
    // there now was left by a git process that was killed.
    let _ = fs::remove_file(&index_lock);

    open_repo(place, work, deadline).await?;
    let candidates = match candidates(place, work, deadline).await {
        Ok(found) => found,
        Err(GitError::Failed(why)) if index.exists() => {
            // A copied index git can't read here (a split index, say): start afresh.
            tracing::info!("private index dropped: {why}");
            fs::remove_file(&index)?;
            candidates(place, work, deadline).await?
        }
        Err(e) => return Err(e),
    };
    let skipped =
        if candidates.is_empty() { Vec::new() } else { add(place, work, candidates, deadline).await? };

    let tree = oid_line(place.git.run(work.call(place, LIST_LIMIT).arg("write-tree"), deadline).await?)?;
    let sub = match work.prefix.is_empty() {
        true => tree.clone(),
        false => {
            let prefix = literal(b"--prefix=", &work.prefix);
            let out =
                place.git.run(work.call(place, LIST_LIMIT).arg("write-tree").arg(prefix), deadline).await?;
            // The folder holds nothing git keeps: no subtree.
            if out.ok() { oid_line(out)? } else { EMPTY_TREE.to_owned() }
        }
    };
    let head = user_head(place, deadline).await?;

    let mut message = format!(
        "{MESSAGE}\n\ng7-top {}\ng7-prefix {}\ng7-head {head}\n",
        hex(work.top.as_os_str().as_bytes()),
        hex(&work.prefix)
    );
    for path in &skipped {
        message.push_str(&format!("g7-skip {}\n", hex(path)));
    }
    let commit = place
        .git
        .run(
            work.call(place, LIST_LIMIT)
                .args(["commit-tree", "--no-gpg-sign", &tree])
                .env("GIT_AUTHOR_NAME", WHO)
                .env("GIT_AUTHOR_EMAIL", EMAIL)
                .env("GIT_COMMITTER_NAME", WHO)
                .env("GIT_COMMITTER_EMAIL", EMAIL)
                .input(message.into_bytes()),
            deadline,
        )
        .await?;
    let commit = oid_line(commit)?;
    if let Some(name) = to_ref {
        set_ref(place, name, &commit, deadline).await?;
    }
    Ok(Recorded { commit, tree: sub, head, skipped })
}

fn lock_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

fn oid_line(out: Out) -> Result<String, GitError> {
    let out = out.check()?;
    let line = String::from_utf8_lossy(out.line()).into_owned();
    if oid_ok(&line) { Ok(line) } else { Err(GitError::Failed("not an object id".into())) }
}

pub(crate) async fn set_ref(place: &Place, name: &str, oid: &str, deadline: Instant) -> Result<(), GitError> {
    let call = Call::new(&place.repo, REF_LIMIT).arg(git_dir(place)).args(["update-ref", name, oid]);
    place.git.run(call, deadline).await?.check().map(drop)
}

/// `--git-dir` for calls that need no work tree.
pub(crate) fn git_dir(place: &Place) -> OsString {
    literal(b"--git-dir=", place.repo.as_os_str().as_bytes())
}

/// `HEAD` of the user's repository, read where the folder is.
async fn user_head(place: &Place, deadline: Instant) -> Result<String, GitError> {
    let call = |args: &[&str]| Call::new(&place.folder, PROBE_LIMIT).args(args);
    let symbolic = place.git.run(call(&["symbolic-ref", "-q", "HEAD"]), deadline).await?;
    let name = match symbolic.code {
        Some(0) => String::from_utf8_lossy(symbolic.line()).into_owned(),
        Some(1) => "detached".to_owned(),
        _ => return Err(GitError::Failed(symbolic.why())),
    };
    let oid = place.git.run(call(&["rev-parse", "-q", "--verify", "HEAD^{commit}"]), deadline).await?;
    let oid = match oid.code {
        Some(0) => String::from_utf8_lossy(oid.line()).into_owned(),
        _ => "unborn".to_owned(),
    };
    let ok = |s: &str| !s.is_empty() && s.len() <= 256 && s.bytes().all(|b| b.is_ascii_graphic());
    if ok(&name) && ok(&oid) { Ok(format!("{name} {oid}")) } else { Err(GitError::Failed("odd HEAD".into())) }
}

/// Makes G7's repository when it isn't there; names the user's objects as
/// an alternate; copies the user's `info/exclude`; seeds the private index.
async fn open_repo(place: &Place, work: &Work, deadline: Instant) -> Result<(), GitError> {
    let repo = &place.repo;
    if repo.join("HEAD").symlink_metadata().is_err() {
        let parent = repo.parent().ok_or_else(|| GitError::Failed("no checkpoints folder".into()))?;
        fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
        let name = repo.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let fresh = parent.join(format!(".{name}.new-{}", stamp()));
        let init = Call::new(parent, LIST_LIMIT)
            .args(["init", "--bare", "--quiet", "--template=", "--object-format=sha1"])
            .arg(&fresh);
        let made = place.git.run(init, deadline).await.and_then(Out::check);
        let made = made.and_then(|_| -> Result<(), GitError> {
            fs::set_permissions(&fresh, fs::Permissions::from_mode(0o700))?;
            fs::rename(&fresh, repo).or_else(|e| if repo.join("HEAD").exists() { Ok(()) } else { Err(e) })?;
            Ok(())
        });
        let _ = fs::remove_dir_all(&fresh);
        made?;
    }
    let (repo, work, index) = (repo.clone(), work.clone(), work.index(place));
    tokio::task::spawn_blocking(move || -> io::Result<()> {
        if SHARE_OBJECTS {
            borrow_objects(&repo, &work.objects)?;
            seed_index(&work.user_index, &index)?;
        }
        copy_exclude(&work.exclude, &repo.join("info").join("exclude"))
    })
    .await
    .map_err(|e| GitError::Failed(e.to_string()))??;
    Ok(())
}

fn stamp() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}

/// Names `objects` in G7's repository's alternates, once.
fn borrow_objects(repo: &Path, objects: &Path) -> io::Result<()> {
    let want = objects.as_os_str().as_bytes();
    if !objects.is_absolute() || want.contains(&b'\n') {
        return Err(io::Error::other("an object directory G7 can't name"));
    }
    let file = repo.join("objects").join("info").join("alternates");
    let now = read_small(&file, 64 << 10)?.unwrap_or_default();
    let mut lines: Vec<&[u8]> = now.split(|&b| b == b'\n').filter(|l| !l.is_empty()).collect();
    if lines.contains(&want) {
        return Ok(());
    }
    let drop = (lines.len() + 1).saturating_sub(MAX_ALTERNATES);
    lines.drain(..drop);
    lines.push(want);
    let mut body = lines.join(&b'\n');
    body.push(b'\n');
    fs::create_dir_all(file.parent().unwrap_or(repo))?;
    replace(&file, &body, None)
}

/// Copies the user's index over the private one when the private one is
/// missing or older, keeping its mtime so git's racy-entry check works the
/// same on the copy.
fn seed_index(user: &Path, ours: &Path) -> io::Result<()> {
    let Ok(theirs) = fs::symlink_metadata(user) else { return Ok(()) };
    if !theirs.is_file() || theirs.len() > MAX_INDEX {
        return Ok(());
    }
    let stale = match fs::symlink_metadata(ours) {
        Ok(mine) => mine.modified()? < theirs.modified()?,
        Err(_) => true,
    };
    if !stale {
        return Ok(());
    }
    let Some(body) = read_small(user, MAX_INDEX)? else { return Ok(()) };
    replace(ours, &body, Some(theirs.modified()?))
}

/// The user's `info/exclude` (rules the user keeps out of `.gitignore`)
/// as G7's own, so the same files are ignored.
fn copy_exclude(user: &Path, ours: &Path) -> io::Result<()> {
    let body = read_small(user, MAX_EXCLUDE)?.unwrap_or_default();
    if read_small(ours, MAX_EXCLUDE)?.as_deref() == Some(&body[..]) {
        return Ok(());
    }
    fs::create_dir_all(ours.parent().unwrap_or(Path::new("/")))?;
    replace(ours, &body, None)
}

/// A regular file's bytes, read without following a link and at most
/// `max`; `None` when it isn't there. A link, another kind of file or a
/// longer one is an error.
pub(crate) fn read_small(path: &Path, max: u64) -> io::Result<Option<Vec<u8>>> {
    let file = match open_nofollow(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > max {
        return Err(io::Error::other("not a small regular file"));
    }
    let mut body = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.take(max + 1).read_to_end(&mut body)?;
    if body.len() as u64 > max {
        return Err(io::Error::other("grew past its limit"));
    }
    Ok(Some(body))
}

fn open_nofollow(path: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags};
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    Ok(File::from(fd))
}

/// Writes `path` whole through a new file beside it.
fn replace(path: &Path, body: &[u8], mtime: Option<SystemTime>) -> io::Result<()> {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".new-{}", stamp()));
    let fresh = PathBuf::from(name);
    let written = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&fresh)?;
        file.write_all(body)?;
        if let Some(mtime) = mtime {
            file.set_modified(mtime)?;
        }
        drop(file);
        fs::rename(&fresh, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&fresh);
    }
    written
}

/// New files and tracked files whose stat differs from the private index,
/// inside the folder: what `add` will have to hash. From the top level.
async fn candidates(place: &Place, work: &Work, deadline: Instant) -> Result<Vec<Vec<u8>>, GitError> {
    let others = work
        .call(place, LIST_LIMIT)
        .args(["ls-files", "--others", "--exclude-standard", "-z", "--"])
        .args(work.spec());
    let others = place.git.run(others, deadline).await?;
    if others.over {
        return Err(GitError::Large);
    }
    let others = others.check()?;
    let dirty = work
        .call(place, LIST_LIMIT)
        .args(["diff-files", "--name-only", "-z", "--ignore-submodules=dirty", "--"])
        .args(work.spec());
    let dirty = place.git.run(dirty, deadline).await?;
    if dirty.over {
        return Err(GitError::Large);
    }
    let dirty = dirty.check()?;
    let found: Vec<Vec<u8>> =
        split_z(&others.stdout).chain(split_z(&dirty.stdout)).map(<[u8]>::to_vec).collect();
    if found.len() > MAX_CANDIDATES {
        return Err(GitError::Large);
    }
    Ok(found)
}

pub(crate) fn split_z(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes.split(|&b| b == 0).filter(|p| !p.is_empty())
}

/// Leaves out what is over the limits, then adds the rest of the folder to
/// the private index. Returns the paths left out.
async fn add(
    place: &Place,
    work: &Work,
    candidates: Vec<Vec<u8>>,
    deadline: Instant,
) -> Result<Vec<Vec<u8>>, GitError> {
    let top = work.top.clone();
    let sized = tokio::task::spawn_blocking(move || {
        candidates
            .into_iter()
            .map(|path| {
                let size = fs::symlink_metadata(top.join(OsStr::from_bytes(&path)))
                    .ok()
                    .filter(fs::Metadata::is_file)
                    .map(|m| m.len());
                (path, size)
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| GitError::Failed(e.to_string()))?;
    let left_out = leave_out(sized);
    if left_out.len() > MAX_LEFT_OUT {
        return Err(GitError::Large);
    }
    let mut specs = work.spec_line();
    specs.push(0);
    for path in &left_out {
        specs.extend_from_slice(b":(exclude,literal)");
        specs.extend_from_slice(path);
        specs.push(0);
    }
    let call = work
        .call(place, deadline.saturating_duration_since(Instant::now()))
        .args(["-c", "core.safecrlf=false", "add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"])
        .input(specs);
    let out = place.git.run(call, deadline).await?;
    // Exit 1 naming ignored paths: the folder itself is ignored; what could
    // be added was added.
    let ignored = out.code == Some(1) && out.stderr.contains("ignored by one of your .gitignore");
    if !out.ok() && !ignored {
        return Err(GitError::Failed(out.why()));
    }
    Ok(left_out)
}

/// Reconcile §2: a regular file over [`MAX_FILE_BYTES`] is left out; the
/// rest are taken smallest first until they total [`MAX_CAPTURE_BYTES`],
/// and everything after that is left out. Paths without a size (not a
/// regular file, or gone) cost nothing.
pub(crate) fn leave_out(sized: Vec<(Vec<u8>, Option<u64>)>) -> Vec<Vec<u8>> {
    let mut files: Vec<(u64, Vec<u8>)> =
        sized.into_iter().filter_map(|(path, size)| size.map(|size| (size, path))).collect();
    files.sort();
    let mut total = 0u64;
    let mut full = false;
    let mut out = Vec::new();
    for (size, path) in files {
        if size > MAX_FILE_BYTES || full || total + size > MAX_CAPTURE_BYTES {
            full |= size <= MAX_FILE_BYTES;
            out.push(path);
        } else {
            total += size;
        }
    }
    out
}

// ---- Reading restore points back ----

/// What a restore point's commit records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Meta {
    pub oid: String,
    /// The whole tree.
    pub tree: String,
    pub top: Vec<u8>,
    pub prefix: Vec<u8>,
    pub head: String,
    /// Left out, from the top level.
    pub skipped: Vec<Vec<u8>>,
}

/// The restore points named by `revs` (refs of G7's or object ids), in
/// order; `None` for one that is gone.
pub(crate) async fn metas(
    place: &Place,
    revs: &[&str],
    deadline: Instant,
) -> Result<Vec<Option<Meta>>, GitError> {
    if revs.iter().any(|rev| !rev_ok(rev)) {
        return Err(GitError::Gone);
    }
    if place.repo.join("HEAD").symlink_metadata().is_err() {
        return Ok(vec![None; revs.len()]);
    }
    let input: Vec<u8> = revs.iter().flat_map(|rev| format!("{rev}\n").into_bytes()).collect();
    let call = Call::new(&place.repo, LIST_LIMIT)
        .arg(git_dir(place))
        .args(["cat-file", "--batch"])
        .input(input)
        .cap(MAX_META_OUT);
    let out = place.git.run(call, deadline).await?.check()?;
    let mut rest = &out.stdout[..];
    let mut found = Vec::with_capacity(revs.len());
    for _ in revs {
        let end =
            rest.iter().position(|&b| b == b'\n').ok_or_else(|| GitError::Failed("short batch".into()))?;
        let header = String::from_utf8_lossy(&rest[..end]).into_owned();
        rest = &rest[end + 1..];
        let fields: Vec<&str> = header.split(' ').collect();
        let [oid, kind, size] = fields.as_slice() else {
            found.push(None);
            continue;
        };
        let size: usize = size.parse().map_err(|_| GitError::Failed("odd batch size".into()))?;
        if rest.len() < size + 1 {
            return Err(GitError::Failed("short batch".into()));
        }
        let body = &rest[..size];
        rest = &rest[size + 1..];
        found.push(if *kind == "commit" && oid_ok(oid) { parse_meta(oid, body) } else { None });
    }
    Ok(found)
}

fn parse_meta(oid: &str, body: &[u8]) -> Option<Meta> {
    let text = String::from_utf8_lossy(body);
    let (headers, message) = text.split_once("\n\n")?;
    let tree = headers.lines().find_map(|l| l.strip_prefix("tree "))?.to_owned();
    if !oid_ok(&tree) || !message.starts_with(MESSAGE) {
        return None;
    }
    let (mut top, mut prefix, mut head, mut skipped) = (None, None, None, Vec::new());
    for line in message.lines() {
        if let Some(v) = line.strip_prefix("g7-top ") {
            top = unhex(v);
        } else if let Some(v) = line.strip_prefix("g7-prefix ") {
            prefix = unhex(v);
        } else if let Some(v) = line.strip_prefix("g7-head ") {
            head = Some(v.to_owned());
        } else if let Some(v) = line.strip_prefix("g7-skip ") {
            skipped.push(unhex(v)?);
        }
    }
    Some(Meta { oid: oid.to_owned(), tree, top: top?, prefix: prefix?, head: head?, skipped })
}

/// One file pair of `diff-tree --raw -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawPair {
    pub old_mode: String,
    pub new_mode: String,
    pub old_oid: String,
    pub new_oid: String,
    /// `A`, `D`, `M`, `T`, `R`, `C`, …
    pub status: u8,
    /// From the top level; the new path of a rename.
    pub path: Vec<u8>,
    pub from: Option<Vec<u8>>,
}

/// One `--numstat -z` record; `None` counts: a binary file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NumPair {
    pub adds: Option<u32>,
    pub dels: Option<u32>,
    pub path: Vec<u8>,
}

/// Parses `diff-tree --raw [--numstat] -z`. Stops at anything malformed and
/// says so with `true`.
pub(crate) fn parse_pairs(bytes: &[u8]) -> (Vec<RawPair>, Vec<NumPair>, bool) {
    let mut parts = bytes.split(|&b| b == 0).peekable();
    let (mut raw, mut num) = (Vec::new(), Vec::new());
    while let Some(part) = parts.next() {
        if part.is_empty() && parts.peek().is_none() {
            break;
        }
        if let Some(head) = part.strip_prefix(b":") {
            let head = String::from_utf8_lossy(head);
            let fields: Vec<&str> = head.split(' ').collect();
            let [old_mode, new_mode, old_oid, new_oid, status] = fields.as_slice() else {
                return (raw, num, true);
            };
            let Some(&code) = status.as_bytes().first() else { return (raw, num, true) };
            let Some(first) = parts.next().filter(|p| !p.is_empty()) else { return (raw, num, true) };
            let (path, from) = if matches!(code, b'R' | b'C') {
                let Some(second) = parts.next().filter(|p| !p.is_empty()) else { return (raw, num, true) };
                (second.to_vec(), Some(first.to_vec()))
            } else {
                (first.to_vec(), None)
            };
            raw.push(RawPair {
                old_mode: (*old_mode).to_owned(),
                new_mode: (*new_mode).to_owned(),
                old_oid: (*old_oid).to_owned(),
                new_oid: (*new_oid).to_owned(),
                status: code,
                path,
                from,
            });
        } else {
            let mut fields = part.splitn(3, |&b| b == b'\t');
            let (Some(adds), Some(dels), Some(path)) = (fields.next(), fields.next(), fields.next()) else {
                return (raw, num, true);
            };
            let count = |f: &[u8]| std::str::from_utf8(f).ok().and_then(|s| s.parse::<u32>().ok());
            let path = if path.is_empty() {
                // A rename: the old path, then the new one.
                let (Some(_), Some(new)) = (parts.next(), parts.next()) else { return (raw, num, true) };
                new.to_vec()
            } else {
                path.to_vec()
            };
            num.push(NumPair { adds: count(adds), dels: count(dels), path });
        }
    }
    (raw, num, false)
}

pub(crate) fn status_of(code: u8) -> FileStatus {
    match code {
        b'A' | b'C' => FileStatus::Added,
        b'D' => FileStatus::Deleted,
        b'T' => FileStatus::Type,
        b'R' => FileStatus::Renamed,
        _ => FileStatus::Modified,
    }
}

/// What changed in the folder from `pre` to `post` (refs of G7's or object
/// ids). A turn's restore points were taken in one folder; two that weren't
/// are [`GitError::Moved`]; one that is gone is [`GitError::Gone`].
pub async fn turn_stats(place: &Place, pre: &str, post: &str) -> Result<TurnStats, GitError> {
    let deadline = Instant::now() + STATS_BUDGET;
    let (a, b) = pair(place, pre, post, deadline).await?;
    let call = Call::new(&place.repo, DIFF_LIMIT)
        .arg(git_dir(place))
        .args([
            "diff-tree",
            "-r",
            "-M",
            "--raw",
            "--numstat",
            "-z",
            "--no-textconv",
            "--no-ext-diff",
            &a.oid,
            &b.oid,
            "--",
        ])
        .args(spec_of(&a.prefix));
    let out = place.git.run(call, deadline).await?;
    if !out.ok() && !out.over {
        return Err(GitError::Failed(out.why()));
    }
    let (raw, num, cut) = parse_pairs(&out.stdout);
    let unrecorded: HashSet<&[u8]> = a.skipped.iter().chain(&b.skipped).map(Vec::as_slice).collect();
    Ok(stats(&raw, &num, &unrecorded, &a.prefix, cut || out.over))
}

/// Two restore points of one folder.
pub(crate) async fn pair(
    place: &Place,
    from: &str,
    to: &str,
    deadline: Instant,
) -> Result<(Meta, Meta), GitError> {
    let mut found = metas(place, &[from, to], deadline).await?.into_iter();
    let (Some(Some(a)), Some(Some(b))) = (found.next(), found.next()) else { return Err(GitError::Gone) };
    if a.top != b.top || a.prefix != b.prefix {
        return Err(GitError::Moved);
    }
    Ok((a, b))
}

/// One path (from the top level) as a pathspec.
pub(crate) fn path_spec(raw: &[u8]) -> OsString {
    literal(b":(literal)", raw)
}

pub(crate) fn spec_of(prefix: &[u8]) -> Vec<OsString> {
    match prefix.strip_suffix(b"/") {
        Some(folder) => vec![literal(b":(literal)", folder)],
        None => Vec::new(),
    }
}

fn stats(
    raw: &[RawPair],
    num: &[NumPair],
    unrecorded: &HashSet<&[u8]>,
    prefix: &[u8],
    cut: bool,
) -> TurnStats {
    let counts: HashMap<&[u8], &NumPair> = num.iter().map(|n| (n.path.as_slice(), n)).collect();
    let mut stats = TurnStats { more: cut, ..TurnStats::default() };
    let mut listed = Vec::new();
    for pair in raw {
        let left_out = |p: &Option<Vec<u8>>| p.as_deref().is_some_and(|p| unrecorded.contains(p));
        if unrecorded.contains(pair.path.as_slice()) || left_out(&pair.from) {
            continue;
        }
        let n = counts.get(pair.path.as_slice());
        let (adds, dels) = n.map_or((0, 0), |n| (n.adds.unwrap_or(0), n.dels.unwrap_or(0)));
        stats.files = stats.files.saturating_add(1);
        stats.adds = stats.adds.saturating_add(adds);
        stats.dels = stats.dels.saturating_add(dels);
        let path = folder_path(&pair.path, prefix);
        let from = pair.from.as_ref().map(|f| folder_path(f, prefix));
        match (path, from) {
            (Some(path), None) | (Some(path), Some(Some(_))) => listed.push(TurnFile {
                path,
                status: status_of(pair.status),
                from: pair.from.as_ref().and_then(|f| folder_path(f, prefix)),
                adds,
                dels,
                binary: n.is_some_and(|n| n.adds.is_none()),
            }),
            _ => stats.more = true,
        }
    }
    let mut left: Vec<String> = unrecorded.iter().filter_map(|p| folder_path(p, prefix)).collect();
    left.sort();
    left.dedup();
    stats.skipped = u32::try_from(left.len()).unwrap_or(u32::MAX);
    let room = MAX_TURN_FILES.saturating_sub(left.len());
    if listed.len() > room {
        listed.truncate(room);
        stats.more = true;
    }
    stats.rows = listed;
    stats.rows.extend(left.into_iter().take(MAX_TURN_FILES).map(|path| TurnFile {
        path,
        status: FileStatus::Unrecorded,
        from: None,
        adds: 0,
        dels: 0,
        binary: false,
    }));
    stats
}

// ---- Keeping and pruning ----

/// Deletes every ref of G7's under `prefix` (see [`super::thread_refs`],
/// [`super::turn_refs`]) in the folder's G7 repository. Returns how many.
pub async fn delete_refs(place: &Place, prefix: &str) -> Result<u32, GitError> {
    if !ref_ok(prefix.trim_end_matches('/')) {
        return Err(GitError::Failed("not a ref prefix of G7's".into()));
    }
    let deadline = Instant::now() + PRUNE_BUDGET;
    if place.repo.join("HEAD").symlink_metadata().is_err() {
        return Ok(0);
    }
    let _held = place.git.lock(&place.repo, deadline).await?;
    let names = ref_names(place, prefix.trim_end_matches('/'), deadline).await?;
    if names.is_empty() {
        return Ok(0);
    }
    let mut leftovers = vec![place.repo.join("packed-refs.lock")];
    leftovers.extend(names.iter().map(|name| lock_of(&place.repo.join(name))));
    let _leftovers = Leftovers(leftovers);
    let input: Vec<u8> = names.iter().flat_map(|name| format!("delete {name}\0\0").into_bytes()).collect();
    let call = Call::new(&place.repo, REF_LIMIT)
        .arg(git_dir(place))
        .args(["update-ref", "--stdin", "-z"])
        .input(input);
    place.git.run(call, deadline).await?.check()?;
    Ok(u32::try_from(names.len()).unwrap_or(u32::MAX))
}

async fn ref_names(place: &Place, pattern: &str, deadline: Instant) -> Result<Vec<String>, GitError> {
    let call = Call::new(&place.repo, REF_LIMIT)
        .arg(git_dir(place))
        .args(["for-each-ref", "--format=%(refname)", pattern])
        .cap(MAX_REFS_OUT);
    let out = place.git.run(call, deadline).await?;
    if out.over {
        return Err(GitError::Large);
    }
    let out = out.check()?;
    Ok(String::from_utf8_lossy(&out.stdout).lines().filter(|name| ref_ok(name)).map(str::to_owned).collect())
}

/// The threads that have refs in the folder's G7 repository (to sweep the
/// ones the database no longer has).
pub async fn thread_ids(place: &Place) -> Result<Vec<String>, GitError> {
    let deadline = Instant::now() + PRUNE_BUDGET;
    if place.repo.join("HEAD").symlink_metadata().is_err() {
        return Ok(Vec::new());
    }
    let names = ref_names(place, "refs/g7", deadline).await?;
    let mut ids: Vec<String> = names
        .iter()
        .filter_map(|name| name.strip_prefix("refs/g7/")?.split('/').next())
        .map(str::to_owned)
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// `git gc --prune=now` on G7's repository, after refs were deleted. Never
/// on the user's: G7's repository packs only its own objects.
pub async fn gc(place: &Place) -> Result<(), GitError> {
    let deadline = Instant::now() + GC_BUDGET;
    if place.repo.join("HEAD").symlink_metadata().is_err() {
        return Ok(());
    }
    let _held = place.git.lock(&place.repo, deadline).await?;
    let call = Call::new(&place.repo, GC_BUDGET).arg(git_dir(place)).args([
        "-c",
        "pack.threads=1",
        "gc",
        "--prune=now",
        "--quiet",
    ]);
    place.git.run(call, deadline).await?.check().map(drop)
}

/// Removes the folder's G7 repository (the folder left the agent folders).
pub async fn remove_repo(place: &Place) -> Result<(), GitError> {
    let deadline = Instant::now() + PRUNE_BUDGET;
    let _held = place.git.lock(&place.repo, deadline).await?;
    let repo = place.repo.clone();
    tokio::task::spawn_blocking(move || match fs::remove_dir_all(&repo) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    })
    .await
    .map_err(|e| GitError::Failed(e.to_string()))??;
    Ok(())
}

/// Bytes on disk under `checkpoints` (`store.info`'s `checkpoint_bytes`).
/// Blocking: call it off the async threads.
pub fn store_bytes(checkpoints: &Path) -> u64 {
    fn walk(dir: &Path, depth: u32) -> u64 {
        let Ok(entries) = fs::read_dir(dir) else { return 0 };
        entries
            .flatten()
            .map(|entry| match entry.metadata() {
                Ok(m) if m.is_dir() && depth < 8 => walk(&entry.path(), depth + 1),
                Ok(m) if m.is_file() => m.len(),
                _ => 0,
            })
            .sum()
    }
    walk(checkpoints, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sized(items: &[(&str, Option<u64>)]) -> Vec<(Vec<u8>, Option<u64>)> {
        items.iter().map(|(p, s)| (p.as_bytes().to_vec(), *s)).collect()
    }

    #[test]
    fn large_files_and_the_49th_mib_are_left_out_smallest_first() {
        const MIB: u64 = 1 << 20;
        let out = leave_out(sized(&[
            ("big", Some(11 * MIB)),
            ("a", Some(9 * MIB)),
            ("b", Some(9 * MIB)),
            ("c", Some(9 * MIB)),
            ("d", Some(9 * MIB)),
            ("e", Some(9 * MIB)),
            ("small", Some(5 * MIB)),
            ("dir", None),
        ]));
        // 5 + 9 * 4 = 41 MiB taken; the fifth 9 MiB file would pass 48.
        assert_eq!(out, vec![b"e".to_vec(), b"big".to_vec()]);
        assert!(leave_out(sized(&[("x", Some(MAX_FILE_BYTES))])).is_empty());
        assert_eq!(leave_out(sized(&[("x", Some(MAX_FILE_BYTES + 1))])), vec![b"x".to_vec()]);
    }

    #[test]
    fn raw_and_numstat_records_parse_and_stop_at_garbage() {
        let bytes = b":000000 100644 0000000000000000000000000000000000000000 87ae6b695deceaf160611414f7dcd5c7366b2e79 A\0sub/b.bin\0:100644 100644 4ae8ef021bf6fcfff43a13be5abfa52bb6fb5dbc 4ae8ef021bf6fcfff43a13be5abfa52bb6fb5dbc R100\0sub/new.txt\0sub/new2.txt\0-\t-\tsub/b.bin\x000\t0\t\0sub/new.txt\0sub/new2.txt\0";
        let (raw, num, cut) = parse_pairs(bytes);
        assert!(!cut);
        assert_eq!(raw.len(), 2);
        assert_eq!(raw[1].path, b"sub/new2.txt");
        assert_eq!(raw[1].from.as_deref(), Some(&b"sub/new.txt"[..]));
        assert_eq!(num[0], NumPair { adds: None, dels: None, path: b"sub/b.bin".to_vec() });
        assert_eq!(num[1].path, b"sub/new2.txt");
        let (_, _, cut) = parse_pairs(b":100644 100644 x\0a\0");
        assert!(cut);
        let (_, _, cut) = parse_pairs(b":100644 100644 a b R100\0only-one\0");
        assert!(cut);
    }

    #[test]
    fn a_commit_message_round_trips_and_a_foreign_one_is_not_ours() {
        let body = format!(
            "tree {EMPTY_TREE}\nauthor G7 Agents <g7-agents@localhost> 1 +0000\n\n{MESSAGE}\n\ng7-top {}\ng7-prefix {}\ng7-head refs/heads/main unborn\ng7-skip {}\n",
            hex(b"/tmp/x"),
            hex(b"sub/"),
            hex(b"sub/\xffbig\nname")
        );
        let meta = parse_meta(EMPTY_TREE, body.as_bytes()).unwrap();
        assert_eq!(meta.top, b"/tmp/x");
        assert_eq!(meta.prefix, b"sub/");
        assert_eq!(meta.skipped, vec![b"sub/\xffbig\nname".to_vec()]);
        assert_eq!(meta.head, "refs/heads/main unborn");
        assert!(
            parse_meta(EMPTY_TREE, format!("tree {EMPTY_TREE}\n\nsomething else\n").as_bytes()).is_none()
        );
    }

    #[test]
    fn unrecorded_paths_come_last_and_are_not_counted() {
        let pair = |path: &str, status: u8| RawPair {
            old_mode: "100644".into(),
            new_mode: "100644".into(),
            old_oid: EMPTY_TREE.into(),
            new_oid: EMPTY_TREE.into(),
            status,
            path: path.as_bytes().to_vec(),
            from: None,
        };
        let raw = vec![pair("sub/a.txt", b'M'), pair("sub/big.bin", b'M'), pair("other/x", b'A')];
        let num = vec![
            NumPair { adds: Some(2), dels: Some(1), path: b"sub/a.txt".to_vec() },
            NumPair { adds: Some(9), dels: Some(9), path: b"sub/big.bin".to_vec() },
        ];
        let skipped: HashSet<&[u8]> = [&b"sub/big.bin"[..], &b"sub/huge.iso"[..]].into_iter().collect();
        let stats = stats(&raw, &num, &skipped, b"sub/", false);
        assert_eq!((stats.files, stats.adds, stats.dels, stats.skipped), (2, 2, 1, 2));
        let rows: Vec<(&str, FileStatus)> = stats.rows.iter().map(|r| (r.path.as_str(), r.status)).collect();
        assert_eq!(
            rows,
            vec![
                ("a.txt", FileStatus::Modified),
                ("big.bin", FileStatus::Unrecorded),
                ("huge.iso", FileStatus::Unrecorded)
            ]
        );
        // `other/x` is outside the folder: counted, not listed.
        assert!(stats.more);
    }
}
