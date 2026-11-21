// SPDX-License-Identifier: MPL-2.0
//! The uncommitted diff of one file (§SC3.3): what the file on disk changes
//! from `HEAD`, with the same paging, caps and cleaning as a turn's diff.
//! An untracked file is diffed against nothing; an untracked folder gets a
//! list of the names inside instead.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::argv::{Op, arg_path};
use super::status::trim_slash;
use super::{GitStatus, Reason, Scm, ScmError, Spot, lossy};
use crate::api::{MAX_PATCH_FILE, clean};

/// Names an untracked folder's row lists.
pub const MAX_FOLDER_NAMES: usize = 50;
/// Entries of an untracked folder a review hashes; past it the commit is PC only.
pub const MAX_FOLDER_ENTRIES: usize = 2000;
const WALK_TIME: Duration = Duration::from_secs(5);

/// One page of a file's uncommitted patch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Patch {
    /// Unified diff text, cleaned, cut at a line end.
    pub text: String,
    /// Where the next page starts.
    pub next: Option<u64>,
    /// The patch passed 1 MiB: the rest isn't served.
    pub truncated: bool,
    pub binary: bool,
    /// An untracked folder: the first names inside, relative to the thread's folder.
    pub names: Vec<String>,
    /// An untracked folder: names inside past `names` (counted up to 2,000).
    pub more_names: u32,
}

impl Scm {
    /// The page at `offset` of `path`'s patch; `path` must be a file of
    /// `known`, the status the host last read for the thread.
    pub async fn patch(
        &self,
        spot: &Spot,
        known: &GitStatus,
        path: &str,
        offset: u64,
    ) -> Result<Patch, ScmError> {
        let file = known.files.iter().find(|f| f.path == path).ok_or(ScmError::new(Reason::NotFound))?;
        let fresh = self.porcelain(spot, usize::MAX).await?;
        let entry = fresh.find(path).ok_or(ScmError::new(Reason::NotFound))?;
        if file.folder || entry.file.folder {
            let top = spot.top().to_owned();
            let dir = trim_slash(&entry.raw).to_vec();
            let prefix = spot.prefix().to_vec();
            let walked = tokio::task::spawn_blocking(move || walk_folder(&top, &dir, MAX_FOLDER_ENTRIES))
                .await
                .map_err(|_| ScmError::new(Reason::Failed))?;
            let mut names: Vec<String> = walked
                .files
                .iter()
                .filter_map(|raw| raw.strip_prefix(prefix.as_slice()).map(lossy))
                .collect();
            let more = names.len().saturating_sub(MAX_FOLDER_NAMES);
            names.truncate(MAX_FOLDER_NAMES);
            let more = u32::try_from(more).unwrap_or(u32::MAX).saturating_add(u32::from(walked.more));
            return Ok(Patch { names, more_names: more, ..Patch::default() });
        }
        let new = fresh.oid.is_none() || entry.kind == b'?';
        let op = if new {
            Op::PatchNew { path: arg_path(&entry.raw) }
        } else {
            Op::Patch { path: arg_path(&entry.raw), from: entry.from_raw.as_deref().map(arg_path) }
        };
        let out = self.read(&op, spot.top(), None).await?;
        let ok = match out.code {
            Some(0) => true,
            Some(1) => new,
            _ => false,
        };
        if !ok && !out.over {
            return Err(ScmError::new(Reason::Failed));
        }
        let whole = shape(&out.stdout, out.over);
        let (text, next) = whole.page(offset).ok_or(ScmError::new(Reason::BadRequest))?;
        Ok(Patch {
            text: text.to_owned(),
            next,
            truncated: whole.truncated,
            binary: whole.binary,
            ..Patch::default()
        })
    }
}

/// The patch as served: binary seen, cleaned, cut to [`MAX_PATCH_FILE`] at
/// a line end (as a turn's patch is).
pub(crate) fn shape(raw: &[u8], over: bool) -> crate::host::git::Patch {
    use crate::host::git::Patch as Whole;
    let head_end = raw.windows(3).position(|w| w == b"@@ ").unwrap_or(raw.len());
    let binary = raw[..head_end]
        .split(|&b| b == b'\n')
        .any(|line| line.starts_with(b"Binary files ") || line.starts_with(b"GIT binary patch"));
    if binary {
        return Whole { binary: true, ..Whole::default() };
    }
    let mut text = clean(&String::from_utf8_lossy(raw));
    let mut truncated = over;
    if over {
        let keep = text.rfind('\n').map_or(0, |nl| nl + 1);
        text.truncate(keep);
    }
    if text.len() > MAX_PATCH_FILE {
        let keep = text.as_bytes()[..MAX_PATCH_FILE].iter().rposition(|&b| b == b'\n').map_or(0, |nl| nl + 1);
        text.truncate(keep);
        truncated = true;
    }
    Whole { text, truncated, ..Whole::default() }
}

/// What a walk of an untracked folder found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Walked {
    /// Files, links and nested repositories, from the top level, sorted.
    pub files: Vec<Vec<u8>>,
    /// More than `max` (or the walk took too long): the list is partial.
    pub more: bool,
}

/// The entries git would add under `dir` (from the top level): files and
/// links not ignored, a nested repository as one entry, nothing followed
/// through a link. Blocking.
pub(crate) fn walk_folder(top: &Path, dir: &[u8], max: usize) -> Walked {
    let root = top.join(OsStr::from_bytes(dir));
    let mut builder = ignore::WalkBuilder::new(&root);
    builder
        .standard_filters(true)
        .hidden(false)
        .ignore(false)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git");
    let started = Instant::now();
    let mut walked = Walked::default();
    let mut nested: Vec<PathBuf> = Vec::new();
    for found in builder.build() {
        if walked.files.len() > max || started.elapsed() > WALK_TIME {
            walked.more = true;
            break;
        }
        let Ok(entry) = found else { continue };
        let Some(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if nested.iter().any(|n| path.starts_with(n)) {
            continue;
        }
        if kind.is_dir() {
            if entry.depth() > 0 && path.join(".git").symlink_metadata().is_ok() {
                nested.push(path.to_owned());
            } else {
                continue;
            }
        }
        let Ok(rel) = path.strip_prefix(top) else { continue };
        walked.files.push(rel.as_os_str().as_bytes().to_vec());
    }
    if walked.files.len() > max {
        walked.files.truncate(max);
        walked.more = true;
    }
    walked.files.sort();
    walked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_controls_and_overflow() {
        let bin = b"diff --git a/x b/x\nnew file mode 100644\nBinary files /dev/null and b/x differ\n";
        assert!(shape(bin, false).binary);
        let text = b"diff --git a/x b/x\n@@ -1 +1 @@\n-a\x1b[31m\r\n+b\n+partial";
        let shaped = shape(text, true);
        assert_eq!(shaped.text, "diff --git a/x b/x\n@@ -1 +1 @@\n-a[31m\n+b\n");
        assert!(shaped.truncated);
        let big = "+".repeat(99) + "\n";
        let shaped = shape(big.repeat(20_000).as_bytes(), false);
        assert!(shaped.truncated && shaped.text.len() <= MAX_PATCH_FILE && shaped.text.ends_with('\n'));
    }

    #[test]
    fn a_folder_walk_skips_ignored_files_and_counts_a_nested_repository_once() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path();
        std::fs::create_dir_all(top.join("docs/img")).unwrap();
        std::fs::create_dir_all(top.join("docs/inner/.git")).unwrap();
        std::fs::write(top.join("docs/inner/file"), "x").unwrap();
        std::fs::write(top.join("docs/a.md"), "a").unwrap();
        std::fs::write(top.join("docs/.hidden"), "h").unwrap();
        std::fs::write(top.join("docs/img/p.png"), "p").unwrap();
        std::fs::write(top.join("docs/skip.log"), "s").unwrap();
        std::fs::write(top.join(".gitignore"), "*.log\n").unwrap();
        let walked = walk_folder(top, b"docs", 100);
        let names: Vec<String> = walked.files.iter().map(|f| lossy(f)).collect();
        assert_eq!(names, ["docs/.hidden", "docs/a.md", "docs/img/p.png", "docs/inner"]);
        assert!(!walked.more);
        let few = walk_folder(top, b"docs", 2);
        assert!(few.more && few.files.len() == 2);
    }
}
