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
/// The words a user denies a request with (`approval.answer` `message`).
pub const MAX_DENY_MESSAGE: usize = 2000;
/// Capabilities in one `hello`, and the length of each.
pub const MAX_CAPS: usize = 16;
pub const MAX_CAP: usize = 32;
/// The file a tool step is about (`Tool.path`).
pub const MAX_TOOL_PATH: usize = 512;
/// What the host tells the agent when the user denied a request and said why.
pub const DENY_SAID_PREFIX: &str = "The user denied this and said: ";

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

// ---- Control (phase 2) ----

/// Models an agent lists, and each one's fields.
pub const MAX_MODELS: usize = 40;
pub const MAX_MODEL_ID: usize = 64;
pub const MAX_MODEL_NAME: usize = 64;
pub const MAX_MODEL_NOTE: usize = 120;
/// Effort levels per model, and the length of each.
pub const MAX_EFFORTS: usize = 8;
pub const MAX_EFFORT: usize = 16;
/// Usage windows per agent.
pub const MAX_LIMITS: usize = 4;
pub const MAX_LIMIT_WINDOW: usize = 32;
/// `agents.changed` for a change of limits alone, at most this often per
/// agent unless a window's `state` changed.
pub const LIMITS_EVENT_MS: u64 = 30_000;
/// Messages waiting in a thread's queue.
pub const MAX_QUEUED: usize = 5;
/// A queued message's id: 16 lowercase hex digits.
pub const QUEUED_ID_LEN: usize = 16;
/// Images in one message.
pub const MAX_IMAGES: usize = 4;
/// An image id: 32 lowercase hex digits.
pub const IMAGE_ID_LEN: usize = 32;
pub const MAX_IMAGE_NAME: usize = 128;
/// `image.add_path`: the file read at most, and the largest side decoded.
pub const MAX_IMAGE_FILE: u64 = 20 * 1024 * 1024;
pub const MAX_IMAGE_SIDE: u32 = 12_000;
/// `image.begin`: an upload's size at most.
pub const MAX_IMAGE_UPLOAD: u32 = 10_485_760;
/// `image.chunk`: the base64 text of one chunk at most.
pub const MAX_IMAGE_CHUNK: usize = 131_072;
/// An upload not ended this long after its last chunk is dropped.
pub const IMAGE_UPLOAD_IDLE_MS: u64 = 60_000;
/// `image.read`: the largest side asked for, and the bytes returned at most.
pub const MAX_THUMB_SIDE: u16 = 1024;
pub const MAX_THUMB_BYTES: usize = 150 * 1024;
/// MCP server names in a thread's `mcp`.
pub const MAX_MCP_NAMES: usize = 32;
pub const MAX_MCP_NAME: usize = 64;
/// Questions in one request, and their fields.
pub const MAX_ASKS: usize = 4;
pub const MAX_ASK_ID: usize = 64;
pub const MAX_ASK_HEADER: usize = 40;
pub const MAX_ASK_TEXT: usize = 500;
pub const MAX_ASK_OPTIONS: usize = 6;
pub const MAX_OPTION_LABEL: usize = 80;
pub const MAX_OPTION_NOTE: usize = 200;
/// "Something else": the user's own answer.
pub const MAX_ANSWER_OTHER: usize = 500;
/// Questions waiting per thread; a third is dismissed at once.
pub const MAX_QUESTIONS_PER_THREAD: usize = 2;
/// How long a question waits for an answer (as an approval does).
pub const QUESTION_LIFETIME_MS: u64 = APPROVAL_LIFETIME_MS;
/// `commands.list`.
pub const MAX_COMMANDS: usize = 300;
pub const MAX_COMMAND_NAME: usize = 64;
pub const MAX_COMMAND_NOTE: usize = 200;
pub const MAX_COMMAND_HINT: usize = 80;
/// `files.search`.
pub const MAX_FILES_QUERY: usize = 128;
pub const MAX_FILES_LIMIT: u32 = 50;
pub const DEFAULT_FILES_LIMIT: u32 = 20;
pub const MAX_FILE_HIT_PATH: usize = 512;
/// Drafts kept; the oldest goes first.
pub const MAX_DRAFTS: usize = 64;
pub const MAX_DRAFT_KEY: usize = 64;
/// Drafts are written this long after the last change.
pub const DRAFT_SAVE_MS: u64 = 1000;
/// Prompts kept in `history.jsonl`, the length of each, and `history.list`'s limit.
pub const MAX_HISTORY: usize = 200;
pub const MAX_HISTORY_PROMPT: usize = 2048;
pub const MAX_HISTORY_LIMIT: u32 = 50;
pub const DEFAULT_HISTORY_LIMIT: u32 = 50;
/// The keep-awake inhibitor is released this long after the last thread stopped working.
pub const KEEP_AWAKE_RELEASE_MS: u64 = 60_000;
/// What `plan.implement` sends, as the user's message.
pub const IMPLEMENT_PLAN_TEXT: &str = "Implement the plan.";
/// A plan's text, at most.
pub const MAX_PLAN_ITEM: usize = 16_384;

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
    // Control (phase 2).
    pub const THREAD_CONFIGURE: &str = "thread.configure";
    pub const QUEUE_EDIT: &str = "queue.edit";
    pub const QUEUE_REMOVE: &str = "queue.remove";
    pub const QUEUE_SEND_NOW: &str = "queue.send_now";
    pub const QUESTION_ANSWER: &str = "question.answer";
    pub const QUESTION_DISMISS: &str = "question.dismiss";
    pub const PLAN_IMPLEMENT: &str = "plan.implement";
    pub const FILES_SEARCH: &str = "files.search";
    pub const COMMANDS_LIST: &str = "commands.list";
    pub const IMAGE_ADD_PATH: &str = "image.add_path";
    pub const IMAGE_BEGIN: &str = "image.begin";
    pub const IMAGE_CHUNK: &str = "image.chunk";
    pub const IMAGE_END: &str = "image.end";
    pub const IMAGE_DROP: &str = "image.drop";
    pub const IMAGE_READ: &str = "image.read";
    pub const DRAFT_SET: &str = "draft.set";
    pub const DRAFT_GET: &str = "draft.get";
    pub const HISTORY_LIST: &str = "history.list";
    pub const FOLDERS_SET: &str = "folders.set";
    pub const SETTINGS_SET: &str = "settings.set";
}

/// What a client says it understands (`HelloParams.caps`) and what a host
/// offers (`HelloResult.caps`).
pub mod cap {
    /// The client understands item kind `reasoning` and the item fields
    /// added with it; a client without it gets reasoning as a tool step.
    pub const ITEMS2: &str = "items2";
    /// The host takes `message` with a `deny` answer.
    pub const DENY_MESSAGE: &str = "deny_message";
    /// The host has the phase 2 methods: models and modes, the queue and
    /// steering, questions, plans, files, commands, images, drafts.
    pub const CONTROL: &str = "control";
    /// The client understands item kinds `plan` and `question` and mode
    /// `plan`; a client without it gets them as [`super::Item::legacy`] and
    /// [`super::ThreadInfo::legacy`] make them.
    pub const ITEMS3: &str = "items3";
    /// What this host offers.
    pub const HOST: [&str; 3] = [ITEMS2, DENY_MESSAGE, CONTROL];
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
    pub const QUESTION_REQUESTED: &str = "question.requested";
    pub const QUESTION_RESOLVED: &str = "question.resolved";
    pub const DRAFT_CHANGED: &str = "draft.changed";
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
    /// `draft.set` with a `rev` that isn't the stored draft's; the error's
    /// `data` holds `{"draft": Draft}`.
    Conflict,
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
            Self::Conflict => "CONFLICT",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The message of the `BAD_REQUEST` a host answers a method it doesn't have
/// with, in every version: clients tell an older host by it.
pub const UNKNOWN_METHOD: &str = "unknown method";

/// The `error` member of a failed response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
    /// What some errors carry besides their message (`CONFLICT`: the stored draft).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// An error with `message` cut to [`MAX_RPC_ERROR_MESSAGE`].
    pub fn new(code: ErrorCode, message: impl AsRef<str>) -> Self {
        Self { code, message: cut(message.as_ref(), MAX_RPC_ERROR_MESSAGE).to_owned(), data: None }
    }

    /// The draft a `CONFLICT` from `draft.set` carries.
    pub fn conflict_draft(&self) -> Option<Draft> {
        let data = self.data.as_ref().filter(|_| self.code == ErrorCode::Conflict)?;
        serde_json::from_value(data.get("draft")?.clone()).ok()
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
    /// Changes nothing: the agent proposes a plan for the user to review.
    /// Asks as `supervised` does if the agent asks at all.
    Plan,
}

impl Mode {
    /// A mode a phone may choose (`supervised`, `plan`).
    pub fn phone_may(self) -> bool {
        matches!(self, Self::Supervised | Self::Plan)
    }
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
    /// The agent's thinking. Sent only to clients that said `items2`; see
    /// [`Item::legacy`].
    Reasoning,
    /// A plan proposed in plan mode (`plan_state`). Sent as it is only to
    /// clients that said `items3`.
    Plan,
    /// The agent asked the user something ([`Question`]); the text is the
    /// "question: answer" list once answered, absent when any ask is secret.
    /// Sent as it is only to clients that said `items3`.
    Question,
    /// A kind this version doesn't know (written by a newer host). Kept in
    /// the store as it is, never sent to a client.
    #[serde(other)]
    Unknown,
}

/// Where an assistant message stands in its turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Said on the way (before a tool call, commentary).
    Interim,
    /// The turn's answer.
    Final,
}

/// What a tool step does, for its icon and its words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolClass {
    Command,
    Edit,
    Read,
    Search,
    Web,
    Mcp,
    Agent,
    Tasks,
    Other,
    /// Claude writing its plan file in plan mode: no row, and never sent to
    /// a phone (its path is outside the folder).
    Plan,
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

/// What an agent can do here (`AgentInfo.can`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Can {
    /// Mode `plan`.
    Plan,
    /// It may ask the user questions.
    Questions,
    /// A message sent while it works reaches the running turn.
    Steer,
    Images,
    /// Its use of the user's MCP servers follows the folder's switch.
    McpSwitch,
    /// `commands.list` has its slash commands or skills.
    Commands,
}

/// How close a usage window is to its limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitState {
    #[default]
    Ok,
    Warn,
    Reached,
}

/// How a user message reached a running turn (`Item.via`); absent: it
/// started its turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    /// Sent while the agent worked, into its turn.
    Steer,
    /// Waited in the queue, then started its turn.
    Queue,
}

/// Where a plan stands (`Item.plan_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanState {
    Proposed,
    Implemented,
    /// A newer plan came after it.
    Replaced,
}

/// What `thread.send` does while the thread works.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dispatch {
    /// The client's default; the host reads a bare `auto` as `queue`.
    #[default]
    Auto,
    Steer,
    Queue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    Command,
    Skill,
}

/// How a question ended (`question.resolved`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionOutcome {
    Answered,
    Dismissed,
    Expired,
    /// The turn ended first.
    Cancelled,
}

// ---- Records ----

/// A model an agent's CLI reports (§Q4.1). G7 has no list of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// What is passed to the CLI.
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The CLI's default.
    #[serde(default)]
    pub default: bool,
    /// The effort levels it takes, as the CLI names them; empty: none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    /// It reads images.
    #[serde(default)]
    pub images: bool,
    /// A short name (Claude's `opus`, `sonnet`): listed first.
    #[serde(default)]
    pub short: bool,
}

impl ModelInfo {
    /// Cleaned, cut and bounded; `None` without an id.
    pub fn sanitized(&self) -> Option<ModelInfo> {
        let id = clean_cut(self.id.trim(), MAX_MODEL_ID).0;
        if id.is_empty() {
            return None;
        }
        let name = match clean_cut(self.name.trim(), MAX_MODEL_NAME).0 {
            name if name.is_empty() => id.clone(),
            name => name,
        };
        let effort = |e: &String| Some(clean_cut(e.trim(), MAX_EFFORT).0).filter(|e| !e.is_empty());
        Some(ModelInfo {
            id,
            name,
            note: self.note.as_ref().map(|n| clean_cut(n.trim(), MAX_MODEL_NOTE).0).filter(|n| !n.is_empty()),
            default: self.default,
            efforts: self.efforts.iter().filter_map(effort).take(MAX_EFFORTS).collect(),
            default_effort: self.default_effort.as_ref().and_then(effort),
            images: self.images,
            short: self.short,
        })
    }
}

/// One usage window, as the CLI reports it (§Q4.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limit {
    /// As the CLI names it: `"five_hour"`, `"seven_day"`, or minutes (`"300"`).
    pub window: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_pct: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_ms: Option<u64>,
    #[serde(default, deserialize_with = "lenient_or_default")]
    pub state: LimitState,
}

/// What a thread's context holds, after its last model call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUse {
    pub used: u64,
    pub max: u64,
}

/// The user's MCP servers in a thread, as the CLI reported them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMcp {
    /// The folder's switch when the process started.
    pub on: bool,
    pub connected: u16,
    pub need_login: u16,
    /// Never sent to a phone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
}

/// The MCP servers an agent last reported in a folder (kept by the host).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderMcp {
    pub agent: AgentId,
    pub names: Vec<String>,
}

/// An image the host holds, re-encoded (§Q4.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRef {
    /// 32 lowercase hex digits.
    pub id: String,
    pub name: String,
    pub bytes: u32,
    pub w: u16,
    pub h: u16,
}

/// A message waiting for the running turn to end (§Q3.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queued {
    /// 16 lowercase hex digits.
    pub id: String,
    pub text: String,
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageRef>,
    pub created_ms: u64,
}

/// One option of an [`Ask`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One question of a [`Question`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    pub id: String,
    pub header: String,
    pub text: String,
    /// Several options may be picked.
    #[serde(default)]
    pub multi: bool,
    /// The user may answer in their own words.
    #[serde(default)]
    pub other: bool,
    /// Answered on the PC only; the answer is never stored or sent on.
    #[serde(default)]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<AskOption>,
}

impl Ask {
    /// Cleaned and cut to the limits; `None` without an id or text.
    pub fn sanitized(&self) -> Option<Ask> {
        let id = clean_cut(&self.id, MAX_ASK_ID).0;
        let text = clean_cut(self.text.trim(), MAX_ASK_TEXT).0;
        if id.is_empty() || text.is_empty() {
            return None;
        }
        let options: Vec<AskOption> = self
            .options
            .iter()
            .map(|o| AskOption {
                label: clean_cut(o.label.trim(), MAX_OPTION_LABEL).0,
                note: o
                    .note
                    .as_ref()
                    .map(|n| clean_cut(n.trim(), MAX_OPTION_NOTE).0)
                    .filter(|n| !n.is_empty()),
            })
            .filter(|o| !o.label.is_empty())
            .take(MAX_ASK_OPTIONS)
            .collect();
        Some(Ask {
            id,
            header: clean_cut(self.header.trim(), MAX_ASK_HEADER).0,
            text,
            multi: self.multi && !options.is_empty(),
            // Without options, only the user's own words can answer it.
            other: self.other || options.is_empty(),
            secret: self.secret,
            options,
        })
    }
}

/// The agent asks the user (§Q4.4). Single use, first answer wins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// 32 lowercase hex digits, random.
    pub token: String,
    pub thread_id: String,
    /// Its `question` item.
    pub item_id: String,
    /// Some ask is secret.
    pub secret: bool,
    /// Empty when sent to a client that may not see a secret question.
    pub asks: Vec<Ask>,
    pub created_ms: u64,
    pub expires_ms: u64,
}

impl Question {
    /// As a client that isn't on this PC gets it: a secret question without its asks.
    pub fn redacted(&self) -> Question {
        if self.secret { Question { asks: Vec::new(), ..self.clone() } } else { self.clone() }
    }
}

/// The user's answer to one [`Ask`]. Its `Debug` shows only the id: a
/// secret ask's answer never reaches a log.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub id: String,
    /// The options picked, by label.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// The user's own words ("Something else").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other: Option<String>,
}

impl std::fmt::Debug for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Answer").field("id", &self.id).finish_non_exhaustive()
    }
}

/// A slash command or skill an agent has (§Q4.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Placeholder text for its arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    pub kind: CommandKind,
}

/// A file or directory `files.search` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHit {
    /// Relative to the folder, `/` separators.
    pub path: String,
    pub dir: bool,
}

/// A composer's unsent text (§Q4.10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    /// A thread id, or `new:<folder id>`.
    pub key: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageRef>,
    pub rev: u64,
    /// Who set it last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Origin>,
}

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
    /// The models the CLI reports; empty until it has.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelInfo>,
    #[serde(default, deserialize_with = "lenient_list", skip_serializing_if = "Vec::is_empty")]
    pub can: Vec<Can>,
    /// Usage windows, as the CLI last reported them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<Limit>,
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
            models: Vec::new(),
            can: Vec::new(),
            limits: Vec::new(),
        }
    }

    /// Installed and signed in: a thread can start.
    pub fn ready(&self) -> bool {
        self.installed && self.signed_in
    }

    pub fn can(&self, can: Can) -> bool {
        self.can.contains(&can)
    }

    /// The model `id` names in the list.
    pub fn model(&self, id: &str) -> Option<&ModelInfo> {
        self.models.iter().find(|m| m.id == id)
    }

    /// The CLI's default model, when the list says which.
    pub fn default_model(&self) -> Option<&ModelInfo> {
        self.models.iter().find(|m| m.default)
    }
}

fn yes() -> bool {
    true
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
    /// Threads here load the user's own MCP servers (from their next process).
    #[serde(default = "yes")]
    pub mcp: bool,
    /// The servers each agent last reported here; names only. Never sent to a phone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_seen: Vec<FolderMcp>,
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
    /// The model for its next turns; absent: the CLI's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The model the CLI says answered last; clients show it when it differs from `model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_model: Option<String>,
    /// Messages waiting for the running turn to end, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queued: Vec<Queued>,
    /// The last turn didn't finish: nothing queued is sent until the user says.
    #[serde(default)]
    pub queue_held: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextUse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<ThreadMcp>,
    /// Questions waiting (not counted in `pending`).
    #[serde(default)]
    pub asking: u32,
}

impl ThreadInfo {
    /// The thread as a client that didn't say `items3` gets it: mode `plan`
    /// reads as `supervised` (it asks as supervised does).
    pub fn legacy(&self) -> ThreadInfo {
        match self.mode {
            Mode::Plan => ThreadInfo { mode: Mode::Supervised, ..self.clone() },
            _ => self.clone(),
        }
    }

    /// As a client gets it: [`ThreadInfo::legacy`] without `items3`.
    pub fn for_client(&self, items3: bool) -> ThreadInfo {
        if items3 { self.clone() } else { self.legacy() }
    }
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
    /// The turn it belongs to, 1-based per thread; absent on items stored
    /// before turns were numbered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    /// Kind `assistant`: whether this is the turn's answer, once known.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub phase: Option<Phase>,
    /// When it finished (an answer done, a tool ended, thinking ended).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    /// The id of the tool item this was produced inside (a sub-agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Kind `user`: the tool item this message was a denial of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_of: Option<String>,
    /// Kind `user`: sent while the agent worked (`steer`) or from the queue.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub via: Option<Via>,
    /// Kind `user`: the images sent with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageRef>,
    /// Kind `plan`.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub plan_state: Option<PlanState>,
    /// Kind `question`: how many asks, and how it ended; `origin` is who answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<QuestionInfo>,
}

/// A `question` item's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionInfo {
    /// Its asks, 1 to [`MAX_ASKS`].
    pub count: u8,
    /// Some ask is secret: the item never has the answers.
    #[serde(default, skip_serializing_if = "is_false")]
    pub secret: bool,
    /// Absent while it waits.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub outcome: Option<QuestionOutcome>,
}

impl Item {
    /// An item with only the fields every kind has.
    pub fn new(seq: u64, id: impl Into<String>, kind: ItemKind, ts_ms: u64) -> Self {
        Self {
            seq,
            id: id.into(),
            kind,
            ts_ms,
            rev: 0,
            text: None,
            truncated: false,
            origin: None,
            tool: None,
            turn: None,
            phase: None,
            end_ms: None,
            group: None,
            deny_of: None,
            via: None,
            images: Vec::new(),
            plan_state: None,
            question: None,
        }
    }

    /// A tool step of this item's seq, id, time and revision.
    fn as_tool(&self, name: &str, title: &str, status: ToolStatus) -> Item {
        Item {
            tool: Some(Tool {
                name: name.into(),
                title: title.into(),
                detail: None,
                status,
                output: None,
                class: None,
                path: None,
                by_rule: false,
            }),
            rev: self.rev,
            turn: self.turn,
            end_ms: self.end_ms,
            ..Item::new(self.seq, self.id.clone(), ItemKind::Tool, self.ts_ms)
        }
    }

    /// The item as a client that didn't say `items2` gets it: thinking
    /// becomes a tool step titled "Thinking" (running until it ends), with
    /// no text, so that client's item numbers stay unbroken. Other kinds are
    /// unchanged.
    pub fn legacy(&self) -> Item {
        if self.kind != ItemKind::Reasoning {
            return self.clone();
        }
        let status = if self.end_ms.is_some() { ToolStatus::Ok } else { ToolStatus::Running };
        Item { turn: None, end_ms: None, ..self.as_tool("reasoning", "Thinking", status) }
    }

    /// The item as a client that didn't say `items3` gets it: a plan is an
    /// assistant message with the plan's text (the turn's answer); a
    /// question is a tool step "Question", running until it is resolved,
    /// with no text. Other kinds are unchanged.
    pub fn legacy3(&self) -> Item {
        match self.kind {
            ItemKind::Plan => Item {
                kind: ItemKind::Assistant,
                phase: Some(Phase::Final),
                plan_state: None,
                question: None,
                ..self.clone()
            },
            ItemKind::Question => {
                let status = if self.end_ms.is_some() { ToolStatus::Ok } else { ToolStatus::Running };
                self.as_tool("question", "Question", status)
            }
            _ => self.clone(),
        }
    }

    /// The item as a client gets it: `None` for a kind no client may see,
    /// the [`Item::legacy`] form for one without `items2`.
    pub fn for_client(&self, items2: bool) -> Option<Item> {
        self.for_caps(items2, true)
    }

    /// [`Item::for_client`], and the [`Item::legacy3`] form for a client without `items3`.
    pub fn for_caps(&self, items2: bool, items3: bool) -> Option<Item> {
        match self.kind {
            ItemKind::Unknown => None,
            ItemKind::Reasoning if !items2 => Some(self.legacy()),
            ItemKind::Plan | ItemKind::Question if !items3 => Some(self.legacy3()),
            _ => Some(self.clone()),
        }
    }
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
    /// What kind of step it is; absent on steps stored before classes.
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub class: Option<ToolClass>,
    /// The file an edit or read is about: relative to the thread's folder
    /// when inside it, else absolute and cut from the left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The host allowed it from the thread's "Always allow" rule, without
    /// asking ("Allowed by your rule for this thread").
    #[serde(default, skip_serializing_if = "is_false")]
    pub by_rule: bool,
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
    /// The tool item the request belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
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
    /// What this client understands ([`cap`]): at most [`MAX_CAPS`], each at
    /// most [`MAX_CAP`] bytes. Absent: none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caps: Vec<String>,
}

impl HelloParams {
    /// A client of `kind` speaking this version, understanding `caps`.
    pub fn new(client: ClientKind, caps: &[&str]) -> Self {
        Self {
            client,
            version: API_VERSION,
            bridge_token: None,
            caps: caps.iter().map(|c| (*c).to_owned()).collect(),
        }
    }

    /// Within the limits.
    pub fn caps_valid(&self) -> bool {
        self.caps.len() <= MAX_CAPS && self.caps.iter().all(|c| c.len() <= MAX_CAP)
    }

    pub fn has(&self, cap: &str) -> bool {
        self.caps.iter().any(|c| c == cap)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResult {
    pub version: u32,
    pub host_version: String,
    pub pid: u32,
    /// What this host offers ([`cap`]); empty from a host older than them.
    #[serde(default)]
    pub caps: Vec<String>,
}

impl HelloResult {
    pub fn has(&self, cap: &str) -> bool {
        self.caps.iter().any(|c| c == cap)
    }
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
    /// Absent: the CLI's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// `model` was typed by the user: it need not be in the agent's list.
    #[serde(default, skip_serializing_if = "is_false")]
    pub custom_model: bool,
    /// Pending image ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

impl ThreadStartParams {
    /// A start with the CLI's default model and no images.
    pub fn new(agent: AgentId, folder_id: String, prompt: String, mode: Mode, origin: Origin) -> Self {
        Self {
            agent,
            folder_id,
            prompt,
            mode,
            origin,
            model: None,
            effort: None,
            custom_model: false,
            images: Vec::new(),
        }
    }
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
    /// What to do when the thread works; absent: `auto`, read as `queue`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<Dispatch>,
    /// Pending image ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    /// Applied before the turn starts, as `thread.configure` would; refused while it works.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub custom_model: bool,
}

impl ThreadSendParams {
    /// Text only, as a client before phase 2 sends it.
    pub fn new(thread_id: String, text: String, origin: Origin) -> Self {
        Self {
            thread_id,
            text,
            origin,
            dispatch: None,
            images: Vec::new(),
            model: None,
            effort: None,
            mode: None,
            custom_model: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSendResult {
    /// The message waits in the queue (asked for, or the agent can't be steered).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued: Option<Queued>,
}

/// `thread.configure`: what is absent is unchanged; an empty `model` or
/// `effort` goes back to the CLI's default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadConfigureParams {
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub custom_model: bool,
    /// Who changed it; absent: this PC. A phone may set only `supervised` or `plan`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Origin>,
}

/// `queue.edit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEditParams {
    pub thread_id: String,
    pub id: String,
    pub text: String,
    /// Who edited it; absent: this PC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Origin>,
}

/// `queue.remove`, `queue.send_now`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueItemParams {
    pub thread_id: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Origin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnswerParams {
    pub token: String,
    pub answers: Vec<Answer>,
    pub by: By,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionDismissParams {
    pub token: String,
    pub by: By,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanImplementParams {
    pub thread_id: String,
    pub item_id: String,
    /// `supervised` or `auto_edit` (not from a phone).
    pub mode: Mode,
    pub by: By,
}

/// `files.search`: one of `folder_id` and `thread_id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesSearchParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesSearchResult {
    pub files: Vec<FileHit>,
    /// The index is partial or more matched.
    pub more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandsListParams {
    pub agent: AgentId,
    pub folder_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandsListResult {
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageAddPathParams {
    pub path: String,
}

/// `image.add_path`, `image.end`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageResult {
    pub image: ImageRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageBeginParams {
    pub name: String,
    /// The upload's size; more bytes than this end it.
    pub bytes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageBeginResult {
    pub upload: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageChunkParams {
    pub upload: String,
    /// 0-based, consecutive.
    pub seq: u32,
    /// Base64, at most [`MAX_IMAGE_CHUNK`] characters.
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageEndParams {
    pub upload: String,
}

/// `image.drop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageIdParams {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageReadParams {
    pub id: String,
    /// The largest side wanted, at most [`MAX_THUMB_SIDE`].
    pub max: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageReadResult {
    pub mime: String,
    /// Base64.
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftSetParams {
    /// A thread id, or `new:<folder id>`.
    pub key: String,
    pub text: String,
    /// Pending image ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    /// The stored draft's `rev`, or 0 for a new one.
    pub rev: u64,
    /// Who typed it; absent: this PC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Origin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftSetResult {
    pub rev: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftGetParams {
    pub key: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftGetResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<Draft>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryListResult {
    /// Newest first.
    pub prompts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldersSetParams {
    pub id: String,
    pub mcp: bool,
}

/// `settings.set`: what is absent is unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsSetParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_awake: Option<bool>,
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
    /// Questions waiting, oldest first (a host with `control`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<Question>,
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
    /// With `deny` only: what the user wants instead, 1 to
    /// [`MAX_DENY_MESSAGE`] bytes. Only to a host that offers
    /// [`cap::DENY_MESSAGE`]; an older one drops it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
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
    /// Keep this PC from sleeping while an agent works (on by default).
    #[serde(default = "yes")]
    pub keep_awake: bool,
}

impl Default for HostSettings {
    fn default() -> Self {
        Self { follow_sessions: true, keep_awake: true }
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeAskResult {
    pub behavior: BridgeBehavior,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// With `allow`: the tool's input as it should run (a question's answers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<Value>,
}

impl BridgeAskResult {
    pub fn allow() -> Self {
        Self { behavior: BridgeBehavior::Allow, message: None, updated_input: None }
    }

    pub fn deny(message: impl Into<String>) -> Self {
        Self { behavior: BridgeBehavior::Deny, message: Some(message.into()), updated_input: None }
    }
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
    /// What the user said when denying it, cleaned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionRequestedEvent {
    pub question: Question,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionResolvedEvent {
    pub token: String,
    pub thread_id: String,
    pub outcome: QuestionOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<By>,
}

/// A draft changed: clients call `draft.get` for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftChangedEvent {
    pub key: String,
    pub rev: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Origin>,
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
    #[serde(rename = "question.requested")]
    QuestionRequested(QuestionRequestedEvent),
    #[serde(rename = "question.resolved")]
    QuestionResolved(QuestionResolvedEvent),
    #[serde(rename = "draft.changed")]
    DraftChanged(DraftChangedEvent),
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
            Self::QuestionRequested(_) => event::QUESTION_REQUESTED,
            Self::QuestionResolved(_) => event::QUESTION_RESOLVED,
            Self::DraftChanged(_) => event::DRAFT_CHANGED,
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
            Self::QuestionRequested(e) => Some(&e.question.thread_id),
            Self::QuestionResolved(e) => Some(&e.thread_id),
            Self::AgentsChanged(_)
            | Self::DraftChanged(_)
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

/// `s` cut to at most `max` bytes by dropping its start: `…` and its end,
/// at a character boundary (a long path keeps its file name).
pub fn cut_left(s: &str, max: usize) -> String {
    const MORE: &str = "…";
    if s.len() <= max {
        return s.to_owned();
    }
    if max < MORE.len() {
        return String::new();
    }
    format!("{MORE}{}", tail(s, max - MORE.len()))
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

/// An optional field whose value a later version may widen: a value this
/// version doesn't know reads as absent instead of failing the whole record.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|v| serde_json::from_value(v).ok()))
}

/// A list a later version may widen: entries this version doesn't know are dropped.
fn lenient_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let values = Option::<Vec<Value>>::deserialize(deserializer)?.unwrap_or_default();
    Ok(values.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

/// A field a later version may widen: a value this version doesn't know reads as the default.
fn lenient_or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    Ok(lenient(deserializer)?.unwrap_or_default())
}

/// A queued message's id: [`QUEUED_ID_LEN`] lowercase hex digits.
pub fn is_valid_queued_id(id: &str) -> bool {
    id.len() == QUEUED_ID_LEN && is_valid_id(id)
}

/// An image id: [`IMAGE_ID_LEN`] lowercase hex digits.
pub fn is_valid_image_id(id: &str) -> bool {
    id.len() == IMAGE_ID_LEN && is_valid_id(id)
}

/// A draft key: a thread id, or `new:` and a folder id.
pub fn is_valid_draft_key(key: &str) -> bool {
    key.len() <= MAX_DRAFT_KEY
        && (is_valid_thread_id(key) || key.strip_prefix("new:").is_some_and(is_valid_folder_id))
}

/// A model id a user typed (§Q7): one argv element, never a flag. At most
/// [`MAX_MODEL_ID`] bytes, no whitespace or control character, not starting with `-`.
pub fn is_valid_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_MODEL_ID
        && !id.starts_with('-')
        && !id.chars().any(|c| c.is_whitespace() || c.is_control() || is_stripped_char(c))
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
            model: None,
            effort: None,
            answered_model: None,
            queued: Vec::new(),
            queue_held: false,
            context: None,
            mcp: None,
            asking: 0,
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
            item_id: Some("toolu_1".into()),
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
            mcp: true,
            mcp_seen: Vec::new(),
        });
        round_trip(&thread());
        round_trip(&Item {
            rev: 1,
            truncated: true,
            tool: Some(Tool {
                name: "Bash".into(),
                title: "Run command".into(),
                detail: Some("ls".into()),
                status: ToolStatus::Denied,
                output: Some("x".into()),
                class: Some(ToolClass::Command),
                path: None,
                by_rule: true,
            }),
            turn: Some(2),
            end_ms: Some(4),
            group: Some("toolu_0".into()),
            ..Item::new(1, "toolu_1", ItemKind::Tool, 3)
        });
        round_trip(&Item {
            text: Some("Done.".into()),
            phase: Some(Phase::Final),
            turn: Some(1),
            ..Item::new(2, "m:0", ItemKind::Assistant, 3)
        });
        round_trip(&Item {
            text: Some("Use a temp dir".into()),
            origin: Some(Origin::phone("p", "Pixel")),
            deny_of: Some("toolu_1".into()),
            ..Item::new(3, "user-3", ItemKind::User, 3)
        });
        round_trip(&Item { text: Some(String::new()), ..Item::new(4, "rs_1", ItemKind::Reasoning, 3) });
        round_trip(&approval());
        round_trip(&HelloParams {
            client: ClientKind::Bridge,
            version: 1,
            bridge_token: Some("aa".into()),
            caps: vec![],
        });
        round_trip(&HelloParams::new(ClientKind::Link, &[cap::ITEMS2]));
        round_trip(&HelloResult {
            version: 1,
            host_version: "0.1.12".into(),
            pid: 7,
            caps: cap::HOST.map(str::to_owned).to_vec(),
        });
        round_trip(&ThreadStartParams::new(
            AgentId::Claude,
            "f0123456789ab".into(),
            "hi".into(),
            Mode::AutoEdit,
            Origin::launcher(),
        ));
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
            questions: vec![],
        });
        round_trip(&ApprovalAnswerParams {
            token: "a".into(),
            decision: Decision::AllowThread,
            by: Origin::cli(),
            message: None,
        });
        round_trip(&ApprovalAnswerParams {
            token: "a".into(),
            decision: Decision::Deny,
            by: Origin::launcher(),
            message: Some("Use a temp dir".into()),
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
        round_trip(&BridgeAskResult::deny("The user denied this."));
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
                message: None,
            }),
            Event::ApprovalResolved(ApprovalResolvedEvent {
                token: "a".into(),
                thread_id: "b".into(),
                outcome: ApprovalOutcome::Denied,
                by: Some(Origin::phone("p", "Pixel")),
                message: Some("Use a temp dir".into()),
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
            Event::SettingsChanged(HostSettings { follow_sessions: false, keep_awake: true }),
            Event::SessionItem(SessionItemEvent {
                session_id: session().id,
                item: Item { text: Some("hi".into()), ..Item::new(65, "u-1", ItemKind::User, 1) },
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
    fn thinking_reaches_an_older_client_as_a_tool_step() {
        let open = Item {
            text: Some(String::new()),
            turn: Some(3),
            ..Item::new(7, "msg_1:0", ItemKind::Reasoning, 11)
        };
        let legacy = open.legacy();
        assert_eq!(
            serde_json::to_value(&legacy).unwrap(),
            json!({"seq": 7, "id": "msg_1:0", "kind": "tool", "ts_ms": 11, "rev": 0,
                   "tool": {"name": "reasoning", "title": "Thinking", "status": "running"}})
        );
        let done = Item {
            rev: 1,
            end_ms: Some(20),
            text: Some("Look first.".into()),
            truncated: true,
            ..open.clone()
        };
        assert_eq!(
            serde_json::to_value(done.legacy()).unwrap(),
            json!({"seq": 7, "id": "msg_1:0", "kind": "tool", "ts_ms": 11, "rev": 1,
                   "tool": {"name": "reasoning", "title": "Thinking", "status": "ok"}})
        );
        assert_eq!(done.for_client(true), Some(done.clone()));
        assert_eq!(done.for_client(false), Some(done.legacy()));
        let user = Item { text: Some("hi".into()), ..Item::new(1, "user-1", ItemKind::User, 1) };
        assert_eq!(user.legacy(), user);
        assert_eq!(user.for_client(false), Some(user.clone()));
        assert_eq!(Item::new(2, "x", ItemKind::Unknown, 1).for_client(true), None);
    }

    #[test]
    fn later_words_read_as_absent_not_as_errors() {
        let item: Item = serde_json::from_value(json!({
            "seq": 1, "id": "a", "kind": "assistant", "ts_ms": 1, "rev": 0, "phase": "draft",
        }))
        .unwrap();
        assert_eq!(item.phase, None);
        let item: Item = serde_json::from_value(json!({
            "seq": 1, "id": "a", "kind": "assistant", "ts_ms": 1, "rev": 0, "phase": "interim",
        }))
        .unwrap();
        assert_eq!(item.phase, Some(Phase::Interim));
        let tool: Tool = serde_json::from_value(json!({
            "name": "x", "title": "x", "status": "ok", "class": "teleport", "path": "a.rs"
        }))
        .unwrap();
        assert_eq!((tool.class, tool.path.as_deref()), (None, Some("a.rs")));
        let item: Item = serde_json::from_value(json!({
            "seq": 4, "id": "a", "kind": "hologram", "ts_ms": 1, "rev": 0, "text": "x"
        }))
        .unwrap();
        assert_eq!(item.kind, ItemKind::Unknown);
        // An item from before this release has none of the new fields.
        let old: Item = serde_json::from_value(json!({
            "seq": 1, "id": "user-1", "kind": "user", "ts_ms": 1, "rev": 0, "text": "hi"
        }))
        .unwrap();
        assert_eq!((old.turn, old.phase, old.end_ms, old.group, old.deny_of), (None, None, None, None, None));
        assert_eq!(serde_json::to_value(ToolClass::Mcp).unwrap(), json!("mcp"));
        assert_eq!(serde_json::to_value(ItemKind::Reasoning).unwrap(), json!("reasoning"));
    }

    #[test]
    fn hello_caps_are_optional_and_bounded() {
        let old: HelloParams = serde_json::from_value(json!({"client": "launcher", "version": 1})).unwrap();
        assert!(old.caps.is_empty() && old.caps_valid() && !old.has(cap::ITEMS2));
        assert_eq!(
            serde_json::to_value(HelloParams::new(ClientKind::Test, &[])).unwrap(),
            json!({"client": "test", "version": 1}),
            "no caps: the line an older host has always read"
        );
        let new = HelloParams::new(ClientKind::Link, &[cap::ITEMS2, "later"]);
        assert!(new.has(cap::ITEMS2) && new.caps_valid());
        let too_many = HelloParams { caps: vec!["x".into(); MAX_CAPS + 1], ..new.clone() };
        assert!(!too_many.caps_valid());
        let too_long = HelloParams { caps: vec!["x".repeat(MAX_CAP + 1)], ..new };
        assert!(!too_long.caps_valid());
        let old_host: HelloResult =
            serde_json::from_value(json!({"version": 1, "host_version": "0.1.15", "pid": 3})).unwrap();
        assert!(!old_host.has(cap::DENY_MESSAGE));
        let answer: ApprovalAnswerParams =
            serde_json::from_value(json!({"token": "a", "decision": "deny", "by": {"kind": "launcher"}}))
                .unwrap();
        assert_eq!(answer.message, None);
    }

    #[test]
    fn long_paths_keep_their_end() {
        assert_eq!(cut_left("src/a.rs", 512), "src/a.rs");
        let long = format!("/{}/file.rs", "d".repeat(600));
        let cut = cut_left(&long, MAX_TOOL_PATH);
        assert!(cut.len() <= MAX_TOOL_PATH && cut.starts_with('…') && cut.ends_with("/file.rs"));
        let wide = "é".repeat(400);
        assert!(cut_left(&wide, 11).len() <= 11);
        assert_eq!(cut_left("abcdef", 2), "");
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
