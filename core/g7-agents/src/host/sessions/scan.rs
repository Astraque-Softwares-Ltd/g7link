// SPDX-License-Identifier: MPL-2.0
//! The import scan (§U4.1, reconcile §6 and §7): every Claude and Codex
//! session on this PC, compressed rollouts included, into the store's
//! `found` table. Separate from live following: no time window, its own
//! bounds, and nothing but file names, stamps and the first 64 KiB of a
//! file (decompressed) is read.
//!
//! A session is read again only when one of its files changed (device,
//! inode, size or time) or a file came or went. A Codex thread is one row
//! however many rollout files hold it: its parts are grouped by the
//! thread's UUID, each checked against its own `session_meta.payload.id`.
//!
//! One scan at a time, on the blocking pool; rows are written in batches as
//! they are found and `sessions.scanned` tells the connections that asked.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::watch;

use super::claude_files::ClaudeTail;
use super::codex_files::{self, CodexMeta, CodexTail};
use super::import::{self, LineReader};
use super::paths::{self, RolloutName, Roots};
use super::procs::{self, Comm};
use super::tail::{self, Stamp};
use super::{FoundStore, MAX_META_LINE, codex_open, folder_of, last_component};
use crate::api::*;
use crate::host::Host;
use crate::host::db::{FoundFile, FoundQuery, FoundRow};
use crate::host::store::now_ms;

/// Transcripts per agent a scan looks at, newest first.
pub const MAX_PER_AGENT: usize = 20_000;
/// File heads (decompressed) one scan reads at most.
pub const MAX_META_BYTES: u64 = 64 << 20;
/// What a scan reads of a file: its first line and what follows, to here.
pub const HEAD_BYTES: u64 = 65_536;
/// `sessions.scan` doesn't start another scan this soon after one.
pub const RESCAN_AFTER: Duration = Duration::from_secs(30);
/// [`hourly`]: at most this often.
pub const SCAN_EVERY: Duration = Duration::from_secs(3600);
const MAX_PROJECT_FOLDERS: usize = 512;
const MAX_PROJECT_ENTRIES: usize = 4096;
const MAX_DAY_ENTRIES: usize = 512;
/// Year, month and day folders listed per level.
const MAX_DATE_ENTRIES: usize = 512;
/// Rows written per batch.
const BATCH: usize = 64;

/// How the import scan stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStatus {
    /// A scan is running.
    pub scanning: bool,
    /// The last scan stopped at a bound before it saw everything.
    pub partial: bool,
    /// Sessions on disk the running (or last) scan has seen.
    pub seen: u32,
}

/// The scan [`scan`] started or found running.
#[derive(Debug, Clone)]
pub struct ScanHandle {
    rx: watch::Receiver<ScanStatus>,
}

impl ScanHandle {
    pub fn status(&self) -> ScanStatus {
        *self.rx.borrow()
    }

    /// Waits until no scan is running.
    pub async fn finished(mut self) -> ScanStatus {
        let done = self.rx.wait_for(|s| !s.scanning).await.map(|s| *s);
        done.unwrap_or_else(|_| *self.rx.borrow())
    }
}

/// The scan's state in [`super::Sessions`].
#[derive(Debug)]
pub(crate) struct ScanState {
    status: watch::Sender<ScanStatus>,
    inner: Mutex<ScanInner>,
}

#[derive(Debug, Default)]
struct ScanInner {
    last_done: Option<Instant>,
    stop: Arc<AtomicBool>,
    /// Files read and not taken (another thread's id, no `session_meta`),
    /// so they aren't read again while unchanged.
    refused: HashMap<PathBuf, Stamp>,
}

impl Default for ScanState {
    fn default() -> Self {
        Self { status: watch::Sender::new(ScanStatus::default()), inner: Mutex::new(ScanInner::default()) }
    }
}

impl ScanState {
    fn lock(&self) -> MutexGuard<'_, ScanInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ends a running scan at its next file (shutdown).
    pub(crate) fn stop(&self) {
        self.lock().stop.store(true, Ordering::SeqCst);
    }
}

/// Starts the import scan unless one is running or one finished in the last
/// [`RESCAN_AFTER`]. `sessions.scan` calls it; it answers at once.
pub fn scan(host: &Arc<Host>) -> ScanHandle {
    start(host, RESCAN_AFTER)
}

/// Starts the import scan when none ran in the last hour: for the host to
/// call while a client is connected (§U4.1).
pub fn hourly(host: &Arc<Host>) -> ScanHandle {
    start(host, SCAN_EVERY)
}

fn start(host: &Arc<Host>, fresh_for: Duration) -> ScanHandle {
    let state = &host.sessions.found_scan;
    let handle = ScanHandle { rx: state.status.subscribe() };
    let Ok(store) = host.sessions.found_store() else { return handle };
    if !host.sessions.enabled {
        return handle;
    }
    let mut inner = state.lock();
    if state.status.borrow().scanning || inner.last_done.is_some_and(|at| at.elapsed() < fresh_for) {
        return handle;
    }
    let stop = Arc::new(AtomicBool::new(false));
    inner.stop = stop.clone();
    let job = ScanJob {
        roots: host.sessions.roots.clone(),
        folders: host.session_folders(),
        titles: None,
        refused: std::mem::take(&mut inner.refused),
        stop,
    };
    drop(inner);
    state.status.send_modify(|s| {
        s.scanning = true;
        s.seen = 0;
    });
    let weak = Arc::downgrade(host);
    tokio::spawn(async move {
        let events = weak.clone();
        let ran = tokio::task::spawn_blocking(move || {
            let mut last: Option<Instant> = None;
            let mut progress = |seen: u32| {
                let Some(host) = events.upgrade() else { return };
                host.sessions.found_scan.status.send_modify(|s| s.seen = seen);
                if last.is_none_or(|at| at.elapsed() >= Duration::from_millis(SCAN_EVENT_MS)) {
                    last = Some(Instant::now());
                    let partial = host.sessions.found_scan.status.borrow().partial;
                    host.hub.broadcast(&Event::SessionsScanned(SessionsScannedEvent {
                        total: seen,
                        scanning: true,
                        partial,
                    }));
                }
            };
            run(job, &*store, &mut progress)
        })
        .await;
        let Some(host) = weak.upgrade() else { return };
        let state = &host.sessions.found_scan;
        let report = match ran {
            Ok(Ok(report)) => Some(report),
            Ok(Err(e)) => {
                tracing::warn!("the session scan couldn't save what it found: {}", e.kind());
                None
            }
            Err(_) => None,
        };
        {
            let mut inner = state.lock();
            inner.last_done = Some(Instant::now());
            if let Some(report) = &report {
                inner.refused = report.refused.clone();
            }
        }
        state.status.send_modify(|s| {
            s.scanning = false;
            if let Some(report) = &report {
                s.partial = report.partial;
                s.seen = report.seen;
            }
        });
        let status = *state.status.borrow();
        host.hub.broadcast(&Event::SessionsScanned(SessionsScannedEvent {
            total: status.seen,
            scanning: false,
            partial: status.partial,
        }));
    });
    handle
}

/// `sessions.scan`: starts or continues the scan and answers at once from
/// the store (§U7.2).
pub async fn answer(host: &Arc<Host>, p: SessionsScanParams) -> Result<SessionsScanResult, RpcError> {
    if p.query.as_ref().is_some_and(|q| q.len() > MAX_SCAN_QUERY) {
        return Err(RpcError::new(ErrorCode::BadRequest, "That search is too long."));
    }
    let store = host.sessions.found_store()?;
    let status = scan(host).status();
    let q = FoundQuery {
        query: p.query,
        agent: p.agent,
        show: p.show.unwrap_or_default(),
        before_ms: p.before_ms,
        limit: p.limit.unwrap_or(DEFAULT_SCAN_LIMIT).clamp(1, MAX_SCAN_LIMIT),
    };
    let roots = host.sessions.roots.clone();
    let folders = host.session_folders();
    let (found, next_ms, total) = tokio::task::spawn_blocking(move || {
        let page = store.found(&q)?;
        let open = open_flags(&roots, &page.rows, now_ms());
        let found: Vec<Found> =
            page.rows.iter().zip(open).map(|(row, open)| found_of(row, &folders, open)).collect();
        Ok::<_, io::Error>((found, page.next_ms, page.total))
    })
    .await
    .map_err(|_| RpcError::new(ErrorCode::Internal, "The read failed."))?
    .map_err(|e| {
        tracing::warn!("the sessions found couldn't be read: {}", e.kind());
        RpcError::new(ErrorCode::Unavailable, "G7 couldn't read its thread store.")
    })?;
    Ok(SessionsScanResult { found, next_ms, total, scanning: status.scanning, partial: status.partial })
}

/// A row as a client sees it. The folder is looked up again, so adding an
/// agent folder frees its sessions at once.
pub fn found_of(row: &FoundRow, folders: &[(String, String, PathBuf)], open: bool) -> Found {
    let cwd = row.cwd.clone().unwrap_or_default();
    let folder = row.cwd.as_deref().and_then(|c| folder_of(c, folders).1);
    let title = Some(row.title.trim())
        .filter(|t| !t.is_empty())
        .map(|t| clean_cut(t, MAX_TITLE).0)
        .unwrap_or_else(|| row.agent.display_name().to_owned());
    Found {
        id: row.id.clone(),
        agent: row.agent,
        title,
        folder_label: folder.map_or_else(
            || if cwd.is_empty() { String::new() } else { last_component(&cwd) },
            |f| f.1.clone(),
        ),
        folder_id: folder.map(|f| f.0.clone()),
        cwd: cut(&cwd, MAX_PATH).to_owned(),
        first_ms: row.first_ms,
        updated_ms: row.updated_ms,
        bytes: row.bytes,
        packed: row.packed(),
        source: row.source.unwrap_or(SessionSource::Other),
        open,
        thread_id: row.thread_id.clone(),
        damaged: row.damaged,
    }
}

/// Which of `rows` something has open now, from one process scan.
pub fn open_flags(roots: &Roots, rows: &[FoundRow], now: u64) -> Vec<bool> {
    if rows.is_empty() {
        return Vec::new();
    }
    let scan = procs::scan(&roots.proc_root, crate::client::euid(), std::process::id());
    let statuses = procs::statuses(roots, &scan);
    let locks: Vec<String> = if rows.iter().any(|r| r.agent == AgentId::Codex) {
        tail::list_dir(&roots.codex_locks(), super::MAX_LOCKS).into_iter().map(|(name, _)| name).collect()
    } else {
        Vec::new()
    };
    rows.iter()
        .map(|row| match row.agent {
            AgentId::Claude => {
                statuses.get(&row.native_id).is_some_and(|s| s.live)
                    || scan
                        .procs
                        .values()
                        .any(|p| p.comm == Comm::Claude && p.names_claude_session(&row.native_id))
            }
            AgentId::Codex => {
                codex_open(&scan, &row.native_id, &locks, now, row.updated_ms, row.cwd.as_deref(), None)
            }
        })
        .collect()
}

// ---- One scan ----

/// What one scan needs, owned, for the blocking pool.
#[derive(Debug, Clone)]
pub struct ScanJob {
    pub roots: Roots,
    /// `(id, label, canonical path)` of the agent folders.
    pub folders: Vec<(String, String, PathBuf)>,
    /// Thread names from Codex's index; `None`: read them.
    pub titles: Option<HashMap<String, String>>,
    /// Files refused before and unchanged since.
    pub refused: HashMap<PathBuf, Stamp>,
    pub stop: Arc<AtomicBool>,
}

/// What a scan did.
#[derive(Debug, Clone, Default)]
pub struct ScanReport {
    /// Sessions on disk it saw.
    pub seen: u32,
    /// Rows written (new or changed).
    pub written: u32,
    /// Files dropped because they are gone.
    pub forgotten: u32,
    /// It stopped at a bound before it saw everything.
    pub partial: bool,
    /// Names in `archived_sessions` that aren't rollouts where one may be.
    pub ignored: u32,
    pub refused: HashMap<PathBuf, Stamp>,
}

/// A file a scan may take.
#[derive(Debug, Clone)]
struct Candidate {
    agent: AgentId,
    /// The session's (thread's) UUID.
    uuid: String,
    /// The `<ts>` of a rollout's name; empty for a transcript.
    stamp: String,
    path: PathBuf,
    meta: Stamp,
    packed: bool,
}

impl Candidate {
    fn file(&self) -> FoundFile {
        FoundFile {
            path: self.path.to_string_lossy().into_owned(),
            dev: self.meta.dev,
            ino: self.meta.ino,
            bytes: self.meta.size,
            mtime_ms: self.meta.mtime_ms(),
            stamp: self.stamp.clone(),
            packed: self.packed,
        }
    }
}

fn same(file: &FoundFile, c: &Candidate) -> bool {
    let f = c.file();
    file.path == f.path
        && file.dev == f.dev
        && file.ino == f.ino
        && file.bytes == f.bytes
        && file.mtime_ms == f.mtime_ms
}

/// Lists a folder; also whether it held more than `max` entries.
fn list(dir: &Path, max: usize) -> (Vec<(String, PathBuf)>, bool) {
    let entries = tail::list_dir(dir, max + 1);
    let more = entries.len() > max;
    (entries.into_iter().take(max).collect(), more)
}

fn is_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// Everything the walk found, by agent.
#[derive(Debug, Default)]
struct Walk {
    files: Vec<Candidate>,
    partial: bool,
    ignored: u32,
}

impl Walk {
    fn rollout(&mut self, name: &str, path: PathBuf) -> bool {
        let Some(RolloutName { stamp, thread, packed, .. }) = paths::rollout_name(name) else { return false };
        // A link or anything but a regular file is never opened.
        let Some(meta) = tail::stamp(&path) else { return true };
        self.files.push(Candidate {
            agent: AgentId::Codex,
            uuid: thread.to_owned(),
            stamp: stamp.to_owned(),
            path,
            meta,
            packed,
        });
        true
    }

    fn claude(&mut self, roots: &Roots) {
        let (folders, more) = list(&roots.claude_projects(), MAX_PROJECT_FOLDERS);
        self.partial |= more;
        for (_, folder) in folders {
            if !is_dir(&folder) {
                continue;
            }
            let (entries, more) = list(&folder, MAX_PROJECT_ENTRIES);
            self.partial |= more;
            for (name, path) in entries {
                let Some(uuid) = paths::transcript_uuid(&name) else { continue };
                let Some(meta) = tail::stamp(&path) else { continue };
                self.files.push(Candidate {
                    agent: AgentId::Claude,
                    uuid: uuid.to_owned(),
                    stamp: String::new(),
                    path,
                    meta,
                    packed: false,
                });
            }
        }
    }

    /// `YYYY/MM/DD/rollout-…` under `root`; names that are not dates or
    /// rollouts are counted in `ignored` when `count` is set.
    fn days(&mut self, root: &Path, count: bool) {
        let (years, more) = list(root, MAX_DATE_ENTRIES);
        self.partial |= more;
        for (year, year_path) in years {
            if !paths::is_date_folder(&year, 0) || !is_dir(&year_path) {
                self.ignored += u32::from(count);
                continue;
            }
            self.months(&year_path, count);
        }
    }

    fn months(&mut self, year: &Path, count: bool) {
        let (months, more) = list(year, MAX_DATE_ENTRIES);
        self.partial |= more;
        for (month, month_path) in months {
            if !paths::is_date_folder(&month, 1) || !is_dir(&month_path) {
                self.ignored += u32::from(count);
                continue;
            }
            let (days, more) = list(&month_path, MAX_DATE_ENTRIES);
            self.partial |= more;
            for (day, day_path) in days {
                if !paths::is_date_folder(&day, 2) || !is_dir(&day_path) {
                    self.ignored += u32::from(count);
                    continue;
                }
                let (entries, more) = list(&day_path, MAX_DAY_ENTRIES);
                self.partial |= more;
                for (name, path) in entries {
                    if !self.rollout(&name, path) {
                        self.ignored += u32::from(count);
                    }
                }
            }
        }
    }

    /// `archived_sessions`: rollouts directly in it, or under `YYYY/MM/DD`
    /// (its layout couldn't be seen; reconcile §7).
    fn archived(&mut self, root: &Path) {
        let (entries, more) = list(root, MAX_DAY_ENTRIES);
        self.partial |= more;
        for (name, path) in entries {
            if self.rollout(&name, path.clone()) {
                continue;
            }
            if paths::is_date_folder(&name, 0) && is_dir(&path) {
                self.months(&path, true);
            } else {
                self.ignored += 1;
            }
        }
    }
}

/// Codex's thread names, as live following reads them.
fn codex_titles(roots: &Roots) -> HashMap<String, String> {
    tail::read_window(&roots.codex_index(), None, 0)
        .map(|w| {
            let mut joined = Vec::new();
            for line in w.lines {
                joined.extend_from_slice(&line.bytes);
                joined.push(b'\n');
            }
            codex_files::index_titles(&joined)
        })
        .unwrap_or_default()
}

/// The first line of a file and the lines after it up to [`HEAD_BYTES`],
/// decompressed; also how many bytes that took.
pub(crate) fn head_lines(path: &Path, packed: bool) -> io::Result<(Vec<(u64, Value)>, u64)> {
    let mut lines = LineReader::new(import::open(path, packed)?, None, MAX_META_LINE);
    let mut out = Vec::new();
    let mut buf = Vec::new();
    while let Some(offset) = lines.next_line(&mut buf)? {
        if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
            out.push((offset, v));
        } else if offset == 0 {
            out.push((0, Value::Null));
        }
        if lines.pos() >= HEAD_BYTES {
            break;
        }
    }
    Ok((out, lines.pos()))
}

/// What the head of one rollout part said.
struct Part {
    candidate: Candidate,
    /// `None`: it couldn't be read.
    meta: Option<CodexMeta>,
    head: Vec<(u64, Value)>,
}

/// The facts a row needs from the lines at the start of a conversation:
/// the first prompt (the title, unless the agent named the session) and when.
fn first_prompt(
    agent: AgentId,
    head: &[(u64, Value)],
    mtime_ms: u64,
) -> (Option<String>, Option<u64>, ClaudeFacts) {
    let mut first_ms = None;
    match agent {
        AgentId::Claude => {
            let mut tail = ClaudeTail::new(None);
            for (offset, v) in head {
                for item in tail.feed_value(v, *offset, mtime_ms) {
                    if item.kind == ItemKind::User && first_ms.is_none() {
                        first_ms = Some(item.ts_ms);
                    }
                }
            }
            let facts = ClaudeFacts { cwd: tail.line_cwd.clone(), entrypoint: tail.entrypoint.clone() };
            (tail.title(), first_ms, facts)
        }
        AgentId::Codex => {
            let mut tail = CodexTail::new(None);
            for (offset, v) in head {
                for item in tail.feed_value(v, *offset, mtime_ms) {
                    if item.kind == ItemKind::User && first_ms.is_none() {
                        first_ms = Some(item.ts_ms);
                    }
                }
            }
            (tail.title(), first_ms, ClaudeFacts::default())
        }
    }
}

#[derive(Debug, Default)]
struct ClaudeFacts {
    cwd: Option<String>,
    entrypoint: Option<String>,
}

/// Where a Claude session was started, from its first prompt's `entrypoint`.
fn claude_source(entrypoint: Option<&str>) -> Option<SessionSource> {
    match entrypoint? {
        "cli" => Some(SessionSource::Terminal),
        "claude-desktop" => Some(SessionSource::DesktopApp),
        _ => Some(SessionSource::Other),
    }
}

fn title_of(title: Option<String>) -> String {
    title.map(|t| clean_cut(t.trim(), MAX_TITLE).0).unwrap_or_default()
}

/// Reads heads within the scan's budget.
struct Budget {
    left: u64,
    out: bool,
}

impl Budget {
    fn head(&mut self, path: &Path, packed: bool) -> Option<io::Result<Vec<(u64, Value)>>> {
        if self.left == 0 {
            self.out = true;
            return None;
        }
        let read = head_lines(path, packed);
        let used = read.as_ref().map_or(HEAD_BYTES, |(_, used)| *used);
        self.left = self.left.saturating_sub(used.max(1));
        Some(read.map(|(lines, _)| lines))
    }
}

fn claude_row(
    id: String,
    c: &Candidate,
    folders: &[(String, String, PathBuf)],
    budget: &mut Budget,
) -> Option<FoundRow> {
    let head = budget.head(&c.path, false)?;
    let mut row = FoundRow {
        id,
        agent: AgentId::Claude,
        native_id: c.uuid.clone(),
        cwd: None,
        folder_id: None,
        title: String::new(),
        first_ms: None,
        updated_ms: c.meta.mtime_ms(),
        bytes: c.meta.size,
        source: None,
        base_thread: None,
        base_end: None,
        damaged: false,
        thread_id: None,
        files: vec![c.file()],
    };
    match head {
        Ok(head) => {
            let (title, first_ms, facts) = first_prompt(AgentId::Claude, &head, row.updated_ms);
            row.title = title_of(title);
            row.first_ms = first_ms;
            row.source = claude_source(facts.entrypoint.as_deref());
            row.cwd = facts.cwd.map(|c| cut(&c, MAX_PATH).to_owned());
        }
        Err(e) => {
            tracing::debug!("a transcript couldn't be read: {}", e.kind());
            row.damaged = true;
        }
    }
    row.folder_id = row.cwd.as_deref().and_then(|c| folder_of(c, folders).1).map(|f| f.0.clone());
    Some(row)
}

/// One row for a Codex thread from its parts: the newest part is where it
/// continues; the parts its history comes from lead back to the first.
fn codex_row(
    id: String,
    uuid: &str,
    candidates: &[Candidate],
    titles: &HashMap<String, String>,
    folders: &[(String, String, PathBuf)],
    budget: &mut Budget,
    refused: &mut HashMap<PathBuf, Stamp>,
) -> Option<FoundRow> {
    let mut parts: Vec<Part> = Vec::new();
    for c in candidates {
        if refused.get(&c.path).is_some_and(|s| *s == c.meta) {
            continue;
        }
        let head = budget.head(&c.path, c.packed)?;
        match head {
            Ok(head) => {
                let meta = head.first().and_then(|(_, v)| codex_files::meta_of(v));
                // A file is part of this thread only when its first line says so.
                if meta.as_ref().and_then(|m| m.id.as_deref()) != Some(uuid) {
                    refused.insert(c.path.clone(), c.meta);
                    continue;
                }
                parts.push(Part { candidate: c.clone(), meta, head });
            }
            Err(e) => {
                tracing::debug!("a rollout couldn't be read: {}", e.kind());
                parts.push(Part { candidate: c.clone(), meta: None, head: Vec::new() });
            }
        }
    }
    parts.sort_by(|a, b| {
        (&a.candidate.stamp, &a.candidate.path).cmp(&(&b.candidate.stamp, &b.candidate.path))
    });
    let newest = parts.iter().rev().find(|p| p.meta.is_some()).or(parts.last())?;
    // Back from the newest part through the parts its history names.
    let by_segment: HashMap<String, &Part> = parts
        .iter()
        .filter_map(|p| {
            let name = p.candidate.path.file_name()?.to_str()?;
            Some((paths::rollout_name(name)?.segment().to_owned(), p))
        })
        .collect();
    let mut root = newest;
    let mut seen: HashSet<&Path> = HashSet::from([newest.candidate.path.as_path()]);
    let mut base = None;
    while let Some(hb) = root.meta.as_ref().and_then(|m| m.history_base.as_ref()) {
        match by_segment.get(&hb.segment) {
            Some(part)
                if seen.insert(part.candidate.path.as_path()) && seen.len() <= import::MAX_BASES + 1 =>
            {
                root = *part;
            }
            Some(_) => break,
            None => {
                base = Some((hb.segment.clone(), hb.end_byte));
                break;
            }
        }
    }
    let mtime = newest.candidate.meta.mtime_ms();
    let (title, first_ms, _) = first_prompt(AgentId::Codex, &root.head, mtime);
    let meta = newest.meta.clone().unwrap_or_default();
    let cwd = meta.cwd.clone().or_else(|| root.meta.as_ref().and_then(|m| m.cwd.clone()));
    let title = titles.get(uuid).cloned().or(title);
    Some(FoundRow {
        id,
        agent: AgentId::Codex,
        native_id: uuid.to_owned(),
        folder_id: cwd.as_deref().and_then(|c| folder_of(c, folders).1).map(|f| f.0.clone()),
        cwd: cwd.map(|c| cut(&c, MAX_PATH).to_owned()),
        title: title_of(title),
        first_ms,
        updated_ms: parts.iter().map(|p| p.candidate.meta.mtime_ms()).max().unwrap_or(0),
        bytes: parts.iter().map(|p| p.candidate.meta.size).sum(),
        source: newest.meta.as_ref().map(CodexMeta::source),
        base_thread: base.as_ref().map(|(segment, _)| segment.clone()),
        base_end: base.map(|(_, end)| end),
        damaged: parts.iter().any(|p| p.meta.is_none()),
        thread_id: None,
        files: parts.iter().map(|p| p.candidate.file()).collect(),
    })
}

/// One scan: walks the roots, reads what changed, writes the rows and
/// forgets files that are gone. `progress` gets the sessions seen so far.
pub fn run(
    mut job: ScanJob,
    store: &dyn FoundStore,
    progress: &mut dyn FnMut(u32),
) -> io::Result<ScanReport> {
    let roots = &job.roots;
    let mut walk = Walk::default();
    walk.claude(roots);
    walk.days(&roots.codex_sessions(), false);
    walk.archived(&roots.codex_archived());
    if walk.ignored > 0 {
        tracing::info!(
            count = walk.ignored,
            "names in archived_sessions that aren't rollouts were left alone"
        );
    }
    let mut partial = walk.partial;
    // Per agent, the newest first, at most MAX_PER_AGENT files.
    walk.files.sort_by(|a, b| b.meta.mtime_ns.cmp(&a.meta.mtime_ns).then_with(|| a.path.cmp(&b.path)));
    let mut per_agent = [0usize; 2];
    let mut groups: Vec<(String, Vec<Candidate>)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut present: HashSet<String> = HashSet::new();
    for c in walk.files {
        present.insert(c.path.to_string_lossy().into_owned());
        let count = &mut per_agent[c.agent as usize];
        if *count >= MAX_PER_AGENT {
            partial = true;
            continue;
        }
        *count += 1;
        let Some(id) = paths::session_id(c.agent, &c.uuid) else { continue };
        match index.get(&id) {
            Some(&at) => groups[at].1.push(c),
            None => {
                index.insert(id.clone(), groups.len());
                groups.push((id, vec![c]));
            }
        }
    }
    let known_files = store.found_files()?;
    let mut known: HashMap<&str, Vec<&FoundFile>> = HashMap::new();
    for (id, file) in &known_files {
        known.entry(id.as_str()).or_default().push(file);
    }
    let titles = match job.titles.take() {
        Some(titles) => titles,
        None if per_agent[AgentId::Codex as usize] > 0 => codex_titles(roots),
        None => HashMap::new(),
    };
    let mut budget = Budget { left: MAX_META_BYTES, out: false };
    let mut report = ScanReport::default();
    let mut batch: Vec<FoundRow> = Vec::with_capacity(BATCH);
    for (id, candidates) in &groups {
        if job.stop.load(Ordering::SeqCst) {
            partial = true;
            break;
        }
        let taken: Vec<&Candidate> =
            candidates.iter().filter(|c| job.refused.get(&c.path).is_none_or(|s| *s != c.meta)).collect();
        if taken.is_empty() {
            continue;
        }
        let unchanged = known.get(id.as_str()).is_some_and(|files| {
            files.len() == taken.len() && taken.iter().all(|c| files.iter().any(|f| same(f, c)))
        });
        if unchanged {
            report.seen += 1;
            continue;
        }
        let row = match candidates[0].agent {
            AgentId::Claude => claude_row(id.clone(), &candidates[0], &job.folders, &mut budget),
            AgentId::Codex => codex_row(
                id.clone(),
                &candidates[0].uuid,
                candidates,
                &titles,
                &job.folders,
                &mut budget,
                &mut job.refused,
            ),
        };
        if let Some(row) = row {
            report.seen += 1;
            batch.push(row);
        } else if budget.out {
            report.seen += 1;
        }
        if batch.len() >= BATCH {
            store.upsert_found(&batch)?;
            report.written += batch.len() as u32;
            batch.clear();
            progress(report.seen);
        }
    }
    if !batch.is_empty() {
        store.upsert_found(&batch)?;
        report.written += batch.len() as u32;
    }
    // Files that are gone; a session keeps its row and its link to a thread.
    let gone: Vec<(String, String)> = known_files
        .iter()
        .filter(|(_, f)| !present.contains(&f.path) && tail::stamp(Path::new(&f.path)).is_none())
        .map(|(id, f)| (id.clone(), f.path.clone()))
        .collect();
    if !gone.is_empty() {
        store.forget_found_files(&gone)?;
    }
    job.refused.retain(|path, _| present.contains(path.to_string_lossy().as_ref()));
    report.forgotten = gone.len() as u32;
    report.partial = partial || budget.out;
    report.ignored = walk.ignored;
    report.refused = job.refused;
    progress(report.seen);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::sessions::import::tests::{PART, THREAD, claude_transcript, pack, rollout, store};
    use crate::host::test_host;
    use serde_json::json;

    fn job(roots: &Roots) -> ScanJob {
        ScanJob {
            roots: roots.clone(),
            folders: vec![("f1".into(), "app".into(), PathBuf::from("/home/me/code/app"))],
            titles: None,
            refused: HashMap::new(),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    fn roots(dir: &Path) -> Roots {
        Roots {
            claude_home: dir.join("claude"),
            codex_home: dir.join("codex"),
            proc_root: dir.join("proc"),
            claude_app_dir: None,
        }
    }

    fn all(store: &dyn FoundStore) -> Vec<FoundRow> {
        store.found(&FoundQuery { limit: MAX_SCAN_LIMIT, ..FoundQuery::default() }).unwrap().rows
    }

    #[test]
    fn a_scan_lists_both_agents_and_reads_again_only_what_changed() {
        let dir = tempfile::tempdir().unwrap();
        let roots = roots(dir.path());
        let store = store(dir.path());
        let transcript = claude_transcript(&roots.claude_home, THREAD, "/home/me/code/app", 2);
        let day = roots.codex_sessions().join("2026/11/19");
        std::fs::create_dir_all(&day).unwrap();
        let meta = json!({"id": PART, "cwd": "/elsewhere", "originator": "codex-tui", "creator_user_id": "user-secret"});
        let packed = day.join(format!("rollout-2026-11-19T09-00-00-{PART}.jsonl.zst"));
        std::fs::write(&packed, pack(&rollout(meta, 0, &[(true, "Explain the build".into())]))).unwrap();
        // Not this thread's, by its first line: never listed.
        let other = "33333333-2222-4333-8444-555555555555";
        let wrong = rollout(json!({"id": THREAD, "cwd": "/w"}), 0, &[(true, "x".into())]);
        std::fs::write(day.join(format!("rollout-2026-11-19T09-00-00-{other}.jsonl")), wrong).unwrap();
        // A link is never followed.
        let linked = "44444444-2222-4333-8444-555555555555";
        std::os::unix::fs::symlink(&transcript, transcript.with_file_name(format!("{linked}.jsonl")))
            .unwrap();

        let mut seen = Vec::new();
        let report = run(job(&roots), &*store, &mut |n| seen.push(n)).unwrap();
        assert_eq!((report.seen, report.written, report.partial), (2, 2, false));
        assert_eq!(report.refused.len(), 1);
        let rows = all(&*store);
        assert_eq!(rows.len(), 2);
        let codex = rows.iter().find(|r| r.agent == AgentId::Codex).unwrap();
        assert_eq!(codex.title, "Explain the build");
        assert_eq!((codex.cwd.as_deref(), codex.folder_id.as_deref()), (Some("/elsewhere"), None));
        assert_eq!(codex.source, Some(SessionSource::Terminal));
        assert!(codex.packed() && !codex.damaged);
        let claude = rows.iter().find(|r| r.agent == AgentId::Claude).unwrap();
        assert_eq!((claude.title.as_str(), claude.folder_id.as_deref()), ("prompt 0", Some("f1")));
        assert_eq!(claude.source, Some(SessionSource::Terminal));
        assert_eq!(claude.first_ms, paths::parse_rfc3339_ms("2026-11-19T09:00:00.000Z"));
        // Nothing changed: nothing read or written.
        let again =
            run(ScanJob { refused: report.refused.clone(), ..job(&roots) }, &*store, &mut |_| {}).unwrap();
        assert_eq!((again.seen, again.written), (2, 0));
        // One file changed: that one only.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .and_then(|mut f| {
                io::Write::write_all(&mut f, b"{\"type\":\"custom-title\",\"customTitle\":\"Renamed\"}\n")
            })
            .unwrap();
        let third =
            run(ScanJob { refused: again.refused.clone(), ..job(&roots) }, &*store, &mut |_| {}).unwrap();
        assert_eq!(third.written, 1);
        assert!(all(&*store).iter().any(|r| r.title == "Renamed"));
        // A compressed file that is cut is listed as damaged.
        let bytes = std::fs::read(&packed).unwrap();
        std::fs::write(&packed, &bytes[..bytes.len() / 2]).unwrap();
        run(job(&roots), &*store, &mut |_| {}).unwrap();
        assert!(all(&*store).iter().any(|r| r.agent == AgentId::Codex && r.damaged));
        // Gone: its file is forgotten, the session stays.
        std::fs::remove_file(&packed).unwrap();
        let gone = run(job(&roots), &*store, &mut |_| {}).unwrap();
        assert_eq!(gone.forgotten, 1);
        assert_eq!(all(&*store).len(), 2);
        assert!(store.found_files().unwrap().iter().all(|(_, f)| !f.path.ends_with(".zst")));
    }

    #[test]
    fn archived_rollouts_count_flat_or_by_day_and_nothing_else_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let roots = roots(dir.path());
        let store = store(dir.path());
        let archived = roots.codex_archived();
        std::fs::create_dir_all(archived.join("2026/10/01")).unwrap();
        let flat = "55555555-2222-4333-8444-555555555555";
        let dated = "66666666-2222-4333-8444-555555555555";
        for (path, id) in [
            (archived.join(format!("rollout-2026-10-01T09-00-00-{flat}.jsonl")), flat),
            (archived.join(format!("2026/10/01/rollout-2026-10-01T09-00-00-{dated}.jsonl")), dated),
        ] {
            std::fs::write(path, rollout(json!({"id": id, "cwd": "/w"}), 0, &[(true, "old".into())]))
                .unwrap();
        }
        std::fs::write(archived.join("notes.txt"), "x").unwrap();
        std::fs::create_dir_all(archived.join("misc/deeper")).unwrap();
        std::fs::write(archived.join("2026/10/01/state_5.sqlite"), "x").unwrap();
        let report = run(job(&roots), &*store, &mut |_| {}).unwrap();
        assert_eq!((report.seen, report.ignored), (2, 3));
        assert_eq!(all(&*store).len(), 2);
    }

    #[test]
    fn a_later_part_joins_its_thread_and_titles_come_from_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let roots = roots(dir.path());
        let store = store(dir.path());
        let day = roots.codex_sessions().join("2026/11/19");
        std::fs::create_dir_all(&day).unwrap();
        let first = rollout(
            json!({"id": THREAD, "cwd": "/w", "originator": "Codex Desktop"}),
            0,
            &[(true, "Start here".into()), (false, "ok".into())],
        );
        let end = first.len() as u64;
        std::fs::write(day.join(format!("rollout-2026-11-19T09-00-00-{THREAD}.jsonl.zst")), pack(&first))
            .unwrap();
        let meta = json!({"id": THREAD, "cwd": "/w", "originator": "Codex Desktop",
                          "history_base": {"thread_id": THREAD, "end_ordinal_exclusive": 3, "end_byte_offset": end}});
        std::fs::write(
            day.join(format!("rollout-2026-11-19T10-00-00-{THREAD}_{PART}.jsonl")),
            rollout(meta, 3, &[(true, "Then this".into())]),
        )
        .unwrap();
        run(job(&roots), &*store, &mut |_| {}).unwrap();
        let rows = all(&*store);
        assert_eq!(rows.len(), 1, "one thread, one row");
        assert_eq!(rows[0].files.len(), 2);
        assert_eq!(rows[0].title, "Start here");
        assert_eq!(rows[0].source, Some(SessionSource::DesktopApp));
        assert_eq!((rows[0].base_thread.as_deref(), rows[0].base_end), (None, None));
    }

    #[tokio::test]
    async fn sessions_scan_answers_at_once_and_tells_who_asked() {
        let dir = tempfile::tempdir().unwrap();
        let host = test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let none = answer(&host, SessionsScanParams::default()).await.unwrap_err();
        assert_eq!(none.code, ErrorCode::Unavailable);
        let scanner = host.hub.test_subscriber(crate::host::Caps::default());
        host.hub.set(scanner, |slot| slot.scans = true);
        let other = host.hub.test_subscriber(crate::host::Caps::default());
        claude_transcript(&host.config.claude_home, THREAD, "/home/me/code/app", 1);
        host.sessions.attach_store(store(dir.path()));
        let long = SessionsScanParams {
            query: Some("x".repeat(MAX_SCAN_QUERY + 1)),
            ..SessionsScanParams::default()
        };
        assert_eq!(answer(&host, long).await.unwrap_err().code, ErrorCode::BadRequest);
        let first = answer(&host, SessionsScanParams::default()).await.unwrap();
        assert!(first.scanning);
        let status = scan(&host).finished().await;
        assert!(!status.scanning && !status.partial);
        assert_eq!(status.seen, 1);
        let page = answer(&host, SessionsScanParams::default()).await.unwrap();
        assert!(!page.scanning, "a scan that just ran isn't run again");
        assert_eq!(page.found.len(), 1);
        let found = &page.found[0];
        assert_eq!(
            (found.title.as_str(), found.folder_label.as_str(), found.open),
            ("prompt 0", "app", false)
        );
        assert_eq!(found.cwd, "/home/me/code/app");
        let scanned = host.hub.test_lines(scanner);
        assert!(
            scanned.iter().any(|l| l.contains("\"sessions.scanned\"") && l.contains("\"scanning\":false"))
        );
        assert!(host.hub.test_lines(other).iter().all(|l| !l.contains("sessions.scanned")));
    }
}
