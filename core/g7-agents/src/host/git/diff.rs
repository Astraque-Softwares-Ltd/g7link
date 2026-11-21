// SPDX-License-Identifier: MPL-2.0
//! One file's patch between two restore points (§U5.4): only a path the
//! diff between them lists, at most [`MAX_PATCH_FILE`] bytes of it, cleaned
//! like tool output, served in pages cut at line ends. Nothing is cached:
//! the refs are the store.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::checkpoint::{git_dir, pair, parse_pairs, path_spec, spec_of};
use super::run::{Call, GitError, MAX_PATCH_OUT};
use super::{Place, folder_path};
use crate::api::{MAX_PATCH_FILE, MAX_PATCH_PAGE, clean};

const LIST_LIMIT: Duration = Duration::from_secs(5);
const PATCH_LIMIT: Duration = Duration::from_secs(10);
const PATCH_BUDGET: Duration = Duration::from_secs(16);

/// A file's patch, whole or cut.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Patch {
    /// Unified diff text (`-U3`), control characters removed, cut at a line end.
    pub text: String,
    /// A binary file: no text.
    pub binary: bool,
    /// The patch passed [`MAX_PATCH_FILE`]: the rest isn't served.
    pub truncated: bool,
    /// The file was left out of one of the two restore points: no text.
    pub unrecorded: bool,
}

impl Patch {
    /// The page starting at `offset` (0, or a `next` this returned): at most
    /// [`MAX_PATCH_PAGE`] bytes ending at a line end (a single longer line is
    /// cut at a character), and where the next page starts. `None`: an
    /// offset this patch never gave.
    pub fn page(&self, offset: u64) -> Option<(&str, Option<u64>)> {
        let start = usize::try_from(offset).ok()?;
        if start > self.text.len() || !self.text.is_char_boundary(start) {
            return None;
        }
        let rest = &self.text[start..];
        let mut end = rest.len();
        if end > MAX_PATCH_PAGE {
            let window = &rest.as_bytes()[..MAX_PATCH_PAGE];
            end = match window.iter().rposition(|&b| b == b'\n') {
                Some(nl) => nl + 1,
                None => (1..=MAX_PATCH_PAGE).rev().find(|&i| rest.is_char_boundary(i)).unwrap_or(0),
            };
        }
        let next = (start + end < self.text.len()).then(|| (start + end) as u64);
        Some((&rest[..end], next))
    }
}

/// The patch of `path` (relative to the folder, as [`super::turn_stats`]
/// listed it) from `from` to `to` (refs of G7's or object ids). A path the
/// diff between them doesn't list is [`GitError::NotFound`].
pub async fn patch(place: &Place, from: &str, to: &str, path: &str) -> Result<Patch, GitError> {
    let deadline = Instant::now() + PATCH_BUDGET;
    let (a, b) = pair(place, from, to, deadline).await?;
    let left_out: HashSet<String> =
        a.skipped.iter().chain(&b.skipped).filter_map(|raw| folder_path(raw, &a.prefix)).collect();
    if left_out.contains(path) {
        return Ok(Patch { unrecorded: true, ..Patch::default() });
    }
    let list = Call::new(&place.repo, LIST_LIMIT)
        .arg(git_dir(place))
        .args(["diff-tree", "-r", "-M", "--raw", "-z", &a.oid, &b.oid, "--"])
        .args(spec_of(&a.prefix));
    let list = place.git.run(list, deadline).await?;
    if !list.ok() && !list.over {
        return Err(GitError::Failed(list.why()));
    }
    let (pairs, _, _) = parse_pairs(&list.stdout);
    let found = pairs
        .iter()
        .find(|p| folder_path(&p.path, &a.prefix).as_deref() == Some(path))
        .ok_or(GitError::NotFound)?;

    let mut paths = vec![found.path.as_slice()];
    paths.extend(found.from.as_deref());
    let call = Call::new(&place.repo, PATCH_LIMIT)
        .arg(git_dir(place))
        .args([
            "diff-tree",
            "-p",
            "-M",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "-U3",
            &a.oid,
            &b.oid,
            "--",
        ])
        .args(paths.iter().map(|raw| path_spec(raw)))
        .cap(MAX_PATCH_OUT);
    let out = place.git.run(call, deadline).await?;
    if !out.ok() && !out.over {
        return Err(GitError::Failed(out.why()));
    }
    Ok(shape(&out.stdout, out.over))
}

/// The patch as served: binary detected, cleaned, cut to [`MAX_PATCH_FILE`]
/// at a line end.
fn shape(raw: &[u8], over: bool) -> Patch {
    let head_end = raw.windows(3).position(|w| w == b"@@ ").unwrap_or(raw.len());
    let binary = raw[..head_end]
        .split(|&b| b == b'\n')
        .any(|line| line.starts_with(b"Binary files ") || line.starts_with(b"GIT binary patch"));
    if binary {
        return Patch { binary: true, ..Patch::default() };
    }
    let mut text = clean(&String::from_utf8_lossy(raw));
    let mut truncated = over;
    if over {
        // The last line read may be partial.
        let keep = text.rfind('\n').map_or(0, |nl| nl + 1);
        text.truncate(keep);
    }
    if text.len() > MAX_PATCH_FILE {
        let keep = text.as_bytes()[..MAX_PATCH_FILE].iter().rposition(|&b| b == b'\n').map_or(0, |nl| nl + 1);
        text.truncate(keep);
        truncated = true;
    }
    Patch { text, truncated, ..Patch::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_end_at_line_ends_and_reassemble() {
        let line = "x".repeat(1000) + "\n";
        let patch = Patch { text: line.repeat(500), ..Patch::default() };
        let mut offset = 0;
        let mut whole = String::new();
        let mut pages = 0;
        loop {
            let (page, next) = patch.page(offset).unwrap();
            assert!(page.len() <= MAX_PATCH_PAGE && page.ends_with('\n'));
            whole.push_str(page);
            pages += 1;
            match next {
                Some(n) => offset = n,
                None => break,
            }
        }
        assert_eq!(whole, patch.text);
        assert_eq!(pages, 3);
        assert!(patch.page(1).is_some());
        assert!(patch.page(patch.text.len() as u64 + 1).is_none());
        let wide = Patch { text: "é".repeat(MAX_PATCH_PAGE), ..Patch::default() };
        let (page, next) = wide.page(0).unwrap();
        assert!(page.len() <= MAX_PATCH_PAGE && next == Some(page.len() as u64));
        assert!(wide.page(1).is_none());
    }

    #[test]
    fn binary_is_seen_and_controls_and_overflow_are_cut() {
        let bin = b"diff --git a/x b/x\nindex 1..2 100644\nBinary files a/x and b/x differ\n";
        assert!(shape(bin, false).binary);
        let text = b"diff --git a/x b/x\n@@ -1 +1 @@\n-a\x1b[31m\r\n+b\n+partial";
        let shaped = shape(text, true);
        assert_eq!(shaped.text, "diff --git a/x b/x\n@@ -1 +1 @@\n-a[31m\n+b\n");
        assert!(shaped.truncated && !shaped.binary);
        let big = "+".repeat(99) + "\n";
        let shaped = shape(big.repeat(20_000).as_bytes(), false);
        assert!(shaped.truncated && shaped.text.len() <= MAX_PATCH_FILE && shaped.text.ends_with('\n'));
    }
}
