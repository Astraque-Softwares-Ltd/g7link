// SPDX-License-Identifier: MPL-2.0
//! `@` file search (§Q4.6): a list of each folder's files and directories,
//! built with `ignore` on the blocking pool, and ranked by our own code.
//!
//! The list holds names only: nothing is opened or read. It honours
//! `.gitignore` (inside a git repository) and `.ignore`, leaves out hidden
//! entries except `.github`, never follows a link and never leaves the folder.
//! Every hit is checked against the folder again before it is returned.

use std::collections::HashMap;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::api::{
    DEFAULT_FILES_LIMIT, ErrorCode, FileHit, FilesSearchResult, MAX_FILES_LIMIT, MAX_FILES_QUERY, RpcError,
    is_stripped_char,
};

/// Entries one folder's list holds at most; past it the list is partial.
pub const MAX_ENTRIES: usize = 25_000;
/// How long one build may walk; past it the list is partial.
pub const BUILD_TIME: Duration = Duration::from_secs(2);
/// A list older than this is rebuilt when it is queried.
pub const MAX_AGE: Duration = Duration::from_secs(60);
/// A list nobody queried for this long is dropped.
pub const IDLE_DROP: Duration = Duration::from_secs(10 * 60);
/// Folders listed at once; the least recently used goes first.
pub const MAX_FOLDERS: usize = 8;
/// A relative path longer than this is left out of the list.
pub const MAX_HIT_PATH: usize = 512;

/// What a file list may cost; [`Default`] is the contract's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexLimits {
    pub max_entries: usize,
    pub build_time: Duration,
    pub max_age: Duration,
    pub idle_drop: Duration,
    pub max_folders: usize,
}

impl Default for IndexLimits {
    fn default() -> Self {
        Self {
            max_entries: MAX_ENTRIES,
            build_time: BUILD_TIME,
            max_age: MAX_AGE,
            idle_drop: IDLE_DROP,
            max_folders: MAX_FOLDERS,
        }
    }
}

#[derive(Debug)]
struct Entry {
    path: Box<str>,
    /// [`lower`] of `path`, when it differs.
    lower: Option<Box<str>>,
    dir: bool,
    /// Seconds since the epoch; 0 when unknown.
    modified: u64,
    /// `/`s in `path`.
    depth: u16,
}

impl Entry {
    fn lower(&self) -> &str {
        self.lower.as_deref().unwrap_or(&self.path)
    }
}

/// One folder's list, as built.
#[derive(Debug)]
pub struct Index {
    root: PathBuf,
    entries: Vec<Entry>,
    partial: bool,
    built: Instant,
}

impl Index {
    /// The folder, canonical.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A limit stopped the walk before it had seen everything.
    pub fn partial(&self) -> bool {
        self.partial
    }

    /// Whether the list holds `path` (relative, `/` separated).
    pub fn contains(&self, path: &str) -> bool {
        self.entries.iter().any(|e| &*e.path == path)
    }
}

/// A character in lower case when that is one character, else as it is, so
/// a path and its lower-case form line up character by character.
fn lower_char(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

fn lower(s: &str) -> String {
    s.chars().map(lower_char).collect()
}

/// Hidden entries are left out, except `.github`.
fn listed_name(name: &std::ffi::OsStr) -> bool {
    let bytes = name.as_encoded_bytes();
    bytes.first() != Some(&b'.') || bytes == b".github"
}

/// `rel` as `/`-separated text, when every component is a plain name that is
/// valid text without control or direction characters, and it is not too long.
fn relative_text(rel: &Path) -> Option<String> {
    let mut out = String::new();
    for component in rel.components() {
        let Component::Normal(name) = component else { return None };
        let name = name.to_str()?;
        if name.is_empty() || name.chars().any(|c| c.is_control() || is_stripped_char(c)) {
            return None;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(name);
    }
    (!out.is_empty() && out.len() <= MAX_HIT_PATH).then_some(out)
}

/// Whether `rel` is a relative path of plain names: no `..`, `.`, empty or
/// absolute component.
pub fn is_safe_relative(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.starts_with('/')
        && !rel.contains('\0')
        && rel.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

/// `rel` checked against the folder right now: plain names, still there,
/// not a link, of the kind the list said, and inside `root` once resolved.
fn still_inside(root: &Path, rel: &str, dir: bool) -> bool {
    if !is_safe_relative(rel) {
        return false;
    }
    let path = root.join(rel);
    let Ok(meta) = std::fs::symlink_metadata(&path) else { return false };
    if meta.file_type().is_symlink() || meta.is_dir() != dir {
        return false;
    }
    std::fs::canonicalize(&path).is_ok_and(|real| real.starts_with(root) && real != root)
}

/// Walks `folder` into a list (blocking: run it on the blocking pool).
/// `stop` ends the walk early, leaving the list partial.
pub fn build_index(folder: &Path, limits: &IndexLimits, stop: &AtomicBool) -> io::Result<Index> {
    let root = std::fs::canonicalize(folder)?;
    if !std::fs::metadata(&root)?.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotADirectory, "not a folder"));
    }
    let mut builder = ignore::WalkBuilder::new(&root);
    builder
        .standard_filters(true)
        .hidden(false)
        .git_global(false)
        .require_git(true)
        .follow_links(false)
        .same_file_system(true)
        .filter_entry(|entry| entry.depth() == 0 || listed_name(entry.file_name()));
    let started = Instant::now();
    let mut entries = Vec::new();
    let mut partial = false;
    for found in builder.build() {
        if stop.load(Ordering::Relaxed)
            || entries.len() >= limits.max_entries
            || started.elapsed() >= limits.build_time
        {
            partial = true;
            break;
        }
        let Ok(entry) = found else { continue };
        if entry.depth() == 0 {
            continue;
        }
        let Some(kind) = entry.file_type() else { continue };
        if !kind.is_dir() && !kind.is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(&root) else { continue };
        let Some(path) = relative_text(rel) else { continue };
        let modified = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        let lowered = lower(&path);
        let depth = u16::try_from(path.bytes().filter(|&b| b == b'/').count()).unwrap_or(u16::MAX);
        entries.push(Entry {
            lower: (lowered != path).then(|| lowered.into_boxed_str()),
            path: path.into_boxed_str(),
            dir: kind.is_dir(),
            modified,
            depth,
        });
    }
    Ok(Index { root, entries, partial, built: Instant::now() })
}

// ---- Ranking ----

const RUN_START_COMPONENT: i32 = 8;
const RUN_START_WORD: i32 = 5;
const ALL_IN_NAME: i32 = 12;
const EXACT_NAME: i32 = 40;
const EXACT_STEM: i32 = 25;
const PER_DEPTH: i32 = 2;
const MAX_GAP_PENALTY: i32 = 15;

/// Where the needle's characters sit in `hay`: the shortest window ending at
/// the earliest point where the whole needle has been seen, matched from its
/// end so runs stay together.
fn match_positions(hay: &[char], needle: &[char], out: &mut Vec<usize>) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut qi = 0;
    let mut end = None;
    for (i, &c) in hay.iter().enumerate() {
        if c == needle[qi] {
            qi += 1;
            if qi == needle.len() {
                end = Some(i);
                break;
            }
        }
    }
    let Some(end) = end else { return false };
    out.clear();
    out.resize(needle.len(), 0);
    let mut qi = needle.len();
    for i in (0..=end).rev() {
        if hay[i] == needle[qi - 1] {
            qi -= 1;
            out[qi] = i;
            if qi == 0 {
                break;
            }
        }
    }
    qi == 0
}

fn word_boundary(prev: char, cur: char) -> bool {
    !prev.is_alphanumeric() || (prev.is_lowercase() && cur.is_uppercase())
}

/// The score of a match at `positions` (offsets into the whole path).
fn score_positions(lower: &[char], orig: &[char], positions: &[usize], name_from: usize) -> i32 {
    let mut score = 0i32;
    let mut run = 0i32;
    for (k, &p) in positions.iter().enumerate() {
        if k > 0 && positions[k - 1] + 1 == p {
            run += 1;
        } else {
            run = 1;
            if p == 0 || lower[p - 1] == '/' {
                score += RUN_START_COMPONENT;
            } else if word_boundary(orig[p - 1], orig[p]) {
                score += RUN_START_WORD;
            }
        }
        score += run;
    }
    if let (Some(&first), Some(&last)) = (positions.first(), positions.last()) {
        let gaps = i32::try_from(last - first + 1 - positions.len()).unwrap_or(i32::MAX);
        score -= gaps.min(MAX_GAP_PENALTY);
        if first >= name_from {
            score += ALL_IN_NAME;
        }
    }
    score
}

/// Scratch space reused across candidates.
#[derive(Default)]
struct Scratch {
    lower: Vec<char>,
    orig: Vec<char>,
    positions: Vec<usize>,
}

/// Whether `needle` is a subsequence of `hay` (a cheap first test).
fn is_subsequence(hay: &str, needle: &[char]) -> bool {
    let mut want = needle.iter();
    let mut next = want.next();
    for c in hay.chars() {
        match next {
            Some(&n) if n == c => next = want.next(),
            Some(_) => {}
            None => break,
        }
    }
    next.is_none()
}

/// The best score of `needle` in `entry`, or `None` when it doesn't match.
/// A needle with `/` matches across components; one without must fit in a
/// single component.
fn score_entry(entry: &Entry, needle: &[char], slash: bool, scratch: &mut Scratch) -> Option<i32> {
    let lower = entry.lower();
    if !is_subsequence(lower, needle) {
        return None;
    }
    scratch.lower.clear();
    scratch.lower.extend(lower.chars());
    scratch.orig.clear();
    scratch.orig.extend(entry.path.chars());
    let hay = &scratch.lower;
    let name_from = hay.iter().rposition(|&c| c == '/').map_or(0, |i| i + 1);
    let mut best: Option<i32> = None;
    if slash {
        if match_positions(hay, needle, &mut scratch.positions) {
            best = Some(score_positions(hay, &scratch.orig, &scratch.positions, name_from));
        }
    } else {
        let mut start = 0;
        while start <= hay.len() {
            let end = hay[start..].iter().position(|&c| c == '/').map_or(hay.len(), |i| start + i);
            if match_positions(&hay[start..end], needle, &mut scratch.positions) {
                for p in &mut scratch.positions {
                    *p += start;
                }
                let s = score_positions(hay, &scratch.orig, &scratch.positions, name_from);
                best = Some(best.map_or(s, |b| b.max(s)));
            }
            start = end + 1;
        }
    }
    let mut score = best?;
    let name = &hay[name_from..];
    if name == needle {
        score += EXACT_NAME;
    } else if let Some(dot) = name.iter().rposition(|&c| c == '.')
        && dot > 0
        && &name[..dot] == needle
    {
        score += EXACT_STEM;
    }
    Some(score - PER_DEPTH * i32::from(entry.depth))
}

/// The query as the matcher sees it: trimmed, in lower case, at most
/// [`MAX_FILES_QUERY`] characters, without a leading `./`.
fn needle(query: &str) -> Vec<char> {
    let query = query.trim();
    let query = query.strip_prefix("./").unwrap_or(query);
    query.chars().filter(|c| !is_stripped_char(*c)).take(MAX_FILES_QUERY).map(lower_char).collect()
}

/// The number of hits to return for a request's `limit`.
fn hits_wanted(limit: Option<u32>) -> usize {
    let limit = limit.unwrap_or(DEFAULT_FILES_LIMIT).clamp(1, MAX_FILES_LIMIT);
    usize::try_from(limit).unwrap_or(1)
}

/// Ranks `index` for `query` (blocking: it checks each hit on disk). An empty
/// query lists the folder's top level, then its most recently changed files.
/// `limit` as `files.search` takes it: absent is [`DEFAULT_FILES_LIMIT`].
pub fn rank(index: &Index, query: &str, limit: Option<u32>) -> FilesSearchResult {
    let limit = hits_wanted(limit);
    let needle = needle(query);
    let order: Vec<&Entry> =
        if needle.is_empty() { browse_order(index, limit) } else { match_order(index, &needle) };
    let mut files = Vec::with_capacity(limit.min(order.len()));
    let mut more = index.partial;
    // A hit that vanished or moved since the walk is skipped; looking at a few
    // more candidates than asked keeps the answer full without unbounded stats.
    let mut checks_left = limit.saturating_mul(3);
    for entry in &order {
        if files.len() == limit {
            more = true;
            break;
        }
        if checks_left == 0 {
            more = true;
            break;
        }
        checks_left -= 1;
        if still_inside(&index.root, &entry.path, entry.dir) {
            files.push(FileHit { path: entry.path.to_string(), dir: entry.dir });
        }
    }
    FilesSearchResult { files, more }
}

fn match_order<'a>(index: &'a Index, needle: &[char]) -> Vec<&'a Entry> {
    let slash = needle.contains(&'/');
    let mut scratch = Scratch::default();
    let mut scored: Vec<(i32, &Entry)> = index
        .entries
        .iter()
        .filter_map(|e| score_entry(e, needle, slash, &mut scratch).map(|s| (s, e)))
        .collect();
    scored.sort_by(|(sa, a), (sb, b)| {
        sb.cmp(sa).then_with(|| a.path.len().cmp(&b.path.len())).then_with(|| a.path.cmp(&b.path))
    });
    scored.into_iter().map(|(_, e)| e).collect()
}

/// Top level first (directories, then files, by name), at most half the
/// answer; then the most recently changed files.
fn browse_order(index: &Index, limit: usize) -> Vec<&Entry> {
    let mut top: Vec<&Entry> = index.entries.iter().filter(|e| e.depth == 0).collect();
    top.sort_by(|a, b| {
        b.dir.cmp(&a.dir).then_with(|| a.lower().cmp(b.lower())).then_with(|| a.path.cmp(&b.path))
    });
    let shown_top = top.len().min((limit / 2).max(1));
    let mut recent: Vec<&Entry> = index.entries.iter().filter(|e| !e.dir && e.depth > 0).collect();
    recent.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)));
    let mut out: Vec<&Entry> = top[..shown_top].to_vec();
    out.extend(recent);
    out.extend_from_slice(&top[shown_top..]);
    out
}

// ---- The lists the host keeps ----

type Built = Result<Arc<Index>, io::ErrorKind>;

#[derive(Debug)]
struct Slot {
    index: Option<Arc<Index>>,
    /// Set while a build runs; it receives the build's outcome.
    building: Option<watch::Receiver<Option<Built>>>,
    /// A turn ended while a build ran: build again when it is done.
    again: bool,
    used: Instant,
}

#[derive(Debug)]
struct Shared {
    limits: IndexLimits,
    slots: Mutex<HashMap<PathBuf, Slot>>,
    stop: Arc<AtomicBool>,
    builds: Mutex<Vec<JoinHandle<()>>>,
}

impl Shared {
    fn slots(&self) -> MutexGuard<'_, HashMap<PathBuf, Slot>> {
        self.slots.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn track(&self, handle: JoinHandle<()>) {
        let mut builds = self.builds.lock().unwrap_or_else(|e| e.into_inner());
        builds.retain(|h| !h.is_finished());
        builds.push(handle);
    }
}

/// The host's file lists, one per folder in use. Builds run on the blocking
/// pool and are waited for by [`FileIndex::shutdown`]. Call its methods from
/// inside the runtime.
#[derive(Debug)]
pub struct FileIndex {
    shared: Arc<Shared>,
}

impl Default for FileIndex {
    fn default() -> Self {
        Self::new(IndexLimits::default())
    }
}

/// Starts a build of `folder`'s list and marks `slot` as building. The
/// caller tracks the returned task once it has let go of the slots.
fn start_build(
    shared: &Arc<Shared>,
    folder: &Path,
    slot: &mut Slot,
) -> (watch::Receiver<Option<Built>>, JoinHandle<()>) {
    let (tx, rx) = watch::channel(None);
    slot.building = Some(rx.clone());
    slot.again = false;
    let task_shared = shared.clone();
    let folder = folder.to_owned();
    let handle = tokio::spawn(async move {
        let limits = task_shared.limits;
        let stop = task_shared.stop.clone();
        let walk_folder = folder.clone();
        let built: Built =
            match tokio::task::spawn_blocking(move || build_index(&walk_folder, &limits, &stop)).await {
                Ok(Ok(index)) => Ok(Arc::new(index)),
                Ok(Err(e)) => Err(e.kind()),
                Err(_) => Err(io::ErrorKind::Other),
            };
        let next = {
            let mut slots = task_shared.slots();
            match slots.get_mut(&folder) {
                Some(slot) => {
                    slot.building = None;
                    if let Ok(index) = &built {
                        slot.index = Some(index.clone());
                    }
                    let again = std::mem::take(&mut slot.again);
                    (again && !task_shared.stop.load(Ordering::SeqCst))
                        .then(|| start_build(&task_shared, &folder, slot).1)
                }
                None => None,
            }
        };
        tx.send_replace(Some(built));
        if let Some(handle) = next {
            task_shared.track(handle);
        }
    });
    (rx, handle)
}

fn build_error(kind: io::ErrorKind) -> RpcError {
    match kind {
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
            RpcError::new(ErrorCode::NotFound, "That folder doesn't exist.")
        }
        io::ErrorKind::PermissionDenied => RpcError::new(ErrorCode::BadRequest, "That folder can't be read."),
        _ => RpcError::new(ErrorCode::Internal, "Couldn't list the folder."),
    }
}

impl FileIndex {
    pub fn new(limits: IndexLimits) -> Self {
        Self {
            shared: Arc::new(Shared {
                limits,
                slots: Mutex::new(HashMap::new()),
                stop: Arc::new(AtomicBool::new(false)),
                builds: Mutex::new(Vec::new()),
            }),
        }
    }

    /// `files.search` in `folder` (an absolute folder path the host lists).
    /// A list older than [`MAX_AGE`] answers this query and is rebuilt behind it.
    pub async fn search(
        &self,
        folder: &Path,
        query: &str,
        limit: Option<u32>,
    ) -> Result<FilesSearchResult, RpcError> {
        if self.shared.stop.load(Ordering::SeqCst) {
            return Err(RpcError::new(ErrorCode::Unavailable, "The host is stopping."));
        }
        if !folder.is_absolute() {
            return Err(RpcError::new(ErrorCode::BadRequest, "The folder must be absolute."));
        }
        let now = Instant::now();
        let mut started = None;
        let ready = {
            let mut slots = self.shared.slots();
            Self::drop_idle(&mut slots, now, self.shared.limits.idle_drop);
            if !slots.contains_key(folder) {
                while slots.len() >= self.shared.limits.max_folders.max(1) {
                    let Some(oldest) = slots.iter().min_by_key(|(_, s)| s.used).map(|(k, _)| k.clone())
                    else {
                        break;
                    };
                    slots.remove(&oldest);
                }
            }
            let slot = slots.entry(folder.to_owned()).or_insert_with(|| Slot {
                index: None,
                building: None,
                again: false,
                used: now,
            });
            slot.used = now;
            match (&slot.index, &slot.building) {
                (Some(index), building) => {
                    let index = index.clone();
                    if building.is_none() && index.built.elapsed() >= self.shared.limits.max_age {
                        let (_, handle) = start_build(&self.shared, folder, slot);
                        started = Some(handle);
                    }
                    Ok(index)
                }
                (None, Some(building)) => Err(building.clone()),
                (None, None) => {
                    let (rx, handle) = start_build(&self.shared, folder, slot);
                    started = Some(handle);
                    Err(rx)
                }
            }
        };
        if let Some(handle) = started {
            self.shared.track(handle);
        }
        let index = match ready {
            Ok(index) => index,
            Err(mut rx) => {
                let outcome = rx
                    .wait_for(Option::is_some)
                    .await
                    .map_err(|_| RpcError::new(ErrorCode::Internal, "Couldn't list the folder."))?
                    .clone();
                match outcome {
                    Some(Ok(index)) => index,
                    Some(Err(kind)) => return Err(build_error(kind)),
                    None => return Err(RpcError::new(ErrorCode::Internal, "Couldn't list the folder.")),
                }
            }
        };
        let query = query.to_owned();
        tokio::task::spawn_blocking(move || rank(&index, &query, limit))
            .await
            .map_err(|_| RpcError::new(ErrorCode::Internal, "The search failed."))
    }

    /// A turn in `folder` ended: its list, if it has one, is rebuilt.
    pub fn turn_ended(&self, folder: &Path) {
        if self.shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let handle = {
            let mut slots = self.shared.slots();
            let Some(slot) = slots.get_mut(folder) else { return };
            if slot.building.is_some() {
                slot.again = true;
                return;
            }
            if slot.index.is_none() {
                return;
            }
            start_build(&self.shared, folder, slot).1
        };
        self.shared.track(handle);
    }

    /// Drops lists nobody queried for [`IDLE_DROP`]. Cheap; the host calls it
    /// from its periodic check.
    pub fn sweep(&self) {
        let mut slots = self.shared.slots();
        Self::drop_idle(&mut slots, Instant::now(), self.shared.limits.idle_drop);
    }

    fn drop_idle(slots: &mut HashMap<PathBuf, Slot>, now: Instant, idle: Duration) {
        slots.retain(|_, s| s.building.is_some() || now.duration_since(s.used) < idle);
    }

    /// Folders with a list (or a build) right now.
    pub fn folders(&self) -> usize {
        self.shared.slots().len()
    }

    /// Stops every build and waits for them; later searches are refused.
    pub async fn shutdown(&self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        loop {
            let handles: Vec<JoinHandle<()>> =
                std::mem::take(&mut *self.shared.builds.lock().unwrap_or_else(|e| e.into_inner()));
            if handles.is_empty() {
                break;
            }
            for handle in handles {
                let _ = handle.await;
            }
        }
        self.shared.slots().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn entry(path: &str) -> Entry {
        let lowered = lower(path);
        Entry {
            lower: (lowered != path).then(|| lowered.into_boxed_str()),
            path: path.into(),
            dir: false,
            modified: 0,
            depth: u16::try_from(path.matches('/').count()).unwrap(),
        }
    }

    #[test]
    fn the_match_window_keeps_runs_together() {
        let mut out = Vec::new();
        assert!(match_positions(&chars("aab"), &chars("ab"), &mut out));
        assert_eq!(out, [1, 2]);
        assert!(!match_positions(&chars("ba"), &chars("ab"), &mut out));
        assert!(!match_positions(&chars(""), &chars("a"), &mut out));
    }

    #[test]
    fn without_a_slash_the_needle_fits_in_one_component() {
        let mut scratch = Scratch::default();
        let e = entry("src/main.rs");
        assert!(score_entry(&e, &chars("main"), false, &mut scratch).is_some());
        assert!(score_entry(&e, &chars("smr"), false, &mut scratch).is_none());
        assert!(score_entry(&e, &chars("s/m"), true, &mut scratch).is_some());
    }

    #[test]
    fn upper_case_paths_match_lower_case_queries() {
        let mut scratch = Scratch::default();
        let e = entry("Docs/ReadMe.MD");
        assert!(score_entry(&e, &needle("readme"), false, &mut scratch).is_some());
        assert!(score_entry(&e, &needle("README"), false, &mut scratch).is_some());
    }

    #[test]
    fn unsafe_relative_paths_are_refused() {
        for bad in ["", "/etc", "a//b", "../a", "a/..", "./a", "a/./b", "a/", "a\0b"] {
            assert!(!is_safe_relative(bad), "{bad:?}");
        }
        assert!(is_safe_relative("a/b.rs"));
        assert!(is_safe_relative(".github/workflows/ci.yml"));
    }

    #[test]
    fn a_name_with_control_characters_is_not_listed() {
        assert_eq!(relative_text(Path::new("a/b\nc")), None);
        assert_eq!(relative_text(Path::new("a/\u{202e}txt.exe")), None);
        assert_eq!(relative_text(Path::new("a/b")), Some("a/b".into()));
        assert_eq!(relative_text(Path::new(&"x".repeat(MAX_HIT_PATH + 1))), None);
    }
}
