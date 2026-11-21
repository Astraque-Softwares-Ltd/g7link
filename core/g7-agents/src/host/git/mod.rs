// SPDX-License-Identifier: MPL-2.0
//! Restore points and diffs (§U5; reconcile §1-§4).
//!
//! Before and after each turn the host records the thread's folder as a git
//! commit in G7's own bare repository, one per agent folder, under
//! `checkpoints/<folder id>.git` in the host's data folder. Nothing is
//! written into the user's `.git`: G7's repository names the user's object
//! directory as an alternate, so content the user's repository already has
//! is not stored twice, and every command runs as
//! `git --git-dir=<G7's repository> --work-tree=<the user's top level>` with
//! hooks off. [`run`] runs git, [`checkpoint`] records and compares restore
//! points and prunes them, [`diff`] serves one file's patch, and [`restore`]
//! is the only code in G7 that changes the user's files.

pub mod checkpoint;
pub mod diff;
pub mod restore;
pub mod run;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::api::MAX_DIFF_PATH;

pub use checkpoint::{
    Capture, TurnStats, capture, checkpoints_dir, delete_refs, gc, remove_repo, store_bytes, thread_ids,
    turn_stats,
};
pub use diff::{Patch, patch};
pub use restore::{Blocked, Left, LeftWhy, PlanFile, RestorePlan, apply_restore, plan_restore};
pub use run::{Git, GitError};

/// The time a turn waits for its *pre* restore point.
pub const PRE_BUDGET: Duration = Duration::from_secs(5);
/// The time the *post* restore point may take, in the background.
pub const POST_BUDGET: Duration = Duration::from_secs(20);
/// The time the snapshot a rewind takes first may take.
pub const SNAPSHOT_BUDGET: Duration = Duration::from_secs(20);
/// A new or changed file larger than this is left out of a restore point.
pub const MAX_FILE_BYTES: u64 = 10 << 20;
/// New or changed content one restore point takes, smallest files first.
pub const MAX_CAPTURE_BYTES: u64 = 48 << 20;
/// More new or changed files than this: the turn is `missing/large`.
pub const MAX_CANDIDATES: usize = 50_000;
/// More left-out files than this: the turn is `missing/large`.
pub const MAX_LEFT_OUT: usize = 200;

/// Where one thread's restore points live: G7's repository for the thread's
/// agent folder, and the folder the thread works in (the agent folder or a
/// sub-folder of it). Cheap to clone.
#[derive(Debug, Clone)]
pub struct Place {
    git: Arc<Git>,
    repo: PathBuf,
    folder: PathBuf,
}

impl Place {
    /// `checkpoints`: [`checkpoints_dir`] of the host's data folder.
    /// `None` when `folder_id` is not a plain name or `folder` is not absolute.
    pub fn new(git: Arc<Git>, checkpoints: &Path, folder_id: &str, folder: &Path) -> Option<Place> {
        if !folder.is_absolute() {
            return None;
        }
        Some(Place { git, repo: repo_path(checkpoints, folder_id)?, folder: folder.to_owned() })
    }

    /// G7's repository for the agent folder.
    pub fn repo(&self) -> &Path {
        &self.repo
    }

    /// The thread's folder.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn git(&self) -> &Arc<Git> {
        &self.git
    }
}

/// G7's repository for an agent folder; `None` when the id is not a plain name.
pub fn repo_path(checkpoints: &Path, folder_id: &str) -> Option<PathBuf> {
    let plain = (1..=64).contains(&folder_id.len())
        && folder_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    plain.then(|| checkpoints.join(format!("{folder_id}.git")))
}

fn thread_ok(thread: &str) -> bool {
    (1..=64).contains(&thread.len()) && thread.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

/// Every ref of a thread: `refs/g7/<thread>/`.
pub fn thread_refs(thread: &str) -> Option<String> {
    thread_ok(thread).then(|| format!("refs/g7/{thread}/"))
}

/// A turn's two refs: `refs/g7/<thread>/<turn>/`.
pub fn turn_refs(thread: &str, turn: u32) -> Option<String> {
    thread_ok(thread).then(|| format!("refs/g7/{thread}/{turn}/"))
}

/// The restore point taken before a turn.
pub fn pre_ref(thread: &str, turn: u32) -> Option<String> {
    turn_refs(thread, turn).map(|p| p + "pre")
}

/// The restore point taken after a turn.
pub fn post_ref(thread: &str, turn: u32) -> Option<String> {
    turn_refs(thread, turn).map(|p| p + "post")
}

/// What the folder held just before the thread's last rewind changed it.
pub fn undo_ref(thread: &str) -> Option<String> {
    thread_refs(thread).map(|p| p + "undo")
}

/// The snapshot a rewind preview takes (moved to [`undo_ref`] when applied).
pub(crate) fn preview_ref(thread: &str) -> Option<String> {
    thread_refs(thread).map(|p| p + "preview")
}

/// A ref under `refs/g7/` made of lowercase letters, digits and single `/`
/// between non-empty parts.
pub(crate) fn ref_ok(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("refs/g7/") else { return false };
    !rest.is_empty()
        && name.len() <= 256
        && rest.split('/').all(|part| {
            !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
        })
}

/// A full object id (SHA-1).
pub(crate) fn oid_ok(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// What a caller may name a restore point by: a ref of G7's or an object id.
pub(crate) fn rev_ok(s: &str) -> bool {
    oid_ok(s) || ref_ok(s)
}

/// A path git printed (from the top level of the repository) as a path
/// relative to the thread's folder: `None` unless it is inside the folder,
/// relative, without empty, `.` or `..` parts, and at most
/// [`MAX_DIFF_PATH`] bytes once made UTF-8. Paths from git are repository
/// content, never trusted.
pub(crate) fn folder_path(raw: &[u8], prefix: &[u8]) -> Option<String> {
    let rest = raw.strip_prefix(prefix)?;
    if rest.is_empty() || !path_parts_ok(rest) {
        return None;
    }
    let text = String::from_utf8_lossy(rest);
    (text.len() <= MAX_DIFF_PATH).then(|| text.into_owned())
}

/// Relative, without empty, `.` or `..` parts and without NUL.
pub(crate) fn path_parts_ok(raw: &[u8]) -> bool {
    !raw.is_empty()
        && !raw.contains(&0)
        && raw.split(|&b| b == b'/').all(|part| !part.is_empty() && part != b"." && part != b"..")
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 15)]));
    }
    out
}

pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    s.as_bytes().chunks(2).map(|pair| Some(digit(pair[0])? << 4 | digit(pair[1])?)).collect()
}
