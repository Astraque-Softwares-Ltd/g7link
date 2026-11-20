// SPDX-License-Identifier: MPL-2.0
//! Clients of the host socket: [`Client`] for tokio, [`BlockingClient`] for
//! plain threads, the socket path, and starting the host on demand. A host
//! older than this build (or whose program an upgrade replaced) is swapped
//! for the current one once no thread runs on it.

use std::collections::{HashMap, VecDeque};
use std::io::{self, BufReader as StdBufReader, Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{mpsc, oneshot};

use crate::api::*;
use crate::lines::{self, LineRead};

/// Overrides the socket path (tests, and the approval bridge).
pub const SOCKET_ENV: &str = "G7_AGENTS_SOCKET";
/// Overrides where the host binary is looked for first.
pub const BIN_ENV: &str = "G7_AGENTS_BIN";
/// How long a request waits for its reply, unless it says otherwise.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// After spawning the host, how long connecting is retried, and how often.
pub const SPAWN_WAIT: Duration = Duration::from_secs(3);
pub const SPAWN_RETRY: Duration = Duration::from_millis(50);
/// Events buffered for a client that isn't reading them; later ones are
/// dropped until it catches up.
pub const EVENT_BUFFER: usize = 4096;
/// This build's version: a host reporting an older one is replaced.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// How long a host that was asked to stop gets to stop its agents and exit.
pub const STOP_WAIT: Duration = Duration::from_secs(25);
/// How often a stopping host is looked at until it has exited.
const EXIT_POLL: Duration = Duration::from_millis(50);
/// Threads looked at to learn whether any runs (the host keeps at most 200).
const RUNNING_PAGES: usize = 3;
const PROC: &str = "/proc";
/// The host's program file name, in every package.
const HOST_EXE_NAME: &str = "g7-agents";
/// What the kernel appends to `/proc/<pid>/exe` once the file is gone.
const DELETED: &[u8] = b" (deleted)";
const MAX_STAT: u64 = 4096;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("the agent host is not installed")]
    NotInstalled,
    #[error("the agent host did not start")]
    NotStarted,
    #[error("the running agent host could not be stopped")]
    NotStopped,
    #[error("{0} is not private to this user")]
    Insecure(PathBuf),
    #[error("the connection to the agent host closed")]
    Closed,
    #[error("the agent host did not answer in time")]
    Timeout,
    #[error("unexpected reply from the agent host: {0}")]
    Protocol(String),
    #[error(transparent)]
    Remote(#[from] RpcError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl ClientError {
    /// The host's error code, for errors the host returned.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Self::Remote(e) => Some(e.code),
            _ => None,
        }
    }

    /// The host can't be reached (not installed, not running, gone).
    pub fn is_unreachable(&self) -> bool {
        matches!(self, Self::NotInstalled | Self::NotStarted | Self::Closed | Self::Io(_) | Self::Timeout)
    }

    /// The host doesn't have the method: it is older than this build.
    pub fn is_unknown_method(&self) -> bool {
        matches!(self, Self::Remote(e) if e.code == ErrorCode::BadRequest && e.message == UNKNOWN_METHOD)
    }
}

// ---- Paths ----

fn absolute_env(name: &str) -> Option<PathBuf> {
    let value = std::env::var_os(name)?;
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
}

/// This process's effective uid.
pub fn euid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// The directory that holds the socket and the host's lock:
/// `$XDG_RUNTIME_DIR/g7-agents`, or `/tmp/g7-agents-<uid>` without it.
pub fn default_runtime_dir() -> PathBuf {
    match absolute_env("XDG_RUNTIME_DIR") {
        Some(dir) => dir.join("g7-agents"),
        None => PathBuf::from(format!("/tmp/g7-agents-{}", euid())),
    }
}

/// The socket path: `$G7_AGENTS_SOCKET`, else `host.sock` in
/// [`default_runtime_dir`].
pub fn socket_path() -> PathBuf {
    absolute_env(SOCKET_ENV).unwrap_or_else(|| default_runtime_dir().join("host.sock"))
}

/// Checks `dir` is a real directory owned by this user with no access for
/// anyone else. `Ok(false)` when it doesn't exist.
pub fn check_private_dir(dir: &Path) -> Result<bool, ClientError> {
    let meta = match std::fs::symlink_metadata(dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    if !meta.file_type().is_dir() || meta.uid() != euid() || meta.permissions().mode() & 0o077 != 0 {
        return Err(ClientError::Insecure(dir.to_owned()));
    }
    Ok(true)
}

/// Creates `dir` (and its parents) with mode 0700 if missing, then checks
/// it is private.
pub fn ensure_private_dir(dir: &Path) -> Result<(), ClientError> {
    if !check_private_dir(dir)? {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        // The umask may have narrowed the mode; widen nothing, only fix ours.
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        if !check_private_dir(dir)? {
            return Err(ClientError::Insecure(dir.to_owned()));
        }
    }
    Ok(())
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn search_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|p| is_executable_file(p))
}

/// Where the host binary is, first hit wins: `$G7_AGENTS_BIN`; `g7-agents`
/// beside the current executable; `/usr/lib/g7link/g7-agents`; `g7-agents`
/// on `PATH`; `$XDG_DATA_HOME/g7link-app/g7-agents`; `/opt/g7link/g7-agents`.
pub fn find_host_binary() -> Option<PathBuf> {
    let mut candidates = Vec::with_capacity(6);
    if let Some(path) = absolute_env(BIN_ENV) {
        candidates.push(path);
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join("g7-agents"));
    }
    candidates.push(PathBuf::from("/usr/lib/g7link/g7-agents"));
    if let Some(found) = candidates.iter().find(|p| is_executable_file(p)) {
        return Some(found.clone());
    }
    if let Some(found) = search_path("g7-agents") {
        return Some(found);
    }
    let data_home =
        absolute_env("XDG_DATA_HOME").or_else(|| absolute_env("HOME").map(|home| home.join(".local/share")));
    let mut rest = Vec::with_capacity(2);
    if let Some(data) = data_home {
        rest.push(data.join("g7link-app/g7-agents"));
    }
    rest.push(PathBuf::from("/opt/g7link/g7-agents"));
    rest.into_iter().find(|p| is_executable_file(p))
}

/// `$XDG_STATE_HOME/g7-agents`, where the host's log goes.
pub fn state_dir() -> Option<PathBuf> {
    absolute_env("XDG_STATE_HOME")
        .or_else(|| absolute_env("HOME").map(|home| home.join(".local/state")))
        .map(|dir| dir.join("g7-agents"))
}

/// Starts `<bin> serve` detached: its own process group, stdin and stdout
/// on `/dev/null`, stderr appended to `host.log` in [`state_dir`].
pub fn spawn_host(bin: &Path) -> io::Result<()> {
    let stderr = state_dir()
        .and_then(|dir| {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).ok()?;
            std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(dir.join("host.log")).ok()
        })
        .map_or_else(Stdio::null, Stdio::from);
    let mut child = Command::new(bin)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .process_group(0)
        .spawn()?;
    // Reap it when it exits so it never lingers as a zombie of ours.
    std::thread::Builder::new().name("g7-agents-reaper".into()).spawn(move || {
        let _ = child.wait();
    })?;
    Ok(())
}

fn is_not_running(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused)
}

// ---- Replacing an outdated host ----

/// `a` is an older version than `b`: dotted numbers compared in order,
/// anything after `-` or `+` ignored. `false` when either can't be read.
pub fn is_older(a: &str, b: &str) -> bool {
    fn parts(version: &str) -> Option<Vec<u64>> {
        let core = version.trim().split(['-', '+']).next()?;
        core.split('.').map(|p| p.parse().ok()).collect::<Option<Vec<u64>>>().filter(|p| !p.is_empty())
    }
    let (Some(mut a), Some(mut b)) = (parts(a), parts(b)) else { return false };
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a < b
}

/// Why a running host should give way to this build's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outdated {
    /// It reports an older version.
    Older,
    /// Its program file was replaced or removed since it started, as a
    /// package upgrade does.
    Replaced,
}

/// The host process behind a connection, as `/proc` showed it right after
/// `hello`: a signal goes to it only while the same pid still has the same
/// start time and program, never to a process that reused the pid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProcess {
    pid: u32,
    /// Field 22 of `/proc/<pid>/stat`: clock ticks after boot.
    start_ticks: u64,
    /// `/proc/<pid>/exe`, " (deleted)" included once the file is gone.
    exe: PathBuf,
}

fn read_small(path: &Path, limit: u64) -> Option<String> {
    let mut text = String::new();
    std::fs::File::open(path).ok()?.take(limit).read_to_string(&mut text).ok()?;
    Some(text)
}

impl HostProcess {
    /// `pid` under `proc_root` (`/proc`), when it is a live `g7-agents` run by
    /// this user.
    pub fn read(proc_root: &Path, pid: u32) -> Option<HostProcess> {
        if pid == 0 {
            return None;
        }
        let dir = proc_root.join(pid.to_string());
        if std::fs::metadata(&dir).ok()?.uid() != euid() {
            return None;
        }
        let stat = read_small(&dir.join("stat"), MAX_STAT)?;
        // Fields after the last `)`, so a `)` in the command name can't shift them.
        let fields: Vec<&str> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
        if matches!(fields.first(), Some(&("Z" | "X" | "x"))) {
            return None;
        }
        let start_ticks = fields.get(19)?.parse().ok()?;
        let exe = std::fs::read_link(dir.join("exe")).ok()?;
        let bytes = exe.as_os_str().as_bytes();
        let file = Path::new(std::ffi::OsStr::from_bytes(bytes.strip_suffix(DELETED).unwrap_or(bytes)));
        (file.file_name()? == HOST_EXE_NAME).then_some(HostProcess { pid, start_ticks, exe })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Its program file was replaced or removed since it started.
    pub fn exe_replaced(&self) -> bool {
        self.exe.as_os_str().as_bytes().ends_with(DELETED)
    }

    /// Sends SIGTERM to this process if it is still the one that was read: a
    /// pidfd pins whatever has the pid now, its start time and program are
    /// checked again, and the signal goes through the pidfd, so it can't
    /// reach a process that reused the pid.
    fn terminate(&self, proc_root: &Path) -> Result<(), ClientError> {
        let raw = i32::try_from(self.pid).map_err(|_| ClientError::NotStopped)?;
        let pid = rustix::process::Pid::from_raw(raw).ok_or(ClientError::NotStopped)?;
        let pidfd: OwnedFd = rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty())
            .map_err(io::Error::from)?;
        if HostProcess::read(proc_root, self.pid).as_ref() != Some(self) {
            return Err(ClientError::NotStopped);
        }
        rustix::process::pidfd_send_signal(&pidfd, rustix::process::Signal::TERM).map_err(io::Error::from)?;
        Ok(())
    }

    /// Waits until this process has exited (gone, a zombie, or its pid
    /// taken by another), at most `limit`.
    async fn exited(&self, proc_root: &Path, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if HostProcess::read(proc_root, self.pid).as_ref() != Some(self) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(EXIT_POLL).await;
        }
    }
}

type SpawnFn = dyn Fn() -> Result<(), ClientError> + Send + Sync;

/// Where the host listens, how it is started, and which version is current.
#[derive(Clone)]
pub struct Launch {
    pub socket: PathBuf,
    /// A host reporting an older version is replaced.
    pub version: String,
    spawn: Arc<SpawnFn>,
}

impl std::fmt::Debug for Launch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Launch")
            .field("socket", &self.socket)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl Launch {
    /// This build's: [`socket_path`], [`VERSION`], and the binary
    /// [`find_host_binary`] finds, started with [`spawn_host`].
    pub fn current() -> Launch {
        Launch {
            socket: socket_path(),
            version: VERSION.to_owned(),
            spawn: Arc::new(|| {
                let bin = find_host_binary().ok_or(ClientError::NotInstalled)?;
                spawn_host(&bin).map_err(ClientError::from)
            }),
        }
    }

    /// A host on `socket` that `spawn` starts (tests, or another binary).
    pub fn new(
        socket: PathBuf,
        version: impl Into<String>,
        spawn: impl Fn() -> io::Result<()> + Send + Sync + 'static,
    ) -> Launch {
        Launch {
            socket,
            version: version.into(),
            spawn: Arc::new(move || spawn().map_err(ClientError::from)),
        }
    }

    fn spawn(&self) -> Result<(), ClientError> {
        (self.spawn)()
    }
}

/// When [`Client::replace`] may stop an outdated host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// Only while no thread is working or waiting on it.
    Idle,
    /// Now: its running threads stop (the user agreed to that).
    Now,
}

// ---- Typed calls ----

/// One method call: its name, its params, and how to read its result.
pub struct Call<T> {
    pub method: &'static str,
    pub params: Value,
    parse: fn(Value) -> Result<T, serde_json::Error>,
}

impl<T> std::fmt::Debug for Call<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Call").field("method", &self.method).finish_non_exhaustive()
    }
}

impl<T> Call<T> {
    fn new<P: Serialize>(
        method: &'static str,
        params: &P,
        parse: fn(Value) -> Result<T, serde_json::Error>,
    ) -> Self {
        Self { method, params: serde_json::to_value(params).unwrap_or(Value::Null), parse }
    }

    pub fn parse(&self, value: Value) -> Result<T, ClientError> {
        (self.parse)(value).map_err(|e| ClientError::Protocol(e.to_string()))
    }
}

fn unit(_: Value) -> Result<(), serde_json::Error> {
    Ok(())
}

fn parse_as<T: DeserializeOwned>(value: Value) -> Result<T, serde_json::Error> {
    serde_json::from_value(value)
}

/// Builders for every method of §2.3 except `hello` and the bridge's.
pub mod calls {
    use super::*;

    pub fn agents_list(refresh: bool) -> Call<Vec<AgentInfo>> {
        Call::new(method::AGENTS_LIST, &AgentsListParams { refresh }, |v| {
            parse_as::<AgentsListResult>(v).map(|r| r.agents)
        })
    }

    pub fn folders_list() -> Call<Vec<Folder>> {
        Call::new(method::FOLDERS_LIST, &Empty {}, |v| parse_as::<FoldersListResult>(v).map(|r| r.folders))
    }

    pub fn folders_add(path: String, label: Option<String>) -> Call<Folder> {
        Call::new(method::FOLDERS_ADD, &FoldersAddParams { path, label }, |v| {
            parse_as::<FolderResult>(v).map(|r| r.folder)
        })
    }

    pub fn folders_remove(id: String) -> Call<()> {
        Call::new(method::FOLDERS_REMOVE, &FoldersRemoveParams { id }, unit)
    }

    pub fn threads_list(limit: Option<u32>, before_ms: Option<u64>) -> Call<Vec<ThreadInfo>> {
        Call::new(method::THREADS_LIST, &ThreadsListParams { limit, before_ms }, |v| {
            parse_as::<ThreadsListResult>(v).map(|r| r.threads)
        })
    }

    pub fn thread_start(params: ThreadStartParams) -> Call<ThreadInfo> {
        Call::new(method::THREAD_START, &params, |v| parse_as::<ThreadResult>(v).map(|r| r.thread))
    }

    pub fn thread_send(thread_id: String, text: String, origin: Origin) -> Call<()> {
        Call::new(method::THREAD_SEND, &ThreadSendParams::new(thread_id, text, origin), unit)
    }

    pub fn thread_interrupt(thread_id: String) -> Call<()> {
        Call::new(method::THREAD_INTERRUPT, &ThreadIdParams { thread_id }, unit)
    }

    pub fn thread_read(params: ThreadReadParams) -> Call<ThreadReadResult> {
        Call::new(method::THREAD_READ, &params, parse_as)
    }

    pub fn thread_delete(thread_id: String) -> Call<()> {
        Call::new(method::THREAD_DELETE, &ThreadIdParams { thread_id }, unit)
    }

    pub fn thread_terminal_command(thread_id: String) -> Call<TerminalCommandResult> {
        Call::new(method::THREAD_TERMINAL_COMMAND, &ThreadIdParams { thread_id }, parse_as)
    }

    pub fn approval_answer(token: String, decision: Decision, by: By) -> Call<()> {
        Call::new(method::APPROVAL_ANSWER, &ApprovalAnswerParams { token, decision, by, message: None }, unit)
    }

    /// `approval.answer` with every field: a denial with the user's words.
    /// Send `message` only to a host that offers [`cap::DENY_MESSAGE`].
    pub fn approval_answer_with(params: ApprovalAnswerParams) -> Call<()> {
        Call::new(method::APPROVAL_ANSWER, &params, unit)
    }

    pub fn subscribe(threads: ThreadSelection, deltas: bool) -> Call<()> {
        Call::new(method::SUBSCRIBE, &SubscribeParams { threads, deltas, sessions: false }, unit)
    }

    /// `subscribe` with every field, `sessions` included.
    pub fn subscribe_with(params: SubscribeParams) -> Call<()> {
        Call::new(method::SUBSCRIBE, &params, unit)
    }

    pub fn unsubscribe() -> Call<()> {
        Call::new(method::UNSUBSCRIBE, &Empty {}, unit)
    }

    pub fn ui_viewing(thread_id: Option<String>) -> Call<()> {
        Call::new(method::UI_VIEWING, &UiViewingParams { thread_id, session_id: None }, unit)
    }

    /// `ui.viewing` with every field, `session_id` included.
    pub fn ui_viewing_with(params: UiViewingParams) -> Call<()> {
        Call::new(method::UI_VIEWING, &params, unit)
    }

    pub fn sessions_list(limit: Option<u32>) -> Call<Vec<Session>> {
        Call::new(method::SESSIONS_LIST, &SessionsListParams { limit }, |v| {
            parse_as::<SessionsListResult>(v).map(|r| r.sessions)
        })
    }

    pub fn session_read(params: SessionReadParams) -> Call<SessionReadResult> {
        Call::new(method::SESSION_READ, &params, parse_as)
    }

    pub fn session_watch(session_id: Option<String>) -> Call<()> {
        Call::new(method::SESSION_WATCH, &SessionWatchParams { session_id }, unit)
    }

    pub fn session_continue(params: SessionContinueParams) -> Call<ThreadInfo> {
        Call::new(method::SESSION_CONTINUE, &params, |v| parse_as::<ThreadResult>(v).map(|r| r.thread))
    }

    pub fn thread_export(params: ThreadExportParams) -> Call<ThreadExportResult> {
        Call::new(method::THREAD_EXPORT, &params, parse_as)
    }

    pub fn settings_get() -> Call<HostSettings> {
        Call::new(method::SETTINGS_GET, &Empty {}, parse_as)
    }

    /// Turns following sessions started outside G7 on or off; the new settings.
    pub fn sessions_set_following(on: bool) -> Call<HostSettings> {
        Call::new(method::SESSIONS_SET_FOLLOWING, &SessionsSetFollowingParams { on }, parse_as)
    }

    // ---- Control (phase 2): only to a host that offers `cap::CONTROL` ----

    /// `thread.send` with every field: steer or queue, images, settings.
    pub fn thread_send_with(params: ThreadSendParams) -> Call<ThreadSendResult> {
        Call::new(method::THREAD_SEND, &params, parse_as)
    }

    pub fn thread_configure(params: ThreadConfigureParams) -> Call<ThreadInfo> {
        Call::new(method::THREAD_CONFIGURE, &params, |v| parse_as::<ThreadResult>(v).map(|r| r.thread))
    }

    pub fn queue_edit(params: QueueEditParams) -> Call<()> {
        Call::new(method::QUEUE_EDIT, &params, unit)
    }

    pub fn queue_remove(params: QueueItemParams) -> Call<()> {
        Call::new(method::QUEUE_REMOVE, &params, unit)
    }

    pub fn queue_send_now(params: QueueItemParams) -> Call<()> {
        Call::new(method::QUEUE_SEND_NOW, &params, unit)
    }

    pub fn question_answer(params: QuestionAnswerParams) -> Call<()> {
        Call::new(method::QUESTION_ANSWER, &params, unit)
    }

    pub fn question_dismiss(token: String, by: By) -> Call<()> {
        Call::new(method::QUESTION_DISMISS, &QuestionDismissParams { token, by }, unit)
    }

    pub fn plan_implement(params: PlanImplementParams) -> Call<()> {
        Call::new(method::PLAN_IMPLEMENT, &params, unit)
    }

    pub fn files_search(params: FilesSearchParams) -> Call<FilesSearchResult> {
        Call::new(method::FILES_SEARCH, &params, parse_as)
    }

    pub fn commands_list(agent: AgentId, folder_id: String) -> Call<Vec<crate::api::Command>> {
        Call::new(method::COMMANDS_LIST, &CommandsListParams { agent, folder_id }, |v| {
            parse_as::<CommandsListResult>(v).map(|r| r.commands)
        })
    }

    /// A picture on this PC, by path (PC clients only).
    pub fn image_add_path(path: String) -> Call<ImageRef> {
        Call::new(method::IMAGE_ADD_PATH, &ImageAddPathParams { path }, |v| {
            parse_as::<ImageResult>(v).map(|r| r.image)
        })
    }

    /// Starts an upload of `bytes`; its id.
    pub fn image_begin(name: String, bytes: u32) -> Call<String> {
        Call::new(method::IMAGE_BEGIN, &ImageBeginParams { name, bytes }, |v| {
            parse_as::<ImageBeginResult>(v).map(|r| r.upload)
        })
    }

    pub fn image_chunk(params: ImageChunkParams) -> Call<()> {
        Call::new(method::IMAGE_CHUNK, &params, unit)
    }

    pub fn image_end(upload: String) -> Call<ImageRef> {
        Call::new(method::IMAGE_END, &ImageEndParams { upload }, |v| {
            parse_as::<ImageResult>(v).map(|r| r.image)
        })
    }

    pub fn image_drop(id: String) -> Call<()> {
        Call::new(method::IMAGE_DROP, &ImageIdParams { id }, unit)
    }

    pub fn image_read(id: String, max: u16) -> Call<ImageReadResult> {
        Call::new(method::IMAGE_READ, &ImageReadParams { id, max }, parse_as)
    }

    /// The draft's new `rev`; a `CONFLICT` error carries the stored draft
    /// ([`RpcError::conflict_draft`]).
    pub fn draft_set(params: DraftSetParams) -> Call<u64> {
        Call::new(method::DRAFT_SET, &params, |v| parse_as::<DraftSetResult>(v).map(|r| r.rev))
    }

    pub fn draft_get(key: String) -> Call<Option<Draft>> {
        Call::new(method::DRAFT_GET, &DraftGetParams { key }, |v| {
            parse_as::<DraftGetResult>(v).map(|r| r.draft)
        })
    }

    /// Prompts sent from this PC, newest first (PC clients only).
    pub fn history_list(limit: Option<u32>) -> Call<Vec<String>> {
        Call::new(method::HISTORY_LIST, &HistoryListParams { limit }, |v| {
            parse_as::<HistoryListResult>(v).map(|r| r.prompts)
        })
    }

    /// A folder's MCP switch (PC clients only).
    pub fn folders_set(id: String, mcp: bool) -> Call<Folder> {
        Call::new(method::FOLDERS_SET, &FoldersSetParams { id, mcp }, |v| {
            parse_as::<FolderResult>(v).map(|r| r.folder)
        })
    }

    /// Changes host settings (PC clients only); the new settings.
    pub fn settings_set(params: SettingsSetParams) -> Call<HostSettings> {
        Call::new(method::SETTINGS_SET, &params, parse_as)
    }
}

/// Generates one typed method per call builder, async or blocking.
macro_rules! with_calls {
    ($gen:ident) => {
        $gen! {
            agents_list(refresh: bool) -> Vec<AgentInfo>;
            folders_list() -> Vec<Folder>;
            folders_add(path: String, label: Option<String>) -> Folder;
            folders_remove(id: String) -> ();
            threads_list(limit: Option<u32>, before_ms: Option<u64>) -> Vec<ThreadInfo>;
            thread_start(params: ThreadStartParams) -> ThreadInfo;
            thread_send(thread_id: String, text: String, origin: Origin) -> ();
            thread_interrupt(thread_id: String) -> ();
            thread_read(params: ThreadReadParams) -> ThreadReadResult;
            thread_delete(thread_id: String) -> ();
            thread_terminal_command(thread_id: String) -> TerminalCommandResult;
            approval_answer(token: String, decision: Decision, by: By) -> ();
            approval_answer_with(params: ApprovalAnswerParams) -> ();
            subscribe(threads: ThreadSelection, deltas: bool) -> ();
            subscribe_with(params: SubscribeParams) -> ();
            unsubscribe() -> ();
            ui_viewing(thread_id: Option<String>) -> ();
            ui_viewing_with(params: UiViewingParams) -> ();
            sessions_list(limit: Option<u32>) -> Vec<Session>;
            session_read(params: SessionReadParams) -> SessionReadResult;
            session_watch(session_id: Option<String>) -> ();
            session_continue(params: SessionContinueParams) -> ThreadInfo;
            thread_export(params: ThreadExportParams) -> ThreadExportResult;
            settings_get() -> HostSettings;
            sessions_set_following(on: bool) -> HostSettings;
            thread_send_with(params: ThreadSendParams) -> ThreadSendResult;
            thread_configure(params: ThreadConfigureParams) -> ThreadInfo;
            queue_edit(params: QueueEditParams) -> ();
            queue_remove(params: QueueItemParams) -> ();
            queue_send_now(params: QueueItemParams) -> ();
            question_answer(params: QuestionAnswerParams) -> ();
            question_dismiss(token: String, by: By) -> ();
            plan_implement(params: PlanImplementParams) -> ();
            files_search(params: FilesSearchParams) -> FilesSearchResult;
            commands_list(agent: AgentId, folder_id: String) -> Vec<crate::api::Command>;
            image_add_path(path: String) -> ImageRef;
            image_begin(name: String, bytes: u32) -> String;
            image_chunk(params: ImageChunkParams) -> ();
            image_end(upload: String) -> ImageRef;
            image_drop(id: String) -> ();
            image_read(id: String, max: u16) -> ImageReadResult;
            draft_set(params: DraftSetParams) -> u64;
            draft_get(key: String) -> Option<Draft>;
            history_list(limit: Option<u32>) -> Vec<String>;
            folders_set(id: String, mcp: bool) -> Folder;
            settings_set(params: SettingsSetParams) -> HostSettings;
        }
    };
}

macro_rules! async_methods {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {
        $(
            pub async fn $name(&self, $($arg: $ty),*) -> Result<$ret, ClientError> {
                self.run(calls::$name($($arg),*)).await
            }
        )*
    };
}

macro_rules! blocking_methods {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {
        $(
            pub fn $name(&mut self, $($arg: $ty),*) -> Result<$ret, ClientError> {
                self.run(calls::$name($($arg),*))
            }
        )*
    };
}

fn encode_request(id: u64, method: &str, params: Value) -> Result<String, ClientError> {
    let mut line = serde_json::to_string(&Request { id, method: method.to_owned(), params })
        .map_err(|e| ClientError::Protocol(e.to_string()))?;
    if line.len() > MAX_LINE {
        return Err(ClientError::Remote(RpcError::new(ErrorCode::BadRequest, "request too large")));
    }
    line.push('\n');
    Ok(line)
}

fn into_result(response: Response) -> Result<Value, ClientError> {
    match (response.result, response.error) {
        (_, Some(error)) => Err(ClientError::Remote(error)),
        (Some(result), None) => Ok(result),
        (None, None) => Ok(Value::Object(Default::default())),
    }
}

fn hello_params(client: ClientKind) -> HelloParams {
    HelloParams::new(client, &[])
}

fn no_hello() -> HelloResult {
    HelloResult { version: 0, host_version: String::new(), pid: 0, caps: Vec::new() }
}

// ---- Async client ----

type Pending = HashMap<u64, oneshot::Sender<Result<Value, ClientError>>>;

struct Shared {
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    /// `None` once the connection has closed.
    pending: Mutex<Option<Pending>>,
    next_id: AtomicU64,
    reader: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        if let Some(reader) = self.reader.lock().unwrap_or_else(|e| e.into_inner()).take() {
            reader.abort();
        }
    }
}

/// Events from the host, in the order it sent them. Ends when the
/// connection closes.
#[derive(Debug)]
pub struct Events {
    rx: mpsc::Receiver<Event>,
}

impl Events {
    pub async fn recv(&mut self) -> Option<Event> {
        self.rx.recv().await
    }

    pub fn try_recv(&mut self) -> Option<Event> {
        self.rx.try_recv().ok()
    }
}

/// A connection to the host for tokio code. Cheap to clone; the connection
/// closes when the last clone is dropped. Needs a tokio runtime.
#[derive(Clone)]
pub struct Client {
    shared: Arc<Shared>,
    /// What this client said, for saying it again to a replacement host.
    params: HelloParams,
    hello: HelloResult,
    /// The host process, when it could be told for sure.
    process: Option<HostProcess>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("hello", &self.hello).finish_non_exhaustive()
    }
}

impl Client {
    /// Connects to the socket at `path` and says `hello`.
    pub async fn connect(path: &Path, hello: HelloParams) -> Result<(Client, Events), ClientError> {
        if let Some(dir) = path.parent()
            && !check_private_dir(dir)?
        {
            return Err(io::Error::from(io::ErrorKind::NotFound).into());
        }
        let stream = UnixStream::connect(path).await?;
        // The kernel's word for who serves the socket, to check the pid `hello` gives.
        let peer = stream.peer_cred().ok().and_then(|cred| cred.pid());
        let (read, write) = stream.into_split();
        let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);
        let shared = Arc::new(Shared {
            writer: tokio::sync::Mutex::new(write),
            pending: Mutex::new(Some(HashMap::new())),
            next_id: AtomicU64::new(1),
            reader: Mutex::new(None),
        });
        let reader = tokio::spawn(read_loop(BufReader::new(read), Arc::downgrade(&shared), events_tx));
        *shared.reader.lock().unwrap_or_else(|e| e.into_inner()) = Some(reader);
        let mut client = Client { shared, params: hello.clone(), hello: no_hello(), process: None };
        let value = client
            .call_value(
                method::HELLO,
                serde_json::to_value(&hello).unwrap_or_default(),
                Some(REQUEST_TIMEOUT),
            )
            .await?;
        client.hello = serde_json::from_value(value).map_err(|e| ClientError::Protocol(e.to_string()))?;
        // Read while the connection is open, so the pid is still the host's.
        client.process = peer
            .filter(|&pid| u32::try_from(pid).ok() == Some(client.hello.pid))
            .and_then(|_| HostProcess::read(Path::new(PROC), client.hello.pid));
        Ok((client, Events { rx: events_rx }))
    }

    /// Connects to the host, starting it first if it isn't running (§1.3).
    /// An outdated host that runs no thread is stopped and the current one
    /// started instead; one with threads running is kept, and
    /// [`Client::outdated`] says so.
    pub async fn connect_or_spawn(client: ClientKind) -> Result<(Client, Events), ClientError> {
        Client::connect_or_spawn_with(&Launch::current(), client).await
    }

    /// [`Client::connect_or_spawn`] for the host `launch` describes.
    pub async fn connect_or_spawn_with(
        launch: &Launch,
        kind: ClientKind,
    ) -> Result<(Client, Events), ClientError> {
        Client::connect_or_spawn_as(launch, hello_params(kind)).await
    }

    /// [`Client::connect_or_spawn_with`] saying `hello` (its capabilities,
    /// say), to this host and to one that replaces it.
    pub async fn connect_or_spawn_as(
        launch: &Launch,
        hello: HelloParams,
    ) -> Result<(Client, Events), ClientError> {
        let (client, events) = match Client::connect(&launch.socket, hello.clone()).await {
            Err(ClientError::Io(e)) if is_not_running(&e) => {
                launch.spawn()?;
                // A host just started is the one this build finds; it isn't replaced again.
                return Client::connect_started(launch, &hello).await;
            }
            other => other?,
        };
        match client.replace(launch, When::Idle).await {
            Ok(Some(replaced)) => Ok(replaced),
            Ok(None) => Ok((client, events)),
            Err(e) => {
                tracing::warn!(error = %e, "the outdated agent host wasn't replaced");
                Ok((client, events))
            }
        }
    }

    /// Connects to a host that was just started, retrying for [`SPAWN_WAIT`].
    async fn connect_started(launch: &Launch, hello: &HelloParams) -> Result<(Client, Events), ClientError> {
        let deadline = Instant::now() + SPAWN_WAIT;
        loop {
            tokio::time::sleep(SPAWN_RETRY).await;
            match Client::connect(&launch.socket, hello.clone()).await {
                Err(ClientError::Io(e)) if is_not_running(&e) && Instant::now() < deadline => continue,
                Err(ClientError::Io(e)) if is_not_running(&e) => return Err(ClientError::NotStarted),
                other => return other,
            }
        }
    }

    /// What the host said to `hello`.
    pub fn host(&self) -> &HelloResult {
        &self.hello
    }

    /// The host process, when it could be told for sure (a `g7-agents` of
    /// this user, the pid `hello` gave matching the socket's peer).
    pub fn process(&self) -> Option<&HostProcess> {
        self.process.as_ref()
    }

    /// Why this host should give way to one of `version` ([`VERSION`] for
    /// this build), if it should.
    pub fn outdated(&self, version: &str) -> Option<Outdated> {
        if is_older(&self.hello.host_version, version) {
            Some(Outdated::Older)
        } else if self.process.as_ref().is_some_and(HostProcess::exe_replaced) {
            Some(Outdated::Replaced)
        } else {
            None
        }
    }

    /// Threads working or waiting on this host.
    pub async fn running_threads(&self) -> Result<usize, ClientError> {
        const PAGE: u32 = 100;
        let mut running = 0;
        let mut before = None;
        for _ in 0..RUNNING_PAGES {
            let page = self.threads_list(Some(PAGE), before).await?;
            running += page.iter().filter(|t| t.state.is_running()).count();
            if page.len() < PAGE as usize {
                break;
            }
            before = page.last().map(|t| t.updated_ms);
        }
        Ok(running)
    }

    /// Replaces this host when it is outdated for `launch` and `when` allows:
    /// stops it (SIGTERM to the process read at connect, verified again),
    /// waits until it has exited, starts the current one and connects to it.
    /// `Ok(None)` when it is current, or threads run on it and `when` is
    /// [`When::Idle`]: it's kept, to be replaced once they finish.
    pub async fn replace(
        &self,
        launch: &Launch,
        when: When,
    ) -> Result<Option<(Client, Events)>, ClientError> {
        let Some(why) = self.outdated(&launch.version) else { return Ok(None) };
        if when == When::Idle && self.running_threads().await? > 0 {
            return Ok(None);
        }
        let process = self.process.as_ref().ok_or(ClientError::NotStopped)?;
        tracing::info!(
            ?why,
            host = %self.hello.host_version,
            ours = %launch.version,
            pid = process.pid,
            "replacing the agent host"
        );
        process.terminate(Path::new(PROC))?;
        if !process.exited(Path::new(PROC), STOP_WAIT).await {
            return Err(ClientError::NotStopped);
        }
        launch.spawn()?;
        Client::connect_started(launch, &self.params).await.map(Some)
    }

    /// Sends a request and waits for its result, at most `timeout` (none:
    /// forever).
    pub async fn call_value(
        &self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value, ClientError> {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let line = encode_request(id, method, params)?;
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.shared.pending.lock().unwrap_or_else(|e| e.into_inner());
            match pending.as_mut() {
                Some(map) => map.insert(id, tx),
                None => return Err(ClientError::Closed),
            };
        }
        let forget = |shared: &Shared| {
            if let Some(map) = shared.pending.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                map.remove(&id);
            }
        };
        let write = async {
            let mut writer = self.shared.writer.lock().await;
            writer.write_all(line.as_bytes()).await
        };
        if let Err(e) = write.await {
            forget(&self.shared);
            return Err(e.into());
        }
        let reply = match timeout {
            Some(limit) => match tokio::time::timeout(limit, rx).await {
                Ok(reply) => reply,
                Err(_) => {
                    forget(&self.shared);
                    return Err(ClientError::Timeout);
                }
            },
            None => rx.await,
        };
        reply.unwrap_or(Err(ClientError::Closed))
    }

    /// Runs a typed call with [`REQUEST_TIMEOUT`].
    pub async fn run<T>(&self, call: Call<T>) -> Result<T, ClientError> {
        let value = self.call_value(call.method, call.params.clone(), Some(REQUEST_TIMEOUT)).await?;
        call.parse(value)
    }

    with_calls!(async_methods);
}

async fn read_loop(
    mut reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    shared: Weak<Shared>,
    events: mpsc::Sender<Event>,
) {
    let mut buf = Vec::new();
    loop {
        match lines::read_line(&mut reader, &mut buf, MAX_REPLY_LINE).await {
            Ok(LineRead::Line) => {}
            Ok(LineRead::Eof | LineRead::TooLong) | Err(_) => break,
        }
        let Ok(text) = std::str::from_utf8(&buf) else { continue };
        match ServerLine::parse(text) {
            Ok(ServerLine::Response(response)) => {
                let Some(shared) = shared.upgrade() else { break };
                let waiter = shared
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_mut()
                    .and_then(|map| map.remove(&response.id));
                // A reply nobody waits for (timed out, or never asked) is dropped.
                if let Some(waiter) = waiter {
                    let _ = waiter.send(into_result(response));
                }
            }
            Ok(ServerLine::Event(line)) => {
                if let Some(event) = Event::from_line(&line)
                    && events.try_send(event).is_err()
                {
                    tracing::warn!("agent host events are not being read; dropped one");
                }
            }
            Err(_) => tracing::debug!("ignored a line from the agent host that isn't JSON"),
        }
    }
    if let Some(shared) = shared.upgrade() {
        // Dropping the waiters makes every outstanding call fail with Closed.
        shared.pending.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

// ---- Blocking client ----

/// A connection to the host for code without a tokio runtime. Events that
/// arrive while waiting for a reply are kept for [`BlockingClient::next_event`].
pub struct BlockingClient {
    reader: StdBufReader<StdUnixStream>,
    writer: StdUnixStream,
    partial: Vec<u8>,
    next_id: u64,
    events: VecDeque<Event>,
    hello: HelloResult,
    timeout: Duration,
}

impl std::fmt::Debug for BlockingClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockingClient").field("hello", &self.hello).finish_non_exhaustive()
    }
}

enum Incoming {
    Line(ServerLine),
    TimedOut,
}

impl BlockingClient {
    /// Connects to the socket at `path` and says `hello`.
    pub fn connect(path: &Path, hello: HelloParams) -> Result<BlockingClient, ClientError> {
        if let Some(dir) = path.parent()
            && !check_private_dir(dir)?
        {
            return Err(io::Error::from(io::ErrorKind::NotFound).into());
        }
        let stream = StdUnixStream::connect(path)?;
        let writer = stream.try_clone()?;
        let mut client = BlockingClient {
            reader: StdBufReader::new(stream),
            writer,
            partial: Vec::new(),
            next_id: 1,
            events: VecDeque::new(),
            hello: no_hello(),
            timeout: REQUEST_TIMEOUT,
        };
        let value = client.call_value(method::HELLO, serde_json::to_value(&hello).unwrap_or_default())?;
        client.hello = serde_json::from_value(value).map_err(|e| ClientError::Protocol(e.to_string()))?;
        Ok(client)
    }

    /// Connects to the host, starting it first if it isn't running (§1.3).
    pub fn connect_or_spawn(client: ClientKind) -> Result<BlockingClient, ClientError> {
        let path = socket_path();
        match BlockingClient::connect(&path, hello_params(client)) {
            Err(ClientError::Io(e)) if is_not_running(&e) => {}
            other => return other,
        }
        let bin = find_host_binary().ok_or(ClientError::NotInstalled)?;
        spawn_host(&bin)?;
        let deadline = Instant::now() + SPAWN_WAIT;
        loop {
            std::thread::sleep(SPAWN_RETRY);
            match BlockingClient::connect(&path, hello_params(client)) {
                Err(ClientError::Io(e)) if is_not_running(&e) && Instant::now() < deadline => continue,
                Err(ClientError::Io(e)) if is_not_running(&e) => return Err(ClientError::NotStarted),
                other => return other,
            }
        }
    }

    pub fn host(&self) -> &HelloResult {
        &self.hello
    }

    /// How long a call waits for its reply (default [`REQUEST_TIMEOUT`]).
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    fn read_incoming(&mut self, timeout: Option<Duration>) -> Result<Incoming, ClientError> {
        self.reader.get_ref().set_read_timeout(timeout)?;
        loop {
            match lines::read_line_blocking(&mut self.reader, &mut self.partial, MAX_REPLY_LINE) {
                Ok(LineRead::Line) => {}
                Ok(LineRead::Eof) => return Err(ClientError::Closed),
                Ok(LineRead::TooLong) => {
                    let _ = self.writer.shutdown(std::net::Shutdown::Both);
                    return Err(ClientError::Protocol("line too long".into()));
                }
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                    return Ok(Incoming::TimedOut);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
            let parsed =
                std::str::from_utf8(&self.partial).ok().and_then(|text| ServerLine::parse(text).ok());
            self.partial.clear();
            if let Some(line) = parsed {
                return Ok(Incoming::Line(line));
            }
        }
    }

    fn keep_event(&mut self, line: &EventLine) {
        if let Some(event) = Event::from_line(line) {
            if self.events.len() >= EVENT_BUFFER {
                self.events.pop_front();
            }
            self.events.push_back(event);
        }
    }

    /// Sends a request and waits for its result.
    pub fn call_value(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let line = encode_request(id, method, params)?;
        self.writer.write_all(line.as_bytes())?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(ClientError::Timeout);
            }
            match self.read_incoming(Some(left))? {
                Incoming::TimedOut => return Err(ClientError::Timeout),
                Incoming::Line(ServerLine::Event(event)) => self.keep_event(&event),
                Incoming::Line(ServerLine::Response(response)) if response.id == id => {
                    return into_result(response);
                }
                Incoming::Line(ServerLine::Response(_)) => {}
            }
        }
    }

    /// Runs a typed call.
    pub fn run<T>(&mut self, call: Call<T>) -> Result<T, ClientError> {
        let value = self.call_value(call.method, call.params.clone())?;
        call.parse(value)
    }

    /// The next event, waiting at most `timeout` (none: forever). `Ok(None)`
    /// when the wait ran out.
    pub fn next_event(&mut self, timeout: Option<Duration>) -> Result<Option<Event>, ClientError> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            let left = match deadline {
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Ok(None);
                    }
                    Some(left)
                }
                None => None,
            };
            match self.read_incoming(left)? {
                Incoming::TimedOut => return Ok(None),
                Incoming::Line(ServerLine::Event(line)) => {
                    if let Some(event) = Event::from_line(&line) {
                        return Ok(Some(event));
                    }
                }
                Incoming::Line(ServerLine::Response(_)) => {}
            }
        }
    }

    with_calls!(blocking_methods);
}

/// A typed result from a raw value (for callers of `call_value`).
pub fn parse_result<T: DeserializeOwned>(value: Value) -> Result<T, ClientError> {
    serde_json::from_value(value).map_err(|e| ClientError::Protocol(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncBufReadExt;
    use tokio::net::UnixListener;

    fn private_tempdir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    #[test]
    fn private_dir_check() {
        let dir = private_tempdir();
        assert!(check_private_dir(dir.path()).unwrap());
        assert!(!check_private_dir(&dir.path().join("missing")).unwrap());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(check_private_dir(dir.path()), Err(ClientError::Insecure(_))));
        let nested = private_tempdir();
        let sub = nested.path().join("a/b");
        ensure_private_dir(&sub).unwrap();
        assert_eq!(std::fs::metadata(&sub).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[tokio::test]
    async fn async_client_matches_replies_and_passes_events() {
        let dir = private_tempdir();
        let path = dir.path().join("host.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = tokio::io::BufReader::new(read).lines();
            let hello: Request = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(hello.method, "hello");
            assert_eq!(hello.params["client"], "test");
            let reply = format!(
                "{{\"event\":\"ui.open\",\"params\":{{\"thread_id\":\"ab\"}}}}\n{{\"id\":{},\"result\":{{\"version\":1,\"host_version\":\"x\",\"pid\":3}}}}\n",
                hello.id
            );
            write.write_all(reply.as_bytes()).await.unwrap();
            let req: Request = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(req.method, "folders.remove");
            let reply = format!(
                "{{\"id\":999,\"result\":{{}}}}\n{{\"id\":{},\"error\":{{\"code\":\"NOT_FOUND\",\"message\":\"no\"}}}}\n",
                req.id
            );
            write.write_all(reply.as_bytes()).await.unwrap();
            let req: Request = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            let reply = format!("{{\"id\":{},\"result\":{{\"folders\":[]}}}}\n", req.id);
            write.write_all(reply.as_bytes()).await.unwrap();
        });
        let (client, mut events) =
            Client::connect(&path, HelloParams::new(ClientKind::Test, &[])).await.unwrap();
        assert_eq!(client.host().pid, 3);
        let err = client.folders_remove("f000000000000".into()).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::NotFound));
        assert!(client.folders_list().await.unwrap().is_empty());
        assert_eq!(events.recv().await, Some(Event::UiOpen(ThreadIdParams { thread_id: "ab".into() })));
        server.await.unwrap();
        // The host is gone: calls fail as closed and the events end.
        assert!(events.recv().await.is_none());
        assert!(matches!(client.folders_list().await, Err(ClientError::Closed | ClientError::Io(_))));
    }

    #[test]
    fn blocking_client_keeps_events_seen_while_waiting() {
        let dir = private_tempdir();
        let path = dir.path().join("host.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            use std::io::BufRead;
            let (stream, _) = listener.accept().unwrap();
            let mut write = stream.try_clone().unwrap();
            let mut lines = std::io::BufReader::new(stream).lines();
            for n in 0..2 {
                let req: Request = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
                let result =
                    if n == 0 { r#"{"version":1,"host_version":"x","pid":3}"# } else { r#"{"agents":[]}"# };
                let reply = format!(
                    "{{\"event\":\"thread.removed\",\"params\":{{\"thread_id\":\"a{n}\"}}}}\n{{\"id\":{},\"result\":{result}}}\n",
                    req.id
                );
                write.write_all(reply.as_bytes()).unwrap();
            }
        });
        let mut client = BlockingClient::connect(&path, HelloParams::new(ClientKind::Test, &[])).unwrap();
        assert!(client.agents_list(false).unwrap().is_empty());
        server.join().unwrap();
        let first = client.next_event(Some(Duration::from_secs(1))).unwrap().unwrap();
        assert_eq!(first.thread_id(), Some("a0"));
        let second = client.next_event(Some(Duration::from_secs(1))).unwrap().unwrap();
        assert_eq!(second.thread_id(), Some("a1"));
        assert!(matches!(client.next_event(Some(Duration::from_millis(50))), Err(ClientError::Closed)));
    }

    #[tokio::test]
    async fn a_full_size_export_reaches_the_client() {
        let dir = private_tempdir();
        let path = dir.path().join("host.sock");
        let listener = UnixListener::bind(&path).unwrap();
        // The longest a reply gets: every byte of the text escaped.
        let text = "\"".repeat(MAX_EXPORT);
        let sent = text.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = tokio::io::BufReader::new(read).lines();
            let hello: Request = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            let reply = format!(
                "{{\"id\":{},\"result\":{{\"version\":1,\"host_version\":\"x\",\"pid\":3}}}}\n",
                hello.id
            );
            write.write_all(reply.as_bytes()).await.unwrap();
            let req: Request = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(req.method, "thread.export");
            let result = ThreadExportResult {
                text: sent,
                truncated: false,
                file_name: "claude-x-1970-01-01.md".into(),
            };
            let line = serde_json::json!({ "id": req.id, "result": result }).to_string();
            assert!(line.len() > MAX_LINE && line.len() <= MAX_REPLY_LINE, "{}", line.len());
            write.write_all(format!("{line}\n").as_bytes()).await.unwrap();
            let _ = lines.next_line().await;
        });
        let (client, _events) =
            Client::connect(&path, HelloParams::new(ClientKind::Test, &[])).await.unwrap();
        let export = client
            .thread_export(ThreadExportParams {
                thread_id: Some("0123456789abcdef0123456789abcdef".into()),
                session_id: None,
                format: ExportFormat::Markdown,
            })
            .await
            .unwrap();
        assert_eq!(export.text, text);
        drop(client);
        server.await.unwrap();
    }

    #[test]
    fn versions_compare_by_their_numbers() {
        assert!(is_older("0.1.13", "0.1.14"));
        assert!(is_older("0.1.9", "0.1.10"), "numbers, not text");
        assert!(is_older("0.1", "0.1.1"));
        assert!(is_older("0.1.14-rc1", "0.1.15"));
        assert!(!is_older("0.1.14", "0.1.14"));
        assert!(!is_older("0.1.14", "0.1.14+build5"));
        assert!(!is_older("0.2.0", "0.1.99"));
        assert!(!is_older("test", "0.1.14"), "a version that can't be read is never older");
        assert!(!is_older("", "0.1.14"));
        assert!(!is_older("0.1.13", "x"));
    }

    /// A fake `/proc/<pid>` with its `stat` (`state`, `start`) and `exe`.
    fn fake_proc(root: &Path, pid: u32, state: &str, start: u64, exe: &str) {
        let dir = root.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let stat = format!(
            "{pid} (g7 (agents)) {state} 1 {pid} {pid} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {start} 1000 1 0"
        );
        std::fs::write(dir.join("stat"), stat).unwrap();
        std::os::unix::fs::symlink(exe, dir.join("exe")).unwrap();
    }

    #[test]
    fn a_host_process_is_a_live_g7_agents_and_an_upgrade_shows() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fake_proc(root, 10, "S", 4242, "/usr/lib/g7link/g7-agents");
        fake_proc(root, 11, "S", 4343, "/usr/lib/g7link/g7-agents (deleted)");
        fake_proc(root, 12, "S", 1, "/usr/bin/sleep");
        fake_proc(root, 13, "Z", 1, "/usr/lib/g7link/g7-agents");
        let current = HostProcess::read(root, 10).unwrap();
        assert_eq!(
            (current.pid(), current.start_ticks),
            (10, 4242),
            "a ')' in the name doesn't shift fields"
        );
        assert!(!current.exe_replaced());
        assert!(HostProcess::read(root, 11).unwrap().exe_replaced());
        assert_eq!(HostProcess::read(root, 12), None, "another program is never taken for the host");
        assert_eq!(HostProcess::read(root, 13), None, "nor a process that has exited");
        assert_eq!(HostProcess::read(root, 14), None);
        assert_eq!(HostProcess::read(root, 0), None);
    }

    /// A process whose program is called `g7-agents` (a copy of `sh`
    /// waiting on stdin), to signal for real.
    fn standin(dir: &Path) -> (std::process::Child, std::process::ChildStdin) {
        let sh = std::fs::canonicalize("/bin/sh").unwrap();
        let exe = dir.join("g7-agents");
        std::fs::copy(&sh, &exe).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Another test forking while the copy was open for writing leaves
        // that child holding it until it execs: "text file busy" for a moment.
        let mut tries = 0;
        let mut child = loop {
            match Command::new(&exe).args(["-c", "read line"]).stdin(Stdio::piped()).spawn() {
                Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy && tries < 50 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(20));
                }
                other => break other.unwrap(),
            }
        };
        let stdin = child.stdin.take().unwrap();
        (child, stdin)
    }

    async fn read_live(pid: u32) -> HostProcess {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(process) = HostProcess::read(Path::new(PROC), pid) {
                return process;
            }
            assert!(Instant::now() < deadline, "the stand-in host didn't start");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn only_the_process_read_at_connect_is_stopped() {
        use std::os::unix::process::ExitStatusExt;
        let dir = private_tempdir();
        let (mut child, _stdin) = standin(dir.path());
        let process = read_live(child.id()).await;
        assert!(!process.exe_replaced());
        // Another process with the same pid would have started at another time.
        let reused = HostProcess { start_ticks: process.start_ticks + 1, ..process.clone() };
        assert!(matches!(reused.terminate(Path::new(PROC)), Err(ClientError::NotStopped)));
        assert!(child.try_wait().unwrap().is_none(), "nothing was signalled");
        process.terminate(Path::new(PROC)).unwrap();
        assert!(process.exited(Path::new(PROC), Duration::from_secs(5)).await, "a zombie counts as gone");
        assert_eq!(child.wait().unwrap().signal(), Some(15));
    }

    #[tokio::test]
    async fn a_replaced_program_shows_on_its_process() {
        let dir = private_tempdir();
        let (mut child, _stdin) = standin(dir.path());
        read_live(child.id()).await;
        std::fs::remove_file(dir.path().join("g7-agents")).unwrap();
        let process = read_live(child.id()).await;
        assert!(process.exe_replaced());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn unknown_methods_are_told_apart() {
        let old = ClientError::Remote(RpcError::new(ErrorCode::BadRequest, UNKNOWN_METHOD));
        assert!(old.is_unknown_method());
        let shape = ClientError::Remote(RpcError::new(ErrorCode::BadRequest, "bad thread list"));
        assert!(!shape.is_unknown_method());
        assert!(!ClientError::Closed.is_unknown_method());
    }

    #[test]
    fn a_world_readable_socket_dir_is_refused() {
        let dir = private_tempdir();
        let open = dir.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        let err =
            BlockingClient::connect(&open.join("host.sock"), hello_params(ClientKind::Test)).unwrap_err();
        assert!(matches!(err, ClientError::Insecure(_)));
    }
}
