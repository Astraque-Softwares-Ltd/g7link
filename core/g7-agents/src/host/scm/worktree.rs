// SPDX-License-Identifier: MPL-2.0
//! Worktrees for threads (§SC3.8): a new branch checked out in a folder of
//! G7's own, from what is committed, and removing that folder again only
//! when nothing in it is uncommitted. The branch always stays. G7 never
//! prunes, never forces, never removes anything on its own.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::argv::Op;
use super::{Lines, Reason, Scm, ScmError, Spot, clean};

/// How long adding or removing a worktree waits for the repository's lock.
const LOCK_WAIT: Duration = Duration::from_secs(30);
/// Entries a size walk counts before it says "over".
const SIZE_ENTRIES: u64 = 200_000;
const SIZE_TIME: Duration = Duration::from_secs(5);
/// Bytes of a branch name in a worktree's folder name.
const SLUG_BYTES: usize = 40;

/// A worktree's commits on no remote, read before it is removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unpushed {
    pub commits: u32,
    pub branch: Option<String>,
}

/// One work tree of a repository, as `Worktrees` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub path: PathBuf,
    pub branch: Option<String>,
    /// The repository's main work tree (listed first).
    pub main: bool,
    pub locked: bool,
    pub prunable: bool,
}

impl Scm {
    /// Adds a worktree with a new branch `branch` from `base` (a name
    /// `Refs` listed; `None`: `HEAD`) in a new folder under `root` (G7's
    /// worktrees folder for the agent folder). Returns the folder. The host
    /// counts its worktrees first.
    pub async fn worktree_add(
        &self,
        spot: &Spot,
        root: &Path,
        branch: &str,
        base: Option<&str>,
        out: &Lines,
    ) -> Result<PathBuf, ScmError> {
        if !root.is_absolute() {
            return Err(ScmError::new(Reason::BadRequest));
        }
        if spot.git_dir().join("index.lock").symlink_metadata().is_ok() {
            return Err(ScmError::new(Reason::GitBusy));
        }
        let fresh = self.porcelain(spot, 0).await?;
        if fresh.oid.is_none() {
            return Err(ScmError::new(Reason::NoCommits));
        }
        let rows = self.known_refs(spot).await?;
        self.check_new_name(spot, branch, &rows).await?;
        let base = match base {
            None => "HEAD".to_owned(),
            Some(name) => rows
                .iter()
                .find(|r| r.name == name)
                .map(|r| r.name.clone())
                .ok_or(ScmError::new(Reason::NotFound))?,
        };
        let path = make_folder(root, branch).map_err(|_| ScmError::new(Reason::Failed))?;
        let held = self.git.lock(spot.top(), Instant::now() + LOCK_WAIT).await;
        let added = match held {
            Ok(_held) => {
                let op = Op::WorktreeAdd { path: path.clone(), branch: branch.to_owned(), base };
                self.act(&op, spot.folder(), None, out, None, &[]).await
            }
            Err(_) => Err(ScmError::new(Reason::GitBusy)),
        };
        self.forget_names(spot);
        match added {
            Ok(added) if added.out.code == Some(0) => Ok(path),
            Ok(_) => {
                let _ = std::fs::remove_dir(&path);
                Err(ScmError::new(Reason::Failed))
            }
            Err(e) => {
                let _ = std::fs::remove_dir(&path);
                Err(e)
            }
        }
    }

    /// Removes the linked worktree at `path` (one of `spot`'s repository's),
    /// refused with `dirty` while it has any uncommitted or untracked file.
    /// The branch and its commits stay; the result says how many are on no
    /// remote.
    pub async fn worktree_remove(&self, spot: &Spot, path: &Path) -> Result<Unpushed, ScmError> {
        let listed = self.worktrees(spot).await?;
        if !listed.iter().any(|w| !w.main && w.path == path) {
            return Err(ScmError::new(Reason::NotFound));
        }
        let unpushed = self.unpushed(path).await?;
        let there = self.spot(path).await.map_err(|_| ScmError::new(Reason::NotFound))?;
        if self.porcelain_in(&there, there.top(), 1).await?.total > 0 {
            return Err(ScmError::new(Reason::Dirty));
        }
        let _held = self
            .git
            .lock(spot.top(), Instant::now() + LOCK_WAIT)
            .await
            .map_err(|_| ScmError::new(Reason::GitBusy))?;
        let lines = Lines::new();
        let removed = self
            .act(&Op::WorktreeRemove { path: path.to_owned() }, spot.folder(), None, &lines, None, &[])
            .await?;
        self.forget_names(spot);
        if removed.out.code == Some(0) {
            Ok(unpushed)
        } else {
            Err(ScmError::with_lines(Reason::Failed, lines.lines()))
        }
    }

    /// The commits of the worktree at `path` that are on no remote, and its branch.
    pub async fn unpushed(&self, path: &Path) -> Result<Unpushed, ScmError> {
        let there = self.spot(path).await.map_err(|_| ScmError::new(Reason::NotFound))?;
        let head = self.porcelain_in(&there, there.top(), 0).await?;
        let out = self.read(&Op::Unpushed, path, None).await?;
        let commits = if out.code == Some(0) {
            String::from_utf8_lossy(out.line()).trim().parse::<u32>().unwrap_or(0)
        } else {
            0
        };
        Ok(Unpushed { commits, branch: head.head })
    }

    /// The repository's work trees, the main one first.
    pub async fn worktrees(&self, spot: &Spot) -> Result<Vec<Listed>, ScmError> {
        let out = self.read(&Op::Worktrees, spot.folder(), None).await?;
        if out.code != Some(0) {
            return Err(ScmError::new(Reason::Failed));
        }
        Ok(parse_worktrees(&out.stdout))
    }
}

/// A new folder for a worktree, mode 0700, under `root` (made 0700 too):
/// `<branch with / as ->-<6 hex>`, another suffix when that one exists.
fn make_folder(root: &Path, branch: &str) -> std::io::Result<PathBuf> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(root)?;
    let slug: String = branch.replace('/', "-").chars().take(SLUG_BYTES).collect();
    let mut last = None;
    for _ in 0..8 {
        let path = root.join(format!("{slug}-{:06x}", rand::random::<u32>() & 0x00ff_ffff));
        match std::fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok(path),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no free name")))
}

/// `Worktrees`' output: records of NUL-ended `key value` lines, an empty
/// line between records. A record's first line names its folder; the line
/// whose value is a `refs/heads/` name is its branch.
pub(crate) fn parse_worktrees(bytes: &[u8]) -> Vec<Listed> {
    let mut out: Vec<Listed> = Vec::new();
    let mut current: Option<Listed> = None;
    for line in bytes.split(|&b| b == 0) {
        if line.is_empty() {
            out.extend(current.take());
            continue;
        }
        let (key, value) = match line.iter().position(|&b| b == b' ') {
            Some(at) => (&line[..at], &line[at + 1..]),
            None => (line, &b""[..]),
        };
        let Some(w) = current.as_mut() else {
            let main = out.is_empty();
            current = Some(Listed {
                path: PathBuf::from(OsStr::from_bytes(value)),
                branch: None,
                main,
                locked: false,
                prunable: false,
            });
            continue;
        };
        if let Some(name) = value.strip_prefix(b"refs/heads/") {
            w.branch = Some(clean::text(&String::from_utf8_lossy(name), 255));
        } else if key == b"locked" {
            w.locked = true;
        } else if key == b"prunable" {
            w.prunable = true;
        }
    }
    out.extend(current);
    out
}

/// The bytes under a worktree's folder, for Setup: links never followed,
/// at most 200,000 entries and 5 s, else `over` with what was counted.
pub async fn worktree_size(path: PathBuf) -> (u64, bool) {
    tokio::task::spawn_blocking(move || size_of(&path)).await.unwrap_or((0, true))
}

fn size_of(root: &Path) -> (u64, bool) {
    let started = Instant::now();
    let mut bytes = 0u64;
    let mut seen = 0u64;
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > SIZE_ENTRIES || started.elapsed() > SIZE_TIME {
                return (bytes, true);
            }
            let Ok(meta) = entry.path().symlink_metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                bytes = bytes.saturating_add(meta.len());
            }
        }
    }
    (bytes, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_lists_are_read() {
        let raw = b"worktree /r\0HEAD 1111111111111111111111111111111111111111\0branch refs/heads/main\0\0worktree /w/g7-x-abc123\0HEAD 2222222222222222222222222222222222222222\0branch refs/heads/g7/x\0locked because\0\0worktree /w/d\0HEAD 3333333333333333333333333333333333333333\0detached\0prunable gitdir file points to non-existent location\0\0";
        let listed = parse_worktrees(raw);
        assert_eq!(listed.len(), 3);
        assert!(listed[0].main && !listed[1].main && !listed[2].main);
        assert_eq!(listed[1].branch.as_deref(), Some("g7/x"));
        assert!(listed[1].locked && listed[2].prunable && listed[2].branch.is_none());
        assert_eq!(listed[1].path, Path::new("/w/g7-x-abc123"));
    }

    #[test]
    fn folders_are_0700_and_never_reused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("worktrees/f1");
        let a = make_folder(&root, "g7/fix").unwrap();
        let b = make_folder(&root, "g7/fix").unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with(&root) && a.file_name().unwrap().to_string_lossy().starts_with("g7-fix-"));
        for p in [&root, &a] {
            assert_eq!(std::fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o700, "{}", p.display());
        }
    }

    #[test]
    fn sizes_count_files_and_skip_links() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/b/f"), vec![0u8; 1000]).unwrap();
        std::fs::write(dir.path().join("g"), vec![0u8; 24]).unwrap();
        std::os::unix::fs::symlink("/", dir.path().join("root")).unwrap();
        let (bytes, over) = size_of(dir.path());
        assert!(!over);
        assert!((1024..1024 + 64).contains(&bytes), "{bytes}");
    }
}
