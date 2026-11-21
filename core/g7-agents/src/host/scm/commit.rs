// SPDX-License-Identifier: MPL-2.0
//! A commit of the files a person ticks (§SC3.4): `Add` then `Commit
//! --only` on exactly those paths, as they are on disk, under the
//! repository's lock. Files the user staged and didn't tick stay staged and
//! out of the commit. The user's hooks and signing run; their lines go to
//! the job as they arrive. A review pins what was read: its id is computed
//! again before anything runs and must match.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};

use super::argv::{Op, pathspecs};
use super::patch::{MAX_FOLDER_ENTRIES, walk_folder};
use super::sha256::Sha256;
use super::status::{Entry, Porcelain, trim_slash};
use super::{
    GitStatus, Lines, MAX_MESSAGE, MAX_STATUS_FILES, Reason, Scm, ScmError, Spot, Stop, clean, lossy,
};

/// How long a commit waits for the repository's lock (a restore point may hold it).
const LOCK_WAIT: Duration = Duration::from_secs(30);
/// Bytes hashed of a file G7 hashes itself (a link, a path git can't be handed).
const MAX_SELF_HASH: u64 = 64 << 20;

/// What a person ticked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picks {
    /// Paths as the status listed them, relative to the thread's folder.
    Paths(Vec<String>),
    /// Everything in the folder (offered when the list was cut).
    All,
}

/// One reviewed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFile {
    pub path: String,
    /// `gone` for a deletion; a folder's is over its entries.
    pub id: String,
    pub bytes: u64,
    pub folder: bool,
    /// Entries of an untracked folder.
    pub entries: u32,
}

/// What a review pinned (§SC3.4): `id` is over `HEAD` and every picked
/// path's id. A commit given `id` refuses with `changed` unless it is still
/// what the files hash to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub id: String,
    /// The status fingerprint the review was made against.
    pub fingerprint: String,
    pub files: Vec<ReviewFile>,
    /// A folder past 2,000 entries: only the PC may commit this.
    pub pc_only: bool,
}

/// A commit made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    pub oid7: String,
    pub subject: String,
    pub files: u32,
}

impl Scm {
    /// Pins the picked files as they are now (§SC3.4).
    pub async fn review(&self, spot: &Spot, known: &GitStatus, picks: &Picks) -> Result<Review, ScmError> {
        let fresh = self.porcelain(spot, MAX_STATUS_FILES).await?;
        let entries = picked(known, &fresh, picks)?;
        if entries.is_empty() {
            return Err(ScmError::new(Reason::Nothing));
        }
        let mut review = self.pin(spot, &fresh, &entries).await?;
        review.fingerprint = known.fingerprint.clone();
        Ok(review)
    }

    /// Commits the picked files with `message` (§SC3.4). The host checks
    /// first that no thread works in this work tree.
    // The hand-off signature of phase 4 §SC10.3, kept as agreed with H.
    #[allow(clippy::too_many_arguments)]
    pub async fn commit(
        &self,
        spot: &Spot,
        known: &GitStatus,
        picks: &Picks,
        message: &str,
        review: Option<&str>,
        out: &Lines,
        stop: &Stop,
    ) -> Result<Committed, ScmError> {
        let message = message.trim();
        if message.is_empty() || message.len() > MAX_MESSAGE || message.contains('\0') {
            return Err(ScmError::new(Reason::BadMessage));
        }
        if matches!(picks, Picks::Paths(paths) if paths.is_empty()) {
            return Err(ScmError::new(Reason::Nothing));
        }
        if spot.git_dir().join("index.lock").symlink_metadata().is_ok() {
            return Err(ScmError::new(Reason::GitBusy));
        }
        if super::status::doing(spot.git_dir()).is_some() {
            return Err(ScmError::new(Reason::Doing));
        }
        let fresh = self.porcelain(spot, MAX_STATUS_FILES).await?;
        if fresh.oid.is_some() && fresh.head.is_none() {
            return Err(ScmError::new(Reason::Detached));
        }
        if fresh.conflicts > 0 {
            return Err(ScmError::new(Reason::Conflicts));
        }
        let entries = picked(known, &fresh, picks)?;
        let all = matches!(picks, Picks::All);
        if entries.is_empty() || (all && fresh.total == 0) {
            return Err(ScmError::new(Reason::Nothing));
        }
        let ident = self.read(&Op::Ident, spot.folder(), None).await?;
        if ident.code != Some(0) {
            return Err(ScmError::new(Reason::Identity));
        }
        if let Some(asked) = review
            && self.pin(spot, &fresh, &entries).await?.id != asked
        {
            return Err(ScmError::new(Reason::Changed));
        }

        // `Add` takes the paths git can still find on disk or in the index; a
        // staged deletion and a rename's old name are in neither, and `Commit`
        // records them from both names.
        let (adds, specs) = if all {
            (b":(literal).\0".to_vec(), b":(literal).\0".to_vec())
        } else {
            let mut addable: Vec<&[u8]> = Vec::new();
            for entry in &entries {
                for path in entry.paths() {
                    let on_disk = spot.top().join(OsStr::from_bytes(path)).symlink_metadata().is_ok();
                    let in_index = path == trim_slash(&entry.raw) && entry.xy[1] == b'D';
                    if on_disk || in_index {
                        addable.push(path);
                    }
                }
            }
            (pathspecs(addable), pathspecs(entries.iter().flat_map(|e| e.paths()).map(top_spec)))
        };
        let _held = self
            .git
            .lock(spot.top(), Instant::now() + LOCK_WAIT)
            .await
            .map_err(|_| ScmError::new(Reason::GitBusy))?;
        if stop.is_stopped() {
            return Err(ScmError::new(Reason::Stopped));
        }
        // An empty list would read as "everything" to `add -A`: then there is nothing to add.
        if !adds.is_empty() {
            let added = self.act(&Op::Add, spot.folder(), Some(adds), out, Some(stop), &[]).await?;
            if added.stopped {
                return Err(ScmError::new(Reason::Stopped));
            }
            if added.out.code != Some(0) {
                return Err(ScmError::new(Reason::Failed));
            }
        }
        let op = Op::Commit { message: message.to_owned() };
        let made = self.act(&op, spot.folder(), Some(specs), out, Some(stop), &[]).await?;
        if made.stopped {
            return Err(ScmError::new(Reason::Stopped));
        }
        match made.out.code {
            Some(0) => {}
            Some(1) if made.seen > 0 => return Err(ScmError::new(Reason::Hook)),
            _ => return Err(ScmError::new(Reason::Failed)),
        }
        let last = self.read(&Op::LastCommit, spot.folder(), None).await?;
        let record = last.stdout.split(|&b| b == 0).next().unwrap_or_default();
        let text = String::from_utf8_lossy(record);
        let mut parts = text.splitn(3, '\u{1f}');
        let oid = parts.next().unwrap_or_default();
        let subject = parts.nth(1).unwrap_or_default();
        let files = if all { fresh.total } else { u32::try_from(entries.len()).unwrap_or(u32::MAX) };
        Ok(Committed { oid7: oid.chars().take(7).collect(), subject: clean::text(subject, 200), files })
    }

    /// The review of `entries` as they are now.
    async fn pin(&self, spot: &Spot, fresh: &Porcelain, entries: &[&Entry]) -> Result<Review, ScmError> {
        let top = spot.top().to_owned();
        let prefix = spot.prefix().to_vec();
        let wanted: Vec<(String, bool, Vec<Vec<u8>>)> = entries
            .iter()
            .map(|e| (e.file.path.clone(), e.file.folder, e.paths().map(<[u8]>::to_vec).collect()))
            .collect();
        // What each path is, off the async threads: a folder's entries, sizes, links.
        let looked = tokio::task::spawn_blocking(move || {
            wanted
                .into_iter()
                .map(|(path, folder, raws)| {
                    let parts: Vec<(Vec<u8>, Seen)> = if folder {
                        let walked = walk_folder(&top, &raws[0], MAX_FOLDER_ENTRIES);
                        let mut parts: Vec<(Vec<u8>, Seen)> =
                            walked.files.into_iter().map(|f| (f.clone(), look(&top, &f))).collect();
                        if walked.more {
                            parts.push((Vec::new(), Seen::TooMany));
                        }
                        parts
                    } else {
                        raws.iter().map(|r| (r.clone(), look(&top, r))).collect()
                    };
                    (path, folder, parts)
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|_| ScmError::new(Reason::Failed))?;

        // Regular files go to `Ids` in one call; their ids come back in order.
        let mut asked: Vec<Vec<u8>> = Vec::new();
        for (_, _, parts) in &looked {
            for (raw, seen) in parts {
                if *seen == Seen::Git {
                    asked.push(raw.clone());
                }
            }
        }
        let mut ids = self.ids(spot, &asked).await?.into_iter();

        let mut review =
            Review { id: String::new(), fingerprint: String::new(), files: Vec::new(), pc_only: false };
        let mut whole = Sha256::new();
        whole.field(fresh.oid.as_deref().unwrap_or("unborn").as_bytes());
        for (path, folder, parts) in looked {
            let mut file = Sha256::new();
            let mut bytes = 0u64;
            let mut count = 0u32;
            for (raw, seen) in &parts {
                let id = match seen {
                    Seen::Git => ids.next().ok_or(ScmError::new(Reason::Changed))?,
                    Seen::Gone => "gone".to_owned(),
                    Seen::Own(id) => id.clone(),
                    Seen::TooMany => {
                        review.pc_only = true;
                        "more".to_owned()
                    }
                };
                bytes = bytes.saturating_add(size(&spot.top().join(OsStr::from_bytes(raw))));
                count = count.saturating_add(1);
                let name = raw.strip_prefix(prefix.as_slice()).unwrap_or(raw);
                file.field(name);
                file.field(id.as_bytes());
            }
            let id = match (folder, parts.as_slice()) {
                (false, [(_, Seen::Gone)]) => "gone".to_owned(),
                _ => file.short(),
            };
            whole.field(path.as_bytes());
            whole.field(id.as_bytes());
            review.files.push(ReviewFile {
                path,
                id,
                bytes,
                folder,
                entries: if folder { count } else { 0 },
            });
        }
        review.id = whole.short();
        Ok(review)
    }

    /// `Ids` of regular files, from the top level, in order.
    async fn ids(&self, spot: &Spot, paths: &[Vec<u8>]) -> Result<Vec<String>, ScmError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut input = Vec::new();
        for path in paths {
            input.extend_from_slice(path);
            input.push(b'\n');
        }
        let out = self.read(&Op::Ids, spot.top(), Some(input)).await?;
        if out.code != Some(0) {
            // A file went away or changed kind while it was looked at.
            return Err(ScmError::new(Reason::Changed));
        }
        let ids: Vec<String> =
            out.stdout.split(|&b| b == b'\n').filter(|l| !l.is_empty()).map(lossy).collect();
        if ids.len() != paths.len() || !ids.iter().all(|id| crate::host::git::oid_ok(id)) {
            return Err(ScmError::new(Reason::Changed));
        }
        Ok(ids)
    }
}

/// How a picked path is identified.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    /// A regular file `Ids` can hash.
    Git,
    Gone,
    /// Hashed here: a link's target, a file whose name git's list can't
    /// carry (a newline), anything else by its kind.
    Own(String),
    /// A folder past its bound.
    TooMany,
}

fn look(top: &Path, raw: &[u8]) -> Seen {
    let path = top.join(OsStr::from_bytes(raw));
    let Ok(meta) = path.symlink_metadata() else { return Seen::Gone };
    let kind = meta.file_type();
    if kind.is_file() && !raw.contains(&b'\n') {
        return Seen::Git;
    }
    let mut sha = Sha256::new();
    if kind.is_symlink() {
        sha.field(b"link");
        sha.field(
            std::fs::read_link(&path)
                .map(|t| t.as_os_str().as_bytes().to_vec())
                .unwrap_or_default()
                .as_slice(),
        );
    } else if kind.is_file() {
        sha.field(b"file");
        match read_bounded(&path) {
            Some(bytes) => sha.field(&bytes),
            None => return Seen::Gone,
        }
    } else {
        sha.field(b"other");
        sha.field(format!("{:o} {} {}", meta.mode(), meta.len(), meta.mtime_nsec()).as_bytes());
    }
    Seen::Own(sha.short())
}

fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_SELF_HASH).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn size(path: &Path) -> u64 {
    path.symlink_metadata().map(|m| if m.is_file() { m.len() } else { 0 }).unwrap_or(0)
}

/// A path from the top level as a commit names it: a folder without its `/`.
fn top_spec(raw: &[u8]) -> &[u8] {
    trim_slash(raw)
}

/// The entries of the fresh status a person picked: each path must be one
/// the host showed (`known`); one that no longer differs is left out.
pub(crate) fn picked<'a>(
    known: &GitStatus,
    fresh: &'a Porcelain,
    picks: &Picks,
) -> Result<Vec<&'a Entry>, ScmError> {
    match picks {
        Picks::All => Ok(fresh.entries.iter().collect()),
        Picks::Paths(paths) => {
            if paths.len() > MAX_STATUS_FILES {
                return Err(ScmError::new(Reason::BadRequest));
            }
            let mut out: Vec<&Entry> = Vec::new();
            for path in paths {
                if !known.files.iter().any(|f| &f.path == path) {
                    return Err(ScmError::new(Reason::NotFound));
                }
                if let Some(entry) = fresh.find(path)
                    && !out.iter().any(|e| std::ptr::eq(*e, entry))
                {
                    out.push(entry);
                }
            }
            Ok(out)
        }
    }
}
