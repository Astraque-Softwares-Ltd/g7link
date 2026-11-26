// SPDX-License-Identifier: MPL-2.0
//! Copies of an agent's session (§U6.2, reconcile §5): what rewind,
//! edit-from-here and fork continue from.
//!
//! Codex copies a thread with `thread/fork { lastTurnId }` (stable schema):
//! at once, from a short-lived `app-server`, for a rewind; or from the
//! thread's next process, for a fork. Claude continues a copy with
//! `--resume <x> --fork-session` [docs: cli-reference, headless, sessions],
//! where `x` is the session's id, or the absolute path of a cut copy of its
//! transcript: every byte before the line whose `uuid` is the mark, copied
//! verbatim into a 0600 file in the host's runtime directory. Claude names
//! the copy only when its first message runs [spike S-3.6], so a Claude copy
//! is always made by the thread's next process; a rewind first checks that
//! Claude can open the cut.
//!
//! The source session is only read: G7 never writes, moves or deletes a
//! transcript or a rollout, nor anything under the CLIs' own directories.

use std::ffi::OsString;
use std::future::Future;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::api::{AgentId, Can, ErrorCode, Mode, RpcError};
use crate::host::agents::{AgentProcess, OutLine, Probe};
use crate::host::claude::{self, CutFile};
use crate::host::codex;
use crate::host::db::TurnRow;
use crate::host::sessions::{paths, tail};
use crate::host::threads::Entry;
use crate::host::{Host, env};

/// The runtime directory's folder for cut copies and the check's config.
pub const CUT_DIR: &str = "cut";
/// The largest transcript a cut is made from.
pub const MAX_TRANSCRIPT: u64 = 1 << 30;
/// A line longer than this is copied as it is and never taken for a mark.
const MAX_LINE: usize = 256 << 20;
/// A native mark's length at most.
pub const MAX_MARK: usize = 64;
/// Project folders looked through for a transcript not where its folder's
/// name says.
const MAX_PROJECT_FOLDERS: usize = 4096;
/// Entries of the cut folder removed at host start, at most.
const MAX_SWEPT: usize = 4096;
/// How long Claude has to answer the check's `initialize`: it took 0.75 s
/// and a bad copy failed in 0.5 s [spike S-3.7, 3.8]; a loaded laptop and
/// plugins get room.
const CHECK_WAIT: Duration = Duration::from_secs(15);
/// The check's request id.
const CHECK_REQUEST: &str = "g7-fork-check";
/// How long a Codex `app-server` has to start and copy a thread.
const CODEX_WAIT: Duration = Duration::from_secs(30);

// ---- Marks ----

/// A mark as an agent writes it: 1 to [`MAX_MARK`] ASCII letters, digits,
/// `-` and `_` (Claude's line `uuid`, Codex's turn id).
pub fn is_mark(mark: &str) -> bool {
    !mark.is_empty()
        && mark.len() <= MAX_MARK
        && mark.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Where a copy of a session ends, in the agent's own words. Claude: the
/// mark of the first turn the copy leaves out (the cut goes before its
/// line). Codex: the mark of the last turn it keeps (`lastTurnId`). Made by
/// [`through`], which knows which is which.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NativeMark(String);

impl NativeMark {
    pub fn new(mark: &str) -> Option<Self> {
        is_mark(mark).then(|| Self(mark.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NativeMark {
    type Error = &'static str;

    fn try_from(mark: String) -> Result<Self, Self::Error> {
        if is_mark(&mark) { Ok(Self(mark)) } else { Err("not a native mark") }
    }
}

impl From<NativeMark> for String {
    fn from(mark: NativeMark) -> Self {
        mark.0
    }
}

/// Where a copy that keeps turns `1..=last` ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Through {
    /// The whole session: `last` is the newest turn (Claude), or Codex has
    /// no mark for it and it is the newest.
    Latest,
    /// At a mark ([`NativeMark`]).
    Mark(NativeMark),
}

impl Through {
    /// What [`fork_native`] and [`launch_hint`] take.
    pub fn mark(&self) -> Option<&NativeMark> {
        match self {
            Self::Latest => None,
            Self::Mark(mark) => Some(mark),
        }
    }
}

/// Where a copy that keeps the thread's turns `1..=last` ends, from its
/// turns (`turns`: at least `last` and every turn after it, in any order;
/// cut ones are skipped). `None` when it can't be made: `last` is not one of
/// them, or a turn whose mark it needs has none (a turn G7 didn't run, §U6.2).
pub fn through(agent: AgentId, turns: &[TurnRow], last: u32) -> Option<Through> {
    let live = || turns.iter().filter(|t| !t.cut);
    let mark_of = |t: &TurnRow| t.native_mark.as_deref().and_then(NativeMark::new);
    let kept = live().find(|t| t.turn == last)?;
    let next = live().filter(|t| t.turn > last).min_by_key(|t| t.turn);
    match agent {
        AgentId::Claude => match next {
            None => Some(Through::Latest),
            Some(next) => mark_of(next).map(Through::Mark),
        },
        AgentId::Codex => match (mark_of(kept), next) {
            (Some(mark), _) => Some(Through::Mark(mark)),
            (None, None) => Some(Through::Latest),
            (None, Some(_)) => None,
        },
    }
}

// ---- What a fork hands the host ----

/// How a thread's next process starts from a copy of a session (§U6.5's
/// lazy fork, and a Claude rewind). The host keeps it with the thread until
/// that process names the copy ([`crate::host::agents::Step::NativeId`]),
/// across restarts ([`LaunchHint::encode`]). Never sent to a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchHint {
    pub agent: AgentId,
    /// The session the copy is made from.
    pub from: String,
    /// Where the copy ends; absent: the whole session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<NativeMark>,
}

impl LaunchHint {
    /// `None` when `from` isn't a session id of `agent`.
    pub fn new(agent: AgentId, from: &str, mark: Option<&NativeMark>) -> Option<Self> {
        is_native_id(agent, from).then(|| Self { agent, from: from.to_owned(), mark: mark.cloned() })
    }

    /// One line of JSON, for the thread's row.
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// What [`LaunchHint::encode`] wrote; `None` for anything else.
    pub fn decode(text: &str) -> Option<Self> {
        let hint: Self = serde_json::from_str(text).ok()?;
        is_native_id(hint.agent, &hint.from).then_some(hint)
    }
}

/// A session id as the agent names one: Claude's lowercase UUID (its
/// transcript's name), Codex's thread id.
pub fn is_native_id(agent: AgentId, id: &str) -> bool {
    match agent {
        AgentId::Claude => paths::is_uuid(id),
        AgentId::Codex => is_mark(id),
    }
}

/// A copy of an agent's session, as [`fork_native`] made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Forked {
    /// The copy exists: its native id (Codex).
    Now(String),
    /// The thread's next process makes it (Claude, after checking that the
    /// copy opens).
    AtNextStart(LaunchHint),
}

/// What the agent can do with copies of its sessions (`AgentInfo.can`):
/// both agents copy from the latest message and from any marked turn
/// [spike S-3, S-5, S-6], when the probe found a CLI that can run.
pub fn can(agent: AgentId, probe: &Probe) -> &'static [Can] {
    let runs =
        probe.info.id == agent && probe.info.installed && probe.bin.is_some() && probe.info.problem.is_none();
    if runs { &[Can::Fork, Can::ForkAt] } else { &[] }
}

// ---- What the host calls ----

/// The thread's session, copied to end at `through` (absent: the whole
/// session), for a rewind (§U6.3 step 3). Codex: `thread/fork` now, from a
/// short-lived `app-server` (no model call), and the copy's id. Claude: the
/// cut is made and Claude is asked to open it (no message, so no model call
/// and no file written [spike S-3.7]); the copy itself is made by the
/// thread's next process.
///
/// The returned future borrows `host` only, so the caller can take what it
/// needs under the registry lock and release it before awaiting.
pub fn fork_native<'h>(
    host: &'h Host,
    entry: &Entry,
    through: Option<&NativeMark>,
) -> impl Future<Output = Result<Forked, RpcError>> + Send + 'h {
    let job = Job::of(host, entry, through);
    async move {
        let job = job?;
        let probe = host.agent_info(job.hint.agent).await;
        let bin = probe.bin.filter(|_| probe.info.installed).ok_or_else(|| missing(job.hint.agent))?;
        match job.hint.agent {
            AgentId::Codex => codex_fork(&bin, &job).await.map(Forked::Now),
            AgentId::Claude => {
                claude_check(&bin, &job).await?;
                Ok(Forked::AtNextStart(job.hint))
            }
        }
    }
}

/// The hint for a fork made lazily (§U6.5, reconcile §5): nothing is run
/// and nothing is written until the new thread's first message.
pub fn launch_hint(entry: &Entry, through: Option<&NativeMark>) -> Result<LaunchHint, RpcError> {
    let native = entry.info.native_id.as_deref().ok_or_else(nothing_yet)?;
    LaunchHint::new(entry.info.agent, native, through).ok_or_else(nothing_yet)
}

/// What [`claude::Launch::fork`] carries for a thread whose pending copy
/// is `encoded` ([`LaunchHint::encode`]).
pub fn launch_fork(config: &crate::host::Config, encoded: &str) -> claude::ForkFrom {
    claude::ForkFrom {
        hint: encoded.to_owned(),
        claude_home: config.claude_home.clone(),
        cut_dir: config.runtime_dir.join(CUT_DIR),
    }
}

/// How a Codex thread whose pending copy is `encoded` starts:
/// [`codex::CodexDriver::forking`] with this. `None`: not a Codex copy.
pub fn codex_fork_start(encoded: &str) -> Option<codex::ForkStart> {
    let hint = LaunchHint::decode(encoded).filter(|h| h.agent == AgentId::Codex)?;
    Some(codex::ForkStart { last_turn: hint.mark.map(String::from), from: hint.from })
}

/// Claude's start from a pending copy in `cwd`: the session's id, or a cut
/// copy of its transcript when the copy ends at a mark. Blocking (it reads
/// the transcript, at most [`MAX_TRANSCRIPT`]); [`claude::spawn_session`]
/// calls it. The error's message is for the user.
pub fn claude_fork_start(from: &claude::ForkFrom, cwd: &Path) -> io::Result<claude::ForkStart> {
    let hint = LaunchHint::decode(&from.hint)
        .filter(|h| h.agent == AgentId::Claude)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "This copy isn't Claude's."))?;
    cut_start(&hint, cwd, &from.claude_home, &from.cut_dir).map_err(|e| {
        let kind = match &e {
            CutError::Io(io) => io.kind(),
            CutError::NoTranscript | CutError::NoMark => io::ErrorKind::NotFound,
            CutError::Twice | CutError::TooLarge => io::ErrorKind::InvalidData,
        };
        io::Error::new(kind, e.rpc().message)
    })
}

/// The cut (or the session's id when the copy is the whole session),
/// ready for `--resume`. Blocking.
fn cut_start(
    hint: &LaunchHint,
    cwd: &Path,
    claude_home: &Path,
    cut_dir: &Path,
) -> Result<claude::ForkStart, CutError> {
    let from = hint.from.clone();
    let Some(mark) = &hint.mark else {
        return Ok(claude::ForkStart { resume: from.clone(), from, cut: None });
    };
    let transcript = find_transcript(claude_home, cwd, &from).ok_or(CutError::NoTranscript)?;
    let cut = make_cut(&transcript, mark, cut_dir)?;
    // `--resume` takes the copy by its absolute path [docs: cli-reference].
    let resume =
        cut.path().to_str().filter(|_| cut.path().is_absolute()).map(str::to_owned).ok_or_else(|| {
            CutError::Io(io::Error::new(io::ErrorKind::InvalidInput, "not an absolute UTF-8 path"))
        })?;
    Ok(claude::ForkStart { from, resume, cut: Some(cut) })
}

/// Removes cut copies and check configs a host before this one left (§U6.2
/// step 4: "in any case … when the host starts"). Only names this module
/// makes, only regular files.
pub fn sweep(runtime_dir: &Path) {
    let dir = runtime_dir.join(CUT_DIR);
    if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return;
    }
    for (name, path) in tail::list_dir(&dir, MAX_SWEPT) {
        let ours = claude::is_cut_name(&name) || is_check_config_name(&name);
        if ours && std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

// ---- One job ----

/// What a copy needs, taken from the thread and the host's config.
#[derive(Debug, Clone)]
struct Job {
    hint: LaunchHint,
    cwd: PathBuf,
    mode: Mode,
    env: Vec<(OsString, OsString)>,
    cut_dir: PathBuf,
    claude_home: PathBuf,
}

impl Job {
    fn new(host: &Host, cwd: PathBuf, mode: Mode, hint: LaunchHint) -> Self {
        Self {
            env: env::child_env(hint.agent, host.config.env.iter().cloned()),
            hint,
            cwd,
            mode,
            cut_dir: host.config.runtime_dir.join(CUT_DIR),
            claude_home: host.config.claude_home.clone(),
        }
    }

    fn of(host: &Host, entry: &Entry, through: Option<&NativeMark>) -> Result<Self, RpcError> {
        let hint = launch_hint(entry, through)?;
        let cwd = entry.cwd.clone().ok_or_else(|| {
            RpcError::new(ErrorCode::Unavailable, "This thread's folder isn't one of the agent folders.")
        })?;
        Ok(Self::new(host, cwd, entry.info.mode, hint))
    }
}

fn missing(agent: AgentId) -> RpcError {
    RpcError::new(ErrorCode::Unavailable, format!("{} isn't available.", agent.display_name()))
}

fn nothing_yet() -> RpcError {
    RpcError::new(ErrorCode::Unavailable, "There's no conversation to copy yet.")
}

fn not_found(agent: AgentId) -> RpcError {
    RpcError::new(ErrorCode::NotFound, format!("{} no longer has that message.", agent.display_name()))
}

fn copy_failed(agent: AgentId) -> RpcError {
    RpcError::new(
        ErrorCode::Unavailable,
        format!("{} couldn't open a copy of this conversation.", agent.display_name()),
    )
}

// ---- Claude: the cut ----

/// Why a cut wasn't made.
#[derive(Debug)]
pub enum CutError {
    /// The session's transcript isn't there.
    NoTranscript,
    /// No line has the mark as its own `uuid`.
    NoMark,
    /// Two lines do: which one is meant can't be told.
    Twice,
    /// The transcript is over [`MAX_TRANSCRIPT`].
    TooLarge,
    Io(io::Error),
}

impl From<io::Error> for CutError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl CutError {
    fn rpc(&self) -> RpcError {
        match self {
            Self::NoTranscript | Self::NoMark => not_found(AgentId::Claude),
            Self::Twice => RpcError::new(ErrorCode::Unavailable, "Claude's session has that message twice."),
            Self::TooLarge => RpcError::new(ErrorCode::Unavailable, "Claude's session is too large to copy."),
            Self::Io(_) => RpcError::new(ErrorCode::Internal, "Couldn't copy Claude's session."),
        }
    }
}

/// A working directory as Claude names its project folder: every character
/// but an ASCII letter or digit becomes `-`, one per UTF-16 unit
/// [docs: sessions, "Where transcripts are stored"; spike S-3.1].
pub fn project_folder(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .flat_map(|c| {
            let n = if c.is_ascii_alphanumeric() { 1 } else { c.len_utf16() };
            std::iter::repeat_n(if c.is_ascii_alphanumeric() { c } else { '-' }, n)
        })
        .collect()
}

/// The transcript of Claude's session `session` run in `cwd`:
/// `<claude home>/projects/<project folder>/<session>.jsonl`, else the same
/// name in another project folder (a long path's folder is named
/// differently). Folders and the file are taken only when they are not
/// links; the file is opened later without following one.
pub fn find_transcript(claude_home: &Path, cwd: &Path, session: &str) -> Option<PathBuf> {
    if !paths::is_uuid(session) {
        return None;
    }
    let projects = claude_home.join("projects");
    let name = format!("{session}.jsonl");
    let is_dir = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir());
    let is_file = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file());
    if !is_dir(&projects) {
        return None;
    }
    let derived = projects.join(project_folder(cwd));
    if is_dir(&derived) && is_file(&derived.join(&name)) {
        return Some(derived.join(name));
    }
    tail::list_dir(&projects, MAX_PROJECT_FOLDERS)
        .into_iter()
        .map(|(_, folder)| folder)
        .filter(|folder| is_dir(folder))
        .map(|folder| folder.join(&name))
        .find(|path| is_file(path))
}

/// The top-level `uuid` of a JSON line, when it is `mark`. Lines that don't
/// hold the mark's bytes at all are not parsed.
fn has_mark(line: &[u8], mark: &str) -> bool {
    #[derive(Deserialize)]
    struct Head {
        uuid: Option<Value>,
    }
    let needle = mark.as_bytes();
    // An object only: serde would also read a struct from an array.
    let object = line.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{');
    if !object || !line.windows(needle.len()).any(|w| w == needle) {
        return false;
    }
    serde_json::from_slice::<Head>(line).ok().and_then(|h| h.uuid).is_some_and(|u| u.as_str() == Some(mark))
}

/// Copies every byte of `transcript` before the line whose own `uuid` is
/// `mark` into a new cut file in `dir` (§U6.2 step 2). Lines are copied
/// verbatim; nothing else of them is read. The mark must be on exactly one
/// line. Blocking: run it with `spawn_blocking`.
pub fn make_cut(transcript: &Path, mark: &NativeMark, dir: &Path) -> Result<CutFile, CutError> {
    let (file, meta) = tail::open_regular(transcript).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => CutError::NoTranscript,
        _ => CutError::Io(e),
    })?;
    if meta.len() > MAX_TRANSCRIPT {
        return Err(CutError::TooLarge);
    }
    crate::client::ensure_private_dir(dir).map_err(|e| CutError::Io(io::Error::other(e.to_string())))?;
    let (cut, out) = CutFile::create(dir)?;
    let mut out = io::BufWriter::with_capacity(256 << 10, out);
    let mut reader = BufReader::with_capacity(256 << 10, file.take(MAX_TRANSCRIPT + 1));
    let mark = mark.as_str();
    let mut found = false;
    let mut line: Vec<u8> = Vec::new();
    // A line past MAX_LINE is copied in pieces and never taken for the mark.
    let mut long = false;
    let mut total: u64 = 0;
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            break;
        }
        let (take, ends) = match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (chunk.len(), false),
        };
        total += take as u64;
        if total > MAX_TRANSCRIPT {
            return Err(CutError::TooLarge);
        }
        if long {
            if !found {
                out.write_all(&chunk[..take])?;
            }
        } else {
            line.extend_from_slice(&chunk[..take]);
            if line.len() > MAX_LINE {
                if !found {
                    out.write_all(&line)?;
                }
                line.clear();
                long = true;
            }
        }
        reader.consume(take);
        if ends {
            if !long {
                take_line(&line, mark, &mut found, &mut out)?;
            }
            line.clear();
            long = false;
        }
    }
    if !long && !line.is_empty() {
        take_line(&line, mark, &mut found, &mut out)?;
    }
    if !found {
        return Err(CutError::NoMark);
    }
    out.into_inner().map_err(io::IntoInnerError::into_error)?.sync_all()?;
    Ok(cut)
}

fn take_line(line: &[u8], mark: &str, found: &mut bool, out: &mut impl Write) -> Result<(), CutError> {
    if has_mark(line, mark) {
        if *found {
            return Err(CutError::Twice);
        }
        *found = true;
    } else if !*found {
        out.write_all(line)?;
    }
    Ok(())
}

/// [`cut_start`] off the async threads.
async fn claude_start(job: &Job) -> Result<claude::ForkStart, RpcError> {
    let (hint, cwd, home, dir) =
        (job.hint.clone(), job.cwd.clone(), job.claude_home.clone(), job.cut_dir.clone());
    tokio::task::spawn_blocking(move || cut_start(&hint, &cwd, &home, &dir))
        .await
        .map_err(|_| RpcError::new(ErrorCode::Internal, "Couldn't copy Claude's session."))?
        .map_err(|e| {
            if let CutError::Io(io) = &e {
                tracing::warn!("couldn't cut a transcript: {}", io.kind());
            }
            e.rpc()
        })
}

const CHECK_CONFIG_PREFIX: &str = "check-";

/// A name of the check's MCP config: `check-<16 hex>.json`.
fn is_check_config_name(name: &str) -> bool {
    name.strip_prefix(CHECK_CONFIG_PREFIX).and_then(|n| n.strip_suffix(".json")).is_some_and(|hex| {
        hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// A file removed when dropped.
#[derive(Debug)]
struct Removed(PathBuf);

impl Drop for Removed {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// An empty MCP config for the check, 0600 (Claude starts no MCP server).
fn check_config(dir: &Path) -> io::Result<Removed> {
    use std::os::unix::fs::OpenOptionsExt;
    crate::client::ensure_private_dir(dir).map_err(|e| io::Error::other(e.to_string()))?;
    let name: u64 = rand::random();
    let path = dir.join(format!("{CHECK_CONFIG_PREFIX}{name:016x}.json"));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    let removed = Removed(path);
    file.write_all(br#"{"mcpServers":{}}"#)?;
    Ok(removed)
}

/// Asks Claude to open the copy (reconcile §5): opened = `initialize`
/// answered and the process alive; failed = a `result` (Claude's
/// `error_during_execution`) or an exit first. The cut is removed after.
async fn claude_check(bin: &Path, job: &Job) -> Result<(), RpcError> {
    let start = claude_start(job).await?;
    let config = check_config(&job.cut_dir)
        .map_err(|_| RpcError::new(ErrorCode::Internal, "Couldn't copy Claude's session."))?;
    let args = claude::check_args(&config.0, &start.resume);
    let (mut process, mut lines) = AgentProcess::spawn(bin, &args, &job.cwd, &job.env).map_err(|e| {
        tracing::warn!("couldn't start Claude to check a copy: {}", e.kind());
        copy_failed(AgentId::Claude)
    })?;
    let answered = process.write_line(&claude::initialize_line(CHECK_REQUEST)).await
        && tokio::time::timeout(CHECK_WAIT, check_verdict(&mut lines)).await.unwrap_or(false);
    let alive = answered && process.child.try_wait().is_ok_and(|status| status.is_none());
    // No message was written: closing stdin ends it [spike S-3.7].
    let _ = process.finish().await;
    drop(start);
    if alive { Ok(()) } else { Err(copy_failed(AgentId::Claude)) }
}

async fn check_verdict(lines: &mut mpsc::Receiver<OutLine>) -> bool {
    while let Some(line) = lines.recv().await {
        let OutLine::Line(bytes) = line else { return false };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else { continue };
        if let Some(verdict) = claude::check_answer(&value, CHECK_REQUEST) {
            return verdict;
        }
    }
    false
}

// ---- Codex: thread/fork ----

/// One `thread/fork` exchange with a fresh `app-server` [spike S-5.2]:
/// `initialize` (with the experimental API as the adapter asks, without it
/// when refused), `initialized`, then the fork. Requests Codex sends are
/// refused; notifications (the fork's own `thread/started` among them) are
/// ignored.
#[derive(Debug)]
pub struct CodexFork {
    params: Value,
    forked: bool,
}

/// What [`CodexFork`] wants after a line.
#[derive(Debug, PartialEq)]
pub enum CodexStep {
    /// Write these lines and read on.
    Write(Vec<Value>),
    /// The copy's thread id.
    Done(String),
    Failed(RpcError),
}

const INIT_ID: u64 = 1;
const PLAIN_INIT_ID: u64 = 2;
const FORK_ID: u64 = 3;

impl CodexFork {
    pub fn new(params: Value) -> Self {
        Self { params, forked: false }
    }

    /// The first line.
    pub fn start(&self) -> Value {
        codex::initialize_request_with(INIT_ID, true)
    }

    pub fn on_line(&mut self, line: &Value) -> CodexStep {
        let id = line.get("id").filter(|id| !id.is_null());
        if line.get("method").is_some() {
            return match id {
                Some(id) => CodexStep::Write(vec![
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "not supported"}}),
                ]),
                None => CodexStep::Write(Vec::new()),
            };
        }
        let error = line.get("error");
        match id.and_then(Value::as_u64) {
            Some(INIT_ID) if error.is_some() => {
                CodexStep::Write(vec![codex::initialize_request_with(PLAIN_INIT_ID, false)])
            }
            Some(INIT_ID | PLAIN_INIT_ID) if error.is_none() && !self.forked => {
                self.forked = true;
                CodexStep::Write(vec![
                    codex::initialized(),
                    codex::fork_request(FORK_ID, self.params.clone()),
                ])
            }
            Some(PLAIN_INIT_ID) if error.is_some() => CodexStep::Failed(copy_failed(AgentId::Codex)),
            Some(FORK_ID) => match error {
                Some(e) if codex::is_turn_not_found(e) => CodexStep::Failed(not_found(AgentId::Codex)),
                Some(e) => {
                    let message = e.get("message").and_then(Value::as_str).unwrap_or_default();
                    tracing::warn!("Codex refused a fork: {}", crate::api::cut(message, 200));
                    CodexStep::Failed(copy_failed(AgentId::Codex))
                }
                None => match line.pointer("/result/thread/id").and_then(Value::as_str) {
                    Some(id) if is_native_id(AgentId::Codex, id) => CodexStep::Done(id.to_owned()),
                    _ => CodexStep::Failed(copy_failed(AgentId::Codex)),
                },
            },
            _ => CodexStep::Write(Vec::new()),
        }
    }
}

async fn codex_fork(bin: &Path, job: &Job) -> Result<String, RpcError> {
    let cwd = job.cwd.to_string_lossy();
    let last = job.hint.mark.as_ref().map(NativeMark::as_str);
    let mut fork = CodexFork::new(codex::fork_params(&job.hint.from, last, &cwd, job.mode));
    let (mut process, mut lines) = AgentProcess::spawn(bin, &["app-server".to_owned()], &job.cwd, &job.env)
        .map_err(|e| {
        tracing::warn!("couldn't start Codex to fork: {}", e.kind());
        missing(AgentId::Codex)
    })?;
    let exchange = async {
        if !process.write_line(&fork.start().to_string()).await {
            return Err(copy_failed(AgentId::Codex));
        }
        while let Some(line) = lines.recv().await {
            let OutLine::Line(bytes) = line else { break };
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else { continue };
            match fork.on_line(&value) {
                CodexStep::Write(out) => {
                    for line in out {
                        if !process.write_line(&line.to_string()).await {
                            return Err(copy_failed(AgentId::Codex));
                        }
                    }
                }
                CodexStep::Done(id) => return Ok(id),
                CodexStep::Failed(e) => return Err(e),
            }
        }
        Err(copy_failed(AgentId::Codex))
    };
    let result =
        tokio::time::timeout(CODEX_WAIT, exchange).await.unwrap_or_else(|_| Err(copy_failed(AgentId::Codex)));
    let _ = process.finish().await;
    result
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::api::AgentInfo;

    const SESSION: &str = "5e55e55e-0000-4000-8000-000000000001";
    const A1: &str = "a1111111-1111-4111-8111-111111111111";
    const B1: &str = "b1111111-1111-4111-8111-111111111111";
    const C1: &str = "c1111111-1111-4111-8111-111111111111";
    const CODEX_COPY: &str = "019a7c10-0000-7000-8000-00000000c001";

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
    }

    /// The fixture's lines, each with its newline.
    fn transcript_lines() -> Vec<Vec<u8>> {
        fixture("claude-20-transcript.jsonl").split_inclusive(|&b| b == b'\n').map(<[u8]>::to_vec).collect()
    }

    /// The bytes before the line that has `"uuid":"<mark>"` as written.
    fn before_mark(bytes: &[u8], mark: &str) -> Vec<u8> {
        let text = std::str::from_utf8(bytes).unwrap();
        let at = text.find(&format!("\"uuid\":\"{mark}\"")).unwrap();
        bytes[..text[..at].rfind('\n').map_or(0, |i| i + 1)].to_vec()
    }

    fn mark(text: &str) -> NativeMark {
        NativeMark::new(text).unwrap()
    }

    fn cut_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn marks() {
        for good in [A1, "019a7c10-0000-7000-8000-00000000b001", "tu_1", &"x".repeat(MAX_MARK)] {
            assert!(is_mark(good), "{good}");
        }
        for bad in ["", "a/b", "../x", "a b", "é", "a\n", &"x".repeat(MAX_MARK + 1)] {
            assert!(!is_mark(bad), "{bad:?}");
            assert!(NativeMark::new(bad).is_none());
        }
        assert_eq!(serde_json::to_value(mark(A1)).unwrap(), json!(A1));
        assert!(serde_json::from_value::<NativeMark>(json!("../x")).is_err());
    }

    #[test]
    fn hints_round_trip_and_refuse_what_isnt_a_session() {
        let hint = LaunchHint::new(AgentId::Claude, SESSION, Some(&mark(B1))).unwrap();
        let text = hint.encode();
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({"agent": "claude", "from": SESSION, "mark": B1})
        );
        assert_eq!(LaunchHint::decode(&text), Some(hint));
        let whole = LaunchHint::new(AgentId::Codex, "019a7c10-0000-7000-8000-00000000a001", None).unwrap();
        assert_eq!(LaunchHint::decode(&whole.encode()), Some(whole));
        // Claude's sessions are lowercase UUIDs: never a path, never a name.
        for from in [
            "/run/user/1000/g7-agents/cut/0123456789abcdef.jsonl",
            "../x",
            "5E55E55E-0000-4000-8000-000000000001",
            "",
        ] {
            assert!(LaunchHint::new(AgentId::Claude, from, None).is_none(), "{from}");
        }
        for text in [
            r#"{"agent":"claude","from":"../x"}"#,
            r#"{"agent":"codex","from":"a b"}"#,
            r#"{"agent":"claude","from":"5e55e55e-0000-4000-8000-000000000001","mark":"a/b"}"#,
            r#"{"agent":"gemini","from":"x"}"#,
            "not json",
        ] {
            assert!(LaunchHint::decode(text).is_none(), "{text}");
        }
    }

    fn turn(n: u32, mark: Option<&str>, cut: bool) -> TurnRow {
        TurnRow { native_mark: mark.map(str::to_owned), cut, ..TurnRow::new(n, u64::from(n) * 10, 0) }
    }

    #[test]
    fn where_a_copy_ends() {
        let turns = [turn(1, Some("m1"), false), turn(2, Some("m2"), false), turn(3, Some("m3"), false)];
        let at = |m: &str| Some(Through::Mark(mark(m)));
        // Claude cuts before the next turn's line; Codex keeps through the turn.
        assert_eq!(through(AgentId::Claude, &turns, 1), at("m2"));
        assert_eq!(through(AgentId::Codex, &turns, 1), at("m1"));
        assert_eq!(through(AgentId::Claude, &turns, 3), Some(Through::Latest));
        assert_eq!(through(AgentId::Codex, &turns, 3), at("m3"));
        assert_eq!(through(AgentId::Claude, &turns, 9), None, "no such turn");
        assert_eq!(through(AgentId::Claude, &turns, 0), None, "before turn 1 is no copy");
        // Turns G7 didn't run (brought in) have no marks.
        let brought = [
            turn(1, None, false),
            turn(2, None, false),
            turn(3, Some("m3"), false),
            turn(4, Some("m4"), false),
        ];
        assert_eq!(through(AgentId::Claude, &brought, 1), None);
        assert_eq!(through(AgentId::Codex, &brought, 1), None);
        assert_eq!(through(AgentId::Claude, &brought, 2), at("m3"), "Claude only needs the next turn's mark");
        assert_eq!(through(AgentId::Codex, &brought, 2), None);
        assert_eq!(through(AgentId::Codex, &brought, 3), at("m3"));
        // From the latest message, without marks: the whole session.
        assert_eq!(through(AgentId::Codex, &brought[..2], 2), Some(Through::Latest));
        assert_eq!(through(AgentId::Claude, &brought[..2], 2), Some(Through::Latest));
        // Cut turns are not in the session any more.
        let rewound = [
            turn(1, Some("m1"), false),
            turn(2, Some("m2"), true),
            turn(3, Some("m3"), true),
            turn(4, Some("m4"), false),
        ];
        assert_eq!(through(AgentId::Claude, &rewound, 1), at("m4"));
        assert_eq!(through(AgentId::Claude, &rewound, 2), None);
        assert_eq!(through(AgentId::Codex, &rewound, 4), at("m4"));
        // A mark that isn't one is no mark.
        let odd = [turn(1, Some("m1"), false), turn(2, Some("../x"), false)];
        assert_eq!(through(AgentId::Claude, &odd, 1), None);
        assert_eq!(Through::Latest.mark(), None);
        assert_eq!(at("m1").unwrap().mark(), Some(&mark("m1")));
    }

    #[test]
    fn project_folders() {
        assert_eq!(project_folder(Path::new("/home/u/src/g7")), "-home-u-src-g7");
        assert_eq!(
            project_folder(Path::new("/run/media/u/Local Disk/x_y.z")),
            "-run-media-u-Local-Disk-x-y-z"
        );
        // One `-` per UTF-16 unit: "é" is one, "😀" two.
        assert_eq!(project_folder(Path::new("/tmp/é/😀")), "-tmp-----");
    }

    /// `<home>/projects/<folder>/<SESSION>.jsonl` with `bytes`.
    fn place(home: &Path, folder: &str, bytes: &[u8]) -> PathBuf {
        let dir = home.join("projects").join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{SESSION}.jsonl"));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn transcripts_are_found_by_name_and_never_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("claude");
        let cwd = Path::new("/work/g7");
        assert_eq!(find_transcript(&home, cwd, SESSION), None);
        let derived = place(&home, &project_folder(cwd), b"{}\n");
        assert_eq!(find_transcript(&home, cwd, SESSION), Some(derived.clone()));
        assert_eq!(find_transcript(&home, cwd, &SESSION.to_uppercase()), None);
        assert_eq!(find_transcript(&home, cwd, "../../x"), None);
        // A long path's folder is named otherwise: looked for by name.
        std::fs::remove_dir_all(derived.parent().unwrap()).unwrap();
        let elsewhere = place(&home, "-work-g7-1a2b3c", b"{}\n");
        assert_eq!(find_transcript(&home, cwd, SESSION), Some(elsewhere.clone()));
        // A linked folder or a linked file is not taken.
        std::fs::remove_dir_all(elsewhere.parent().unwrap()).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join(format!("{SESSION}.jsonl")), b"{}\n").unwrap();
        std::os::unix::fs::symlink(&outside, home.join("projects").join(project_folder(cwd))).unwrap();
        assert_eq!(find_transcript(&home, cwd, SESSION), None);
        let real = home.join("projects").join("-real");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(
            outside.join(format!("{SESSION}.jsonl")),
            real.join(format!("{SESSION}.jsonl")),
        )
        .unwrap();
        assert_eq!(find_transcript(&home, cwd, SESSION), None);
    }

    #[test]
    fn the_cut_is_every_byte_before_the_marks_line() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = fixture("claude-20-transcript.jsonl");
        let transcript = dir.path().join("t.jsonl");
        std::fs::write(&transcript, &bytes).unwrap();
        let cuts = dir.path().join("run").join(CUT_DIR);
        let lines = transcript_lines();
        // B1 is named in an earlier command and in the next line's
        // `parentUuid`: only its own line's `uuid` counts.
        for (m, kept) in [(A1, 3), (B1, 12), (C1, 17)] {
            let cut = make_cut(&transcript, &mark(m), &cuts).unwrap();
            let got = std::fs::read(cut.path()).unwrap();
            assert_eq!(got, lines[..kept].concat(), "{m}");
            assert_eq!(got, before_mark(&bytes, m), "{m}");
            assert_eq!(std::fs::metadata(cut.path()).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&cuts).unwrap().permissions().mode() & 0o777, 0o700);
            assert!(cut.path().is_absolute());
        }
        assert!(cut_files(&cuts).is_empty(), "each cut went with its guard");
        assert_eq!(std::fs::read(&transcript).unwrap(), bytes, "the transcript is only read");
        // The mark's line last, without a newline.
        let short = dir.path().join("short.jsonl");
        let mut head = lines[..13].concat();
        head.pop();
        std::fs::write(&short, &head).unwrap();
        let cut = make_cut(&short, &mark(B1), &cuts).unwrap();
        assert_eq!(std::fs::read(cut.path()).unwrap(), lines[..12].concat());
    }

    #[test]
    fn a_cut_past_a_multi_megabyte_line() {
        let dir = tempfile::tempdir().unwrap();
        let lines = transcript_lines();
        let big = format!(
            "{{\"type\":\"attachment\",\"uuid\":\"d1111111-1111-4111-8111-111111111111\",\"attachment\":{{\"content\":\"{} {B1} {}\"}}}}\n",
            "x".repeat(3 << 20),
            "y".repeat(2 << 20)
        );
        let mut bytes = lines[..5].concat();
        bytes.extend_from_slice(big.as_bytes());
        bytes.extend_from_slice(&lines[5..].concat());
        let transcript = dir.path().join("t.jsonl");
        std::fs::write(&transcript, &bytes).unwrap();
        let cut = make_cut(&transcript, &mark(B1), &dir.path().join("cut")).unwrap();
        let got = std::fs::read(cut.path()).unwrap();
        assert!(got.len() > 5 << 20);
        assert_eq!(got, [lines[..5].concat(), big.into_bytes(), lines[5..12].concat()].concat());
    }

    #[test]
    fn no_cut_without_exactly_one_marked_line() {
        let dir = tempfile::tempdir().unwrap();
        let cuts = dir.path().join("cut");
        let lines = transcript_lines();
        let transcript = dir.path().join("t.jsonl");
        std::fs::write(&transcript, lines.concat()).unwrap();
        let missing = mark("e1111111-1111-4111-8111-111111111111");
        assert!(matches!(make_cut(&transcript, &missing, &cuts), Err(CutError::NoMark)));
        // Named only inside another line's text: not a mark.
        let nested = dir.path().join("nested.jsonl");
        std::fs::write(&nested, format!("{{\"message\":{{\"uuid\":\"{B1}\"}}}}\n{{\"text\":\"{B1}\"}}\n"))
            .unwrap();
        assert!(matches!(make_cut(&nested, &mark(B1), &cuts), Err(CutError::NoMark)));
        // Twice: which one is meant can't be told.
        let twice = dir.path().join("twice.jsonl");
        std::fs::write(&twice, [lines[..13].concat(), lines[12].clone(), lines[13..].concat()].concat())
            .unwrap();
        assert!(matches!(make_cut(&twice, &mark(B1), &cuts), Err(CutError::Twice)));
        assert!(matches!(
            make_cut(&dir.path().join("none.jsonl"), &mark(B1), &cuts),
            Err(CutError::NoTranscript)
        ));
        // A link is not followed.
        let link = dir.path().join("link.jsonl");
        std::os::unix::fs::symlink(&transcript, &link).unwrap();
        assert!(matches!(make_cut(&link, &mark(B1), &cuts), Err(CutError::Io(_))));
        // Past 1 GiB (a sparse file): refused before reading.
        let huge = dir.path().join("huge.jsonl");
        std::fs::File::create(&huge).unwrap().set_len(MAX_TRANSCRIPT + 1).unwrap();
        assert!(matches!(make_cut(&huge, &mark(B1), &cuts), Err(CutError::TooLarge)));
        assert!(cut_files(&cuts).is_empty(), "a failed cut leaves nothing");
        assert_eq!(CutError::NoMark.rpc().code, ErrorCode::NotFound);
        assert_eq!(CutError::NoTranscript.rpc().code, ErrorCode::NotFound);
    }

    #[test]
    fn has_mark_reads_the_lines_own_uuid_only() {
        assert!(has_mark(format!("{{\"uuid\":\"{B1}\",\"x\":1}}\n").as_bytes(), B1));
        assert!(!has_mark(format!("{{\"parentUuid\":\"{B1}\"}}").as_bytes(), B1));
        assert!(!has_mark(format!("{{\"m\":{{\"uuid\":\"{B1}\"}}}}").as_bytes(), B1));
        assert!(!has_mark(format!("[\"{B1}\"]").as_bytes(), B1));
        assert!(!has_mark(format!("{{\"uuid\":\"{B1}\"").as_bytes(), B1), "a torn line");
        assert!(!has_mark(format!("{{\"uuid\":\"{B1}x\"}}").as_bytes(), B1));
        assert!(!has_mark(b"{\"uuid\":7}", B1));
    }

    #[test]
    fn the_host_start_sweeps_only_its_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let cuts = dir.path().join(CUT_DIR);
        crate::client::ensure_private_dir(&cuts).unwrap();
        let (cut, _) = CutFile::create(&cuts).unwrap();
        let left = cut.path().to_owned();
        std::mem::forget(cut);
        let config = check_config(&cuts).unwrap();
        let config_path = config.0.clone();
        std::mem::forget(config);
        std::fs::write(cuts.join("keep.txt"), b"x").unwrap();
        let outside = dir.path().join("outside.jsonl");
        std::fs::write(&outside, b"x").unwrap();
        std::os::unix::fs::symlink(&outside, cuts.join("fedcba9876543210.jsonl")).unwrap();
        sweep(dir.path());
        assert!(!left.exists() && !config_path.exists());
        assert!(cuts.join("keep.txt").exists());
        assert!(outside.exists() && std::fs::symlink_metadata(cuts.join("fedcba9876543210.jsonl")).is_ok());
        assert!(is_check_config_name("check-0123456789abcdef.json"));
        assert!(!is_check_config_name("check-0123456789abcdef.jsonl"));
        sweep(&dir.path().join("nothing-here"));
    }

    fn reply(id: u64, result: Value) -> Value {
        json!({"id": id, "result": result})
    }

    #[test]
    fn the_codex_exchange() {
        let params = codex::fork_params("th", Some("tu_1"), "/w", Mode::Ask);
        let mut fork = CodexFork::new(params.clone());
        assert_eq!(fork.start()["method"], "initialize");
        assert_eq!(fork.start()["params"]["capabilities"], json!({"experimentalApi": true}));
        let CodexStep::Write(lines) = fork.on_line(&reply(INIT_ID, json!({}))) else { panic!() };
        assert_eq!(lines, [codex::initialized(), codex::fork_request(FORK_ID, params.clone())]);
        // Notifications (the copy's own `thread/started`) are not answers.
        assert_eq!(
            fork.on_line(&json!({"method": "thread/started", "params": {"thread": {"id": "th_copy"}}})),
            CodexStep::Write(Vec::new())
        );
        // A request of Codex's is refused, never left waiting.
        let CodexStep::Write(refusal) =
            fork.on_line(&json!({"id": 9, "method": "item/tool/requestUserInput", "params": {}}))
        else {
            panic!()
        };
        assert_eq!(refusal[0]["id"], 9);
        assert_eq!(refusal[0]["error"]["code"], -32601);
        assert_eq!(fork.on_line(&reply(INIT_ID, json!({}))), CodexStep::Write(Vec::new()), "asked once");
        assert_eq!(
            fork.on_line(&reply(FORK_ID, json!({"thread": {"id": CODEX_COPY}}))),
            CodexStep::Done(CODEX_COPY.into())
        );

        // Without the experimental API.
        let mut fork = CodexFork::new(params.clone());
        let refused = json!({"id": INIT_ID, "error": {"code": -32600, "message": "experimentalApi"}});
        let CodexStep::Write(lines) = fork.on_line(&refused) else { panic!() };
        assert_eq!(lines[0]["id"], PLAIN_INIT_ID);
        assert_eq!(lines[0]["params"]["capabilities"], Value::Null);
        assert!(
            matches!(fork.on_line(&reply(PLAIN_INIT_ID, json!({}))), CodexStep::Write(l) if l.len() == 2)
        );
        let mut fork = CodexFork::new(params.clone());
        fork.on_line(&refused);
        let failed = fork.on_line(&json!({"id": PLAIN_INIT_ID, "error": {"code": -1, "message": "no"}}));
        assert!(matches!(failed, CodexStep::Failed(e) if e.code == ErrorCode::Unavailable));

        // A turn Codex doesn't have, another refusal, a bad id.
        let failed = |line: Value| {
            let mut fork = CodexFork::new(params.clone());
            fork.on_line(&reply(INIT_ID, json!({})));
            match fork.on_line(&line) {
                CodexStep::Failed(e) => e.code,
                other => panic!("{other:?}"),
            }
        };
        let gone = json!({"id": FORK_ID, "error": {"code": -32600, "message": "turn not found: tu_1"}});
        assert_eq!(failed(gone), ErrorCode::NotFound);
        assert_eq!(
            failed(json!({"id": FORK_ID, "error": {"code": -32603, "message": "boom"}})),
            ErrorCode::Unavailable
        );
        assert_eq!(failed(reply(FORK_ID, json!({"thread": {"id": "../x"}}))), ErrorCode::Unavailable);
        assert_eq!(failed(reply(FORK_ID, json!({}))), ErrorCode::Unavailable);
    }

    #[test]
    fn both_agents_copy_when_their_cli_runs() {
        let probe = |id: AgentId, installed: bool, bin: bool, problem: Option<&str>| Probe {
            info: AgentInfo { installed, problem: problem.map(str::to_owned), ..AgentInfo::missing(id) },
            bin: bin.then(|| PathBuf::from("/usr/bin/x")),
            commands: Vec::new(),
        };
        for agent in AgentId::ALL {
            assert_eq!(can(agent, &probe(agent, true, true, None)), [Can::Fork, Can::ForkAt]);
            assert!(can(agent, &probe(agent, false, false, None)).is_empty());
            assert!(can(agent, &probe(agent, true, true, Some("Update Claude Code"))).is_empty());
        }
        assert!(can(AgentId::Claude, &probe(AgentId::Codex, true, true, None)).is_empty());
    }

    #[test]
    fn the_environment_keeps_transcript_rewriting_and_auto_resume_out() {
        // Either would break a cut: one rewrites the transcript in place, the
        // other starts a turn on resume [spike S-0e].
        const KEPT_OUT: [&str; 2] =
            ["CLAUDE_CODE_TRANSCRIPT_LOCAL_GC", "CLAUDE_CODE_RESUME_INTERRUPTED_TURN"];
        let vars: Vec<(OsString, OsString)> = KEPT_OUT
            .iter()
            .map(|k| (OsString::from(k), OsString::from("1")))
            .chain([(OsString::from("HOME"), OsString::from("/h"))])
            .collect();
        for agent in AgentId::ALL {
            let child = env::child_env(agent, vars.clone());
            assert!(child.iter().any(|(k, _)| k == "HOME"));
            for key in KEPT_OUT {
                assert!(!child.iter().any(|(k, _)| k == key), "{key}");
            }
        }
        let source = include_str!("env.rs");
        let code = &source[..source.find("#[cfg(test)]").unwrap_or(source.len())];
        for key in KEPT_OUT {
            assert!(!code.contains(key), "{key} named in env.rs");
        }
    }

    // ---- The processes, with stand-ins for the CLIs ----

    fn stand_in(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn job(dir: &Path, agent: AgentId, from: &str, mark: Option<&str>) -> Job {
        let cwd = dir.join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        Job {
            hint: LaunchHint::new(agent, from, mark.map(super::tests::mark).as_ref()).unwrap(),
            cwd,
            mode: Mode::Ask,
            env: vec![(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))],
            cut_dir: dir.join("run").join(CUT_DIR),
            claude_home: dir.join("claude"),
        }
    }

    fn q(path: &Path) -> String {
        format!("'{}'", path.display())
    }

    #[tokio::test]
    async fn claude_opens_the_cut_and_the_cut_goes() {
        let dir = tempfile::tempdir().unwrap();
        let job = job(dir.path(), AgentId::Claude, SESSION, Some(B1));
        let bytes = fixture("claude-20-transcript.jsonl");
        place(&job.claude_home, &project_folder(&job.cwd), &bytes);
        let (args, given, stdin) =
            (dir.path().join("args"), dir.path().join("given"), dir.path().join("stdin"));
        let bin = stand_in(
            dir.path(),
            "claude-cli",
            &format!(
                "printf '%s\\n' \"$@\" > {args}\nprev=\nfor a in \"$@\"; do [ \"$prev\" = --resume ] && cat \"$a\" > {given}; prev=$a; done\n\
                 IFS= read -r line\nprintf '%s\\n' \"$line\" > {stdin}\n\
                 printf '%s\\n' '{{\"type\":\"control_response\",\"response\":{{\"subtype\":\"success\",\"request_id\":\"{CHECK_REQUEST}\",\"response\":{{}}}}}}'\n\
                 cat > /dev/null\n",
                args = q(&args),
                given = q(&given),
                stdin = q(&stdin)
            ),
        );
        claude_check(&bin, &job).await.unwrap();
        assert_eq!(std::fs::read(&given).unwrap(), before_mark(&bytes, B1), "Claude was given the cut");
        let args: Vec<String> = std::fs::read_to_string(&args).unwrap().lines().map(str::to_owned).collect();
        let resume = args.windows(2).find(|w| w[0] == "--resume").map(|w| w[1].clone()).unwrap();
        assert!(Path::new(&resume).starts_with(&job.cut_dir) && Path::new(&resume).is_absolute());
        assert_eq!(args.last().map(String::as_str), Some("--fork-session"));
        let sent: Value = serde_json::from_str(&std::fs::read_to_string(&stdin).unwrap()).unwrap();
        assert_eq!(sent["request"]["subtype"], "initialize");
        assert!(cut_files(&job.cut_dir).is_empty(), "the cut and the config are gone");
        assert_eq!(std::fs::read(place_path(&job)).unwrap(), bytes, "the transcript is unchanged");

        // From the latest message: no cut, the session's id.
        let latest = Job { hint: LaunchHint::new(AgentId::Claude, SESSION, None).unwrap(), ..job.clone() };
        claude_check(&bin, &latest).await.unwrap();
        let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
        assert!(args.lines().collect::<Vec<_>>().windows(2).any(|w| w == ["--resume", SESSION]));
    }

    fn place_path(job: &Job) -> PathBuf {
        job.claude_home.join("projects").join(project_folder(&job.cwd)).join(format!("{SESSION}.jsonl"))
    }

    #[tokio::test]
    async fn a_copy_claude_cant_open_fails_and_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let job = job(dir.path(), AgentId::Claude, SESSION, Some(B1));
        place(&job.claude_home, &project_folder(&job.cwd), &fixture("claude-20-transcript.jsonl"));
        // As Claude does with a file it can't read [spike S-3.8].
        let bin = stand_in(
            dir.path(),
            "claude-cli",
            "printf '%s\\n' '{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"is_error\":true,\"num_turns\":0,\"errors\":[\"No conversation found\"]}'\nexit 1\n",
        );
        let e = claude_check(&bin, &job).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::Unavailable);
        assert!(cut_files(&job.cut_dir).is_empty());
        // An exit without a word.
        let bin = stand_in(dir.path(), "claude-cli2", "exit 0\n");
        assert!(claude_check(&bin, &job).await.is_err());
        assert!(cut_files(&job.cut_dir).is_empty());
    }

    #[tokio::test]
    async fn a_missing_mark_starts_no_process() {
        let dir = tempfile::tempdir().unwrap();
        let job = job(dir.path(), AgentId::Claude, SESSION, Some("e1111111-1111-4111-8111-111111111111"));
        place(&job.claude_home, &project_folder(&job.cwd), &fixture("claude-20-transcript.jsonl"));
        let ran = dir.path().join("ran");
        let bin = stand_in(dir.path(), "claude-cli", &format!("touch {}\n", q(&ran)));
        let e = claude_check(&bin, &job).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
        assert!(!ran.exists(), "no process");
        assert!(cut_files(&job.cut_dir).is_empty());
        // No transcript at all: the same.
        let other = Job { hint: LaunchHint::new(AgentId::Claude, A1, Some(&mark(B1))).unwrap(), ..job };
        assert_eq!(claude_check(&bin, &other).await.unwrap_err().code, ErrorCode::NotFound);
        assert!(!ran.exists());
    }

    #[tokio::test]
    async fn a_start_hands_the_cut_to_the_next_process() {
        let dir = tempfile::tempdir().unwrap();
        let job = job(dir.path(), AgentId::Claude, SESSION, Some(C1));
        let bytes = fixture("claude-20-transcript.jsonl");
        place(&job.claude_home, &project_folder(&job.cwd), &bytes);
        let start = claude_start(&job).await.unwrap();
        assert_eq!(start.from, SESSION);
        let cut = start.cut.as_ref().unwrap();
        assert_eq!(start.resume, cut.path().to_str().unwrap());
        assert_eq!(std::fs::read(cut.path()).unwrap(), before_mark(&bytes, C1));
        drop(start);
        assert!(cut_files(&job.cut_dir).is_empty(), "unused, it goes");
        let whole = Job { hint: LaunchHint::new(AgentId::Claude, SESSION, None).unwrap(), ..job };
        let start = claude_start(&whole).await.unwrap();
        assert_eq!((start.resume.as_str(), start.cut.is_none()), (SESSION, true));
    }

    fn codex_stand_in(dir: &Path, fork_reply: &str) -> PathBuf {
        let (init, fork, refused) = (dir.join("init"), dir.join("fork"), dir.join("refused"));
        stand_in(
            dir,
            "codex",
            &format!(
                "[ \"$1\" = app-server ] || exit 2\n\
                 IFS= read -r line; printf '%s\\n' \"$line\" > {init}\n\
                 printf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\n\
                 IFS= read -r line\n\
                 IFS= read -r line; printf '%s\\n' \"$line\" > {fork}\n\
                 printf '%s\\n' '{{\"method\":\"thread/started\",\"params\":{{\"thread\":{{\"id\":\"{CODEX_COPY}\"}}}}}}'\n\
                 printf '%s\\n' '{{\"id\":0,\"method\":\"item/tool/requestUserInput\",\"params\":{{}}}}'\n\
                 IFS= read -r line; printf '%s\\n' \"$line\" > {refused}\n\
                 printf '%s\\n' '{fork_reply}'\n\
                 cat > /dev/null\n",
                init = q(&init),
                fork = q(&fork),
                refused = q(&refused),
            ),
        )
    }

    #[tokio::test]
    async fn codex_forks_through_a_turn_from_a_fresh_process() {
        let dir = tempfile::tempdir().unwrap();
        let source = "019a7c10-0000-7000-8000-00000000a001";
        let job = job(dir.path(), AgentId::Codex, source, Some("019a7c10-0000-7000-8000-00000000b001"));
        let bin = codex_stand_in(
            dir.path(),
            &format!("{{\"id\":3,\"result\":{{\"thread\":{{\"id\":\"{CODEX_COPY}\"}}}}}}"),
        );
        assert_eq!(codex_fork(&bin, &job).await.unwrap(), CODEX_COPY);
        let read = |name: &str| -> Value {
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(name)).unwrap()).unwrap()
        };
        assert_eq!(read("init")["method"], "initialize");
        let fork = read("fork");
        assert_eq!(fork["method"], "thread/fork");
        assert_eq!(
            fork["params"],
            codex::fork_params(
                source,
                Some("019a7c10-0000-7000-8000-00000000b001"),
                &job.cwd.to_string_lossy(),
                Mode::Ask
            )
        );
        assert_eq!(read("refused")["error"]["code"], -32601);

        let bin = codex_stand_in(
            dir.path(),
            "{\"id\":3,\"error\":{\"code\":-32600,\"message\":\"turn not found: 019a7c10-0000-7000-8000-00000000b001\"}}",
        );
        assert_eq!(codex_fork(&bin, &job).await.unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn a_pending_copy_starts_the_next_process() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("claude");
        let cwd = Path::new("/work/g7");
        let bytes = fixture("claude-20-transcript.jsonl");
        place(&home, &project_folder(cwd), &bytes);
        let from = |hint: &LaunchHint| claude::ForkFrom {
            hint: hint.encode(),
            claude_home: home.clone(),
            cut_dir: dir.path().join("run").join(CUT_DIR),
        };
        let at_b = LaunchHint::new(AgentId::Claude, SESSION, Some(&mark(B1))).unwrap();
        let start = claude_fork_start(&from(&at_b), cwd).unwrap();
        assert_eq!(std::fs::read(&start.resume).unwrap(), before_mark(&bytes, B1));
        assert_eq!(start.from, SESSION);
        drop(start);
        let whole = LaunchHint::new(AgentId::Claude, SESSION, None).unwrap();
        let start = claude_fork_start(&from(&whole), cwd).unwrap();
        assert_eq!((start.resume.as_str(), start.cut.is_none()), (SESSION, true));
        // A mark Claude's session doesn't have: nothing starts, the user is told.
        let gone =
            LaunchHint::new(AgentId::Claude, SESSION, Some(&mark("e1111111-1111-4111-8111-111111111111")))
                .unwrap();
        let e = claude_fork_start(&from(&gone), cwd).unwrap_err();
        assert_eq!(
            (e.kind(), e.to_string()),
            (io::ErrorKind::NotFound, "Claude no longer has that message.".to_owned())
        );
        let codex = LaunchHint::new(AgentId::Codex, "th_1", Some(&mark("tu_1"))).unwrap();
        assert_eq!(claude_fork_start(&from(&codex), cwd).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(cut_files(&dir.path().join("run").join(CUT_DIR)).is_empty());
        // Codex: the copy is made by the driver's `thread/fork`.
        assert_eq!(
            codex_fork_start(&codex.encode()),
            Some(codex::ForkStart { from: "th_1".into(), last_turn: Some("tu_1".into()) })
        );
        assert_eq!(codex_fork_start(&at_b.encode()), None, "not Codex's");
        assert_eq!(codex_fork_start("garbage"), None);
    }

    #[test]
    fn a_hint_starts_codex_with_its_mark() {
        let hint = LaunchHint::new(AgentId::Codex, "th_1", Some(&mark("tu_1"))).unwrap();
        assert_eq!(hint.mark.as_ref().map(NativeMark::as_str), Some("tu_1"));
        // The fake CLI scripts for the host's own tests answer what the driver sends.
        for (name, last) in [
            ("codex-20-fork.jsonl", "019a7c10-0000-7000-8000-00000000b001"),
            ("codex-21-fork-unknown-turn.jsonl", "019a7c10-0000-7000-8000-00000000dead"),
        ] {
            let entries: Vec<Value> = String::from_utf8(fixture(name))
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            let fork = entries.iter().find(|e| e["msg"]["method"] == "thread/fork").unwrap();
            assert_eq!(
                fork["msg"]["params"],
                codex::fork_params("019a7c10-0000-7000-8000-00000000a001", Some(last), "/work", Mode::Ask),
                "{name}"
            );
        }
    }
}
