// SPDX-License-Identifier: MPL-2.0
//! What the adapters share: finding the CLIs, probing them (installed,
//! signed in), the steps an adapter reports, and running one agent process
//! with its stdin, stdout and process group.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

use crate::api::{
    self, AgentId, AgentInfo, ApprovalKind, AuthKind, Mode, Outcome, Phase, ThreadError, ToolClass,
    ToolStatus,
};
use crate::host::env;
use crate::lines::{self, LineRead};

/// The oldest Claude that understands our approval tool's answer.
pub const CLAUDE_MIN_VERSION: [u32; 3] = [2, 1, 207];
const PROBE_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
/// After an interrupt, how long the agent has to end the turn itself.
pub const INTERRUPT_GRACE: Duration = Duration::from_secs(10);
/// After SIGTERM, how long before SIGKILL.
pub const TERM_GRACE: Duration = Duration::from_secs(3);
/// After stdin closes, how long a process has to exit on its own.
const EXIT_GRACE: Duration = Duration::from_secs(5);
/// Probe output read at most.
const MAX_PROBE_OUTPUT: u64 = 64 * 1024;
/// stderr kept (debug logs only).
const STDERR_TAIL: usize = 4096;

// ---- What adapters report ----

/// A tool call as shown: a step, and the approval it may need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: String,
    pub title: String,
    pub detail: Option<String>,
    pub kind: ApprovalKind,
    pub command: Option<String>,
    pub diff: Option<String>,
    /// The file the step is about (an edit's, a read's). An approval takes
    /// it only for an edit of that one file.
    pub path: Option<String>,
    /// The agent's own words for what the call does (Claude's `Bash.description`).
    pub description: Option<String>,
    /// What kind of step it is, for clients' icons and words.
    pub class: ToolClass,
    /// The agent's key of the step this one runs inside (a sub-agent's).
    pub group: Option<String>,
}

impl ToolSpec {
    /// A step with only a name and a title, of `class`, asked about as "other".
    pub fn other(name: &str, title: &str, class: ToolClass) -> Self {
        Self {
            name: name.to_owned(),
            title: title.to_owned(),
            detail: None,
            kind: ApprovalKind::Other,
            command: None,
            diff: None,
            path: None,
            description: None,
            class,
            group: None,
        }
    }

    /// What a running step is shown as: the agent's description, else the title.
    pub fn step(&self) -> String {
        let text =
            self.description.as_deref().map(str::trim).filter(|d| !d.is_empty()).unwrap_or(&self.title);
        let line = text.lines().next().unwrap_or_default().trim();
        api::clean_cut(line, api::MAX_STEP).0
    }
}

/// An approval an agent asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalSpec {
    pub kind: ApprovalKind,
    pub title: String,
    pub command: Option<String>,
    pub diff: Option<String>,
    pub path: Option<String>,
    pub reason: Option<String>,
    /// The tool step the approval belongs to.
    pub tool_key: Option<String>,
    /// That step, for when the agent asks before it reported it.
    pub tool: Option<ToolSpec>,
}

impl ApprovalSpec {
    pub fn from_tool(tool: &ToolSpec, tool_key: Option<String>) -> Self {
        Self {
            kind: tool.kind,
            title: tool.title.clone(),
            command: tool.command.clone(),
            diff: tool.diff.clone(),
            path: tool.path.clone().filter(|_| tool.kind == ApprovalKind::Edit),
            reason: None,
            tool_key,
            tool: Some(tool.clone()),
        }
    }
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEnd {
    pub outcome: Outcome,
    pub summary: Option<String>,
    pub error: Option<ThreadError>,
}

impl TurnEnd {
    pub fn done(summary: Option<String>) -> Self {
        Self { outcome: Outcome::Done, summary, error: None }
    }

    pub fn interrupted() -> Self {
        Self { outcome: Outcome::Interrupted, summary: None, error: None }
    }

    pub fn failed(code: &str, message: impl Into<String>) -> Self {
        Self {
            outcome: Outcome::Failed,
            summary: None,
            error: Some(ThreadError { code: code.to_owned(), message: message.into() }),
        }
    }
}

/// Error codes of ours (besides the CLIs' own).
pub mod code {
    pub const AGENT_EXITED: &str = "AGENT_EXITED";
    pub const AGENT_OUTPUT: &str = "AGENT_OUTPUT";
    pub const AGENT_ERROR: &str = "AGENT_ERROR";
    pub const BRIDGE_FAILED: &str = "BRIDGE_FAILED";
    pub const START_FAILED: &str = "START_FAILED";
    pub const USAGE_LIMIT: &str = "usage_limit";
    pub const LOGIN: &str = "login";
}

/// One thing an adapter's mapper saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The agent's own id for the thread (Codex: `thread.id`).
    NativeId(String),
    AssistantOpen {
        key: String,
    },
    AssistantDelta {
        key: String,
        text: String,
    },
    /// An assistant message ended. `final_answer`: it is known now to be
    /// the turn's answer.
    AssistantDone {
        key: String,
        text: String,
        final_answer: bool,
    },
    /// Where an assistant message stands in its turn, once known; it may
    /// come before or after its `AssistantDone`.
    AssistantPhase {
        key: String,
        phase: Phase,
    },
    /// The agent started thinking. Its text, when the CLI gives any, comes
    /// in deltas or with the end.
    ReasoningOpen {
        key: String,
    },
    ReasoningDelta {
        key: String,
        text: String,
    },
    ReasoningDone {
        key: String,
        text: String,
    },
    ToolStart {
        key: String,
        tool: ToolSpec,
    },
    ToolEnd {
        key: String,
        status: ToolStatus,
        output: Option<String>,
    },
    Notice(String),
    /// The agent's plan, replacing the one before.
    Plan(Vec<api::PlanStep>),
    TurnEnded(TurnEnd),
    /// A proposed plan (plan mode) opened: Codex's `plan` item. Its text
    /// streams like an answer's.
    PlanOpen {
        key: String,
    },
    PlanDelta {
        key: String,
        text: String,
    },
    /// The plan is complete with `text` (Codex's `item/completed`, Claude's
    /// `ExitPlanMode`); earlier proposed plans of the thread are replaced.
    PlanDone {
        key: String,
        text: String,
    },
    /// The agent took in a steered message: `id` is its user item's id
    /// ([`UserInput::id`]). Claude: its `isReplay` echo.
    Delivered {
        id: String,
    },
    /// The agent started a turn by itself (Claude: a steer it took in
    /// after its `result`). The registry opens a new turn.
    TurnStarted,
    /// The agent's own mark of the running turn, once per turn as soon as
    /// it is known: Claude's `uuid` of the replayed prompt line that
    /// started it (a steer inside a turn makes none), Codex's turn id. It
    /// comes after [`Step::TurnStarted`] when the agent started the turn
    /// itself, and before [`Step::Delivered`]. A fork or rewind continues
    /// from it.
    TurnMark(String),
    /// What the thread's context holds now.
    Context {
        used: u64,
        max: u64,
    },
    /// The agent's usage windows, replacing the ones before (any thread of
    /// that agent may report them).
    Limits(Vec<api::Limit>),
    /// The user's MCP servers this process loaded (our own bridge left out).
    Mcp {
        names: Vec<String>,
        connected: u16,
        need_login: u16,
    },
    /// The model that answered (not `<synthetic>`).
    AnsweredModel(String),
    /// Claude's `system/init` line: the folder's command names come from it
    /// (`commands.list`).
    Init(Value),
}

/// A plan from the agent's `(text, status)` pairs: at most
/// [`api::MAX_PLAN_STEPS`] entries, each text cleaned and cut, empty ones dropped.
pub fn plan_of<'a>(entries: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<api::PlanStep> {
    entries
        .into_iter()
        .filter(|(text, _)| !text.trim().is_empty())
        .take(api::MAX_PLAN_STEPS)
        .map(|(text, status)| api::PlanStep {
            text: api::clean_cut(text.trim(), api::MAX_PLAN_TEXT).0,
            status: match status {
                "completed" => api::PlanStatus::Completed,
                "in_progress" | "inProgress" => api::PlanStatus::InProgress,
                _ => api::PlanStatus::Pending,
            },
        })
        .collect()
}

/// The user's answer to an approval, as the agent gets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Allow,
    Deny(DenyReason),
    /// Denied by the user, who said what to do instead (cleaned).
    DenySaying(String),
}

impl Answer {
    /// What the agent is told when it is a denial.
    pub fn deny_message(&self) -> Option<String> {
        match self {
            Self::Allow => None,
            Self::Deny(reason) => Some(reason.message().to_owned()),
            Self::DenySaying(words) => Some(format!("{}{words}", api::DENY_SAID_PREFIX)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    User,
    Expired,
    /// The turn ended first.
    Cancelled,
    TooMany,
}

impl DenyReason {
    /// What the agent is told.
    pub fn message(self) -> &'static str {
        match self {
            Self::User | Self::Cancelled => "The user denied this.",
            Self::Expired => "Nobody answered in time. Treat this as denied.",
            Self::TooMany => "Too many requests waiting",
        }
    }
}

/// A question an agent asks the user (§Q4.4), before the host gives it a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionSpec {
    /// The agent's id for it (Codex's `itemId`); the `question` item takes it.
    pub key: Option<String>,
    /// 1 to [`api::MAX_ASKS`]; the host cleans and cuts them.
    pub asks: Vec<api::Ask>,
}

/// The answers to a question, as the agent gets them. Its `Debug` prints
/// nothing of them, and they are overwritten with zeros when dropped: a
/// secret ask's answer exists only here and in the reply to the agent.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct SecretAnswers(Vec<api::Answer>);

impl SecretAnswers {
    pub fn new(answers: Vec<api::Answer>) -> Self {
        Self(answers)
    }

    pub fn answers(&self) -> &[api::Answer] {
        &self.0
    }

    /// The answer to the ask `id`.
    pub fn get(&self, id: &str) -> Option<&api::Answer> {
        self.0.iter().find(|a| a.id == id)
    }
}

fn zero(text: &mut String) {
    let mut bytes = std::mem::take(text).into_bytes();
    bytes.fill(0);
    std::hint::black_box(&bytes);
}

impl Drop for SecretAnswers {
    fn drop(&mut self) {
        for answer in &mut self.0 {
            answer.labels.iter_mut().for_each(zero);
            if let Some(other) = answer.other.as_mut() {
                zero(other);
            }
        }
    }
}

impl std::fmt::Debug for SecretAnswers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretAnswers(..)")
    }
}

/// How a question ended, as the agent is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionReply {
    Answered(SecretAnswers),
    /// The user dismissed it.
    Dismissed,
    /// Nobody answered in time.
    Expired,
    /// The turn ended, or too many were waiting.
    Cancelled,
}

/// What a session sends the registry.
#[derive(Debug)]
pub enum AgentEvent {
    Step(Step),
    Approval {
        spec: ApprovalSpec,
        reply: oneshot::Sender<Answer>,
    },
    /// The agent asks the user (Codex's `item/tool/requestUserInput`).
    Question {
        spec: QuestionSpec,
        reply: oneshot::Sender<QuestionReply>,
    },
    /// The process is gone; `code` is its exit status when it had one.
    Exited {
        code: Option<i32>,
    },
}

/// One process's settings (§Q4.2), from the thread and its folder. The
/// host checks them; adapters map them to fixed flags and fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSettings {
    pub mode: Mode,
    /// Absent: the CLI's default.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The folder's MCP switch.
    pub mcp: bool,
    /// The agent's default model and its default effort, from its list,
    /// for fields that need a model named (Codex's `collaborationMode`).
    pub default_model: Option<String>,
    pub default_effort: Option<String>,
}

impl LaunchSettings {
    /// `mode` with the CLI's defaults and the MCP switch on.
    pub fn new(mode: Mode) -> Self {
        Self { mode, model: None, effort: None, mcp: true, default_model: None, default_effort: None }
    }
}

/// A message for the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInput {
    /// Its user item's id: [`Step::Delivered`] names it.
    pub id: String,
    pub text: String,
    /// The images, as this agent takes them, built by the host from its
    /// image store: Claude `image` content blocks, Codex `localImage`
    /// inputs. They go before the text, in order.
    pub images: Vec<Value>,
}

impl UserInput {
    /// Text alone, with no item id.
    pub fn text(text: impl Into<String>) -> Self {
        Self { id: String::new(), text: text.into(), images: Vec::new() }
    }
}

/// What the registry tells a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCmd {
    /// A message that starts the next turn.
    Send(UserInput),
    /// A message for the running turn (§Q4.5). An adapter that can't take
    /// it now reports nothing for it; the registry only steers agents that
    /// say `can: steer`.
    Steer(UserInput),
    Interrupt,
    /// Close stdin; the process exits when it's done.
    Close,
    /// Close stdin, SIGTERM the process group, SIGKILL after [`TERM_GRACE`].
    Kill,
}

/// Events go here, tagged with the thread and the session generation.
pub type EventTx = mpsc::UnboundedSender<(String, u64, AgentEvent)>;

// ---- Finding and probing the CLIs ----

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The CLI's absolute path: `overridden` if given, else `name` on the
/// child's `PATH` (which includes `~/.local/bin`).
pub fn resolve_cli(
    name: &str,
    overridden: Option<&Path>,
    child_env: &[(OsString, OsString)],
) -> Option<PathBuf> {
    if let Some(path) = overridden {
        return (path.is_absolute() && is_executable(path)).then(|| path.to_owned());
    }
    let path = env::lookup(child_env, "PATH")?;
    std::env::split_paths(path).filter(|d| d.is_absolute()).map(|d| d.join(name)).find(|p| is_executable(p))
}

/// Runs a short command and returns its exit success and stdout.
async fn run_capture(
    bin: &Path,
    args: &[&str],
    child_env: &[(OsString, OsString)],
    timeout: Duration,
) -> Option<(bool, String)> {
    let mut child = Command::new(bin)
        .args(args)
        .env_clear()
        .envs(child_env.iter().map(|(k, v)| (k, v)))
        .current_dir(home_dir(child_env).unwrap_or_else(|| PathBuf::from("/")))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let work = async {
        let mut out = Vec::new();
        (&mut stdout).take(MAX_PROBE_OUTPUT).read_to_end(&mut out).await.ok()?;
        let status = child.wait().await.ok()?;
        Some((status.success(), String::from_utf8_lossy(&out).into_owned()))
    };
    tokio::time::timeout(timeout, work).await.ok().flatten()
}

fn home_dir(child_env: &[(OsString, OsString)]) -> Option<PathBuf> {
    env::lookup(child_env, "HOME").map(PathBuf::from).filter(|p| p.is_absolute())
}

/// `"2.1.310 (Claude Code)"` → `[2, 1, 310]`.
pub fn parse_version(text: &str) -> Option<[u32; 3]> {
    let token = text.split_whitespace().find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let mut parts = token.split(['.', '-', '+']).map(|p| p.parse::<u32>().ok());
    Some([parts.next()??, parts.next().flatten().unwrap_or(0), parts.next().flatten().unwrap_or(0)])
}

fn version_text(text: &str) -> Option<String> {
    let token = text.split_whitespace().find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    Some(api::clean_cut(token, api::MAX_VERSION).0)
}

fn plan_text(plan: Option<&str>) -> Option<String> {
    plan.filter(|p| !p.is_empty()).map(|p| api::clean_cut(p, api::MAX_PLAN).0)
}

/// What `claude auth status` says, reduced to what we keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeAuth {
    pub signed_in: bool,
    pub auth: Option<AuthKind>,
    pub plan: Option<String>,
}

/// Reads `claude auth status`. Nothing else in it (email, org) is kept.
pub fn parse_claude_auth(success: bool, stdout: &str) -> ClaudeAuth {
    let signed_out = ClaudeAuth { signed_in: false, auth: None, plan: None };
    if !success {
        return signed_out;
    }
    let Ok(value) = serde_json::from_str::<Value>(stdout.trim()) else { return signed_out };
    if value.get("loggedIn").and_then(Value::as_bool) != Some(true) {
        return signed_out;
    }
    let auth = if value.get("authMethod").and_then(Value::as_str) == Some("claude.ai") {
        AuthKind::Subscription
    } else if value.get("apiProvider").and_then(Value::as_str) == Some("firstParty") {
        AuthKind::ApiKey
    } else {
        AuthKind::Other
    };
    ClaudeAuth {
        signed_in: true,
        auth: Some(auth),
        plan: plan_text(value.get("subscriptionType").and_then(Value::as_str)),
    }
}

/// The plan's own name from `claude auth status --text`: the "Login method: Claude Ultra account"
/// line gives "Ultra". The JSON only has `subscriptionType` ("max" for several plans). Every other
/// line (email, organization) is ignored.
pub fn claude_plan_label(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find_map(|l| l.strip_prefix("Login method:"))?;
    let label = line.trim().strip_suffix(" account")?.trim();
    let label = label.strip_prefix("Claude ").unwrap_or(label).trim();
    plan_text(Some(label))
}

/// What Codex `account/read` says, reduced to what we keep.
pub fn parse_codex_account(result: &Value) -> ClaudeAuth {
    let account = result.get("account").filter(|a| !a.is_null());
    let signed_in =
        account.is_some() || result.get("requiresOpenaiAuth").and_then(Value::as_bool) == Some(false);
    let auth = account.map(|a| match a.get("type").and_then(Value::as_str) {
        Some("chatgpt") => AuthKind::Subscription,
        Some("apiKey") => AuthKind::ApiKey,
        _ => AuthKind::Other,
    });
    let plan = account.and_then(|a| plan_text(a.get("planType").and_then(Value::as_str)));
    ClaudeAuth { signed_in, auth, plan }
}

/// A probe's result: what to show, and the binary to run.
#[derive(Debug, Clone)]
pub struct Probe {
    pub info: AgentInfo,
    pub bin: Option<PathBuf>,
    /// The slash commands or skills the CLI reported (§Q4.7), unfiltered.
    pub commands: Vec<api::Command>,
}

impl Probe {
    /// An agent that isn't installed.
    pub fn missing(id: AgentId) -> Self {
        Self { info: AgentInfo::missing(id), bin: None, commands: Vec::new() }
    }
}

/// Probes Claude: `--version`, then `auth status`.
pub async fn probe_claude(overridden: Option<&Path>, host_env: &[(OsString, OsString)]) -> Probe {
    let child_env = env::child_env(AgentId::Claude, host_env.iter().cloned());
    let mut info = AgentInfo::missing(AgentId::Claude);
    let Some(bin) = resolve_cli("claude", overridden, &child_env) else {
        return Probe { info, bin: None, commands: Vec::new() };
    };
    let Some((true, version)) = run_capture(&bin, &["--version"], &child_env, PROBE_COMMAND_TIMEOUT).await
    else {
        return Probe { info, bin: None, commands: Vec::new() };
    };
    info.installed = true;
    info.version = version_text(&version);
    if parse_version(&version).is_some_and(|v| v < CLAUDE_MIN_VERSION) {
        info.problem = Some("Update Claude Code".into());
        return Probe { info, bin: Some(bin), commands: Vec::new() };
    }
    let status = run_capture(&bin, &["auth", "status"], &child_env, PROBE_COMMAND_TIMEOUT).await;
    let auth = status.map_or(ClaudeAuth { signed_in: false, auth: None, plan: None }, |(ok, out)| {
        parse_claude_auth(ok, &out)
    });
    info.signed_in = auth.signed_in;
    info.auth = auth.auth;
    info.plan = auth.plan;
    if auth.auth == Some(AuthKind::Subscription)
        && let Some((true, text)) =
            run_capture(&bin, &["auth", "status", "--text"], &child_env, PROBE_COMMAND_TIMEOUT).await
        && let Some(label) = claude_plan_label(&text)
    {
        info.plan = Some(label);
    }
    if env::lookup(&child_env, "ANTHROPIC_API_KEY").is_some_and(|k| !k.is_empty()) {
        // `claude -p` bills the key when it is set.
        info.signed_in = true;
        info.auth = Some(AuthKind::ApiKey);
        info.plan = None;
    }
    info.can = crate::host::catalog::can(AgentId::Claude, true);
    with_fork(AgentId::Claude, Probe { info, bin: Some(bin), commands: Vec::new() })
}

/// `probe` with the fork abilities its CLI gives (§U6.2).
fn with_fork(agent: AgentId, mut probe: Probe) -> Probe {
    let more = crate::host::fork::can(agent, &probe);
    for can in more {
        if !probe.info.can.contains(can) {
            probe.info.can.push(*can);
        }
    }
    probe
}

/// Probes Codex: `--version`, then one app-server conversation for the
/// account, the models and the usage windows (§Q4.1). The windows come back
/// beside the probe: they are kept with the other agents' limits.
pub async fn probe_codex(
    overridden: Option<&Path>,
    host_env: &[(OsString, OsString)],
) -> (Probe, Option<Vec<api::Limit>>) {
    let child_env = env::child_env(AgentId::Codex, host_env.iter().cloned());
    let mut info = AgentInfo::missing(AgentId::Codex);
    let Some(bin) = resolve_cli("codex", overridden, &child_env) else {
        return (Probe { info, bin: None, commands: Vec::new() }, None);
    };
    let Some((true, version)) = run_capture(&bin, &["--version"], &child_env, PROBE_COMMAND_TIMEOUT).await
    else {
        return (Probe { info, bin: None, commands: Vec::new() }, None);
    };
    info.installed = true;
    info.version = version_text(&version);
    let mut limits = None;
    if let Some(probe) = crate::host::catalog::probe_codex(&bin, &child_env).await {
        if let Some(auth) = probe.auth {
            info.signed_in = auth.signed_in;
            info.auth = auth.auth;
            info.plan = auth.plan;
        }
        info.models = probe.models;
        info.can = crate::host::catalog::can(AgentId::Codex, probe.plan_ok);
        limits = Some(probe.limits);
    } else {
        info.can = crate::host::catalog::can(AgentId::Codex, false);
    }
    (with_fork(AgentId::Codex, Probe { info, bin: Some(bin), commands: Vec::new() }), limits)
}

// ---- One agent process ----

/// What the stdout reader passes on.
#[derive(Debug)]
pub enum OutLine {
    Line(Vec<u8>),
    TooLong,
}

/// A running agent process: its own process group, stdin piped, stdout read
/// line by line on a task, stderr's tail kept for debug logs.
#[derive(Debug)]
pub struct AgentProcess {
    pub child: Child,
    pub stdin: Option<ChildStdin>,
    stderr_tail: Option<tokio::task::JoinHandle<String>>,
}

impl AgentProcess {
    /// Starts the process; stdout lines arrive on the returned receiver.
    pub fn spawn(
        bin: &Path,
        args: &[String],
        cwd: &Path,
        child_env: &[(OsString, OsString)],
    ) -> std::io::Result<(Self, mpsc::Receiver<OutLine>)> {
        let mut child = Command::new(bin)
            .args(args)
            .env_clear()
            .envs(child_env.iter().map(|(k, v)| (k, v)))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
        let stderr = child.stderr.take();
        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            let mut reader = BufReader::with_capacity(64 * 1024, stdout);
            let mut buf = Vec::new();
            loop {
                match lines::read_line(&mut reader, &mut buf, api::MAX_AGENT_LINE).await {
                    Ok(LineRead::Line) => {
                        if tx.send(OutLine::Line(std::mem::take(&mut buf))).await.is_err() {
                            break;
                        }
                    }
                    Ok(LineRead::TooLong) => {
                        let _ = tx.send(OutLine::TooLong).await;
                        break;
                    }
                    Ok(LineRead::Eof) | Err(_) => break,
                }
            }
        });
        let stderr_tail = stderr.map(|stderr| {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut tail = String::new();
                let mut line = String::new();
                while reader.read_line(&mut line).await.is_ok_and(|n| n > 0) {
                    tail.push_str(&line);
                    line.clear();
                    if tail.len() > STDERR_TAIL * 2 {
                        tail = api::tail(&tail, STDERR_TAIL).to_owned();
                    }
                }
                api::tail(&tail, STDERR_TAIL).to_owned()
            })
        });
        Ok((Self { child, stdin, stderr_tail }, rx))
    }

    /// Writes one line to stdin. `false` when stdin is closed or broken.
    pub async fn write_line(&mut self, line: &str) -> bool {
        let Some(stdin) = self.stdin.as_mut() else { return false };
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        if stdin.write_all(&bytes).await.is_err() || stdin.flush().await.is_err() {
            self.stdin = None;
            return false;
        }
        true
    }

    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Signals the process group while the child is not yet reaped (so its
    /// pid can't have been reused).
    pub fn signal_group(&self, kill: bool) {
        let Some(pid) = self.child.id().and_then(|id| i32::try_from(id).ok()) else { return };
        let Some(pid) = rustix::process::Pid::from_raw(pid) else { return };
        let signal = if kill { rustix::process::Signal::KILL } else { rustix::process::Signal::TERM };
        let _ = rustix::process::kill_process_group(pid, signal);
    }

    /// Waits for the process to exit after stdin closed: [`EXIT_GRACE`], then
    /// SIGTERM, then SIGKILL after [`TERM_GRACE`]. Returns the exit code.
    pub async fn finish(mut self) -> Option<i32> {
        self.close_stdin();
        let status = match tokio::time::timeout(EXIT_GRACE, self.child.wait()).await {
            Ok(status) => status.ok(),
            Err(_) => {
                self.signal_group(false);
                match tokio::time::timeout(TERM_GRACE, self.child.wait()).await {
                    Ok(status) => status.ok(),
                    Err(_) => {
                        self.signal_group(true);
                        self.child.wait().await.ok()
                    }
                }
            }
        };
        // Children it left in its group go with it.
        if let Some(tail) = self.stderr_tail.take()
            && let Ok(Ok(tail)) = tokio::time::timeout(Duration::from_millis(500), tail).await
            && !tail.is_empty()
            && !status.is_some_and(|s| s.success())
        {
            tracing::debug!(stderr = %tail, "agent process stderr");
        }
        status.and_then(|s| s.code())
    }
}

/// What a driver wants done after it handled something.
#[derive(Debug, Default)]
pub struct Output {
    /// Lines to write to the agent's stdin, in order.
    pub stdin: Vec<String>,
    pub steps: Vec<Step>,
    /// Approvals to ask the user for, keyed by the agent's request id.
    pub approvals: Vec<(Value, ApprovalSpec)>,
    /// Questions to ask the user, keyed by the agent's request id.
    pub questions: Vec<(Value, QuestionSpec)>,
    /// Close stdin after writing.
    pub close: bool,
}

/// One agent's protocol, driven by [`run_session`].
pub trait Driver: Send + 'static {
    /// The first lines, written right after the process starts.
    fn start(&mut self, out: &mut Output);
    /// One parsed stdout line.
    fn on_line(&mut self, line: Value, out: &mut Output);
    /// A follow-up or an interrupt (never `Close`/`Kill`).
    fn on_cmd(&mut self, cmd: SessionCmd, out: &mut Output);
    /// The user's answer to an approval this driver asked for.
    fn on_answer(&mut self, _id: Value, _answer: Answer, _out: &mut Output) {}
    /// How a question this driver asked ended.
    fn on_question_reply(&mut self, _id: Value, _reply: QuestionReply, _out: &mut Output) {}
    /// The agent wrote a line over [`api::MAX_AGENT_LINE`]; the process is stopped.
    fn on_too_long(&mut self) -> Vec<Step>;
    /// The process exited; ends a turn that was still running.
    fn on_exit(&mut self, code: Option<i32>) -> Vec<Step>;
}

/// Runs one agent process to its end: feeds its stdout to `driver`, passes
/// the registry's commands on, and reports steps, approvals and the exit.
pub async fn run_session<D: Driver>(
    mut process: AgentProcess,
    mut lines: mpsc::Receiver<OutLine>,
    mut driver: D,
    thread_id: String,
    generation: u64,
    mut cmds: mpsc::UnboundedReceiver<SessionCmd>,
    events: EventTx,
) {
    let (answers_tx, mut answers) = mpsc::unbounded_channel::<(Value, Reply)>();
    let emit = |event: AgentEvent| {
        let _ = events.send((thread_id.clone(), generation, event));
    };
    let mut out = Output::default();
    driver.start(&mut out);
    apply(&mut process, &mut out, &emit, &answers_tx).await;
    let mut kill_at: Option<tokio::time::Instant> = None;
    let mut cmds_open = true;
    loop {
        tokio::select! {
            line = lines.recv() => match line {
                Some(OutLine::Line(bytes)) => {
                    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else { continue };
                    driver.on_line(value, &mut out);
                    apply(&mut process, &mut out, &emit, &answers_tx).await;
                }
                Some(OutLine::TooLong) => {
                    for step in driver.on_too_long() {
                        emit(AgentEvent::Step(step));
                    }
                    process.close_stdin();
                    process.signal_group(false);
                    kill_at = Some(tokio::time::Instant::now() + TERM_GRACE);
                }
                None => break,
            },
            cmd = cmds.recv(), if cmds_open => match cmd {
                None => {
                    cmds_open = false;
                    process.close_stdin();
                }
                Some(SessionCmd::Close) => process.close_stdin(),
                Some(SessionCmd::Kill) => {
                    process.close_stdin();
                    process.signal_group(false);
                    kill_at.get_or_insert(tokio::time::Instant::now() + TERM_GRACE);
                }
                Some(cmd) => {
                    driver.on_cmd(cmd, &mut out);
                    apply(&mut process, &mut out, &emit, &answers_tx).await;
                }
            },
            Some((id, reply)) = answers.recv() => {
                match reply {
                    Reply::Approval(answer) => driver.on_answer(id, answer, &mut out),
                    Reply::Question(reply) => driver.on_question_reply(id, reply, &mut out),
                }
                apply(&mut process, &mut out, &emit, &answers_tx).await;
            }
            () = sleep_until(kill_at), if kill_at.is_some() => {
                process.signal_group(true);
                kill_at = None;
            }
        }
    }
    let code = process.finish().await;
    for step in driver.on_exit(code) {
        emit(AgentEvent::Step(step));
    }
    emit(AgentEvent::Exited { code });
}

async fn sleep_until(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// The user's answer to something a driver asked, on its way back.
enum Reply {
    Approval(Answer),
    Question(QuestionReply),
}

async fn apply(
    process: &mut AgentProcess,
    out: &mut Output,
    emit: &impl Fn(AgentEvent),
    answers: &mpsc::UnboundedSender<(Value, Reply)>,
) {
    for line in out.stdin.drain(..) {
        if !process.write_line(&line).await {
            break;
        }
    }
    for step in out.steps.drain(..) {
        emit(AgentEvent::Step(step));
    }
    for (id, spec) in out.approvals.drain(..) {
        let (tx, rx) = oneshot::channel();
        emit(AgentEvent::Approval { spec, reply: tx });
        let answers = answers.clone();
        tokio::spawn(async move {
            let answer = rx.await.unwrap_or(Answer::Deny(DenyReason::Cancelled));
            let _ = answers.send((id, Reply::Approval(answer)));
        });
    }
    for (id, spec) in out.questions.drain(..) {
        let (tx, rx) = oneshot::channel();
        emit(AgentEvent::Question { spec, reply: tx });
        let answers = answers.clone();
        tokio::spawn(async move {
            let reply = rx.await.unwrap_or(QuestionReply::Cancelled);
            let _ = answers.send((id, Reply::Question(reply)));
        });
    }
    if std::mem::take(&mut out.close) {
        process.close_stdin();
    }
}

/// The CLI flags for a mode, for logs and tests.
pub fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Supervised => "supervised",
        Mode::AutoEdit => "auto_edit",
        Mode::Plan => "plan",
    }
}

/// A path shown relative to `cwd` when it's inside it.
pub fn display_path(path: &str, cwd: &Path) -> String {
    let p = Path::new(path);
    match p.strip_prefix(cwd) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().into_owned(),
        _ => path.to_owned(),
    }
}

/// `text` as diff lines with `prefix` on each.
pub fn prefixed_lines(text: &str, prefix: char) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 16 + 2);
    for line in text.lines() {
        out.push(prefix);
        out.push_str(line);
        out.push('\n');
    }
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// One file's change as a git-style unified diff, so clients can count
/// its lines and tell its file: `diff --git`, `---`/`+++`, then the hunks.
/// `body`: an added or deleted file's content, or an update's unified diff
/// (with or without headers of its own).
pub fn file_diff(path: &str, kind: &str, move_path: Option<&str>, body: &str) -> String {
    let body = body.trim_end_matches('\n');
    let lines = body.lines().count();
    let (old, new, hunks) = match kind {
        "add" => {
            let hunk = format!("@@ -0,0 +1,{lines} @@\n{}", prefixed_lines(body, '+'));
            ("/dev/null".to_owned(), format!("b/{path}"), (lines > 0).then_some(hunk))
        }
        "delete" => {
            let hunk = format!("@@ -1,{lines} +0,0 @@\n{}", prefixed_lines(body, '-'));
            (format!("a/{path}"), "/dev/null".to_owned(), (lines > 0).then_some(hunk))
        }
        _ => {
            if body.starts_with("diff --git ") {
                return body.to_owned();
            }
            let new = move_path.filter(|m| !m.is_empty()).unwrap_or(path);
            let head = format!("diff --git a/{path} b/{new}");
            if body.starts_with("--- ") {
                return format!("{head}\n{body}");
            }
            (format!("a/{path}"), format!("b/{new}"), (!body.is_empty()).then(|| body.to_owned()))
        }
    };
    let shown = if new == "/dev/null" { path } else { &new[2..] };
    let mut out = format!("diff --git a/{path} b/{shown}\n--- {old}\n+++ {new}");
    if let Some(hunks) = hunks {
        out.push('\n');
        out.push_str(&hunks);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_changes_as_git_diffs() {
        assert_eq!(
            file_diff("notes.txt", "add", None, "a\nb\n"),
            "diff --git a/notes.txt b/notes.txt\n--- /dev/null\n+++ b/notes.txt\n@@ -0,0 +1,2 @@\n+a\n+b"
        );
        assert_eq!(
            file_diff("gone.txt", "delete", None, "bye\n"),
            "diff --git a/gone.txt b/gone.txt\n--- a/gone.txt\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-bye"
        );
        assert_eq!(
            file_diff("src/a.rs", "update", None, "@@ -1 +1 @@\n-old\n+new\n"),
            "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new"
        );
        assert_eq!(
            file_diff("a.rs", "update", Some("b.rs"), "--- a/a.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-x\n+y"),
            "diff --git a/a.rs b/b.rs\n--- a/a.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-x\n+y"
        );
        let whole = "diff --git a/x b/x\n--- a/x\n+++ b/x";
        assert_eq!(file_diff("x", "update", None, whole), whole);
        assert_eq!(
            file_diff("e.txt", "add", None, ""),
            "diff --git a/e.txt b/e.txt\n--- /dev/null\n+++ b/e.txt"
        );
    }

    #[test]
    fn versions_parse() {
        assert_eq!(parse_version("2.1.310 (Claude Code)"), Some([2, 1, 310]));
        assert_eq!(parse_version("codex-cli 0.170.0"), Some([0, 170, 0]));
        assert_eq!(version_text("codex-cli 0.170.0").as_deref(), Some("0.170.0"));
        assert!(parse_version("2.1.206").unwrap() < CLAUDE_MIN_VERSION);
        assert!(parse_version("2.1.207").unwrap() >= CLAUDE_MIN_VERSION);
        assert_eq!(parse_version("nothing"), None);
    }

    #[test]
    fn the_plan_takes_the_clis_own_name() {
        let text =
            "Login method: Claude Ultra account\nOrganization: Example Org\nEmail: someone@example.invalid\n";
        assert_eq!(claude_plan_label(text).as_deref(), Some("Ultra"));
        assert_eq!(claude_plan_label("  Login method: Claude Max account").as_deref(), Some("Max"));
        assert_eq!(claude_plan_label("Login method: Team Plus account").as_deref(), Some("Team Plus"));
        assert_eq!(claude_plan_label("Email: someone@example.invalid"), None, "no login line, no label");
        assert_eq!(claude_plan_label("Login method: Claude Ultra"), None, "not the expected shape");
        assert_eq!(claude_plan_label("Login method:  account"), None, "empty");
        let long = format!("Login method: {} account", "x".repeat(200));
        assert!(claude_plan_label(&long).is_some_and(|l| l.len() <= api::MAX_PLAN));
    }

    #[test]
    fn claude_auth_status_keeps_only_what_we_need() {
        let fixture = include_str!("../../tests/fixtures/claude-auth-status.json");
        let auth = parse_claude_auth(true, fixture);
        assert_eq!(
            auth,
            ClaudeAuth { signed_in: true, auth: Some(AuthKind::Subscription), plan: Some("max".into()) }
        );
        assert!(!parse_claude_auth(false, fixture).signed_in);
        assert!(!parse_claude_auth(true, "Not logged in").signed_in);
        assert!(!parse_claude_auth(true, r#"{"loggedIn":false}"#).signed_in);
        let key =
            parse_claude_auth(true, r#"{"loggedIn":true,"authMethod":"api_key","apiProvider":"firstParty"}"#);
        assert_eq!(key.auth, Some(AuthKind::ApiKey));
    }

    #[test]
    fn codex_account_read() {
        let result = serde_json::json!({"account":{"type":"chatgpt","email":"x","planType":"plus"},"requiresOpenaiAuth":true});
        assert_eq!(
            parse_codex_account(&result),
            ClaudeAuth { signed_in: true, auth: Some(AuthKind::Subscription), plan: Some("plus".into()) }
        );
        assert!(
            !parse_codex_account(&serde_json::json!({"account":null,"requiresOpenaiAuth":true})).signed_in
        );
        assert!(
            parse_codex_account(&serde_json::json!({"account":null,"requiresOpenaiAuth":false})).signed_in
        );
        let key =
            parse_codex_account(&serde_json::json!({"account":{"type":"apiKey"},"requiresOpenaiAuth":true}));
        assert_eq!(key.auth, Some(AuthKind::ApiKey));
    }

    #[test]
    fn plans_are_bounded_and_cleaned() {
        let long = "x".repeat(300);
        let entries: Vec<(&str, &str)> =
            [("Read\u{1b}[0m it", "completed"), ("  ", "pending"), (long.as_str(), "inProgress")]
                .into_iter()
                .chain(std::iter::repeat_n(("more", "odd"), 30))
                .collect();
        let plan = plan_of(entries);
        assert_eq!(plan.len(), api::MAX_PLAN_STEPS);
        assert_eq!(plan[0], api::PlanStep { text: "Read[0m it".into(), status: api::PlanStatus::Completed });
        assert_eq!(plan[1].text.len(), api::MAX_PLAN_TEXT);
        assert_eq!(plan[1].status, api::PlanStatus::InProgress);
        assert_eq!(plan[2].status, api::PlanStatus::Pending);
    }

    #[test]
    fn a_step_is_the_description_else_the_title() {
        let mut spec = ToolSpec {
            name: "Bash".into(),
            title: "Run command".into(),
            detail: None,
            kind: ApprovalKind::Command,
            command: None,
            diff: None,
            path: None,
            description: Some("List the files\nand more".into()),
            class: ToolClass::Command,
            group: None,
        };
        assert_eq!(spec.step(), "List the files");
        spec.description = Some("  ".into());
        assert_eq!(spec.step(), "Run command");
        spec.description = Some("d".repeat(500));
        assert_eq!(spec.step().len(), api::MAX_STEP);
    }

    #[test]
    fn an_approval_takes_the_path_of_an_edit_only() {
        let read = ToolSpec { path: Some("a.rs".into()), ..ToolSpec::other("Read", "Read", ToolClass::Read) };
        assert_eq!(ApprovalSpec::from_tool(&read, None).path, None);
        let edit = ToolSpec { kind: ApprovalKind::Edit, ..read };
        assert_eq!(ApprovalSpec::from_tool(&edit, None).path.as_deref(), Some("a.rs"));
        assert_eq!(
            Answer::DenySaying("Use a temp dir".into()).deny_message().as_deref(),
            Some("The user denied this and said: Use a temp dir")
        );
        assert_eq!(Answer::Deny(DenyReason::User).deny_message().as_deref(), Some("The user denied this."));
        assert_eq!(Answer::Allow.deny_message(), None);
    }

    #[test]
    fn paths_and_diffs() {
        assert_eq!(display_path("/work/a/b.txt", Path::new("/work")), "a/b.txt");
        assert_eq!(display_path("/other/b.txt", Path::new("/work")), "/other/b.txt");
        assert_eq!(prefixed_lines("a\nb\n", '+'), "+a\n+b");
        assert_eq!(prefixed_lines("", '+'), "");
    }
}
