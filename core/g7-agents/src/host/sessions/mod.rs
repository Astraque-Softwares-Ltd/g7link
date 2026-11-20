// SPDX-License-Identifier: MPL-2.0
//! Sessions started outside G7 (a terminal, the Claude or Codex app),
//! followed read-only from the agents' own files (0.1.14 contract §A2).
//!
//! The scan loop runs only while some connection subscribed with `sessions`
//! or watches a session, and only while the host setting "follow sessions"
//! is on. It reads processes and file tails every 5 s and a watched file
//! every 500 ms, with bounded reads, one scan at a time, off the async
//! threads. Nothing from a session file is written anywhere until
//! `session.continue` copies items into a thread of ours.

pub mod claude_files;
pub mod codex_files;
pub mod paths;
pub mod procs;
pub mod tail;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use crate::api::*;
use crate::host::store::now_ms;
use crate::host::threads::{CODEX_AUTO_EDIT, Continued, subdir_of};
use crate::host::{Config, Host};
use claude_files::ClaudeTail;
use codex_files::{CodexMeta, CodexTail};
use paths::Roots;
use procs::{Comm, Status};
use tail::{Follower, Line, Poll, Stamp};

/// Tool steps waiting for a result, per session.
pub(crate) const MAX_OPEN_TOOLS: usize = 256;
/// Content blocks of one line that become items.
pub(crate) const MAX_BLOCKS: usize = 64;
/// A rollout written this recently is open.
const RECENT_WRITE_MS: u64 = 90_000;
/// `session.state` for a change of `updated_ms`, `last_seq` or `step` alone.
const STATE_EVERY: Duration = Duration::from_secs(2);
const MAX_PROJECT_FOLDERS: usize = 512;
const MAX_TRANSCRIPTS: usize = 4096;
const MAX_DAY_ENTRIES: usize = 512;
const MAX_LOCKS: usize = 512;
/// A Codex rollout's first line is read up to this.
const MAX_META_LINE: u64 = 1_048_576;
/// `session.read` and `session.continue` read back at most this much.
const MAX_COLLECT: u64 = 8 * 1024 * 1024;
/// Our own process may write the rollout this long after the turn's end.
const OWN_WRITE_SLACK_MS: u64 = 3000;

/// `seq` for block `index` of the line at `offset`: unique, growing with
/// the file, the same on every read.
pub(crate) fn block_seq(offset: u64, index: usize) -> u64 {
    (offset << 6) | (index.min(MAX_BLOCKS - 1) as u64)
}

/// A command or reminder wrapper (`<command-name>…`, `<environment_context>`).
pub(crate) fn is_wrapper(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('<') && t.as_bytes().iter().take(64).any(|&b| b == b'>')
}

/// The first non-empty line, trimmed.
pub(crate) fn first_line(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned)
}

pub(crate) fn text_item(seq: u64, id: &str, kind: ItemKind, text: &str, ts_ms: u64) -> Item {
    let (text, truncated) = clean_cut(text, MAX_ITEM_TEXT);
    Item { text: Some(text), truncated, ..Item::new(seq, cut(id, MAX_ITEM_ID), kind, ts_ms) }
}

/// Where a session is open, in words: "a terminal", "the Claude app".
pub(crate) fn place(agent: AgentId, source: SessionSource) -> &'static str {
    match (source, agent) {
        (SessionSource::Terminal, _) => "a terminal",
        (SessionSource::DesktopApp, AgentId::Claude) => "the Claude app",
        (SessionSource::DesktopApp, AgentId::Codex) => "the Codex app",
        (SessionSource::Other, _) => "another app",
    }
}

/// A session from the Codex app. Its managed daemon can keep a thread loaded
/// and write to it with no trace this host can read (no command line, no
/// lock), so continuing one here could make two writers: refused in 0.1.14.
fn codex_app(agent: AgentId, source: SessionSource) -> bool {
    agent == AgentId::Codex && source == SessionSource::DesktopApp
}

const CODEX_APP_MESSAGE: &str = "Sessions from the Codex app can't be continued here yet.";

/// `BUSY` for a session something else has open.
pub(crate) fn still_open_message(agent: AgentId, source: SessionSource) -> String {
    format!("Still open in {}. Close it there to continue here.", place(agent, source))
}

fn not_found() -> RpcError {
    RpcError::new(ErrorCode::NotFound, "No such session.")
}

// ---- One file's reader ----

/// A transcript or rollout being read.
#[derive(Debug)]
pub(crate) enum Tail {
    Claude(ClaudeTail),
    Codex(CodexTail),
}

impl Tail {
    fn new(agent: AgentId, cwd: Option<PathBuf>) -> Tail {
        match agent {
            AgentId::Claude => Tail::Claude(ClaudeTail::new(cwd)),
            AgentId::Codex => Tail::Codex(CodexTail::new(cwd)),
        }
    }

    fn feed(&mut self, line: &Line, mtime_ms: u64) -> Vec<Item> {
        match self {
            Tail::Claude(t) => t.feed(line, mtime_ms),
            Tail::Codex(t) => t.feed(line, mtime_ms),
        }
    }

    /// The tail's guess, with a guessed "needs you" as its own value.
    fn guess(&self, quiet_ms: u64) -> SessionState {
        let state = match self {
            Tail::Claude(t) => t.guess(quiet_ms),
            Tail::Codex(t) => t.guess(quiet_ms),
        };
        if state == SessionState::NeedsYou { SessionState::NeedsYouGuess } else { state }
    }

    fn step(&self) -> Option<String> {
        match self {
            Tail::Claude(t) => t.step(),
            Tail::Codex(t) => t.step(),
        }
    }

    fn title(&self) -> Option<String> {
        match self {
            Tail::Claude(t) => t.title(),
            Tail::Codex(t) => t.title(),
        }
    }

    fn summary(&self) -> Option<&str> {
        match self {
            Tail::Claude(t) => t.last_assistant_text.as_deref(),
            Tail::Codex(t) => t.last_agent_message.as_deref(),
        }
    }

    fn last_seq(&self) -> u64 {
        match self {
            Tail::Claude(t) => t.last_seq,
            Tail::Codex(t) => t.last_seq,
        }
    }

    fn line_cwd(&self) -> Option<&str> {
        match self {
            Tail::Claude(t) => t.line_cwd.as_deref(),
            Tail::Codex(_) => None,
        }
    }
}

/// Reads a window and feeds it to a new reader.
fn read_tail(agent: AgentId, path: &Path, cwd: Option<PathBuf>) -> std::io::Result<(tail::Window, Tail)> {
    let window = tail::read_window(path, None, 0)?;
    let mut reader = Tail::new(agent, cwd);
    let mtime = window.stamp.mtime_ms();
    for line in &window.lines {
        reader.feed(line, mtime);
    }
    Ok((window, reader))
}

/// The session's newest items, at most `limit`, before `before` when given,
/// reading back window by window until there are enough. Also whether
/// earlier items exist.
pub(crate) fn collect(
    agent: AgentId,
    path: &Path,
    cwd: Option<PathBuf>,
    before: Option<u64>,
    limit: usize,
) -> std::io::Result<(Vec<Item>, bool)> {
    let before_off = before.map(|s| s >> 6);
    let mut windows = Vec::new();
    let mut upto = before_off;
    let mut lookahead = if before.is_some() { SESSION_TAIL } else { 0 };
    let mut found = 0usize;
    let mut read = 0u64;
    loop {
        let window = tail::read_window(path, upto, lookahead)?;
        lookahead = 0;
        let mtime = window.stamp.mtime_ms();
        let mut counter = Tail::new(agent, cwd.clone());
        for line in window.lines.iter().filter(|l| before_off.is_none_or(|b| l.offset < b)) {
            found += counter.feed(line, mtime).iter().filter(|i| i.rev == 0).count();
        }
        let span = upto.unwrap_or(window.stamp.size).saturating_sub(window.start).max(1);
        read += span;
        let start = window.start;
        windows.push(window);
        if found >= limit || start == 0 || read >= MAX_COLLECT {
            break;
        }
        upto = Some(start);
    }
    let more = windows.last().is_some_and(tail::Window::more_before);
    let mut reader = Tail::new(agent, cwd);
    let mut items: BTreeMap<u64, Item> = BTreeMap::new();
    for window in windows.iter().rev() {
        let mtime = window.stamp.mtime_ms();
        for line in &window.lines {
            for item in reader.feed(line, mtime) {
                items.insert(item.seq, item);
            }
        }
    }
    if let Some(before) = before {
        items.retain(|seq, _| *seq < before);
    }
    let total = items.len();
    let mut kept: Vec<Item> = items.into_values().rev().take(limit).collect();
    kept.reverse();
    Ok((kept, more || total > limit))
}

// ---- The list ----

/// What a scan learned about a session besides its file's tail.
#[derive(Debug, Clone)]
struct Ctx {
    agent: AgentId,
    uuid: String,
    path: PathBuf,
    open: bool,
    /// Claude's live status file: its `status`, `waitingFor`, and when it changed.
    live: Option<(Option<String>, Option<String>, u64)>,
    title_hint: Option<String>,
    cwd: String,
    source: SessionSource,
    folder_id: Option<String>,
    folder_label: String,
    blocked: Option<ContinueBlock>,
}

#[derive(Debug, Clone)]
struct Listed {
    session: Session,
    ctx: Ctx,
}

/// A file's reader, kept while the file is unchanged.
#[derive(Debug)]
struct Cached {
    stamp: Stamp,
    tail: Tail,
    /// Codex: the first line's facts, for this device and inode.
    meta: Option<(u64, u64, CodexMeta)>,
}

/// A watch to start: the session's id and what the scan knows of it.
type NewWatch = (String, Ctx);

/// A session some connection watches.
#[derive(Debug)]
struct Watch {
    follower: Follower,
    tail: Tail,
    items: BTreeMap<u64, Item>,
}

/// The session's state and its time, from the scan's facts and the tail.
fn build(id: &str, ctx: &Ctx, reader: &Tail, mtime_ms: u64, now: u64) -> (Session, u64) {
    let quiet = now.saturating_sub(mtime_ms);
    let guessed = || (reader.guess(quiet), true, None, mtime_ms);
    let (state, state_guess, waiting_for, since_hint) = if !ctx.open {
        (SessionState::Ended, false, None, mtime_ms)
    } else {
        match &ctx.live {
            Some((status, waiting, at)) => match status.as_deref() {
                Some("waiting") => (
                    SessionState::NeedsYou,
                    false,
                    waiting.as_deref().map(|w| clean_cut(w, MAX_WAITING_FOR).0).filter(|w| !w.is_empty()),
                    *at,
                ),
                Some("busy") => (SessionState::Working, false, None, *at),
                Some("idle") => (SessionState::Idle, false, None, *at),
                _ => guessed(),
            },
            None => guessed(),
        }
    };
    let active =
        matches!(state, SessionState::Working | SessionState::NeedsYou | SessionState::NeedsYouGuess);
    let finished = matches!(state, SessionState::Idle | SessionState::Ended);
    let title = ctx
        .title_hint
        .clone()
        .or_else(|| reader.title())
        .map(|t| clean_cut(t.trim(), MAX_TITLE).0)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| ctx.agent.display_name().to_owned());
    let session = Session {
        id: id.to_owned(),
        agent: ctx.agent,
        title,
        cwd: ctx.cwd.clone(),
        folder_id: ctx.folder_id.clone(),
        folder_label: ctx.folder_label.clone(),
        state,
        state_guess,
        state_since_ms: 0,
        waiting_for,
        source: ctx.source,
        open: ctx.open,
        updated_ms: mtime_ms,
        last_seq: reader.last_seq(),
        step: if active { reader.step().map(|s| clean_cut(&s, MAX_STEP).0) } else { None },
        summary: if finished {
            reader
                .summary()
                .map(|t| cut(&collapse_whitespace(&clean(t)), MAX_SUMMARY).to_owned())
                .filter(|t| !t.is_empty())
        } else {
            None
        },
        can_continue: ctx.blocked.is_none(),
        blocked: ctx.blocked,
    };
    (session, since_hint)
}

/// The same session apart from the fields that change on every write.
fn same_but_progress(a: &Session, b: &Session) -> bool {
    let quiet = |s: &Session| Session { updated_ms: 0, last_seq: 0, step: None, ..s.clone() };
    quiet(a) == quiet(b)
}

fn claude_source(
    status: Option<&Status>,
    proc_exe: Option<&Path>,
    matched_argv: bool,
    roots: &Roots,
) -> SessionSource {
    let in_app = |exe: &Path| roots.claude_app_dir.as_ref().is_some_and(|dir| exe.starts_with(dir));
    match status.and_then(|s| s.entrypoint.as_deref()) {
        Some("claude-desktop") => SessionSource::DesktopApp,
        Some("cli") => SessionSource::Terminal,
        _ if proc_exe.is_some_and(in_app) => SessionSource::DesktopApp,
        // Started with the session's id on its command line, outside the app: a terminal.
        _ if matched_argv => SessionSource::Terminal,
        _ => SessionSource::Other,
    }
}

/// What one list scan needs, owned, for `spawn_blocking`.
struct ScanJob {
    roots: Roots,
    euid: u32,
    host_pid: u32,
    now: u64,
    ours: HashSet<String>,
    /// `(id, label, canonical path)`.
    folders: Vec<(String, String, PathBuf)>,
    /// Ready to start a thread, by agent (`None`: not probed yet).
    ready: [Option<bool>; 2],
    cache: HashMap<String, Cached>,
    index: Option<(Stamp, HashMap<String, String>)>,
}

struct ScanDone {
    list: Vec<Listed>,
    cache: HashMap<String, Cached>,
    index: Option<(Stamp, HashMap<String, String>)>,
}

/// The agent folder holding `cwd`: the longest whose path is a
/// component-wise prefix of it once links are resolved.
fn folder_of<'a>(
    cwd: &str,
    folders: &'a [(String, String, PathBuf)],
) -> (PathBuf, Option<&'a (String, String, PathBuf)>) {
    let canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd));
    let best = folders
        .iter()
        .filter(|(_, _, path)| paths::is_within(&canonical, path))
        .max_by_key(|(_, _, path)| path.components().count());
    (canonical, best)
}

fn last_component(cwd: &str) -> String {
    let name = Path::new(cwd)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.to_owned());
    clean_cut(&name, MAX_LABEL).0
}

/// A Codex rollout's first-line facts: from the window when it reached the
/// start, else from the cache for the same file, else read (1 MiB at most).
fn codex_meta(
    path: &Path,
    stamp: &Stamp,
    reader: &Tail,
    cached: Option<(u64, u64, CodexMeta)>,
) -> Option<CodexMeta> {
    if let Tail::Codex(t) = reader
        && let Some(meta) = &t.meta
    {
        return Some(meta.clone());
    }
    if let Some((dev, ino, meta)) = cached
        && dev == stamp.dev
        && ino == stamp.ino
    {
        return Some(meta);
    }
    let head = tail::read_head(path, MAX_META_LINE).ok()?;
    let end = head.iter().position(|&b| b == b'\n')?;
    codex_files::parse_meta(&head[..end])
}

fn scan_blocking(job: ScanJob) -> ScanDone {
    let ScanJob { roots, euid, host_pid, now, ours, folders, ready, mut cache, mut index } = job;
    let scan = procs::scan(&roots.proc_root, euid, host_pid);
    let statuses = procs::statuses(&roots, &scan);
    let oldest = now.saturating_sub(SESSION_WINDOW_MS);
    let live: HashSet<&str> = statuses.values().filter(|s| s.live).map(|s| s.uuid.as_str()).collect();
    let mut found: HashMap<String, (AgentId, String, PathBuf, Stamp)> = HashMap::new();
    let mut keep = |agent: AgentId, uuid: &str, path: PathBuf, stamp: Stamp| {
        let Some(id) = paths::session_id(agent, uuid) else { return };
        if ours.contains(uuid) {
            return;
        }
        if found.get(&id).is_none_or(|(_, _, _, old)| stamp.mtime_ns > old.mtime_ns) {
            found.insert(id, (agent, uuid.to_owned(), path, stamp));
        }
    };
    for (_, folder) in tail::list_dir(&roots.claude_projects(), MAX_PROJECT_FOLDERS) {
        if !std::fs::symlink_metadata(&folder).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        for (name, path) in tail::list_dir(&folder, MAX_TRANSCRIPTS) {
            let Some(uuid) = paths::transcript_uuid(&name) else { continue };
            let Some(stamp) = tail::stamp(&path) else { continue };
            if stamp.mtime_ms() >= oldest || live.contains(uuid) {
                keep(AgentId::Claude, uuid, path, stamp);
            }
        }
    }
    for day in paths::codex_days(now) {
        for (name, path) in tail::list_dir(&roots.codex_day(day), MAX_DAY_ENTRIES) {
            let Some(uuid) = paths::rollout_uuid(&name) else { continue };
            let Some(stamp) = tail::stamp(&path) else { continue };
            if stamp.mtime_ms() >= oldest {
                keep(AgentId::Codex, uuid, path, stamp);
            }
        }
    }
    let mut found: Vec<(String, (AgentId, String, PathBuf, Stamp))> = found.into_iter().collect();
    found.sort_by(|a, b| b.1.3.mtime_ns.cmp(&a.1.3.mtime_ns).then_with(|| a.0.cmp(&b.0)));
    found.truncate(MAX_SESSIONS);

    let any_codex = found.iter().any(|(_, (agent, ..))| *agent == AgentId::Codex);
    let locks: Vec<String> = if any_codex {
        tail::list_dir(&roots.codex_locks(), MAX_LOCKS).into_iter().map(|(name, _)| name).collect()
    } else {
        Vec::new()
    };
    if any_codex {
        let path = roots.codex_index();
        match tail::stamp(&path) {
            Some(stamp) if index.as_ref().is_some_and(|(old, _)| *old == stamp) => {}
            Some(stamp) => {
                let titles = tail::read_window(&path, None, 0).map(|w| {
                    let mut joined = Vec::new();
                    for line in w.lines {
                        joined.extend_from_slice(&line.bytes);
                        joined.push(b'\n');
                    }
                    codex_files::index_titles(&joined)
                });
                index = titles.ok().map(|t| (stamp, t));
            }
            None => index = None,
        }
    }

    let mut next_cache = HashMap::new();
    let mut list = Vec::new();
    for (id, (agent, uuid, path, stamp)) in found {
        let status = statuses.get(&uuid);
        let live_status = status.filter(|s| s.live);
        let old = cache.remove(&id);
        let old_meta = old.as_ref().and_then(|c| c.meta.clone());
        let reader = match old {
            Some(c) if c.stamp == stamp => c.tail,
            _ => {
                let cwd = live_status.and_then(|s| s.cwd.clone()).map(PathBuf::from);
                match read_tail(agent, &path, cwd) {
                    Ok((_, reader)) => reader,
                    Err(e) => {
                        tracing::debug!("a session file couldn't be read: {}", e.kind());
                        continue;
                    }
                }
            }
        };
        let mtime = stamp.mtime_ms();
        let (ctx_open, cwd, source, title_hint, meta) = match agent {
            AgentId::Claude => {
                let argv =
                    scan.procs.values().find(|p| p.comm == Comm::Claude && p.names_claude_session(&uuid));
                let open = live_status.is_some() || argv.is_some();
                let cwd =
                    live_status.and_then(|s| s.cwd.clone()).or_else(|| reader.line_cwd().map(str::to_owned));
                let exe =
                    live_status.and_then(|s| scan.procs.get(&s.pid)).or(argv).and_then(|p| p.exe.as_deref());
                let source = claude_source(status, exe, argv.is_some(), &roots);
                (open, cwd, source, live_status.and_then(|s| s.name.clone()), None)
            }
            AgentId::Codex => {
                let meta = codex_meta(&path, &stamp, &reader, old_meta);
                let cwd = meta.as_ref().and_then(|m| m.cwd.clone());
                let open = codex_open(&scan, &uuid, &locks, now, mtime, cwd.as_deref(), None);
                let source = meta.as_ref().map_or(SessionSource::Other, CodexMeta::source);
                let title = index.as_ref().and_then(|(_, t)| t.get(&uuid).cloned());
                (open, cwd, source, title, meta)
            }
        };
        let Some(cwd) = cwd.filter(|c| c.starts_with('/')) else {
            next_cache
                .insert(id, Cached { stamp, tail: reader, meta: meta.map(|m| (stamp.dev, stamp.ino, m)) });
            continue;
        };
        let (_, folder) = folder_of(&cwd, &folders);
        let blocked = if ctx_open || codex_app(agent, source) {
            Some(ContinueBlock::Open)
        } else if folder.is_none() {
            Some(ContinueBlock::Folder)
        } else if ready[agent as usize] == Some(false) {
            Some(ContinueBlock::Agent)
        } else {
            None
        };
        let ctx = Ctx {
            agent,
            uuid,
            path,
            open: ctx_open,
            live: live_status.map(|s| (s.status.clone(), s.waiting_for.clone(), s.updated_ms)),
            title_hint,
            folder_id: folder.map(|f| f.0.clone()),
            folder_label: folder.map_or_else(|| last_component(&cwd), |f| f.1.clone()),
            cwd: cut(&cwd, MAX_PATH).to_owned(),
            source,
            blocked,
        };
        let (session, _) = build(&id, &ctx, &reader, mtime, now);
        list.push(Listed { session, ctx });
        next_cache.insert(id, Cached { stamp, tail: reader, meta: meta.map(|m| (stamp.dev, stamp.ino, m)) });
    }
    ScanDone { list, cache: next_cache, index }
}

/// Whether some process has a Codex session: its id on a command line, a
/// write lock naming it, a write in the last 90 s (after `ours_until`, when
/// the last writer may have been us), or a Codex in the same folder that
/// started before the last write.
fn codex_open(
    scan: &procs::Scan,
    uuid: &str,
    locks: &[String],
    now: u64,
    mtime: u64,
    cwd: Option<&str>,
    ours_until: Option<u64>,
) -> bool {
    let argv = scan.procs.values().any(|p| p.comm == Comm::Codex && p.mentions(uuid));
    let locked = locks.iter().any(|name| name.contains(uuid));
    let recent = now.saturating_sub(mtime) < RECENT_WRITE_MS
        && ours_until.is_none_or(|ours| mtime > ours + OWN_WRITE_SLACK_MS);
    let same_folder = ours_until.is_none()
        && cwd.is_some_and(|cwd| {
            scan.boot_ms.is_some_and(|boot| {
                scan.procs.values().any(|p| {
                    p.comm == Comm::Codex
                        && !p.is_codex_app_server()
                        && p.cwd.as_deref() == Some(Path::new(cwd))
                        && p.started_ms(boot) < mtime
                })
            })
        });
    argv || locked || recent || same_folder
}

/// A Codex rollout by UUID, in the day folders the scan looks at.
fn find_rollout(roots: &Roots, uuid: &str, now: u64) -> Option<PathBuf> {
    paths::codex_days(now).into_iter().find_map(|day| {
        tail::list_dir(&roots.codex_day(day), MAX_DAY_ENTRIES)
            .into_iter()
            .find(|(name, _)| paths::rollout_uuid(name) == Some(uuid))
            .map(|(_, path)| path)
    })
}

/// Which session `open_now` looks for.
struct OpenQuery<'a> {
    agent: AgentId,
    uuid: &'a str,
    /// The session's file, when known.
    path: Option<PathBuf>,
    cwd: Option<&'a str>,
    /// Set for a thread of ours: a write before it was ours, and Codex's
    /// same-folder rule doesn't apply.
    ours_until: Option<u64>,
}

/// Whether something other than this host has the session open now, from a
/// fresh process scan, and where.
fn open_now(
    roots: &Roots,
    euid: u32,
    host_pid: u32,
    query: OpenQuery<'_>,
    now: u64,
) -> Option<SessionSource> {
    let OpenQuery { agent, uuid, path, cwd, ours_until } = query;
    let scan = procs::scan(&roots.proc_root, euid, host_pid);
    match agent {
        AgentId::Claude => {
            let statuses = procs::statuses(roots, &scan);
            let status = statuses.get(uuid);
            let live = status.filter(|s| s.live);
            let argv = scan.procs.values().find(|p| p.comm == Comm::Claude && p.names_claude_session(uuid));
            if live.is_none() && argv.is_none() {
                return None;
            }
            let exe = live.and_then(|s| scan.procs.get(&s.pid)).or(argv).and_then(|p| p.exe.as_deref());
            Some(claude_source(status, exe, argv.is_some(), roots))
        }
        AgentId::Codex => {
            let path = path.or_else(|| find_rollout(roots, uuid, now));
            let stamp = path.as_deref().and_then(tail::stamp);
            let locks: Vec<String> =
                tail::list_dir(&roots.codex_locks(), MAX_LOCKS).into_iter().map(|(name, _)| name).collect();
            let mtime = stamp.map_or(0, |s| s.mtime_ms());
            if !codex_open(&scan, uuid, &locks, now, mtime, cwd, ours_until) {
                return None;
            }
            let in_terminal = scan
                .procs
                .values()
                .any(|p| p.comm == Comm::Codex && !p.is_codex_app_server() && p.mentions(uuid));
            if in_terminal {
                return Some(SessionSource::Terminal);
            }
            let meta = path.and_then(|p| {
                let head = tail::read_head(&p, MAX_META_LINE).ok()?;
                let end = head.iter().position(|&b| b == b'\n')?;
                codex_files::parse_meta(&head[..end])
            });
            Some(meta.map_or(SessionSource::Other, |m| m.source()))
        }
    }
}

// ---- The registry ----

#[derive(Debug, Default)]
struct Inner {
    /// In the order `sessions.list` gives.
    list: Vec<Listed>,
    listed_at: Option<Instant>,
    cache: HashMap<String, Cached>,
    index: Option<(Stamp, HashMap<String, String>)>,
    /// What each subscriber last heard per session, and when.
    sent: HashMap<String, (Session, Instant)>,
    /// Each session's state and since when.
    since: HashMap<String, (SessionState, u64)>,
    watches: HashMap<String, Watch>,
    running: bool,
    task: Option<tokio::task::JoinHandle<()>>,
}

/// The sessions-started-elsewhere part of the host.
#[derive(Debug)]
pub struct Sessions {
    /// Not turned off for this host (`G7_AGENTS_NO_SESSIONS`, or no home).
    enabled: bool,
    /// The user's "Follow sessions started outside G7".
    following: AtomicBool,
    roots: Roots,
    /// One scan or watch tick at a time.
    scan_lock: tokio::sync::Mutex<()>,
    inner: Mutex<Inner>,
}

impl Sessions {
    pub fn new(config: &Config) -> Sessions {
        Sessions {
            enabled: config.sessions,
            following: AtomicBool::new(true),
            roots: config.session_roots(),
            scan_lock: tokio::sync::Mutex::new(()),
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn following(&self) -> bool {
        self.following.load(Ordering::SeqCst)
    }

    pub(crate) fn set_following_loaded(&self, on: bool) {
        self.following.store(on, Ordering::SeqCst);
    }

    /// Sessions are followed: allowed for this host and on.
    pub fn active(&self) -> bool {
        self.enabled && self.following()
    }

    /// Stops the scan loop (shutdown).
    pub fn stop(&self) {
        let mut inner = self.lock();
        if let Some(task) = inner.task.take() {
            task.abort();
        }
        inner.running = false;
        inner.watches.clear();
    }
}

impl Host {
    fn session_folders(&self) -> Vec<(String, String, PathBuf)> {
        self.folders_list()
            .into_iter()
            .map(|f| {
                let path = std::fs::canonicalize(&f.path).unwrap_or_else(|_| PathBuf::from(&f.path));
                (f.id, f.label, path)
            })
            .collect()
    }

    fn agents_ready(&self) -> [Option<bool>; 2] {
        let Ok(cache) = self.probes.try_lock() else { return [None, None] };
        let blocked = self.lock_reg().login_blocked.clone();
        let ready = |probe: &Option<crate::host::agents::Probe>, agent: AgentId| {
            probe.as_ref().map(|p| p.info.ready() && p.bin.is_some() && !blocked.contains_key(&agent))
        };
        [ready(&cache.claude, AgentId::Claude), ready(&cache.codex, AgentId::Codex)]
    }

    /// Starts the scan loop if something wants sessions and it isn't running.
    pub fn sessions_wake(self: &Arc<Self>) {
        if !self.sessions.active() {
            return;
        }
        let mut inner = self.sessions.lock();
        if inner.running {
            return;
        }
        inner.running = true;
        inner.task = Some(tokio::spawn(scan_loop(Arc::downgrade(self))));
    }

    /// Runs a list scan now; the scan lock must be held.
    async fn scan_locked(self: &Arc<Self>) {
        let job = {
            let ours = self.native_ids();
            let folders = self.session_folders();
            let ready = self.agents_ready();
            let mut inner = self.sessions.lock();
            ScanJob {
                roots: self.sessions.roots.clone(),
                euid: crate::client::euid(),
                host_pid: std::process::id(),
                now: now_ms(),
                ours,
                folders,
                ready,
                cache: std::mem::take(&mut inner.cache),
                index: inner.index.take(),
            }
        };
        let done = tokio::task::spawn_blocking(move || scan_blocking(job)).await;
        let Ok(done) = done else { return };
        if !self.sessions.active() {
            return;
        }
        let mut inner = self.sessions.lock();
        inner.cache = done.cache;
        inner.index = done.index;
        inner.listed_at = Some(Instant::now());
        let mut list = done.list;
        // Where a session ran is known while it runs; keep it once it ends.
        for listed in list.iter_mut().filter(|l| l.ctx.source == SessionSource::Other) {
            if let Some(old) = inner.list.iter().find(|o| o.session.id == listed.session.id)
                && old.ctx.source != SessionSource::Other
            {
                listed.ctx.source = old.ctx.source;
                listed.session.source = old.ctx.source;
            }
        }
        self.publish(&mut inner, list);
    }

    /// A list scan, unless a recent one is good enough.
    async fn scan_if_stale(self: &Arc<Self>) {
        let fresh = |s: &Self| {
            s.sessions
                .lock()
                .listed_at
                .is_some_and(|at| at.elapsed() < Duration::from_millis(SESSION_LIST_POLL_MS))
        };
        if fresh(self) {
            return;
        }
        let _scan = self.sessions.scan_lock.lock().await;
        if !fresh(self) {
            self.scan_locked().await;
        }
    }

    /// Sends what changed between the list clients have and `list`, and keeps `list`.
    fn publish(&self, inner: &mut Inner, mut list: Vec<Listed>) {
        let now = now_ms();
        for listed in &mut list {
            let id = listed.session.id.clone();
            let hint = match (&listed.ctx.live, listed.session.open) {
                (Some((_, _, at)), true) if !listed.session.state_guess => *at,
                _ => listed.session.updated_ms,
            };
            let since = match inner.since.get(&id) {
                Some((state, since)) if *state == listed.session.state => *since,
                Some(_) => now,
                None => hint,
            };
            inner.since.insert(id, (listed.session.state, since));
            listed.session.state_since_ms = since;
        }
        list.sort_by(|a, b| {
            a.session
                .rank()
                .cmp(&b.session.rank())
                .then_with(|| b.session.updated_ms.cmp(&a.session.updated_ms))
                .then_with(|| a.session.id.cmp(&b.session.id))
        });
        let ids: HashSet<&str> = list.iter().map(|l| l.session.id.as_str()).collect();
        for old in &inner.list {
            if !ids.contains(old.session.id.as_str()) {
                self.hub.broadcast(&Event::SessionRemoved(SessionRemovedEvent {
                    session_id: old.session.id.clone(),
                }));
            }
        }
        inner.sent.retain(|id, _| ids.contains(id.as_str()));
        inner.since.retain(|id, _| ids.contains(id.as_str()));
        for listed in &list {
            Self::announce(self, inner, &listed.session);
        }
        inner.list = list;
    }

    /// `session.state` for one session when it changed enough to tell.
    fn announce(&self, inner: &mut Inner, session: &Session) {
        let send = match inner.sent.get(&session.id) {
            None => true,
            Some((prev, at)) => {
                prev != session && (!same_but_progress(prev, session) || at.elapsed() >= STATE_EVERY)
            }
        };
        if send {
            inner.sent.insert(session.id.clone(), (session.clone(), Instant::now()));
            self.hub.broadcast(&Event::SessionState(SessionResult { session: session.clone() }));
        }
    }

    /// The listed session `id`, scanning first when the list is stale or
    /// doesn't have it.
    async fn listed(self: &Arc<Self>, id: &str) -> Option<Listed> {
        if !self.sessions.active() || !is_valid_session_id(id) {
            return None;
        }
        let find = |s: &Self| s.sessions.lock().list.iter().find(|l| l.session.id == id).cloned();
        self.scan_if_stale().await;
        if let Some(found) = find(self) {
            return Some(found);
        }
        let _scan = self.sessions.scan_lock.lock().await;
        self.scan_locked().await;
        find(self)
    }

    /// `sessions.list`.
    pub async fn sessions_list(self: &Arc<Self>, limit: usize) -> Vec<Session> {
        if !self.sessions.active() {
            return Vec::new();
        }
        self.scan_if_stale().await;
        self.sessions.lock().list.iter().take(limit).map(|l| l.session.clone()).collect()
    }

    /// `session.read`.
    pub async fn session_read(self: &Arc<Self>, p: SessionReadParams) -> Result<SessionReadResult, RpcError> {
        let listed = self.listed(&p.session_id).await.ok_or_else(not_found)?;
        let limit = p.limit.unwrap_or(DEFAULT_READ_LIMIT).clamp(1, MAX_READ_LIMIT) as usize;
        let ctx = listed.ctx.clone();
        let read = tokio::task::spawn_blocking(move || {
            collect(ctx.agent, &ctx.path, Some(PathBuf::from(&ctx.cwd)), p.before_seq, limit)
        })
        .await
        .map_err(|_| RpcError::new(ErrorCode::Internal, "The read failed."))?;
        let (mut items, mut more_before) = read.map_err(|_| not_found())?;
        let base = serde_json::to_vec(&SessionReadResult {
            session: listed.session.clone(),
            items: Vec::new(),
            more_before: true,
        })
        .map_or(0, |v| v.len());
        let mut budget = MAX_READ_RESULT.saturating_sub(base + 16);
        let mut keep = items.len();
        for (i, item) in items.iter().enumerate().rev() {
            let size = serde_json::to_vec(item).map_or(0, |v| v.len() + 1);
            if size > budget {
                break;
            }
            budget -= size;
            keep = i;
        }
        if keep > 0 {
            items.drain(..keep);
            more_before = true;
        }
        Ok(SessionReadResult { session: listed.session, items, more_before })
    }

    /// A listed session and its newest items (at most [`MAX_ITEMS_KEPT`]), for an export.
    pub(crate) async fn session_items_for_export(
        self: &Arc<Self>,
        id: &str,
    ) -> Result<(Session, Vec<Item>), RpcError> {
        let listed = self.listed(id).await.ok_or_else(not_found)?;
        let ctx = listed.ctx;
        let items = tokio::task::spawn_blocking(move || {
            collect(ctx.agent, &ctx.path, Some(PathBuf::from(&ctx.cwd)), None, MAX_ITEMS_KEPT)
        })
        .await
        .map_err(|_| RpcError::new(ErrorCode::Internal, "The read failed."))?
        .map_err(|_| not_found())?
        .0;
        Ok((listed.session, items))
    }

    /// `session.watch`: names the one session this connection follows.
    pub async fn session_watch(
        self: &Arc<Self>,
        client: u64,
        session_id: Option<String>,
    ) -> Result<(), RpcError> {
        let Some(id) = session_id else {
            self.hub.set_watch(client, None);
            return Ok(());
        };
        let listed = self.listed(&id).await.ok_or_else(not_found)?;
        if !self.hub.set_watch(client, Some(id.clone())) {
            return Err(RpcError::new(ErrorCode::Limit, "4 sessions are being watched."));
        }
        let scan = self.sessions.scan_lock.lock().await;
        let missing = !self.sessions.lock().watches.contains_key(&id);
        if missing {
            let ctx = listed.ctx;
            let made = tokio::task::spawn_blocking(move || new_watch(&ctx)).await;
            if let Ok(Ok(watch)) = made {
                self.sessions.lock().watches.entry(id).or_insert(watch);
            }
        }
        drop(scan);
        self.sessions_wake();
        Ok(())
    }

    /// One loop tick: watched files every time, the list when `list_due`.
    async fn sessions_tick(self: &Arc<Self>, list_due: bool) {
        let _scan = self.sessions.scan_lock.lock().await;
        let watched = self.hub.watched_sessions();
        let (work, create): (Vec<(String, Ctx, Watch)>, Vec<NewWatch>) = {
            let mut inner = self.sessions.lock();
            inner.watches.retain(|id, _| watched.contains(id));
            let ctx_of = |inner: &Inner, id: &str| {
                inner.list.iter().find(|l| l.session.id == id).map(|l| l.ctx.clone())
            };
            let mut work = Vec::new();
            let mut create = Vec::new();
            for id in &watched {
                let Some(ctx) = ctx_of(&inner, id) else { continue };
                match inner.watches.remove(id) {
                    Some(watch) => work.push((id.clone(), ctx, watch)),
                    None => create.push((id.clone(), ctx)),
                }
            }
            (work, create)
        };
        let now = now_ms();
        let polled = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for (id, ctx) in create {
                if let Ok(watch) = new_watch(&ctx) {
                    out.push((id, ctx, watch, Vec::new(), None));
                }
            }
            for (id, ctx, mut watch) in work {
                let changed = poll_watch(&ctx, &mut watch);
                let stamp = tail::stamp(&ctx.path);
                out.push((id, ctx, watch, changed, stamp));
            }
            out
        })
        .await;
        if let Ok(polled) = polled
            && self.sessions.active()
        {
            let mut inner = self.sessions.lock();
            for (id, ctx, watch, changed, stamp) in polled {
                for item in &changed {
                    self.hub.broadcast(&Event::SessionItem(SessionItemEvent {
                        session_id: id.clone(),
                        item: item.clone(),
                    }));
                }
                if !changed.is_empty()
                    && let Some(stamp) = stamp
                {
                    let (mut session, _) = build(&id, &ctx, &watch.tail, stamp.mtime_ms(), now);
                    let since = match inner.since.get(&id) {
                        Some((state, since)) if *state == session.state => *since,
                        _ => now,
                    };
                    inner.since.insert(id.clone(), (session.state, since));
                    session.state_since_ms = since;
                    if let Some(listed) = inner.list.iter_mut().find(|l| l.session.id == id) {
                        listed.session = session.clone();
                    }
                    self.announce(&mut inner, &session);
                }
                inner.watches.insert(id, watch);
            }
        }
        if list_due {
            self.scan_locked().await;
        }
    }

    /// Whether something else has one of our threads' sessions open (the
    /// resume guard), and where.
    pub(crate) async fn open_elsewhere(
        self: &Arc<Self>,
        agent: AgentId,
        native: &str,
        ours_until: u64,
    ) -> Option<SessionSource> {
        if !self.sessions.enabled || !paths::is_uuid(native) {
            return None;
        }
        let roots = self.sessions.roots.clone();
        let uuid = native.to_owned();
        let euid = crate::client::euid();
        let pid = std::process::id();
        tokio::task::spawn_blocking(move || {
            let query = OpenQuery { agent, uuid: &uuid, path: None, cwd: None, ours_until: Some(ours_until) };
            open_now(&roots, euid, pid, query, now_ms())
        })
        .await
        .ok()
        .flatten()
    }

    /// `session.continue` (§A2.9).
    pub async fn session_continue(
        self: &Arc<Self>,
        p: SessionContinueParams,
    ) -> Result<ThreadInfo, RpcError> {
        if !p.origin.is_valid() {
            return Err(RpcError::new(ErrorCode::BadRequest, "Bad origin."));
        }
        let listed = self.listed(&p.session_id).await.ok_or_else(not_found)?;
        let ctx = listed.ctx.clone();
        let agent = ctx.agent;
        if p.origin.kind == OriginKind::Phone && p.mode != Mode::Supervised {
            return Err(RpcError::new(
                ErrorCode::Denied,
                "A phone can only continue a session as a thread that asks before each change.",
            ));
        }
        if agent == AgentId::Codex && p.mode == Mode::AutoEdit && !CODEX_AUTO_EDIT {
            return Err(RpcError::new(ErrorCode::Unavailable, "Codex can't edit without asking yet."));
        }
        if codex_app(agent, ctx.source) {
            return Err(RpcError::new(ErrorCode::Busy, CODEX_APP_MESSAGE));
        }
        let open = {
            let roots = self.sessions.roots.clone();
            let ctx = ctx.clone();
            let euid = crate::client::euid();
            let pid = std::process::id();
            tokio::task::spawn_blocking(move || {
                let query = OpenQuery {
                    agent: ctx.agent,
                    uuid: &ctx.uuid,
                    path: Some(ctx.path.clone()),
                    cwd: Some(ctx.cwd.as_str()),
                    ours_until: None,
                };
                open_now(&roots, euid, pid, query, now_ms())
            })
            .await
            .map_err(|_| RpcError::new(ErrorCode::Internal, "The check failed."))?
        };
        if let Some(source) = open {
            return Err(RpcError::new(ErrorCode::Busy, still_open_message(agent, source)));
        }
        let folders = self.session_folders();
        let (canonical, folder) = {
            let cwd = ctx.cwd.clone();
            let folders = folders.clone();
            tokio::task::spawn_blocking(move || {
                let (canonical, folder) = folder_of(&cwd, &folders);
                (canonical, folder.cloned())
            })
            .await
            .map_err(|_| RpcError::new(ErrorCode::Internal, "The check failed."))?
        };
        let Some((folder_id, _, folder_path)) = folder else {
            return Err(RpcError::new(
                ErrorCode::Denied,
                format!("Add {} to your agent folders first.", last_component(&ctx.cwd)),
            ));
        };
        let subdir = subdir_of(&canonical, &folder_path)
            .map_err(|()| RpcError::new(ErrorCode::Denied, "That folder isn't allowed."))?;
        let probe = self.probe(false).await[agent as usize].clone();
        if probe.bin.is_none() || !probe.info.ready() {
            return Err(Self::unavailable(&probe));
        }
        if let Some(message) = self.lock_reg().login_blocked.get(&agent) {
            return Err(RpcError::new(ErrorCode::Unavailable, message));
        }
        let read_ctx = ctx.clone();
        let items = tokio::task::spawn_blocking(move || {
            collect(read_ctx.agent, &read_ctx.path, Some(PathBuf::from(&read_ctx.cwd)), None, CONTINUE_ITEMS)
        })
        .await
        .map_err(|_| RpcError::new(ErrorCode::Internal, "The read failed."))?
        .map_err(|_| not_found())?
        .0;
        let thread = self.insert_continued(Continued {
            agent,
            folder_id,
            subdir,
            title: listed.session.title.clone(),
            mode: p.mode,
            origin: p.origin,
            native_id: ctx.uuid.clone(),
            items,
            notice: format!("Continued from a session started in {}.", place(agent, ctx.source)),
        })?;
        {
            let mut inner = self.sessions.lock();
            inner.list.retain(|l| l.session.id != p.session_id);
            inner.sent.remove(&p.session_id);
            inner.since.remove(&p.session_id);
            inner.cache.remove(&p.session_id);
            inner.watches.remove(&p.session_id);
        }
        self.hub.broadcast(&Event::SessionRemoved(SessionRemovedEvent { session_id: p.session_id }));
        Ok(thread)
    }

    /// `settings.get`.
    pub fn settings(&self) -> HostSettings {
        HostSettings { follow_sessions: self.sessions.following(), keep_awake: self.keep_awake_on() }
    }

    /// `sessions.set_following`: stored, announced, and the scanner stops or starts.
    pub async fn sessions_set_following(self: &Arc<Self>, on: bool) -> Result<HostSettings, RpcError> {
        let _serial = self.folders_lock.lock().await;
        if self.sessions.following() == on {
            return Ok(self.settings());
        }
        let settings = HostSettings { follow_sessions: on, ..self.settings() };
        self.store
            .save_settings(settings)
            .await
            .map_err(|_| RpcError::new(ErrorCode::Internal, "Couldn't save the setting."))?;
        self.sessions.set_following_loaded(on);
        self.hub.broadcast(&Event::SettingsChanged(settings));
        if on {
            self.sessions_wake();
        } else {
            let _scan = self.sessions.scan_lock.lock().await;
            self.hub.clear_watches();
            let mut inner = self.sessions.lock();
            for listed in std::mem::take(&mut inner.list) {
                self.hub
                    .broadcast(&Event::SessionRemoved(SessionRemovedEvent { session_id: listed.session.id }));
            }
            let task = inner.task.take();
            *inner = Inner::default();
            if let Some(task) = task {
                task.abort();
            }
        }
        Ok(settings)
    }
}

/// Starts following a session: a window read, then appends from its end.
fn new_watch(ctx: &Ctx) -> std::io::Result<Watch> {
    let (window, reader) = read_tail(ctx.agent, &ctx.path, Some(PathBuf::from(&ctx.cwd)))?;
    let mut items = BTreeMap::new();
    let mut replay = Tail::new(ctx.agent, Some(PathBuf::from(&ctx.cwd)));
    let mtime = window.stamp.mtime_ms();
    for line in &window.lines {
        for item in replay.feed(line, mtime) {
            items.insert(item.seq, item);
        }
    }
    trim(&mut items);
    Ok(Watch { follower: Follower::after(&window), tail: reader, items })
}

fn trim(items: &mut BTreeMap<u64, Item>) {
    while items.len() > SESSION_ITEMS_KEPT {
        items.pop_first();
    }
}

/// Reads what was appended; the items added or changed. After a rewrite
/// the window is read again and only items that differ are sent, with a
/// higher `rev`.
fn poll_watch(ctx: &Ctx, watch: &mut Watch) -> Vec<Item> {
    let mut changed = Vec::new();
    match watch.follower.poll(&ctx.path) {
        Ok(Poll::Unchanged) | Err(_) => {}
        Ok(Poll::Lines(lines)) => {
            let mtime = tail::stamp(&ctx.path).map_or_else(now_ms, |s| s.mtime_ms());
            for line in &lines {
                for item in watch.tail.feed(line, mtime) {
                    watch.items.insert(item.seq, item.clone());
                    changed.push(item);
                }
            }
            trim(&mut watch.items);
        }
        Ok(Poll::Reset) => {
            let Ok(fresh) = new_watch(ctx) else { return changed };
            for (seq, item) in &fresh.items {
                match watch.items.get(seq) {
                    Some(old) if Item { rev: old.rev, ..item.clone() } == *old => {}
                    Some(old) => changed.push(Item { rev: old.rev.max(item.rev) + 1, ..item.clone() }),
                    None => changed.push(item.clone()),
                }
            }
            let mut items = fresh.items;
            for item in &changed {
                items.insert(item.seq, item.clone());
            }
            *watch = Watch { items, ..fresh };
        }
    }
    changed
}

async fn scan_loop(host: Weak<Host>) {
    let every = SESSION_LIST_POLL_MS / SESSION_WATCH_POLL_MS;
    let mut tick: u64 = 0;
    loop {
        let Some(host) = host.upgrade() else { return };
        {
            let mut inner = host.sessions.lock();
            if !host.sessions.active() || !host.hub.wants_sessions() {
                inner.running = false;
                inner.watches.clear();
                inner.task = None;
                return;
            }
        }
        host.sessions_tick(tick.is_multiple_of(every)).await;
        drop(host);
        tick += 1;
        tokio::time::sleep(Duration::from_millis(SESSION_WATCH_POLL_MS)).await;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::host::test_host;

    pub(crate) fn fixture_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sessions").join(name)
    }

    /// A fixture's lines with their offsets.
    pub(crate) fn fixture_lines(name: &str) -> Vec<Line> {
        tail::read_window(&fixture_path(name), None, 0).unwrap().lines
    }

    /// Feeds every line; the items in `seq` order at their last `rev`.
    pub(crate) fn feed_fixture(name: &str, mut feed: impl FnMut(&Line) -> Vec<Item>) -> Vec<Item> {
        let mut items = BTreeMap::new();
        for line in fixture_lines(name) {
            for item in feed(&line) {
                items.insert(item.seq, item);
            }
        }
        items.into_values().collect()
    }

    #[test]
    fn wrappers_and_seqs() {
        assert!(is_wrapper("  <environment_context>\n<cwd>/w</cwd>"));
        assert!(!is_wrapper("Use <b> tags"), "text that only mentions a tag later is shown");
        assert!(is_wrapper("<b> tags"));
        assert!(!is_wrapper(&format!("<{}>", "x".repeat(80))), "no > in the first 64 bytes");
        assert_eq!(block_seq(10, 0), 640);
        assert_eq!(block_seq(10, 3), 643);
        assert_eq!(block_seq(10, 200), 640 + 63);
        assert_eq!(first_line("\n  Fix it\nthen"), Some("Fix it".into()));
        assert_eq!(
            still_open_message(AgentId::Claude, SessionSource::Terminal),
            "Still open in a terminal. Close it there to continue here."
        );
        assert_eq!(place(AgentId::Codex, SessionSource::DesktopApp), "the Codex app");
    }

    fn ctx(agent: AgentId, open: bool, live: Option<(&str, Option<&str>)>) -> Ctx {
        Ctx {
            agent,
            uuid: "11111111-2222-4333-8444-555555555555".into(),
            path: PathBuf::from("/x"),
            open,
            live: live.map(|(s, w)| (Some(s.to_owned()), w.map(str::to_owned), 5)),
            title_hint: None,
            cwd: "/home/me/code/app".into(),
            source: SessionSource::Terminal,
            folder_id: None,
            folder_label: "app".into(),
            blocked: Some(ContinueBlock::Folder),
        }
    }

    fn tail_of(name: &str, agent: AgentId) -> Tail {
        let mut reader = Tail::new(agent, None);
        for line in fixture_lines(name) {
            reader.feed(&line, 0);
        }
        reader
    }

    #[test]
    fn the_state_table() {
        let now = 1_000_000;
        let open_tool = tail_of("claude-open-tool.jsonl", AgentId::Claude);
        let idle = tail_of("claude-idle.jsonl", AgentId::Claude);
        let state = |c: &Ctx, t: &Tail, mtime: u64| {
            let (s, _) = build("sc1", c, t, mtime, now);
            (s.state, s.state_guess)
        };
        // Not open: ended, whatever the tail says.
        assert_eq!(state(&ctx(AgentId::Claude, false, None), &open_tool, now), (SessionState::Ended, false));
        // Claude's own status wins.
        let waiting = ctx(AgentId::Claude, true, Some(("waiting", Some("permission prompt"))));
        let (s, since) = build("sc1", &waiting, &idle, now, now);
        assert_eq!((s.state, s.state_guess), (SessionState::NeedsYou, false));
        assert_eq!(s.waiting_for.as_deref(), Some("permission prompt"));
        assert_eq!(since, 5, "since the status changed");
        assert_eq!(
            state(&ctx(AgentId::Claude, true, Some(("busy", None))), &idle, now),
            (SessionState::Working, false)
        );
        assert_eq!(
            state(&ctx(AgentId::Claude, true, Some(("idle", None))), &open_tool, now),
            (SessionState::Idle, false)
        );
        // Without a usable status: the tail's guess, flagged.
        let open = ctx(AgentId::Claude, true, None);
        assert_eq!(state(&open, &open_tool, now), (SessionState::Working, true), "not quiet yet");
        assert_eq!(state(&open, &open_tool, now - SESSION_QUIET_MS), (SessionState::NeedsYouGuess, true));
        assert_eq!(state(&open, &idle, now), (SessionState::Idle, true));
        assert_eq!(
            state(&ctx(AgentId::Claude, true, Some(("odd", None))), &idle, now),
            (SessionState::Idle, true)
        );
        // Codex is always a guess.
        let codex_open = tail_of("codex-open-call.jsonl", AgentId::Codex);
        let codex_idle = tail_of("codex-idle.jsonl", AgentId::Codex);
        let c = ctx(AgentId::Codex, true, None);
        assert_eq!(state(&c, &codex_idle, now), (SessionState::Idle, true));
        assert_eq!(state(&c, &codex_open, now), (SessionState::Working, true));
        assert_eq!(state(&c, &codex_open, now - SESSION_QUIET_MS), (SessionState::NeedsYouGuess, true));
        // Step only while active; summary only when finished.
        let (s, _) = build("sc1", &open, &open_tool, now, now);
        assert_eq!(s.step.as_deref(), Some("Write notes.txt"));
        assert_eq!(s.summary, None);
        let (s, _) = build("sc1", &ctx(AgentId::Claude, false, None), &idle, now, now);
        assert_eq!(s.step, None);
        assert!(s.summary.unwrap().starts_with("There are two"));
        assert_eq!(s.title, "Tidy the readme");
        assert!(!s.can_continue);
    }

    #[test]
    fn collect_pages_back_with_stable_seqs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let filler = "x".repeat(2000);
        let mut body = String::new();
        for n in 0..300 {
            body.push_str(&format!(
                "{{\"type\":\"user\",\"uuid\":\"u{n}\",\"cwd\":\"/w\",\"message\":{{\"role\":\"user\",\"content\":\"prompt {n} {filler}\"}}}}\n"
            ));
        }
        std::fs::write(&path, &body).unwrap();
        let (items, more) = collect(AgentId::Claude, &path, None, None, 50).unwrap();
        assert_eq!(items.len(), 50);
        assert!(more);
        assert!(items.last().unwrap().text.as_deref().unwrap().starts_with("prompt 299 "));
        let (older, more) = collect(AgentId::Claude, &path, None, Some(items[0].seq), 50).unwrap();
        assert_eq!(older.len(), 50);
        assert!(more);
        assert!(older.last().unwrap().seq < items[0].seq);
        assert!(older.last().unwrap().text.as_deref().unwrap().starts_with("prompt 249 "));
        // Reading again gives the same seqs.
        let (again, _) = collect(AgentId::Claude, &path, None, None, 50).unwrap();
        assert_eq!(
            again.iter().map(|i| i.seq).collect::<Vec<_>>(),
            items.iter().map(|i| i.seq).collect::<Vec<_>>()
        );
        // The very start: nothing more before it.
        let (all, more) = collect(AgentId::Claude, &path, None, None, 400).unwrap();
        assert_eq!(all.len(), 300);
        assert!(!more);
    }

    #[test]
    fn a_paged_tool_step_gets_its_later_result() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let lines = fixture_lines("claude-idle.jsonl");
        std::fs::copy(fixture_path("claude-idle.jsonl"), &path).unwrap();
        // Page before the tool's result: the step comes back finished,
        // because the result line (past the page's end) was read too.
        let result =
            lines.iter().find(|l| String::from_utf8_lossy(&l.bytes).contains("tool_result")).unwrap();
        let (items, _) =
            collect(AgentId::Claude, &path, None, Some(block_seq(result.offset, 0)), 50).unwrap();
        assert_eq!(items.len(), 2, "the prompt and the step");
        let tool = items[1].tool.as_ref().unwrap();
        assert_eq!(tool.status, ToolStatus::Ok);
    }

    #[tokio::test]
    async fn a_watch_sends_appended_items_and_a_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::copy(fixture_path("claude-open-tool.jsonl"), &path).unwrap();
        let c = Ctx { path: path.clone(), ..ctx(AgentId::Claude, true, None) };
        let mut watch = new_watch(&c).unwrap();
        assert_eq!(watch.items.len(), 3);
        assert!(poll_watch(&c, &mut watch).is_empty());
        let open_id = watch.items.values().last().unwrap().id.clone();
        let result = format!(
            "{{\"type\":\"user\",\"uuid\":\"r1\",\"cwd\":\"/home/me/code/app\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"tool_result\",\"tool_use_id\":\"{open_id}\",\"content\":\"done\"}}]}}}}\n"
        );
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(result.as_bytes()).unwrap();
        let changed = poll_watch(&c, &mut watch);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].rev, 1);
        assert_eq!(changed[0].tool.as_ref().unwrap().status, ToolStatus::Ok);
        // Rewritten in place with one line changed: only that item is sent again.
        let mut body = std::fs::read_to_string(&path).unwrap();
        body = body.replacen("Write a short note.", "Write a brief note.", 1);
        let tmp = dir.path().join("new.jsonl");
        std::fs::write(&tmp, body).unwrap();
        std::fs::rename(&tmp, &path).unwrap();
        let changed = poll_watch(&c, &mut watch);
        assert_eq!(changed.len(), 1, "{changed:?}");
        assert_eq!(changed[0].text.as_deref(), Some("Write a brief note."));
        assert_eq!(changed[0].rev, 1);
    }

    use std::io::Write as _;

    #[tokio::test]
    async fn nothing_is_read_without_a_subscriber() {
        let dir = tempfile::tempdir().unwrap();
        let host = test_host(dir.path(), APPROVAL_LIFETIME_MS);
        host.sessions_wake();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!host.sessions.lock().running, "the loop stopped at once: nobody wants sessions");
        assert!(host.sessions.lock().listed_at.is_none(), "and read nothing");
    }

    #[tokio::test]
    async fn following_off_lists_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let host = test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let s = host.sessions_set_following(false).await.unwrap();
        assert!(!s.follow_sessions);
        assert!(host.sessions_list(50).await.is_empty());
        assert!(host.sessions.lock().listed_at.is_none());
        assert_eq!(
            host.session_read(SessionReadParams {
                session_id: format!("sc{}", "a".repeat(32)),
                ..Default::default()
            })
            .await
            .unwrap_err()
            .code,
            ErrorCode::NotFound
        );
        assert!(host.sessions_set_following(true).await.unwrap().follow_sessions);
    }
}
