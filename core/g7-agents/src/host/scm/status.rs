// SPDX-License-Identifier: MPL-2.0
//! The status of a thread's folder (§SC3.2): `Status` and `Numstat` read
//! from the folder, git's lock and in-progress files looked at directly,
//! the remotes, the default branch and the last commit. Everything git
//! prints is repository content: parsed from `-z` forms, bounded, never
//! trusted.

use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::UNIX_EPOCH;

use super::argv::Op;
use super::sha256::Sha256;
use super::{
    Change, Doing, GitFile, GitStatus, LastCommit, MAX_REMOTES, MAX_STATUS_FILES, Reason, Remote, RepoState,
    Scm, ScmError, Spot, clean, lossy, now_ms, remote_label, remote_of,
};
use crate::api::MAX_DIFF_PATH;

/// Git's in-progress markers, in the git directory.
const DOING: [(&str, Doing); 6] = [
    ("MERGE_HEAD", Doing::Merge),
    ("rebase-merge", Doing::Rebase),
    ("rebase-apply", Doing::Rebase),
    ("CHERRY_PICK_HEAD", Doing::CherryPick),
    ("REVERT_HEAD", Doing::Revert),
    ("BISECT_LOG", Doing::Bisect),
];

/// One entry of `Status`, with its paths as git wrote them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// From the top level, as git wrote it (an untracked folder ends in `/`).
    pub raw: Vec<u8>,
    /// A rename's or copy's old path, from the top level.
    pub from_raw: Option<Vec<u8>>,
    /// `1`, `2`, `u` or `?`.
    pub kind: u8,
    pub xy: [u8; 2],
    /// Relative to the thread's folder, for clients.
    pub file: GitFile,
    /// The record as git printed it, for the fingerprint.
    record: Vec<u8>,
}

impl Entry {
    /// The paths a commit of this entry names: both of a rename.
    pub fn paths(&self) -> impl Iterator<Item = &[u8]> {
        std::iter::once(trim_slash(&self.raw)).chain(self.from_raw.as_deref())
    }
}

/// `Status` parsed.
#[derive(Debug, Clone, Default)]
pub(crate) struct Porcelain {
    /// `None`: no commit yet.
    pub oid: Option<String>,
    /// `None`: detached.
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ab: Option<(u32, u32)>,
    headers: Vec<Vec<u8>>,
    pub entries: Vec<Entry>,
    /// Entries in the folder, kept or not.
    pub total: u32,
    pub more: bool,
    pub conflicts: u32,
}

impl Porcelain {
    /// The entry a client named by its path.
    pub fn find(&self, path: &str) -> Option<&Entry> {
        let mut found = self.entries.iter().filter(|e| e.file.path == path);
        let first = found.next()?;
        // Two raw paths can read alike once made UTF-8: such a name is no one's.
        found.next().is_none().then_some(first)
    }
}

impl Scm {
    /// The folder's status (§SC3.2). A folder that is no longer a
    /// repository (removed, say) reads as `repo: "none"`.
    pub async fn status(&self, spot: &Spot) -> Result<GitStatus, ScmError> {
        let parsed = match self.porcelain(spot, MAX_STATUS_FILES).await {
            Ok(parsed) => parsed,
            Err(e) if e.reason == Reason::NotRepo => return Ok(GitStatus::without(RepoState::None)),
            Err(e) => return Err(e),
        };
        let (numstat, remotes, last) = tokio::join!(
            async { if parsed.oid.is_some() { self.numstat(spot).await } else { Ok(HashMap::new()) } },
            self.remotes(spot),
            async { if parsed.oid.is_some() { self.last_commit(spot).await } else { None } },
        );
        let numstat = numstat?;
        let remotes = remotes?;

        let mut status = GitStatus { repo: RepoState::Ok, checked_ms: now_ms(), ..GitStatus::default() };
        status.unborn = parsed.oid.is_none();
        status.branch = parsed.head.clone();
        if parsed.head.is_none() {
            status.detached = parsed.oid.as_ref().map(|oid| oid.chars().take(7).collect());
        }
        status.upstream = parsed.upstream.clone();
        if let Some((ahead, behind)) = parsed.ab {
            status.ahead = Some(ahead);
            status.behind = Some(behind);
        }
        let upstream_ref = parsed.upstream.as_ref().map(|u| format!("refs/remotes/{u}"));
        let upstream_remote = upstream_ref.as_deref().and_then(|u| remote_of(u, &remotes));
        status.remote = super::pick_remote(&remotes, upstream_remote.as_deref(), None).ok();
        status.remotes = remotes
            .iter()
            .take(MAX_REMOTES)
            .map(|(name, url)| {
                let (label, local) = remote_label(url, self.home());
                Remote { name: name.clone(), label, local }
            })
            .collect();
        if let Some(remote) = &status.remote {
            status.default_branch = self.default_branch(spot, remote).await;
            if let Some(url) = remotes.iter().find(|(n, _)| n == remote).map(|(_, u)| u) {
                status.github = self.gh.github_of(url);
            }
        }
        status.on_default = status.branch.is_some()
            && status.default_branch.is_some()
            && status.branch == status.default_branch;
        status.doing = doing(spot.git_dir());
        status.locked = spot.git_dir().join("index.lock").symlink_metadata().is_ok();
        status.last = last;
        status.fetched_ms = fetched_ms(spot);
        status.more = parsed.more;
        status.total = parsed.total;
        status.conflicts = parsed.conflicts;

        let mut sha = Sha256::new();
        for header in &parsed.headers {
            sha.field(header);
        }
        let top = spot.top().to_owned();
        let mut files = Vec::with_capacity(parsed.entries.len());
        for entry in &parsed.entries {
            let mut file = entry.file.clone();
            if let Some(&(adds, dels, binary)) = numstat.get(&entry.raw) {
                file.binary = binary;
                if !binary {
                    file.adds = Some(adds);
                    file.dels = Some(dels);
                    status.adds = status.adds.saturating_add(adds);
                    status.dels = status.dels.saturating_add(dels);
                }
            }
            sha.field(&entry.record);
            sha.field(format!("{:?} {:?} {}", file.adds, file.dels, file.binary).as_bytes());
            sha.field(&stamp(&top, trim_slash(&entry.raw)));
            files.push(file);
        }
        status.files = files;
        status.fingerprint = sha.short();
        Ok(status)
    }

    /// `Status` from the thread's folder, at most `keep` entries kept.
    pub(crate) async fn porcelain(&self, spot: &Spot, keep: usize) -> Result<Porcelain, ScmError> {
        self.porcelain_in(spot, spot.folder(), keep).await
    }

    /// `Status` from `cwd` (the folder, or the top level for the whole work tree).
    pub(crate) async fn porcelain_in(
        &self,
        spot: &Spot,
        cwd: &Path,
        keep: usize,
    ) -> Result<Porcelain, ScmError> {
        let out = self.read(&Op::Status, cwd, None).await?;
        if out.code != Some(0) {
            if cwd.symlink_metadata().is_err() || out.code == Some(128) {
                return Err(ScmError::new(Reason::NotRepo));
            }
            return Err(ScmError::new(Reason::Failed));
        }
        let prefix: &[u8] = if cwd == spot.top() { b"" } else { spot.prefix() };
        Ok(parse_porcelain(&out.stdout, prefix, keep, out.over))
    }

    /// `Numstat`: path from the top level → (adds, dels, binary).
    async fn numstat(&self, spot: &Spot) -> Result<HashMap<Vec<u8>, (u32, u32, bool)>, ScmError> {
        let out = self.read(&Op::Numstat, spot.folder(), None).await?;
        if out.code != Some(0) && !out.over {
            return Err(ScmError::new(Reason::Failed));
        }
        Ok(parse_numstat(&out.stdout))
    }

    async fn last_commit(&self, spot: &Spot) -> Option<LastCommit> {
        let out = self.read(&Op::LastCommit, spot.folder(), None).await.ok()?;
        if out.code != Some(0) {
            return None;
        }
        let record = out.stdout.split(|&b| b == 0).next()?;
        let text = String::from_utf8_lossy(record);
        let mut parts = text.splitn(3, '\u{1f}');
        let (oid, time, subject) = (parts.next()?, parts.next()?, parts.next().unwrap_or(""));
        if !crate::host::git::oid_ok(oid) {
            return None;
        }
        Some(LastCommit {
            oid7: oid[..7].to_owned(),
            subject: clean::text(subject, 200),
            ms: time.trim().parse::<u64>().ok()?.saturating_mul(1000),
        })
    }

    /// The remote's default branch, as the last fetch recorded it.
    async fn default_branch(&self, spot: &Spot, remote: &str) -> Option<String> {
        let op = Op::RemoteHead { remote: remote.to_owned() };
        let out = self.read(&op, spot.folder(), None).await.ok()?;
        if out.code != Some(0) {
            return None;
        }
        let line = String::from_utf8_lossy(out.line()).into_owned();
        let name = line.strip_prefix(&format!("refs/remotes/{remote}/"))?;
        (!name.is_empty() && name.len() <= 255).then(|| name.to_owned())
    }
}

/// What git is in the middle of, from its marker files.
pub(crate) fn doing(git_dir: &Path) -> Option<Doing> {
    DOING.iter().find(|(name, _)| git_dir.join(name).symlink_metadata().is_ok()).map(|&(_, d)| d)
}

fn fetched_ms(spot: &Spot) -> Option<u64> {
    [spot.git_dir(), spot.common_dir()].iter().find_map(|dir| {
        let modified = dir.join("FETCH_HEAD").symlink_metadata().ok()?.modified().ok()?;
        let ms = modified.duration_since(UNIX_EPOCH).ok()?.as_millis();
        u64::try_from(ms).ok()
    })
}

/// A file's size, modification time and change time, so an edit that
/// leaves git's line as it was still changes the fingerprint.
fn stamp(top: &Path, raw: &[u8]) -> Vec<u8> {
    let path = top.join(std::ffi::OsStr::from_bytes(raw));
    match path.symlink_metadata() {
        Ok(m) => format!("{} {} {} {} {}", m.len(), m.mtime(), m.mtime_nsec(), m.ctime_nsec(), m.ino())
            .into_bytes(),
        Err(_) => b"gone".to_vec(),
    }
}

pub(crate) fn trim_slash(raw: &[u8]) -> &[u8] {
    raw.strip_suffix(b"/").unwrap_or(raw)
}

/// A path from the top level as one relative to the folder (`prefix`):
/// `None` unless inside it, with sound parts and at most 512 bytes. An
/// untracked folder keeps its `/`.
pub(crate) fn rel_path(raw: &[u8], prefix: &[u8]) -> Option<String> {
    let folder = raw.ends_with(b"/");
    let rest = trim_slash(raw).strip_prefix(prefix)?;
    if rest.is_empty() || !crate::host::git::path_parts_ok(rest) {
        return None;
    }
    let mut text = lossy(rest);
    if folder {
        text.push('/');
    }
    (text.len() <= MAX_DIFF_PATH).then_some(text)
}

/// `status --porcelain=2 --branch -z` read into a [`Porcelain`]; entries
/// outside the folder are skipped, entries past `keep` counted.
pub(crate) fn parse_porcelain(bytes: &[u8], prefix: &[u8], keep: usize, cut: bool) -> Porcelain {
    let mut parsed = Porcelain { more: cut, ..Porcelain::default() };
    let mut records = bytes.split(|&b| b == 0).peekable();
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        if let Some(header) = record.strip_prefix(b"# ") {
            read_header(&mut parsed, header);
            parsed.headers.push(record.to_vec());
            continue;
        }
        let (kind, rest) = (record[0], &record[1..]);
        let rest = rest.strip_prefix(b" ").unwrap_or(rest);
        let (fields, from) = match kind {
            b'1' => (7, None),
            b'2' => (8, records.next()),
            b'u' => (9, None),
            b'?' => (0, None),
            _ => continue,
        };
        let mut parts = rest.splitn(fields + 1, |&b| b == b' ');
        let head: Vec<&[u8]> = parts.by_ref().take(fields).collect();
        let Some(raw) = parts.next() else { continue };
        if head.len() != fields {
            continue;
        }
        let xy = match head.first() {
            Some(f) if f.len() == 2 => [f[0], f[1]],
            _ if kind == b'?' => *b"??",
            _ => continue,
        };
        let Some(path) = rel_path(raw, prefix) else { continue };
        if kind == b'u' {
            parsed.conflicts += 1;
        }
        parsed.total = parsed.total.saturating_add(1);
        if parsed.entries.len() >= keep {
            parsed.more = true;
            continue;
        }
        let from_text = from.and_then(|f| rel_path(f, prefix).or_else(|| Some(lossy(f))));
        let file = GitFile {
            folder: kind == b'?' && raw.ends_with(b"/"),
            path,
            from: from_text.filter(|f| f.len() <= MAX_DIFF_PATH),
            status: change(kind, xy),
            staged: staged(kind, xy),
            adds: None,
            dels: None,
            binary: false,
        };
        let mut record = record.to_vec();
        if let Some(from) = from {
            record.push(0);
            record.extend_from_slice(from);
        }
        parsed.entries.push(Entry {
            raw: raw.to_vec(),
            from_raw: from.filter(|f| crate::host::git::path_parts_ok(f)).map(<[u8]>::to_vec),
            kind,
            xy,
            file,
            record,
        });
    }
    parsed
}

fn read_header(parsed: &mut Porcelain, header: &[u8]) {
    let text = String::from_utf8_lossy(header);
    let Some((key, value)) = text.split_once(' ') else { return };
    match key {
        "branch.oid" => {
            parsed.oid = crate::host::git::oid_ok(value).then(|| value.to_owned());
        }
        "branch.head" => {
            let ok = value != "(detached)" && !value.is_empty() && value.len() <= 255;
            parsed.head = ok.then(|| clean::text(value, 255));
        }
        "branch.upstream" => {
            parsed.upstream = (!value.is_empty() && value.len() <= 255).then(|| clean::text(value, 255));
        }
        "branch.ab" => {
            let mut parts = value.split(' ');
            let ahead = parts.next().and_then(|a| a.strip_prefix('+')).and_then(|a| a.parse().ok());
            let behind = parts.next().and_then(|b| b.strip_prefix('-')).and_then(|b| b.parse().ok());
            if let (Some(a), Some(b)) = (ahead, behind) {
                parsed.ab = Some((a, b));
            }
        }
        _ => {}
    }
}

fn change(kind: u8, [x, y]: [u8; 2]) -> Change {
    match kind {
        b'?' => Change::Untracked,
        b'u' => Change::Conflict,
        b'2' if x == b'C' => Change::Added,
        b'2' => Change::Renamed,
        _ if x == b'A' => Change::Added,
        _ if x == b'D' || y == b'D' => Change::Deleted,
        _ if x == b'T' || y == b'T' => Change::Type,
        _ => Change::Modified,
    }
}

fn staged(kind: u8, [x, y]: [u8; 2]) -> super::Staged {
    use super::Staged;
    match kind {
        b'?' | b'u' => Staged::None,
        _ if x == b'.' => Staged::None,
        _ if y == b'.' => Staged::All,
        _ => Staged::Part,
    }
}

/// `diff-index --numstat -z`: path (the new one of a rename) → counts;
/// `-` counts are a binary file.
pub(crate) fn parse_numstat(bytes: &[u8]) -> HashMap<Vec<u8>, (u32, u32, bool)> {
    let mut out = HashMap::new();
    let mut records = bytes.split(|&b| b == 0);
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        let mut parts = record.splitn(3, |&b| b == b'\t');
        let (Some(adds), Some(dels), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let path = if path.is_empty() {
            let _from = records.next();
            match records.next() {
                Some(to) => to,
                None => break,
            }
        } else {
            path
        };
        let binary = adds == b"-" && dels == b"-";
        let num = |n: &[u8]| std::str::from_utf8(n).ok().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
        out.insert(path.to_vec(), (num(adds), num(dels), binary));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const OID: &str = "fa5022f180d0ac827755f93c9bd8dff3c408279b";

    fn sample() -> Vec<u8> {
        let mut s = Vec::new();
        for record in [
            format!("# branch.oid {OID}"),
            "# branch.head main".to_owned(),
            "# branch.upstream origin/main".to_owned(),
            "# branch.ab +2 -1".to_owned(),
            format!("1 A. N... 000000 100644 100644 {OID} {OID} sub/new file.txt"),
            format!("1 .M N... 100644 100644 100644 {OID} {OID} sub/x"),
            format!("2 R. N... 100644 100644 100644 {OID} {OID} R100 sub/b2.txt"),
            "sub/b.txt".to_owned(),
            format!("u UU N... 100644 100644 100644 100644 {OID} {OID} {OID} sub/c.txt"),
            "? sub/docs/".to_owned(),
            "? other/y".to_owned(),
            format!("1 .D N... 100644 100644 000000 {OID} {OID} sub/gone"),
            format!("1 .T N... 100644 100644 120000 {OID} {OID} sub/link"),
        ] {
            s.extend_from_slice(record.as_bytes());
            s.push(0);
        }
        s
    }

    #[test]
    fn porcelain_entries_and_headers() {
        let p = parse_porcelain(&sample(), b"sub/", 100, false);
        assert_eq!(p.oid.as_deref(), Some(OID));
        assert_eq!(p.head.as_deref(), Some("main"));
        assert_eq!(p.upstream.as_deref(), Some("origin/main"));
        assert_eq!(p.ab, Some((2, 1)));
        use super::super::Staged;
        let files: Vec<(&str, Change, Staged, bool)> = p
            .entries
            .iter()
            .map(|e| (e.file.path.as_str(), e.file.status, e.file.staged, e.file.folder))
            .collect();
        assert_eq!(
            files,
            [
                ("new file.txt", Change::Added, Staged::All, false),
                ("x", Change::Modified, Staged::None, false),
                ("b2.txt", Change::Renamed, Staged::All, false),
                ("c.txt", Change::Conflict, Staged::None, false),
                ("docs/", Change::Untracked, Staged::None, true),
                ("gone", Change::Deleted, Staged::None, false),
                ("link", Change::Type, Staged::None, false),
            ]
        );
        let renamed = &p.entries[2];
        assert_eq!(renamed.file.from.as_deref(), Some("b.txt"));
        assert_eq!(renamed.paths().collect::<Vec<_>>(), [&b"sub/b2.txt"[..], b"sub/b.txt"]);
        assert_eq!(p.conflicts, 1);
        assert_eq!((p.total, p.more), (7, false));
        assert_eq!(p.entries[4].paths().next(), Some(&b"sub/docs"[..]));
    }

    #[test]
    fn unborn_detached_and_no_upstream() {
        let raw = b"# branch.oid (initial)\0# branch.head main\0";
        let p = parse_porcelain(raw, b"", 10, false);
        assert_eq!((p.oid, p.head.as_deref(), p.upstream, p.ab), (None, Some("main"), None, None));
        let raw = format!("# branch.oid {OID}\0# branch.head (detached)\0");
        let p = parse_porcelain(raw.as_bytes(), b"", 10, false);
        assert_eq!((p.oid.as_deref(), p.head), (Some(OID), None));
    }

    #[test]
    fn past_the_cap_entries_are_counted_not_kept() {
        let p = parse_porcelain(&sample(), b"sub/", 3, false);
        assert_eq!(p.entries.len(), 3);
        assert_eq!((p.total, p.more), (7, true));
    }

    #[test]
    fn odd_paths_are_skipped_and_odd_bytes_kept_raw() {
        let mut raw = Vec::new();
        for record in [&b"? ../x"[..], b"? a/./b", b"? ok\nline", b"? q\"uote", &[b'?', b' ', b'n', 0xff][..]]
        {
            raw.extend_from_slice(record);
            raw.push(0);
        }
        let p = parse_porcelain(&raw, b"", 10, false);
        let paths: Vec<&str> = p.entries.iter().map(|e| e.file.path.as_str()).collect();
        assert_eq!(paths, ["ok\nline", "q\"uote", "n\u{fffd}"]);
        assert_eq!(p.entries[2].raw, [b'n', 0xff]);
    }

    #[test]
    fn numstat_reads_renames_and_binaries() {
        let raw = b"1\t2\ta.txt\0-\t-\tbin.png\0\x33\t0\t\0old.txt\0new.txt\0";
        let n = parse_numstat(raw);
        assert_eq!(n[&b"a.txt"[..]], (1, 2, false));
        assert_eq!(n[&b"bin.png"[..]], (0, 0, true));
        assert_eq!(n[&b"new.txt"[..]], (3, 0, false));
        assert!(!n.contains_key(&b"old.txt"[..]));
    }
}
