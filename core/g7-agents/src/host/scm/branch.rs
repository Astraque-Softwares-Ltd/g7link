// SPDX-License-Identifier: MPL-2.0
//! Branches (§SC3.7): the list, and switching to one, to one on a remote,
//! or to a new one. A name a client sends is never placed in argv as it
//! came: it must be byte for byte a name `Refs` listed in the last minute,
//! or, for a new one, pass G7's own rules, `RefOk`, and not exist. A switch
//! with changes passes them to git as they are: git carries them or refuses.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::argv::{Op, REFS_FIELDS};
use super::{Branch, Lines, MAX_BRANCHES, Reason, RefRow, Scm, ScmError, Spot, remote_of};

/// How long a switch waits for the repository's lock.
const LOCK_WAIT: Duration = Duration::from_secs(30);

/// The branch list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Branches {
    pub branches: Vec<Branch>,
    pub more: bool,
    pub current: Option<String>,
}

/// Where a switch goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchTo {
    /// A local branch.
    Local(String),
    /// A branch only on a remote, as `remote/name`: a local one of the
    /// same name is made, following it.
    Track(String),
    /// A new branch at `HEAD`.
    New(String),
}

impl Scm {
    /// Local branches first, then those only on a remote, each newest first
    /// (§SC3.7). `query`: a case-folded substring.
    pub async fn branches(&self, spot: &Spot, query: Option<&str>) -> Result<Branches, ScmError> {
        let rows = self.refs(spot).await?;
        Ok(list(&rows, spot.top(), query))
    }

    /// Switches the work tree (§SC3.7); returns the branch now checked
    /// out. `carry`: the person agreed that changes go along (G7 Link's own
    /// connection only; the host checks). The host checks first that no
    /// thread works in this work tree.
    pub async fn switch(
        &self,
        spot: &Spot,
        to: &SwitchTo,
        carry: bool,
        out: &Lines,
    ) -> Result<String, ScmError> {
        if spot.git_dir().join("index.lock").symlink_metadata().is_ok() {
            return Err(ScmError::new(Reason::GitBusy));
        }
        if super::status::doing(spot.git_dir()).is_some() {
            return Err(ScmError::new(Reason::Doing));
        }
        let rows = self.known_refs(spot).await?;
        let locals = || rows.iter().filter(|r| r.remote.is_none());
        let (op, name) = match to {
            SwitchTo::Local(name) => {
                let row = locals().find(|r| &r.name == name).ok_or(ScmError::new(Reason::NotFound))?;
                if row.head {
                    return Ok(name.clone());
                }
                if row.worktree.as_deref().is_some_and(|w| w != spot.top()) {
                    return Err(ScmError::new(Reason::InWorktree));
                }
                (Op::Switch { branch: name.clone() }, name.clone())
            }
            SwitchTo::Track(remote_branch) => {
                let row = rows
                    .iter()
                    .find(|r| r.remote.is_some() && &r.name == remote_branch)
                    .ok_or(ScmError::new(Reason::NotFound))?;
                if locals().any(|r| r.name == row.short) {
                    return Err(ScmError::new(Reason::Exists));
                }
                (Op::SwitchTrack { remote_branch: remote_branch.clone() }, row.short.clone())
            }
            SwitchTo::New(name) => {
                self.check_new_name(spot, name, &rows).await?;
                (Op::SwitchNew { name: name.clone() }, name.clone())
            }
        };
        let dirty = self.porcelain_in(spot, spot.top(), 1).await?.total > 0;
        if dirty && !carry {
            return Err(ScmError::new(Reason::Dirty));
        }
        let _held = self
            .git
            .lock(spot.top(), Instant::now() + LOCK_WAIT)
            .await
            .map_err(|_| ScmError::new(Reason::GitBusy))?;
        let switched = self.act(&op, spot.folder(), None, out, None, &[]).await?;
        self.forget_names(spot);
        match switched.out.code {
            Some(0) => Ok(name),
            Some(1) if dirty => Err(ScmError::new(Reason::WouldOverwrite)),
            _ => Err(ScmError::new(Reason::Failed)),
        }
    }

    /// A new branch's name: G7's rules, then `RefOk`, then not in `rows`.
    pub(crate) async fn check_new_name(
        &self,
        spot: &Spot,
        name: &str,
        rows: &[RefRow],
    ) -> Result<(), ScmError> {
        if !new_name_ok(name) {
            return Err(ScmError::new(Reason::BadName));
        }
        let ok = self.read(&Op::RefOk { name: name.to_owned() }, spot.folder(), None).await?;
        if ok.code != Some(0) {
            return Err(ScmError::new(Reason::BadName));
        }
        if rows.iter().any(|r| r.remote.is_none() && r.name == name) {
            return Err(ScmError::new(Reason::Exists));
        }
        Ok(())
    }
}

/// A new branch name by G7's own rules (§SC3.1), before git is asked: 1 to
/// 100 bytes of ASCII letters, digits and `/ . _ -`; not starting with `-`,
/// `/` or `.`; no `..`, `//` or `@{`; not ending in `/`, `.` or `.lock`;
/// not `HEAD` and not `@`.
pub fn new_name_ok(name: &str) -> bool {
    (1..=100).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
        && !name.starts_with(['-', '/', '.'])
        && !name.contains("..")
        && !name.contains("//")
        && !name.contains("@{")
        && !name.ends_with(['/', '.'])
        && !name.ends_with(".lock")
        && name != "HEAD"
        && name != "@"
}

/// The default branch name for a thread's worktree (§SC3.8): `g7/` and the
/// title in lowercase ASCII letters and digits, other runs `-`, at most 40;
/// `fallback` (6 hex) when nothing is left.
pub fn worktree_branch_name(title: &str, fallback: &str) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    let mut slug: String = slug.chars().take(40).collect();
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() { format!("g7/{fallback}") } else { format!("g7/{slug}") }
}

/// The branch list from `rows` (as [`parse_refs`] read them).
fn list(rows: &[RefRow], top: &Path, query: Option<&str>) -> Branches {
    let query = query.map(str::to_lowercase).filter(|q| !q.is_empty());
    let matches = |row: &RefRow| query.as_ref().is_none_or(|q| row.name.to_lowercase().contains(q.as_str()));
    let local_names: Vec<&str> =
        rows.iter().filter(|r| r.remote.is_none()).map(|r| r.name.as_str()).collect();
    let current = rows.iter().find(|r| r.remote.is_none() && r.head).map(|r| r.name.clone());
    let locals = rows.iter().filter(|r| r.remote.is_none());
    let remotes = rows.iter().filter(|r| r.remote.is_some() && !local_names.contains(&r.short.as_str()));
    let mut out = Branches { current, ..Branches::default() };
    for row in locals.chain(remotes).filter(|r| matches(r)) {
        if out.branches.len() >= MAX_BRANCHES {
            out.more = true;
            break;
        }
        let elsewhere = row.worktree.as_deref().filter(|w| *w != top && !row.head);
        out.branches.push(Branch {
            name: row.name.clone(),
            remote: row.remote.clone(),
            current: row.remote.is_none() && row.head,
            upstream: row.upstream.clone(),
            ahead: row.ahead,
            behind: row.behind,
            worktree: elsewhere.map(|w| {
                let label = w.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                super::clean::text(&label, 64)
            }),
            last_ms: row.last_ms,
        });
    }
    out
}

/// `Refs`' output read into rows: six NUL-ended fields a ref, a newline
/// between refs. A ref whose name isn't plain UTF-8, or is a remote's
/// `HEAD`, is left out.
pub(crate) fn parse_refs(bytes: &[u8], remotes: &[(String, String)]) -> Vec<RefRow> {
    let fields: Vec<&[u8]> = bytes.split(|&b| b == 0).collect();
    let mut rows = Vec::new();
    for record in fields.as_chunks::<REFS_FIELDS>().0 {
        let [name, upstream, track, date, head, worktree] = record;
        let name = name.strip_prefix(b"\n").unwrap_or(name);
        let Ok(full) = std::str::from_utf8(name) else { continue };
        let text = |raw: &[u8]| std::str::from_utf8(raw).ok().filter(|s| !s.is_empty()).map(str::to_owned);
        let (short_name, remote, short) = if let Some(local) = full.strip_prefix("refs/heads/") {
            (local.to_owned(), None, local.to_owned())
        } else if let Some(rest) = full.strip_prefix("refs/remotes/") {
            let remote = remote_of(full, remotes).or_else(|| rest.split('/').next().map(str::to_owned));
            let Some(remote) = remote else { continue };
            let Some(short) = rest.strip_prefix(remote.as_str()).and_then(|r| r.strip_prefix('/')) else {
                continue;
            };
            if short == "HEAD" || short.is_empty() {
                continue;
            }
            (rest.to_owned(), Some(remote), short.to_owned())
        } else {
            continue;
        };
        if short_name.len() > 255 || short_name.chars().any(char::is_control) {
            continue;
        }
        let upstream = text(upstream).map(|u| {
            u.strip_prefix("refs/remotes/").or_else(|| u.strip_prefix("refs/heads/")).unwrap_or(&u).to_owned()
        });
        let (ahead, behind) = track_counts(&String::from_utf8_lossy(track));
        let secs = std::str::from_utf8(date).ok().and_then(|d| d.trim().parse::<u64>().ok()).unwrap_or(0);
        let last_ms = secs.saturating_mul(1000);
        rows.push(RefRow {
            name: short_name,
            remote,
            short,
            upstream: upstream.filter(|u| u.len() <= 255),
            ahead,
            behind,
            last_ms,
            head: *head == b"*",
            worktree: (!worktree.is_empty())
                .then(|| PathBuf::from(OsStr::from_bytes(worktree)))
                .filter(|p| p.is_absolute()),
        });
    }
    rows
}

/// `%(upstream:track,nobracket)`: `ahead 1, behind 2`, `ahead 1`,
/// `behind 2`, `gone` or nothing. Counts are known only with an upstream
/// that is still there.
fn track_counts(track: &str) -> (Option<u32>, Option<u32>) {
    if track.trim() == "gone" {
        return (None, None);
    }
    let mut ahead = 0;
    let mut behind = 0;
    for part in track.split(',') {
        let mut words = part.split_whitespace();
        match (words.next(), words.next().and_then(|n| n.parse::<u32>().ok())) {
            (Some("ahead"), Some(n)) => ahead = n,
            (Some("behind"), Some(n)) => behind = n,
            _ => {}
        }
    }
    (Some(ahead), Some(behind))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs_output() -> Vec<u8> {
        let mut out = Vec::new();
        for record in [
            ["refs/heads/main", "refs/remotes/origin/main", "ahead 1, behind 2", "1700000000", "*", "/r"],
            ["refs/heads/feat/x", "", "", "1600000000", " ", "/w/g7-x"],
            ["refs/remotes/origin/HEAD", "", "", "1700000000", " ", ""],
            ["refs/remotes/origin/main", "", "", "1700000000", " ", ""],
            ["refs/remotes/origin/only-there", "", "", "1500000000", " ", ""],
            ["refs/remotes/up/stream/x", "", "", "1400000000", " ", ""],
            ["refs/heads/gone-up", "refs/remotes/origin/old", "gone", "1300000000", " ", ""],
        ] {
            for field in record {
                out.extend_from_slice(field.as_bytes());
                out.push(0);
            }
            out.push(b'\n');
        }
        out
    }

    fn remotes() -> Vec<(String, String)> {
        vec![
            ("origin".into(), "https://example.test/o.git".into()),
            ("up/stream".into(), "/srv/u.git".into()),
        ]
    }

    #[test]
    fn refs_read_into_rows() {
        let rows = parse_refs(&refs_output(), &remotes());
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["main", "feat/x", "origin/main", "origin/only-there", "up/stream/x", "gone-up"]);
        assert_eq!((rows[0].ahead, rows[0].behind, rows[0].head), (Some(1), Some(2), true));
        assert_eq!(rows[0].upstream.as_deref(), Some("origin/main"));
        assert_eq!(rows[0].last_ms, 1_700_000_000_000);
        assert_eq!(rows[4].remote.as_deref(), Some("up/stream"));
        assert_eq!(rows[4].short, "x");
        assert_eq!((rows[5].ahead, rows[5].behind), (None, None));
        assert_eq!(rows[1].worktree.as_deref(), Some(Path::new("/w/g7-x")));
    }

    #[test]
    fn the_list_puts_locals_first_and_hides_remote_twins() {
        let rows = parse_refs(&refs_output(), &remotes());
        let listed = list(&rows, Path::new("/r"), None);
        let names: Vec<&str> = listed.branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["main", "feat/x", "gone-up", "origin/only-there", "up/stream/x"]);
        assert_eq!(listed.current.as_deref(), Some("main"));
        assert_eq!(listed.branches[1].worktree.as_deref(), Some("g7-x"));
        assert_eq!(listed.branches[0].worktree, None);
        let found = list(&rows, Path::new("/r"), Some("ONLY"));
        assert_eq!(found.branches.len(), 1);
    }

    #[test]
    fn new_names_follow_g7s_rules_first() {
        for good in ["feat/sync-retry", "g7/a.b_c", "x", &"a".repeat(100)] {
            assert!(new_name_ok(good), "{good}");
        }
        for bad in [
            "-x",
            "@",
            "a..b",
            "x.lock",
            "a b",
            "HEAD",
            &"a".repeat(101),
            "",
            "/a",
            ".a",
            "a/",
            "a.",
            "a//b",
            "a@{1",
            "é",
            "a:b",
            "a~1",
            "a^",
            "a?",
            "a*",
            "a[b",
            "a\\b",
        ] {
            assert!(!new_name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn a_worktree_branch_comes_from_the_title() {
        assert_eq!(worktree_branch_name("Fix the reconnect retry!", "abc123"), "g7/fix-the-reconnect-retry");
        assert_eq!(worktree_branch_name("  ✨ ", "abc123"), "g7/abc123");
        assert_eq!(worktree_branch_name(&"word ".repeat(20), "abc123").len(), 3 + 39);
        assert!(new_name_ok(&worktree_branch_name(&"x-".repeat(50), "abc123")));
    }
}
