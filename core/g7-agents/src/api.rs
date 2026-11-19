// SPDX-License-Identifier: MPL-2.0
//! The host socket API, version 1: every type the host and its clients
//! exchange, the limits, the method and event names and the error codes.
//!
//! Field names are snake_case on the wire; times are Unix milliseconds.
//! Unknown fields are ignored on both sides.

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The API version this crate speaks.
pub const API_VERSION: u32 = 1;

// ---- Limits (bytes of UTF-8 unless noted) ----

/// A request or event line, without its newline.
pub const MAX_LINE: usize = 262_144;
/// Connected clients at once.
pub const MAX_CLIENTS: usize = 16;
/// Lines queued for one client before deltas are dropped.
pub const CLIENT_QUEUE: usize = 1024;
/// Folders listed.
pub const MAX_FOLDERS: usize = 64;
/// Threads kept on disk; the oldest not running are pruned first.
pub const MAX_THREADS_KEPT: usize = 200;
/// Threads `working` or `waiting` at once, all agents together.
pub const MAX_RUNNING: usize = 4;
/// Threads `working` or `waiting` at once for one agent.
pub const MAX_RUNNING_PER_AGENT: usize = 3;
/// Items kept per thread.
pub const MAX_ITEMS_KEPT: usize = 2000;
/// A prompt or a follow-up text.
pub const MAX_PROMPT: usize = 16_384;
pub const MAX_ITEM_TEXT: usize = 16_384;
pub const MAX_TOOL_DETAIL: usize = 4096;
pub const MAX_TOOL_OUTPUT: usize = 2048;
pub const MAX_TOOL_NAME: usize = 64;
pub const MAX_TOOL_TITLE: usize = 200;
pub const MAX_ITEM_ID: usize = 96;
pub const MAX_APPROVAL_TITLE: usize = 200;
pub const MAX_APPROVAL_COMMAND: usize = 2048;
pub const MAX_APPROVAL_DIFF: usize = 8192;
pub const MAX_APPROVAL_PATH: usize = 512;
pub const MAX_APPROVAL_REASON: usize = 300;
/// Approvals waiting per thread; a ninth is denied at once.
pub const MAX_PENDING_PER_THREAD: usize = 8;
/// How long an approval waits for an answer.
pub const APPROVAL_LIFETIME_MS: u64 = 7_200_000;
pub const MAX_LABEL: usize = 64;
pub const MAX_PATH: usize = 4096;
pub const MAX_TITLE: usize = 80;
pub const MAX_SUMMARY: usize = 280;
pub const MAX_ERROR_CODE: usize = 40;
pub const MAX_ERROR_MESSAGE: usize = 200;
pub const MAX_VERSION: usize = 32;
pub const MAX_PLAN: usize = 32;
pub const MAX_PROBLEM: usize = 200;
pub const MAX_NATIVE_ID: usize = 64;
pub const MAX_DEVICE_ID: usize = 64;
pub const MAX_DEVICE_NAME: usize = 64;
/// A `thread.delta` text.
pub const MAX_DELTA_TEXT: usize = 4096;
/// Deltas are flushed this often, or at [`DELTA_FLUSH_BYTES`].
pub const DELTA_FLUSH_MS: u64 = 100;
pub const DELTA_FLUSH_BYTES: usize = 2048;
/// `thread.read`: the whole result is kept under this size.
pub const MAX_READ_RESULT: usize = 200_000;
pub const MAX_READ_LIMIT: u32 = 200;
pub const DEFAULT_READ_LIMIT: u32 = 50;
pub const MAX_LIST_LIMIT: u32 = 100;
pub const DEFAULT_LIST_LIMIT: u32 = 50;
/// Thread ids in one `subscribe`.
pub const MAX_SUBSCRIBE_THREADS: usize = 32;
/// `bridge.ask` fields.
pub const MAX_BRIDGE_TOOL_NAME: usize = 128;
pub const MAX_BRIDGE_TOOL_USE_ID: usize = 128;
/// A line from an agent CLI; longer stops the process.
pub const MAX_AGENT_LINE: usize = 4 * 1024 * 1024;
/// An agent process idle this long after a turn has its stdin closed.
pub const AGENT_IDLE_MS: u64 = 600_000;
/// How long a probe result is reused.
pub const PROBE_CACHE_MS: u64 = 60_000;
/// An error message from the host.
pub const MAX_RPC_ERROR_MESSAGE: usize = 200;

// ---- Sessions started elsewhere (0.1.14) ----

/// Sessions listed at most.
pub const MAX_SESSIONS: usize = 50;
/// Only sessions whose file changed this recently are listed (7 days).
pub const SESSION_WINDOW_MS: u64 = 604_800_000;
/// Distinct sessions watched at once, all connections together.
pub const MAX_WATCHED_SESSIONS: usize = 4;
/// The bytes read from the end of a session file.
pub const SESSION_TAIL: u64 = 262_144;
/// A window is doubled up to this when it holds no whole line; a longer line is skipped.
pub const SESSION_TAIL_MAX: u64 = 4_194_304;
/// A watched file is read at most this much per tick.
pub const SESSION_READ_STEP: u64 = 1_048_576;
/// Items kept in memory per watched session.
pub const SESSION_ITEMS_KEPT: usize = 400;
pub const SESSION_LIST_POLL_MS: u64 = 5000;
pub const SESSION_WATCH_POLL_MS: u64 = 500;
/// An open tool call this quiet is a guessed "needs you".
pub const SESSION_QUIET_MS: u64 = 15_000;
/// A thread's or a session's `step`.
pub const MAX_STEP: usize = 120;
/// A session's `waiting_for`.
pub const MAX_WAITING_FOR: usize = 80;
/// Entries in a thread's plan.
pub const MAX_PLAN_STEPS: usize = 20;
/// One plan entry's text.
pub const MAX_PLAN_TEXT: usize = 200;
/// A `thread.export` text.
pub const MAX_EXPORT: usize = 2 * 1024 * 1024;
/// A line a client reads from the host: a reply to `thread.export` can be
/// longer than [`MAX_LINE`], its text up to twice as long as JSON (`"`, `\`,
/// newline and tab are escaped; other controls never reach it).
pub const MAX_REPLY_LINE: usize = MAX_LINE + 2 * MAX_EXPORT;
/// Items copied into a thread by `session.continue`.
pub const CONTINUE_ITEMS: usize = 200;
/// A Claude status file is read up to this size.
pub const MAX_STATUS_FILE: u64 = 16_384;
/// Process entries looked at per scan.
pub const MAX_PROC_SCAN: usize = 4096;

// ---- Method and event names ----

pub mod method {
    pub const HELLO: &str = "hello";
    pub const AGENTS_LIST: &str = "agents.list";
    pub const FOLDERS_LIST: &str = "folders.list";
    pub const FOLDERS_ADD: &str = "folders.add";
    pub const FOLDERS_REMOVE: &str = "folders.remove";
    pub const THREADS_LIST: &str = "threads.list";
    pub const THREAD_START: &str = "thread.start";
    pub const THREAD_SEND: &str = "thread.send";
    pub const THREAD_INTERRUPT: &str = "thread.interrupt";
    pub const THREAD_READ: &str = "thread.read";
    pub const THREAD_DELETE: &str = "thread.delete";
    pub const THREAD_TERMINAL_COMMAND: &str = "thread.terminal_command";
    pub const APPROVAL_ANSWER: &str = "approval.answer";
    pub const SUBSCRIBE: &str = "subscribe";
    pub const UNSUBSCRIBE: &str = "unsubscribe";
    pub const UI_VIEWING: &str = "ui.viewing";
    pub const BRIDGE_ASK: &str = "bridge.ask";
    pub const BRIDGE_CANCEL: &str = "bridge.cancel";
    pub const SESSIONS_LIST: &str = "sessions.list";
    pub const SESSION_READ: &str = "session.read";
    pub const SESSION_WATCH: &str = "session.watch";
    pub const SESSION_CONTINUE: &str = "session.continue";
    pub const SESSIONS_SET_FOLLOWING: &str = "sessions.set_following";
    pub const SETTINGS_GET: &str = "settings.get";
    pub const THREAD_EXPORT: &str = "thread.export";
}

pub mod event {
    pub const THREAD_STATE: &str = "thread.state";
    pub const THREAD_ITEM: &str = "thread.item";
    pub const THREAD_DELTA: &str = "thread.delta";
    pub const APPROVAL_REQUESTED: &str = "approval.requested";
    pub const APPROVAL_RESOLVED: &str = "approval.resolved";
    pub const THREAD_DONE: &str = "thread.done";
    pub const THREAD_REMOVED: &str = "thread.removed";
    pub const AGENTS_CHANGED: &str = "agents.changed";
    pub const FOLDERS_CHANGED: &str = "folders.changed";
    pub const UI_OPEN: &str = "ui.open";
    pub const SESSION_STATE: &str = "session.state";
    pub const SESSION_REMOVED: &str = "session.removed";
    pub const SESSION_ITEM: &str = "session.item";
    pub const SETTINGS_CHANGED: &str = "settings.changed";
}

// ---- Error codes ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// A request's shape or a limit.
    BadRequest,
    NotFound,
    Denied,
    Busy,
    /// A count is at its maximum.
    Limit,
    /// The agent is not installed or not signed in.
    Unavailable,
    AlreadyAnswered,
    UnsupportedVersion,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BadRequest => "BAD_REQUEST",
            Self::NotFound => "NOT_FOUND",
            Self::Denied => "DENIED",
            Self::Busy => "BUSY",
            Self::Limit => "LIMIT",
            Self::Unavailable => "UNAVAILABLE",
            Self::AlreadyAnswered => "ALREADY_ANSWERED",
            Self::UnsupportedVersion => "UNSUPPORTED_VERSION",
            Self::Internal => "INTERNAL",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The `error` member of a failed response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl RpcError {
    /// An error with `message` cut to [`MAX_RPC_ERROR_MESSAGE`].
    pub fn new(code: ErrorCode, message: impl AsRef<str>) -> Self {
        Self { code, message: cut(message.as_ref(), MAX_RPC_ERROR_MESSAGE).to_owned() }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

// ---- Enums ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentId {
    Claude,
    Codex,
}

impl AgentId {
    pub const ALL: [AgentId; 2] = [AgentId::Claude, AgentId::Codex];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// The product name shown to people ("Claude", "Codex"). Not translated.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }

    /// The command that signs the CLI in, run by the user in a terminal.
    pub fn login_hint(self) -> &'static str {
        match self {
            Self::Claude => "claude auth login",
            Self::Codex => "codex login",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    Subscription,
    ApiKey,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginKind {
    Launcher,
    Phone,
    Cli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Asks before every edit and every command that isn't read-only.
    Supervised,
    /// Edits without asking; still asks before commands.
    AutoEdit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// A turn is running.
    Working,
    /// A turn is running and at least one approval is pending.
    Waiting,
    /// The last turn finished.
    Done,
    Failed,
    Interrupted,
}

impl State {
    /// `working` or `waiting`: a turn is running.
    pub fn is_running(self) -> bool {
        matches!(self, Self::Working | Self::Waiting)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    User,
    Assistant,
    Tool,
    Notice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Ok,
    Error,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    Command,
    Edit,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
    /// Allow this and every later matching request in the thread (PC only).
    AllowThread,
}

/// How a turn ended (`thread.done`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Failed,
    Interrupted,
}

impl Outcome {
    pub fn state(self) -> State {
        match self {
            Self::Done => State::Done,
            Self::Failed => State::Failed,
            Self::Interrupted => State::Interrupted,
        }
    }
}

/// How an approval ended (`approval.resolved`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalOutcome {
    Allowed,
    Denied,
    /// Timed out: denied for the agent.
    Expired,
    /// The turn ended first.
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Launcher,
    Link,
    Bridge,
    Cli,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeBehavior {
    Allow,
    Deny,
}

/// What a session started outside G7 is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Working,
    /// The agent reported it waits for the user (Claude's status file).
    NeedsYou,
    /// Inferred from the file: it may be waiting, or running a long command.
    /// Never counted with `needs_you`.
    NeedsYouGuess,
    Idle,
    /// No process has it.
    Ended,
}

/// One entry of a thread's plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub text: String,
    pub status: PlanStatus,
}

/// Where a session was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSource {
    Terminal,
    DesktopApp,
    Other,
}

/// Why a session can't be continued here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinueBlock {
    /// Something still has it open.
    Open,
    /// Its folder isn't one of the agent folders.
    Folder,
    /// The agent isn't installed or signed in.
    Agent,
}

// ---- Records ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: AgentId,
    /// "Claude" or "Codex".
    pub name: String,
    pub installed: bool,
    pub signed_in: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthKind>,
    /// The plan as the CLI reports it ("max", "plus"). Shown only in G7 Link
    /// desktop Settings; never sent to the phone or the launcher.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// "claude auth login" or "codex login".
    pub login_hint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

impl AgentInfo {
    /// An agent that is not installed.
    pub fn missing(id: AgentId) -> Self {
        Self {
            id,
            name: id.display_name().to_owned(),
            installed: false,
            signed_in: false,
            version: None,
            auth: None,
            plan: None,
            login_hint: id.login_hint().to_owned(),
            problem: None,
        }
    }

    /// Installed and signed in: a thread can start.
    pub fn ready(&self) -> bool {
        self.installed && self.signed_in
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    /// "f" and 12 lowercase hex digits, issued by the host.
    pub id: String,
    pub label: String,
    /// Absolute. Never leaves the PC.
    pub path: String,
    #[serde(default)]
    pub exists: bool,
    pub added_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub kind: OriginKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
}

impl Origin {
    pub fn launcher() -> Self {
        Self { kind: OriginKind::Launcher, device_id: None, device_name: None }
    }

    pub fn cli() -> Self {
        Self { kind: OriginKind::Cli, device_id: None, device_name: None }
    }

    pub fn phone(device_id: impl Into<String>, device_name: impl Into<String>) -> Self {
        Self {
            kind: OriginKind::Phone,
            device_id: Some(device_id.into()),
            device_name: Some(device_name.into()),
        }
    }

    /// Within the limits.
    pub fn is_valid(&self) -> bool {
        self.device_id.as_ref().is_none_or(|s| s.len() <= MAX_DEVICE_ID)
            && self.device_name.as_ref().is_none_or(|s| s.len() <= MAX_DEVICE_NAME)
    }
}

/// Who answered an approval.
pub type By = Origin;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadInfo {
    /// 32 lowercase hex digits, issued by the host.
    pub id: String,
    pub agent: AgentId,
    pub folder_id: String,
    pub folder_label: String,
    /// The first line of the first prompt.
    pub title: String,
    pub state: State,
    pub mode: Mode,
    pub origin: Origin,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub last_seq: u64,
    /// Approvals waiting.
    pub pending: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ThreadError>,
    /// The Claude session uuid or the Codex thread id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_id: Option<String>,
    /// What the running turn is doing: the tool's own description, else its
    /// title. Absent when no turn runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// The agent's plan (Claude's todo list, Codex's turn plan); absent when it made none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<Vec<PlanStep>>,
    /// When `state` last changed.
    #[serde(default)]
    pub state_since_ms: u64,
    /// The working directory relative to the folder; set only by `session.continue`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdir: Option<String>,
    /// Made by `session.continue`.
    #[serde(default)]
    pub continued: bool,
}

/// A session started outside G7 (a terminal, the Claude or Codex app),
/// followed read-only from its own files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// `"sc"` (Claude) or `"sx"` (Codex) and the agent's UUID without dashes.
    pub id: String,
    pub agent: AgentId,
    pub title: String,
    /// Never leaves the PC.
    pub cwd: String,
    /// Set when `cwd` is inside an agent folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    /// That folder's label, else the last component of `cwd`.
    pub folder_label: String,
    pub state: SessionState,
    /// The state was inferred from the file, not reported by the agent.
    #[serde(default)]
    pub state_guess: bool,
    /// When `state` began (as far as the host saw).
    #[serde(default)]
    pub state_since_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
    pub source: SessionSource,
    /// Some process has the session now.
    pub open: bool,
    pub updated_ms: u64,
    pub last_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub can_continue: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<ContinueBlock>,
}

impl Session {
    /// The order `sessions.list` uses: exact "needs you", working, guessed
    /// "needs you", idle, ended.
    pub fn rank(&self) -> u8 {
        match self.state {
            SessionState::NeedsYou if !self.state_guess => 0,
            SessionState::Working => 1,
            SessionState::NeedsYou | SessionState::NeedsYouGuess => 2,
            SessionState::Idle => 3,
            SessionState::Ended => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// 1-based, per thread, never reused.
    pub seq: u64,
    pub id: String,
    pub kind: ItemKind,
    pub ts_ms: u64,
    /// 0 when created, +1 on each update of the same `seq`.
    pub rev: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated: bool,
    /// Kind `user`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
    /// Kind `tool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<Tool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub title: String,
    /// The command, or a diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub status: ToolStatus,
    /// The tail of the output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    /// 32 lowercase hex digits, random, single use.
    pub token: String,
    pub thread_id: String,
    pub kind: ApprovalKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub can_allow_thread: bool,
    pub created_ms: u64,
    pub expires_ms: u64,
}

// ---- Wire envelope ----

/// A client's request line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// The host's reply to a request: `result` or `error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self { id, result: Some(result), error: None }
    }

    pub fn err(id: u64, error: RpcError) -> Self {
        Self { id, result: None, error: Some(error) }
    }
}

/// An event line as it travels: `{"event": name, "params": {…}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventLine {
    pub event: String,
    #[serde(default)]
    pub params: Value,
}

/// A line the host sends.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerLine {
    Response(Response),
    Event(EventLine),
}

impl ServerLine {
    /// Parses one line from the host. An object with `event` is an event;
    /// one with `id` is a response.
    pub fn parse(line: &str) -> Result<Self, serde_json::Error> {
        let value: Value = serde_json::from_str(line)?;
        if value.get("event").is_some() {
            serde_json::from_value(value).map(Self::Event)
        } else {
            serde_json::from_value(value).map(Self::Response)
        }
    }
}

// ---- Method parameters and results ----

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloParams {
    pub client: ClientKind,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResult {
    pub version: u32,
    pub host_version: String,
    pub pid: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentsListParams {
    #[serde(default, skip_serializing_if = "is_false")]
    pub refresh: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentsListResult {
    pub agents: Vec<AgentInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldersListResult {
    pub folders: Vec<Folder>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldersAddParams {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderResult {
    pub folder: Folder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldersRemoveParams {
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadsListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadsListResult {
    pub threads: Vec<ThreadInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadStartParams {
    pub agent: AgentId,
    pub folder_id: String,
    pub prompt: String,
    pub mode: Mode,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadResult {
    pub thread: ThreadInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSendParams {
    pub thread_id: String,
    pub text: String,
    pub origin: Origin,
}

/// `thread.interrupt`, `thread.delete`, `thread.terminal_command`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadIdParams {
    pub thread_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadReadParams {
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadReadResult {
    pub thread: ThreadInfo,
    /// Ascending `seq`.
    pub items: Vec<Item>,
    pub more_before: bool,
    pub pending: Vec<Approval>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCommandResult {
    pub argv: Vec<String>,
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalAnswerParams {
    pub token: String,
    pub decision: Decision,
    pub by: By,
}

/// Which threads a subscription covers: `"all"` or a list of ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadSelection {
    All,
    Ids(Vec<String>),
}

impl ThreadSelection {
    pub fn contains(&self, thread_id: &str) -> bool {
        match self {
            Self::All => true,
            Self::Ids(ids) => ids.iter().any(|id| id == thread_id),
        }
    }
}

impl Serialize for ThreadSelection {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::All => serializer.serialize_str("all"),
            Self::Ids(ids) => ids.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ThreadSelection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::String(s) if s == "all" => Ok(Self::All),
            value @ Value::Array(_) => {
                serde_json::from_value(value).map(Self::Ids).map_err(de::Error::custom)
            }
            _ => Err(de::Error::custom("expected \"all\" or a list of thread ids")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeParams {
    pub threads: ThreadSelection,
    #[serde(default)]
    pub deltas: bool,
    /// Also `session.state` and `session.removed`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub sessions: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiViewingParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionsListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionsListResult {
    pub sessions: Vec<Session>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReadParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReadResult {
    pub session: Session,
    /// Ascending `seq`. Session seqs are not consecutive.
    pub items: Vec<Item>,
    pub more_before: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionWatchParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// What `thread.export` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Markdown,
}

/// `thread.export`: one of `thread_id` and `session_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadExportParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub format: ExportFormat,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadExportResult {
    /// At most [`MAX_EXPORT`] bytes.
    pub text: String,
    /// Items were left out to stay under the limit.
    pub truncated: bool,
    /// A suggested name: `claude-<title>-<yyyy-mm-dd>.md`.
    pub file_name: String,
}

/// Host-wide settings (`settings.get`, `settings.changed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSettings {
    /// Follow sessions started outside G7 (on by default).
    pub follow_sessions: bool,
}

impl Default for HostSettings {
    fn default() -> Self {
        Self { follow_sessions: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionsSetFollowingParams {
    pub on: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionContinueParams {
    pub session_id: String,
    pub mode: Mode,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeAskParams {
    pub tool_name: String,
    pub input: Value,
    #[serde(default)]
    pub tool_use_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeAskResult {
    pub behavior: BridgeBehavior,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeCancelParams {
    pub tool_use_id: String,
}

// ---- Events ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadItemEvent {
    pub thread_id: String,
    pub item: Item,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadDeltaEvent {
    pub thread_id: String,
    pub seq: u64,
    pub item_id: String,
    /// Byte offset of `text` in the item.
    pub offset: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequestedEvent {
    pub approval: Approval,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalResolvedEvent {
    pub token: String,
    pub thread_id: String,
    pub outcome: ApprovalOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<By>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadDoneEvent {
    pub thread_id: String,
    pub outcome: Outcome,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ThreadError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRemovedEvent {
    pub thread_id: String,
}

/// `session.state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionResult {
    pub session: Session,
}

/// `session.removed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRemovedEvent {
    pub session_id: String,
}

/// `session.item`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionItemEvent {
    pub session_id: String,
    pub item: Item,
}

/// An event from the host, typed. Parse an [`EventLine`] with
/// [`Event::from_line`]; an unknown event name gives `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", content = "params")]
pub enum Event {
    #[serde(rename = "thread.state")]
    ThreadState(ThreadResult),
    #[serde(rename = "thread.item")]
    ThreadItem(ThreadItemEvent),
    #[serde(rename = "thread.delta")]
    ThreadDelta(ThreadDeltaEvent),
    #[serde(rename = "approval.requested")]
    ApprovalRequested(ApprovalRequestedEvent),
    #[serde(rename = "approval.resolved")]
    ApprovalResolved(ApprovalResolvedEvent),
    #[serde(rename = "thread.done")]
    ThreadDone(ThreadDoneEvent),
    #[serde(rename = "thread.removed")]
    ThreadRemoved(ThreadRemovedEvent),
    #[serde(rename = "agents.changed")]
    AgentsChanged(AgentsListResult),
    #[serde(rename = "folders.changed")]
    FoldersChanged(FoldersListResult),
    #[serde(rename = "ui.open")]
    UiOpen(ThreadIdParams),
    #[serde(rename = "session.state")]
    SessionState(SessionResult),
    #[serde(rename = "session.removed")]
    SessionRemoved(SessionRemovedEvent),
    #[serde(rename = "session.item")]
    SessionItem(SessionItemEvent),
    #[serde(rename = "settings.changed")]
    SettingsChanged(HostSettings),
}

impl Event {
    /// The typed event, or `None` for an event this version doesn't know or
    /// whose params don't parse.
    pub fn from_line(line: &EventLine) -> Option<Self> {
        let value = serde_json::json!({ "event": line.event, "params": line.params });
        serde_json::from_value(value).ok()
    }

    /// The event's name on the wire.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ThreadState(_) => event::THREAD_STATE,
            Self::ThreadItem(_) => event::THREAD_ITEM,
            Self::ThreadDelta(_) => event::THREAD_DELTA,
            Self::ApprovalRequested(_) => event::APPROVAL_REQUESTED,
            Self::ApprovalResolved(_) => event::APPROVAL_RESOLVED,
            Self::ThreadDone(_) => event::THREAD_DONE,
            Self::ThreadRemoved(_) => event::THREAD_REMOVED,
            Self::AgentsChanged(_) => event::AGENTS_CHANGED,
            Self::FoldersChanged(_) => event::FOLDERS_CHANGED,
            Self::UiOpen(_) => event::UI_OPEN,
            Self::SessionState(_) => event::SESSION_STATE,
            Self::SessionRemoved(_) => event::SESSION_REMOVED,
            Self::SessionItem(_) => event::SESSION_ITEM,
            Self::SettingsChanged(_) => event::SETTINGS_CHANGED,
        }
    }

    /// The thread the event is about, if any.
    pub fn thread_id(&self) -> Option<&str> {
        match self {
            Self::ThreadState(e) => Some(&e.thread.id),
            Self::ThreadItem(e) => Some(&e.thread_id),
            Self::ThreadDelta(e) => Some(&e.thread_id),
            Self::ApprovalRequested(e) => Some(&e.approval.thread_id),
            Self::ApprovalResolved(e) => Some(&e.thread_id),
            Self::ThreadDone(e) => Some(&e.thread_id),
            Self::ThreadRemoved(e) => Some(&e.thread_id),
            Self::UiOpen(e) => Some(&e.thread_id),
            Self::AgentsChanged(_)
            | Self::FoldersChanged(_)
            | Self::SessionState(_)
            | Self::SessionRemoved(_)
            | Self::SessionItem(_)
            | Self::SettingsChanged(_) => None,
        }
    }

    /// The session the event is about, if any.
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::SessionState(e) => Some(&e.session.id),
            Self::SessionRemoved(e) => Some(&e.session_id),
            Self::SessionItem(e) => Some(&e.session_id),
            _ => None,
        }
    }

    /// The line as it travels, without the newline.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

// ---- Ids ----

/// Ids that arrive in requests and become part of a path: `^[a-f0-9]{1,40}$`.
pub fn is_valid_id(id: &str) -> bool {
    (1..=40).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A folder id: `^f[a-f0-9]{12}$`.
pub fn is_valid_folder_id(id: &str) -> bool {
    id.len() == 13
        && id.starts_with('f')
        && id[1..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A thread id or an approval token: 32 lowercase hex digits.
pub fn is_valid_thread_id(id: &str) -> bool {
    id.len() == 32 && is_valid_id(id)
}

/// A session id: `^s[cx][a-f0-9]{32}$`.
pub fn is_valid_session_id(id: &str) -> bool {
    id.len() == 34
        && (id.starts_with("sc") || id.starts_with("sx"))
        && id[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// ---- Text ----

/// `s` cut to at most `max` bytes, at a character boundary.
pub fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The last at most `max` bytes of `s`, starting at a character boundary.
pub fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// Whether `c` is stripped from agent-produced text before it leaves the
/// host: C0 and C1 controls except `\n` and `\t`, and the bidi controls.
pub fn is_stripped_char(c: char) -> bool {
    match c {
        '\n' | '\t' => false,
        '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' => true,
        '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => true,
        _ => false,
    }
}

/// `s` without the characters [`is_stripped_char`] names.
pub fn clean(s: &str) -> String {
    s.chars().filter(|&c| !is_stripped_char(c)).collect()
}

/// Agent-produced text made safe to send: cleaned, then cut to `max` bytes.
/// Returns the text and whether it was cut.
pub fn clean_cut(s: &str, max: usize) -> (String, bool) {
    let cleaned = clean(s);
    if cleaned.len() <= max { (cleaned, false) } else { (cut(&cleaned, max).to_owned(), true) }
}

/// The last `max` bytes of cleaned text, and whether anything was dropped.
pub fn clean_tail(s: &str, max: usize) -> (String, bool) {
    let cleaned = clean(s);
    if cleaned.len() <= max { (cleaned, false) } else { (tail(&cleaned, max).to_owned(), true) }
}

/// Whitespace collapsed to single spaces and trimmed (thread summaries).
pub fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(value: &T) {
        let text = serde_json::to_string(value).unwrap();
        assert!(!text.contains('\n'));
        let back: T = serde_json::from_str(&text).unwrap();
        assert_eq!(&back, value);
    }

    fn thread() -> ThreadInfo {
        ThreadInfo {
            id: "0123456789abcdef0123456789abcdef".into(),
            agent: AgentId::Codex,
            folder_id: "f0123456789ab".into(),
            folder_label: "work".into(),
            title: "List the files".into(),
            state: State::Waiting,
            mode: Mode::Supervised,
            origin: Origin::phone("abc", "Pixel"),
            created_ms: 1,
            updated_ms: 2,
            last_seq: 3,
            pending: 1,
            summary: Some("done".into()),
            error: Some(ThreadError { code: "usage_limit".into(), message: "Usage limit reached".into() }),
            native_id: Some("x".into()),
            step: Some("Run command".into()),
            plan: Some(vec![
                PlanStep { text: "Read the code".into(), status: PlanStatus::Completed },
                PlanStep { text: "Fix it".into(), status: PlanStatus::InProgress },
            ]),
            state_since_ms: 2,
            subdir: Some("app/src".into()),
            continued: true,
        }
    }

    fn session() -> Session {
        Session {
            id: format!("sc{}", "ab".repeat(16)),
            agent: AgentId::Claude,
            title: "Fix the login test".into(),
            cwd: "/home/me/code/app".into(),
            folder_id: Some("f0123456789ab".into()),
            folder_label: "app".into(),
            state: SessionState::NeedsYou,
            state_guess: false,
            state_since_ms: 8,
            waiting_for: Some("permission prompt".into()),
            source: SessionSource::DesktopApp,
            open: true,
            updated_ms: 9,
            last_seq: 64,
            step: Some("Run command".into()),
            summary: None,
            can_continue: false,
            blocked: Some(ContinueBlock::Open),
        }
    }

    fn approval() -> Approval {
        Approval {
            token: "ab".repeat(16),
            thread_id: "cd".repeat(16),
            kind: ApprovalKind::Edit,
            title: "Write hello.txt".into(),
            command: None,
            diff: Some("+hi".into()),
            path: Some("hello.txt".into()),
            reason: None,
            truncated: false,
            can_allow_thread: true,
            created_ms: 5,
            expires_ms: 5 + APPROVAL_LIFETIME_MS,
        }
    }

    #[test]
    fn every_type_round_trips() {
        round_trip(&AgentInfo::missing(AgentId::Claude));
        round_trip(&AgentInfo {
            version: Some("2.1.310".into()),
            auth: Some(AuthKind::ApiKey),
            plan: Some("max".into()),
            problem: Some("x".into()),
            ..AgentInfo::missing(AgentId::Codex)
        });
        round_trip(&Folder {
            id: "f0123456789ab".into(),
            label: "a".into(),
            path: "/a".into(),
            exists: true,
            added_ms: 9,
        });
        round_trip(&thread());
        round_trip(&Item {
            seq: 1,
            id: "toolu_1".into(),
            kind: ItemKind::Tool,
            ts_ms: 3,
            rev: 1,
            text: None,
            truncated: true,
            origin: None,
            tool: Some(Tool {
                name: "Bash".into(),
                title: "Run command".into(),
                detail: Some("ls".into()),
                status: ToolStatus::Denied,
                output: Some("x".into()),
            }),
        });
        round_trip(&approval());
        round_trip(&HelloParams { client: ClientKind::Bridge, version: 1, bridge_token: Some("aa".into()) });
        round_trip(&HelloResult { version: 1, host_version: "0.1.12".into(), pid: 7 });
        round_trip(&ThreadStartParams {
            agent: AgentId::Claude,
            folder_id: "f0123456789ab".into(),
            prompt: "hi".into(),
            mode: Mode::AutoEdit,
            origin: Origin::launcher(),
        });
        round_trip(&ThreadReadParams {
            thread_id: "a".into(),
            before_seq: Some(4),
            after_seq: None,
            limit: Some(2),
        });
        round_trip(&ThreadReadResult {
            thread: thread(),
            items: vec![],
            more_before: true,
            pending: vec![approval()],
        });
        round_trip(&ApprovalAnswerParams {
            token: "a".into(),
            decision: Decision::AllowThread,
            by: Origin::cli(),
        });
        round_trip(&SubscribeParams { threads: ThreadSelection::All, deltas: true, sessions: true });
        round_trip(&SubscribeParams {
            threads: ThreadSelection::Ids(vec!["a".into()]),
            deltas: false,
            sessions: false,
        });
        round_trip(&session());
        round_trip(&SessionsListResult { sessions: vec![session()] });
        round_trip(&SessionReadParams { session_id: session().id, before_seq: Some(640), limit: Some(5) });
        round_trip(&SessionReadResult { session: session(), items: vec![], more_before: true });
        round_trip(&SessionWatchParams { session_id: None });
        round_trip(&SessionContinueParams {
            session_id: session().id,
            mode: Mode::Supervised,
            origin: Origin::phone("p", "Pixel"),
        });
        round_trip(&UiViewingParams { thread_id: None, session_id: Some(session().id) });
        round_trip(&ThreadExportParams {
            thread_id: Some("a".into()),
            session_id: None,
            format: ExportFormat::Markdown,
        });
        round_trip(&ThreadExportResult {
            text: "# t".into(),
            truncated: false,
            file_name: "claude-t-2026-11-19.md".into(),
        });
        round_trip(&TerminalCommandResult { argv: vec!["codex".into(), "resume".into()], cwd: "/a".into() });
        round_trip(&BridgeAskResult {
            behavior: BridgeBehavior::Deny,
            message: Some("The user denied this.".into()),
        });
        round_trip(&Response::err(3, RpcError::new(ErrorCode::AlreadyAnswered, "x")));
        round_trip(&Response::ok(3, json!({})));
        for ev in [
            Event::ThreadState(ThreadResult { thread: thread() }),
            Event::ThreadDelta(ThreadDeltaEvent {
                thread_id: "a".into(),
                seq: 2,
                item_id: "m:0".into(),
                offset: 10,
                text: "hi".into(),
            }),
            Event::ApprovalRequested(ApprovalRequestedEvent { approval: approval() }),
            Event::ApprovalResolved(ApprovalResolvedEvent {
                token: "a".into(),
                thread_id: "b".into(),
                outcome: ApprovalOutcome::Expired,
                by: None,
            }),
            Event::ThreadDone(ThreadDoneEvent {
                thread_id: "a".into(),
                outcome: Outcome::Interrupted,
                summary: String::new(),
                error: None,
            }),
            Event::UiOpen(ThreadIdParams { thread_id: "a".into() }),
            Event::SessionState(SessionResult { session: session() }),
            Event::SessionRemoved(SessionRemovedEvent { session_id: session().id }),
            Event::SettingsChanged(HostSettings { follow_sessions: false }),
            Event::SessionItem(SessionItemEvent {
                session_id: session().id,
                item: Item {
                    seq: 65,
                    id: "u-1".into(),
                    kind: ItemKind::User,
                    ts_ms: 1,
                    rev: 0,
                    text: Some("hi".into()),
                    truncated: false,
                    origin: None,
                    tool: None,
                },
            }),
        ] {
            round_trip(&ev);
            let line = ev.to_line();
            let ServerLine::Event(parsed) = ServerLine::parse(&line).unwrap() else { panic!("not an event") };
            assert_eq!(parsed.event, ev.name());
            assert_eq!(Event::from_line(&parsed), Some(ev));
        }
    }

    #[test]
    fn wire_names_are_the_contracts() {
        assert_eq!(serde_json::to_value(Mode::AutoEdit).unwrap(), json!("auto_edit"));
        assert_eq!(
            serde_json::to_value(ErrorCode::UnsupportedVersion).unwrap(),
            json!("UNSUPPORTED_VERSION")
        );
        assert_eq!(serde_json::to_value(ErrorCode::AlreadyAnswered).unwrap(), json!("ALREADY_ANSWERED"));
        assert_eq!(serde_json::to_value(Decision::AllowThread).unwrap(), json!("allow_thread"));
        assert_eq!(serde_json::to_value(AuthKind::ApiKey).unwrap(), json!("api_key"));
        assert_eq!(serde_json::to_value(ThreadSelection::All).unwrap(), json!("all"));
        for code in [
            ErrorCode::BadRequest,
            ErrorCode::NotFound,
            ErrorCode::Denied,
            ErrorCode::Busy,
            ErrorCode::Limit,
            ErrorCode::Unavailable,
            ErrorCode::AlreadyAnswered,
            ErrorCode::UnsupportedVersion,
            ErrorCode::Internal,
        ] {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
        }
        let ev = Event::UiOpen(ThreadIdParams { thread_id: "a".into() });
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            json!({"event": "ui.open", "params": {"thread_id": "a"}})
        );
        assert_eq!(serde_json::to_value(SessionState::NeedsYou).unwrap(), json!("needs_you"));
        assert_eq!(serde_json::to_value(SessionState::NeedsYouGuess).unwrap(), json!("needs_you_guess"));
        assert_eq!(serde_json::to_value(PlanStatus::InProgress).unwrap(), json!("in_progress"));
        assert!(HostSettings::default().follow_sessions);
        assert_eq!(serde_json::to_value(SessionSource::DesktopApp).unwrap(), json!("desktop_app"));
        assert_eq!(serde_json::to_value(ContinueBlock::Folder).unwrap(), json!("folder"));
        let ev = Event::SessionRemoved(SessionRemovedEvent { session_id: "sx1".into() });
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            json!({"event": "session.removed", "params": {"session_id": "sx1"}})
        );
        assert_eq!(ev.session_id(), Some("sx1"));
        assert_eq!(ev.thread_id(), None);
        // An old client's subscribe has no `sessions`; ours omits it when false.
        let sub: SubscribeParams = serde_json::from_value(json!({"threads": "all", "deltas": true})).unwrap();
        assert!(!sub.sessions);
        assert_eq!(
            serde_json::to_value(SubscribeParams {
                threads: ThreadSelection::All,
                deltas: false,
                sessions: false
            })
            .unwrap(),
            json!({"threads": "all", "deltas": false})
        );
    }

    #[test]
    fn a_stored_thread_from_0_1_13_still_reads() {
        let info: ThreadInfo = serde_json::from_value(json!({
            "id": "0123456789abcdef0123456789abcdef", "agent": "claude", "folder_id": "f0123456789ab",
            "folder_label": "w", "title": "t", "state": "done", "mode": "supervised",
            "origin": {"kind": "launcher"}, "created_ms": 1, "updated_ms": 2, "last_seq": 3, "pending": 0
        }))
        .unwrap();
        assert_eq!((info.step, info.state_since_ms, info.subdir, info.continued), (None, 0, None, false));
        assert_eq!(info.plan, None);
    }

    #[test]
    fn sessions_sort_exact_needs_you_first() {
        let with = |state, state_guess| Session { state, state_guess, ..session() };
        let mut ranks: Vec<u8> = [
            with(SessionState::Ended, false),
            with(SessionState::Idle, true),
            with(SessionState::NeedsYouGuess, true),
            with(SessionState::Working, true),
            with(SessionState::NeedsYou, false),
        ]
        .iter()
        .map(Session::rank)
        .collect();
        assert_eq!(ranks, [4, 3, 2, 1, 0]);
        ranks.sort();
        assert_eq!(ranks, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let info: AgentInfo = serde_json::from_value(json!({
            "id": "claude", "name": "Claude", "installed": true, "signed_in": true,
            "login_hint": "claude auth login", "future": [1, 2]
        }))
        .unwrap();
        assert!(info.ready());
        let req: Request =
            serde_json::from_str(r#"{"id":1,"method":"hello","params":{},"extra":true}"#).unwrap();
        assert_eq!(req.method, "hello");
        let line = ServerLine::parse(r#"{"id":4,"result":{"x":1},"later":0}"#).unwrap();
        assert!(matches!(line, ServerLine::Response(Response { id: 4, .. })));
        let unknown = EventLine { event: "thread.future".into(), params: json!({}) };
        assert_eq!(Event::from_line(&unknown), None);
    }

    #[test]
    fn selection_rejects_other_strings() {
        assert!(serde_json::from_value::<ThreadSelection>(json!("some")).is_err());
        assert!(serde_json::from_value::<ThreadSelection>(json!(3)).is_err());
        assert!(ThreadSelection::Ids(vec!["a".into()]).contains("a"));
        assert!(!ThreadSelection::Ids(vec![]).contains("a"));
    }

    #[test]
    fn ids_are_checked() {
        assert!(is_valid_id("0af"));
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("../x"));
        assert!(!is_valid_id("ABC"));
        assert!(!is_valid_id(&"a".repeat(41)));
        assert!(is_valid_folder_id("f0123456789ab"));
        assert!(!is_valid_folder_id("f0123456789aB"));
        assert!(!is_valid_folder_id("g0123456789ab"));
        assert!(!is_valid_folder_id("f0123456789a/"));
        assert!(is_valid_thread_id(&"a".repeat(32)));
        assert!(!is_valid_thread_id(&"a".repeat(31)));
        assert!(is_valid_session_id(&format!("sc{}", "0a".repeat(16))));
        assert!(is_valid_session_id(&format!("sx{}", "0a".repeat(16))));
        assert!(!is_valid_session_id(&format!("sy{}", "0a".repeat(16))));
        assert!(!is_valid_session_id(&format!("sc{}", "0A".repeat(16))));
        assert!(!is_valid_session_id(&format!("sc{}", "0a".repeat(15))));
        assert!(!is_valid_session_id("sc../../etc/passwd/000000000000000"));
    }

    #[test]
    fn text_is_cut_at_char_boundaries_and_cleaned() {
        assert_eq!(cut("héllo", 2), "h");
        assert_eq!(cut("abc", 5), "abc");
        assert_eq!(tail("abcé", 2), "é");
        assert_eq!(tail("abcé", 1), "");
        let dirty = "a\u{1b}[31mb\u{202e}c\n\td\u{85}\u{2067}";
        assert_eq!(clean(dirty), "a[31mbc\n\td");
        assert_eq!(clean_cut("abcdef", 3), ("abc".into(), true));
        assert_eq!(clean_tail("abcdef", 3), ("def".into(), true));
        assert_eq!(collapse_whitespace("  a \n b\t c "), "a b c");
        assert_eq!(RpcError::new(ErrorCode::Internal, "x".repeat(300)).message.len(), MAX_RPC_ERROR_MESSAGE);
    }
}
