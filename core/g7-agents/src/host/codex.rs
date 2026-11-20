// SPDX-License-Identifier: MPL-2.0
//! The Codex adapter (§5): `codex app-server` over stdio JSON-RPC, one
//! process per thread.
//!
//! Phase 2 (§Q4): each `turn/start` carries the thread's model and effort,
//! and its collaboration mode (plan or default) when Codex accepted the
//! experimental API; a message sent while a turn runs goes in with
//! `turn/steer`; questions come as `item/tool/requestUserInput`.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::api::{self, ApprovalKind, Limit, Mode, Phase, ToolClass, ToolStatus};
use crate::host::agents::{
    Answer, ApprovalSpec, Driver, LaunchSettings, Output, QuestionReply, QuestionSpec, SessionCmd, Step,
    ToolSpec, TurnEnd, UserInput, code, display_path, file_diff, parse_codex_account, plan_of,
};
use crate::host::claude::usage_limit_message;
use crate::host::{catalog, plan, questions};

/// How we name ourselves to Codex (`clientInfo`, a documented field).
pub const CLIENT_NAME: &str = "G7 Linux";

/// `initialize`, asking for the experimental API (plan mode's one field, §Q4.2).
pub fn initialize_request(id: u64) -> Value {
    initialize_request_with(id, true)
}

/// `initialize`, with or without the experimental API.
pub fn initialize_request_with(id: u64, experimental: bool) -> Value {
    let capabilities = if experimental { json!({"experimentalApi": true}) } else { Value::Null };
    json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": {
        "clientInfo": {"name": CLIENT_NAME, "title": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION")},
        "capabilities": capabilities,
    }})
}

/// The approval policy for a mode. Plan mode asks as supervised does.
pub fn approval_policy(mode: Mode) -> &'static str {
    match mode {
        Mode::Supervised | Mode::Plan => "untrusted",
        Mode::AutoEdit => "on-request",
    }
}

pub const SANDBOX: &str = "workspace-write";

/// `turn/start` asks for reasoning summaries (`ReasoningSummary`), so the
/// thinking line can show what the model thinks about when Codex sends it.
pub const REASONING_SUMMARY: &str = "auto";

/// Denying with the user's words: after the decline, the turn is
/// interrupted so the words go as the next turn at once (§P8.3). Off: the
/// turn finishes by itself first.
pub const INTERRUPT_FOR_REPLY: bool = true;

/// The error code and message of a plan-mode turn this Codex can't run.
pub const PLAN_UNAVAILABLE_CODE: &str = "plan_unavailable";
pub const PLAN_UNAVAILABLE: &str = "Plan isn't available with this version of Codex.";
/// The notice when Codex compacted the conversation.
pub const COMPACTED: &str = "Compacted the conversation.";
/// The one Codex command G7 sends as a request of its own (§Q4.7).
const COMPACT_COMMAND: &str = "/compact";

/// Every method the adapter sends Codex (§Q4.2): each is in the stable
/// schema; the one experimental field it uses is `turn/start.collaborationMode`.
pub const METHODS: [&str; 11] = [
    "initialize",
    "account/read",
    "thread/start",
    "thread/resume",
    "turn/start",
    "turn/steer",
    "turn/interrupt",
    "thread/compact/start",
    "skills/list",
    "model/list",
    "account/rateLimits/read",
];

/// Codex's `phase` of an agent message.
fn phase_of(item: &Value) -> Option<Phase> {
    match s(item, "phase") {
        Some("commentary") => Some(Phase::Interim),
        Some("final_answer") => Some(Phase::Final),
        _ => None,
    }
}

/// A reasoning item's summary as text: its entries, each a string or an
/// object with a string `text`, a blank line between them.
fn summary_text(item: &Value) -> String {
    let Some(entries) = item.get("summary").and_then(Value::as_array) else { return String::new() };
    entries
        .iter()
        .filter_map(|e| e.as_str().or_else(|| s(e, "text")))
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// The command shown for a `commandExecution` item or approval: each
/// `commandActions` entry's command when they all have one, else `command`.
fn command_text(v: &Value) -> String {
    if let Some(actions) = v.get("commandActions").and_then(Value::as_array)
        && !actions.is_empty()
    {
        let parts: Option<Vec<&str>> = actions.iter().map(|a| s(a, "command")).collect();
        if let Some(parts) = parts {
            return parts.join("\n");
        }
    }
    s(v, "command").unwrap_or_default().to_owned()
}

fn command_tool(item: &Value) -> ToolSpec {
    let command = command_text(item);
    ToolSpec {
        name: "shell".into(),
        title: "Run command".into(),
        detail: Some(command.clone()),
        kind: ApprovalKind::Command,
        command: Some(command),
        diff: None,
        path: None,
        description: None,
        class: ToolClass::Command,
        group: None,
    }
}

/// A `fileChange` item as a step; whether it changes one file.
fn file_tool(item: &Value, cwd: &Path) -> (ToolSpec, bool) {
    let changes = item.get("changes").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut detail = String::new();
    let mut paths = Vec::new();
    for change in &changes {
        let path = display_path(s(change, "path").unwrap_or_default(), cwd);
        let kind = change.get("kind").and_then(|k| s(k, "type")).unwrap_or("update");
        let moved = change.get("kind").and_then(|k| s(k, "move_path")).map(|m| display_path(m, cwd));
        if !detail.is_empty() {
            detail.push('\n');
        }
        detail.push_str(&file_diff(&path, kind, moved.as_deref(), s(change, "diff").unwrap_or_default()));
        paths.push(path);
    }
    let single = paths.len() == 1;
    let spec = ToolSpec {
        name: "apply_patch".into(),
        title: "Edit files".into(),
        detail: Some(detail.clone()),
        kind: ApprovalKind::Edit,
        command: None,
        diff: Some(detail),
        path: paths.into_iter().next(),
        description: None,
        class: ToolClass::Edit,
        group: None,
    };
    (spec, single)
}

/// A request Codex sent us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerRequest {
    Approval {
        spec: Box<ApprovalSpec>,
        decline_ok: bool,
    },
    /// The agent asks the user (`item/tool/requestUserInput`).
    Question(QuestionSpec),
    /// A question with nothing we can show: answered empty at once.
    EmptyQuestion,
    /// An MCP server asks for input (`mcpServer/elicitation/request`): declined.
    Elicitation,
    Unsupported,
}

/// Turns Codex notifications and server requests into steps (§5.4, §5.5).
#[derive(Debug)]
pub struct CodexMapper {
    cwd: PathBuf,
    /// File changes by item id (their approvals carry no diff), and whether
    /// each changes one file.
    file_changes: HashMap<String, (ToolSpec, bool)>,
    last_final: Option<String>,
    last_text: Option<String>,
    turn_id: Option<String>,
    turn_running: bool,
    rate_limits: Option<Value>,
    /// Reasoning items started and not completed, and whether each has text.
    reasoning: HashMap<String, bool>,
    /// Agent messages of the turn, in order, and whether each has its phase.
    turn_texts: Vec<(String, bool)>,
    /// The usage windows seen, merged from sparse updates.
    limits: Vec<Limit>,
    /// The user's MCP servers by name: their status and failure reason.
    mcp: BTreeMap<String, (String, Option<String>)>,
    mcp_sent: Option<(u16, u16, Vec<String>)>,
    /// Plan items started and not completed.
    plans: Vec<String>,
    /// This turn's compaction was reported.
    compacted: bool,
}

impl CodexMapper {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            file_changes: HashMap::new(),
            last_final: None,
            last_text: None,
            turn_id: None,
            turn_running: false,
            rate_limits: None,
            reasoning: HashMap::new(),
            turn_texts: Vec::new(),
            limits: Vec::new(),
            mcp: BTreeMap::new(),
            mcp_sent: None,
            plans: Vec::new(),
            compacted: false,
        }
    }

    /// A message's phase, when Codex gave one; records that it has it.
    fn note_text(&mut self, id: &str, phase: Option<Phase>) {
        match self.turn_texts.iter_mut().find(|(k, _)| k == id) {
            Some((_, phased)) => *phased |= phase.is_some(),
            None => self.turn_texts.push((id.to_owned(), phase.is_some())),
        }
    }

    /// The turn completed: a message still without a phase is the answer
    /// when it is the turn's last, else said on the way.
    fn settle_phases(&mut self, steps: &mut Vec<Step>) {
        let last = self.turn_texts.len().saturating_sub(1);
        for (i, (key, phased)) in self.turn_texts.iter_mut().enumerate() {
            if !*phased {
                *phased = true;
                let phase = if i == last { Phase::Final } else { Phase::Interim };
                steps.push(Step::AssistantPhase { key: key.clone(), phase });
            }
        }
    }

    pub fn turn_id(&self) -> Option<&str> {
        self.turn_id.as_deref()
    }

    pub fn turn_running(&self) -> bool {
        self.turn_running
    }

    pub fn turn_started(&mut self) {
        self.turn_running = true;
        self.last_final = None;
        self.last_text = None;
        self.compacted = false;
    }

    pub fn set_turn_id(&mut self, id: Option<String>) {
        if self.turn_running {
            self.turn_id = id;
        }
    }

    fn end_turn(&mut self, end: TurnEnd) -> Step {
        self.turn_running = false;
        self.turn_id = None;
        self.file_changes.clear();
        self.reasoning.clear();
        self.turn_texts.clear();
        self.plans.clear();
        Step::TurnEnded(end)
    }

    /// The compaction's notice, once per turn. A compaction Codex ran
    /// without a turn of its own ends the one we opened for it.
    fn compacted(&mut self, steps: &mut Vec<Step>) {
        if !self.compacted {
            self.compacted = true;
            steps.push(Step::Notice(COMPACTED.to_owned()));
        }
        if self.turn_running && self.turn_id.is_none() {
            steps.push(self.end_turn(TurnEnd::done(None)));
        }
    }

    /// The MCP servers' state changed: reported when what we show changed.
    fn mcp_changed(&mut self, steps: &mut Vec<Step>) {
        let connected = self.mcp.values().filter(|(status, _)| status == "ready").count();
        let need_login = self
            .mcp
            .values()
            .filter(|(_, reason)| reason.as_deref() == Some("reauthenticationRequired"))
            .count();
        let names: Vec<String> = self.mcp.keys().take(api::MAX_MCP_NAMES).cloned().collect();
        let now = (
            u16::try_from(connected).unwrap_or(u16::MAX),
            u16::try_from(need_login).unwrap_or(u16::MAX),
            names,
        );
        if self.mcp_sent.as_ref() != Some(&now) {
            self.mcp_sent = Some(now.clone());
            let (connected, need_login, names) = now;
            steps.push(Step::Mcp { names, connected, need_login });
        }
    }

    /// A notification (a line with `method` and no `id`).
    pub fn notification(&mut self, msg: &Value) -> Vec<Step> {
        let params = msg.get("params").unwrap_or(&Value::Null);
        let mut steps = Vec::new();
        match s(msg, "method").unwrap_or_default() {
            "turn/started" => {
                let id = params.get("turn").and_then(|t| s(t, "id")).map(str::to_owned);
                if id.is_some() {
                    self.turn_id = id;
                }
            }
            "item/started" => {
                let item = params.get("item").unwrap_or(&Value::Null);
                let Some(id) = s(item, "id").map(str::to_owned) else { return steps };
                match s(item, "type").unwrap_or_default() {
                    "agentMessage" => {
                        let phase = phase_of(item);
                        self.note_text(&id, phase);
                        steps.push(Step::AssistantOpen { key: id.clone() });
                        if let Some(phase) = phase {
                            steps.push(Step::AssistantPhase { key: id, phase });
                        }
                    }
                    "plan" => {
                        if !self.plans.contains(&id) {
                            self.plans.push(id.clone());
                            steps.push(Step::PlanOpen { key: id });
                        }
                    }
                    "reasoning" => {
                        self.reasoning.insert(id.clone(), false);
                        steps.push(Step::ReasoningOpen { key: id });
                    }
                    "commandExecution" => steps.push(Step::ToolStart { key: id, tool: command_tool(item) }),
                    "fileChange" => {
                        let (tool, single) = file_tool(item, &self.cwd);
                        self.file_changes.insert(id.clone(), (tool.clone(), single));
                        steps.push(Step::ToolStart { key: id, tool });
                    }
                    kind @ ("mcpToolCall"
                    | "dynamicToolCall"
                    | "webSearch"
                    | "imageView"
                    | "collabAgentToolCall") => {
                        let tool = s(item, "tool").unwrap_or(kind);
                        let title = match (kind, s(item, "server")) {
                            ("mcpToolCall", Some(server)) => format!("{server}: {tool}"),
                            _ => tool.to_owned(),
                        };
                        let class = match kind {
                            "mcpToolCall" => ToolClass::Mcp,
                            "webSearch" => ToolClass::Web,
                            "collabAgentToolCall" => ToolClass::Agent,
                            _ => ToolClass::Other,
                        };
                        steps.push(Step::ToolStart { key: id, tool: ToolSpec::other(kind, &title, class) });
                    }
                    _ => {}
                }
            }
            "item/plan/delta" => {
                if let (Some(key), Some(text)) = (s(params, "itemId"), s(params, "delta"))
                    && !text.is_empty()
                    && self.plans.iter().any(|p| p == key)
                {
                    steps.push(Step::PlanDelta { key: key.to_owned(), text: text.to_owned() });
                }
            }
            "item/reasoning/summaryTextDelta" => {
                if let (Some(key), Some(text)) = (s(params, "itemId"), s(params, "delta"))
                    && !text.is_empty()
                    && let Some(has_text) = self.reasoning.get_mut(key)
                {
                    *has_text = true;
                    steps.push(Step::ReasoningDelta { key: key.to_owned(), text: text.to_owned() });
                }
            }
            "item/reasoning/summaryPartAdded" => {
                if let Some(key) = s(params, "itemId")
                    && self.reasoning.get(key) == Some(&true)
                {
                    steps.push(Step::ReasoningDelta { key: key.to_owned(), text: "\n\n".to_owned() });
                }
            }
            "item/agentMessage/delta" => {
                if let (Some(key), Some(text)) = (s(params, "itemId"), s(params, "delta"))
                    && !text.is_empty()
                {
                    steps.push(Step::AssistantDelta { key: key.to_owned(), text: text.to_owned() });
                }
            }
            "item/completed" => {
                let item = params.get("item").unwrap_or(&Value::Null);
                let Some(id) = s(item, "id").map(str::to_owned) else { return steps };
                let status = s(item, "status").unwrap_or_default();
                match s(item, "type").unwrap_or_default() {
                    "agentMessage" => {
                        let text = s(item, "text").unwrap_or_default().to_owned();
                        let phase = phase_of(item);
                        let final_answer = phase == Some(Phase::Final);
                        if final_answer {
                            self.last_final = Some(text.clone());
                        }
                        if !text.trim().is_empty() {
                            self.last_text = Some(text.clone());
                        }
                        self.note_text(&id, phase);
                        steps.push(Step::AssistantDone { key: id.clone(), text, final_answer });
                        if let Some(phase) = phase {
                            steps.push(Step::AssistantPhase { key: id, phase });
                        }
                    }
                    "plan" => {
                        if !self.plans.contains(&id) {
                            steps.push(Step::PlanOpen { key: id.clone() });
                        }
                        self.plans.retain(|p| *p != id);
                        steps.push(Step::PlanDone { key: id, text: plan::codex_plan_text(item) });
                    }
                    "contextCompaction" => self.compacted(&mut steps),
                    "reasoning" => {
                        self.reasoning.remove(&id);
                        steps.push(Step::ReasoningDone { key: id, text: summary_text(item) });
                    }
                    "commandExecution" => {
                        let status = match status {
                            "completed" if item.get("exitCode").and_then(Value::as_i64) == Some(0) => {
                                ToolStatus::Ok
                            }
                            "declined" => ToolStatus::Denied,
                            _ => ToolStatus::Error,
                        };
                        let output = s(item, "aggregatedOutput").filter(|o| !o.is_empty()).map(str::to_owned);
                        steps.push(Step::ToolEnd { key: id, status, output });
                    }
                    "fileChange" => {
                        let status = match status {
                            "completed" => ToolStatus::Ok,
                            "declined" => ToolStatus::Denied,
                            _ => ToolStatus::Error,
                        };
                        steps.push(Step::ToolEnd { key: id, status, output: None });
                    }
                    "mcpToolCall" | "dynamicToolCall" | "webSearch" | "imageView" | "collabAgentToolCall" => {
                        let status = if matches!(status, "failed" | "error") {
                            ToolStatus::Error
                        } else {
                            ToolStatus::Ok
                        };
                        steps.push(Step::ToolEnd { key: id, status, output: None });
                    }
                    _ => {}
                }
            }
            "error" => {
                if params.get("willRetry").and_then(Value::as_bool) == Some(true)
                    && let Some(message) = params.get("error").and_then(|e| s(e, "message"))
                {
                    steps.push(Step::Notice(message.to_owned()));
                }
            }
            "account/rateLimits/updated" => {
                if let Some(snapshot) = params.get("rateLimits") {
                    catalog::merge_limits(&mut self.limits, catalog::codex_limits(snapshot));
                    if !self.limits.is_empty() {
                        steps.push(Step::Limits(self.limits.clone()));
                    }
                }
                // Only the windows are kept (no plan, credits or account).
                self.rate_limits = params.get("rateLimits").map(|r| {
                    json!({"primary": r.get("primary").cloned(), "secondary": r.get("secondary").cloned()})
                });
            }
            "thread/tokenUsage/updated" => {
                let usage = params.get("tokenUsage").unwrap_or(&Value::Null);
                let used = usage.pointer("/last/totalTokens").and_then(Value::as_u64).unwrap_or(0);
                let max = usage.get("modelContextWindow").and_then(Value::as_u64).unwrap_or(0);
                if used > 0 && max > 0 {
                    steps.push(Step::Context { used, max });
                }
            }
            "mcpServer/startupStatus/updated" => {
                if let (Some(name), Some(status)) = (s(params, "name"), s(params, "status")) {
                    let name = api::clean_cut(name.trim(), api::MAX_MCP_NAME).0;
                    if !name.is_empty()
                        && (self.mcp.contains_key(&name) || self.mcp.len() < api::MAX_MCP_NAMES * 4)
                    {
                        let reason = s(params, "failureReason").map(str::to_owned);
                        self.mcp.insert(name, (status.to_owned(), reason));
                        self.mcp_changed(&mut steps);
                    }
                }
            }
            "model/rerouted" => {
                if let Some(model) =
                    s(params, "toModel").map(|m| api::clean_cut(m.trim(), api::MAX_MODEL_ID).0)
                    && !model.is_empty()
                {
                    steps.push(Step::AnsweredModel(model));
                }
            }
            "thread/compacted" => self.compacted(&mut steps),
            "turn/plan/updated" => {
                if let Some(entries) = params.get("plan").and_then(Value::as_array) {
                    steps.push(Step::Plan(plan_of(
                        entries
                            .iter()
                            .map(|e| (s(e, "step").unwrap_or_default(), s(e, "status").unwrap_or_default())),
                    )));
                }
            }
            "turn/completed" => {
                if !self.turn_running {
                    return steps;
                }
                let turn = params.get("turn").unwrap_or(&Value::Null);
                let end = match s(turn, "status") {
                    Some("completed") => {
                        self.settle_phases(&mut steps);
                        TurnEnd::done(self.last_final.clone().or_else(|| self.last_text.clone()))
                    }
                    Some("interrupted") => TurnEnd::interrupted(),
                    _ => self.failure(turn.get("error").unwrap_or(&Value::Null)),
                };
                steps.push(self.end_turn(end));
            }
            _ => {}
        }
        steps
    }

    /// The latest reset time of the windows the usage limit filled, in seconds.
    fn limit_resets(&self) -> Option<u64> {
        let limits = self.rate_limits.as_ref()?;
        ["primary", "secondary"]
            .iter()
            .filter_map(|w| limits.get(*w))
            .filter(|w| w.get("usedPercent").and_then(Value::as_i64).is_some_and(|p| p >= 100))
            .filter_map(|w| w.get("resetsAt").and_then(Value::as_u64))
            .max()
    }

    fn failure(&self, error: &Value) -> TurnEnd {
        let message = s(error, "message").unwrap_or("The turn failed.").to_owned();
        let info = error.get("codexErrorInfo").unwrap_or(&Value::Null);
        let info_code = match info {
            Value::String(code) => Some(code.clone()),
            Value::Object(map) => map.keys().next().cloned(),
            _ => None,
        };
        match info_code.as_deref() {
            Some("usageLimitExceeded" | "rateLimitExceeded") => {
                TurnEnd::failed(code::USAGE_LIMIT, usage_limit_message(self.limit_resets()))
            }
            Some("unauthorized") => TurnEnd::failed(code::LOGIN, message),
            Some(other) => TurnEnd::failed(crate::api::cut(other, crate::api::MAX_ERROR_CODE), message),
            None => TurnEnd::failed("failed", message),
        }
    }

    /// A server request (a line with `method` and `id`).
    pub fn server_request(&mut self, msg: &Value) -> ServerRequest {
        let params = msg.get("params").unwrap_or(&Value::Null);
        let decline_ok = params
            .get("availableDecisions")
            .and_then(Value::as_array)
            .is_none_or(|list| list.iter().any(|d| d.as_str() == Some("decline")));
        let reason = s(params, "reason").filter(|r| !r.is_empty()).map(str::to_owned);
        let item_id = s(params, "itemId").map(str::to_owned);
        match s(msg, "method").unwrap_or_default() {
            "item/commandExecution/requestApproval" => {
                let tool = command_tool(params);
                let mut spec = ApprovalSpec::from_tool(&tool, item_id);
                spec.reason = reason;
                ServerRequest::Approval { spec: Box::new(spec), decline_ok }
            }
            "item/fileChange/requestApproval" => {
                let (tool, single) =
                    item_id.as_ref().and_then(|id| self.file_changes.get(id)).cloned().unwrap_or_else(|| {
                        let tool = ToolSpec {
                            kind: ApprovalKind::Edit,
                            ..ToolSpec::other("apply_patch", "Edit files", ToolClass::Edit)
                        };
                        (tool, false)
                    });
                let mut spec = ApprovalSpec::from_tool(&tool, item_id);
                // The request names a file only when it changes just that one.
                if !single {
                    spec.path = None;
                }
                if let Some(path) = &spec.path {
                    spec.title = format!("Edit {path}");
                }
                spec.reason = reason;
                ServerRequest::Approval { spec: Box::new(spec), decline_ok }
            }
            "item/tool/requestUserInput" => match questions::codex_spec(params) {
                Some(spec) => ServerRequest::Question(spec),
                None => ServerRequest::EmptyQuestion,
            },
            "mcpServer/elicitation/request" => ServerRequest::Elicitation,
            _ => ServerRequest::Unsupported,
        }
    }

    pub fn on_exit(&mut self, code: Option<i32>, interrupt_sent: bool) -> Vec<Step> {
        if !self.turn_running {
            return Vec::new();
        }
        let end = if interrupt_sent {
            TurnEnd::interrupted()
        } else {
            let text = code.map_or_else(|| "exit signal".to_owned(), |c| format!("exit {c}"));
            TurnEnd::failed(code::AGENT_EXITED, text)
        };
        vec![self.end_turn(end)]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Initialize { experimental: bool },
    AccountRead,
    Thread,
    Skills,
    TurnStart,
    Steer,
    Interrupt,
    Compact,
}

/// A message waiting for its turn.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Waiting {
    input: UserInput,
    /// It came to steer a turn that had ended or refused it: the registry
    /// learns of its turn ([`Step::TurnStarted`]) and of its delivery.
    steer: bool,
}

/// The `turn/start` in flight, to send again without the plan field.
#[derive(Debug, Clone)]
struct InFlight {
    waiting: Waiting,
    params: Value,
}

/// The Codex side of [`crate::host::agents::run_session`].
#[derive(Debug)]
pub struct CodexDriver {
    mapper: CodexMapper,
    cwd: String,
    settings: LaunchSettings,
    /// Codex accepted the experimental API: turns carry `collaborationMode`.
    plan_ok: bool,
    resume: Option<String>,
    native_id: Option<String>,
    /// The model Codex reported for the thread.
    thread_model: Option<String>,
    texts: VecDeque<Waiting>,
    in_flight: Option<InFlight>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    /// `turn/steer` requests waiting for their answer.
    steering: HashMap<u64, Waiting>,
    /// Server requests waiting for the user: whether `decline` may be sent.
    asks: HashMap<String, bool>,
    /// Questions waiting for the user, by request id.
    questions: HashMap<String, QuestionSpec>,
    /// The folder's skills (name → path), once asked for.
    skills: Option<HashMap<String, String>>,
    interrupt_wanted: bool,
    interrupt_sent: bool,
    failed: bool,
}

impl CodexDriver {
    /// A driver for a process in `mode` with the CLI's defaults, sending text alone.
    pub fn new(cwd: PathBuf, mode: Mode, resume: Option<String>, first_text: String) -> Self {
        Self::with_settings(cwd, LaunchSettings::new(mode), resume, UserInput::text(first_text))
    }

    /// A driver for a process with the thread's settings (§Q4.2).
    pub fn with_settings(
        cwd: PathBuf,
        settings: LaunchSettings,
        resume: Option<String>,
        first: UserInput,
    ) -> Self {
        let mut mapper = CodexMapper::new(cwd.clone());
        mapper.turn_started();
        Self {
            mapper,
            cwd: cwd.to_string_lossy().into_owned(),
            settings,
            plan_ok: false,
            resume,
            native_id: None,
            thread_model: None,
            texts: VecDeque::from([Waiting { input: first, steer: false }]),
            in_flight: None,
            next_id: 1,
            pending: HashMap::new(),
            steering: HashMap::new(),
            asks: HashMap::new(),
            questions: HashMap::new(),
            skills: None,
            interrupt_wanted: false,
            interrupt_sent: false,
            failed: false,
        }
    }

    fn request(&mut self, kind: Pending, method: &str, params: Value, out: &mut Output) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, kind);
        out.stdin.push(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string());
        id
    }

    /// The model a turn runs with: the thread's, else the list's default,
    /// else what Codex reported for the thread.
    fn model(&self) -> Option<String> {
        self.settings
            .model
            .clone()
            .filter(|m| catalog::valid_model_id(m))
            .or_else(|| self.settings.default_model.clone().filter(|m| catalog::valid_model_id(m)))
            .or_else(|| self.thread_model.clone())
    }

    /// A message's inputs: its images, a skill it names, then its text.
    fn inputs(&self, input: &UserInput) -> Vec<Value> {
        let mut list: Vec<Value> = input.images.clone();
        if let Some((name, path)) = self.skill_of(&input.text) {
            list.push(json!({"type": "skill", "name": name, "path": path}));
        }
        list.push(json!({"type": "text", "text": input.text, "text_elements": []}));
        list
    }

    /// The skill a message starts with (`/name …`), when the folder has it.
    fn skill_of(&self, text: &str) -> Option<(String, String)> {
        let name = slash_word(text)?;
        let path = self.skills.as_ref()?.get(name)?;
        Some((name.to_owned(), path.clone()))
    }

    fn turn_params(&self, native: &str, input: &UserInput, plan: bool) -> Value {
        let mut params =
            json!({"threadId": native, "input": self.inputs(input), "summary": REASONING_SUMMARY});
        if let Some(model) = self.settings.model.as_deref().filter(|m| catalog::valid_model_id(m)) {
            params["model"] = json!(model);
        }
        let effort = self.settings.effort.as_deref().and_then(catalog::effort_word);
        if let Some(effort) = effort {
            params["effort"] = json!(effort);
        }
        if plan && let Some(model) = self.model() {
            let effort =
                effort.or_else(|| self.settings.default_effort.as_deref().and_then(catalog::effort_word));
            params["collaborationMode"] = json!({
                "mode": if self.settings.mode == Mode::Plan { "plan" } else { "default" },
                "settings": {"model": model, "reasoning_effort": effort, "developer_instructions": null},
            });
        }
        params
    }

    fn start_turn(&mut self, out: &mut Output) {
        let Some(native) = self.native_id.clone() else { return };
        if self.pending.values().any(|p| matches!(p, Pending::TurnStart | Pending::Compact | Pending::Skills))
        {
            return;
        }
        let Some(front) = self.texts.front() else { return };
        // A message naming a skill waits for the folder's skills.
        let needs_skills = self.skills.is_none()
            && slash_word(&front.input.text).is_some_and(|w| format!("/{w}") != COMPACT_COMMAND);
        if needs_skills {
            self.request(Pending::Skills, "skills/list", json!({"cwds": [self.cwd]}), out);
            return;
        }
        let Some(waiting) = self.texts.pop_front() else { return };
        if !self.mapper.turn_running() {
            self.mapper.turn_started();
            if waiting.steer {
                out.steps.push(Step::TurnStarted);
            }
        }
        self.interrupt_sent = false;
        if waiting.input.text.trim() == COMPACT_COMMAND && waiting.input.images.is_empty() {
            self.request(Pending::Compact, "thread/compact/start", json!({"threadId": native}), out);
            self.in_flight = Some(InFlight { waiting, params: Value::Null });
            return;
        }
        if self.settings.mode == Mode::Plan && (!self.plan_ok || self.model().is_none()) {
            out.steps.push(self.mapper.end_turn(TurnEnd::failed(PLAN_UNAVAILABLE_CODE, PLAN_UNAVAILABLE)));
            return self.start_turn(out);
        }
        let params = self.turn_params(&native, &waiting.input, self.plan_ok);
        self.request(Pending::TurnStart, "turn/start", params.clone(), out);
        self.in_flight = Some(InFlight { waiting, params });
    }

    fn send_interrupt(&mut self, out: &mut Output) {
        let (Some(native), Some(turn)) = (self.native_id.clone(), self.mapper.turn_id().map(str::to_owned))
        else {
            return;
        };
        self.interrupt_wanted = false;
        self.interrupt_sent = true;
        self.request(Pending::Interrupt, "turn/interrupt", json!({"threadId": native, "turnId": turn}), out);
    }

    fn fail(&mut self, end: TurnEnd, out: &mut Output) {
        self.failed = true;
        self.texts.clear();
        if self.mapper.turn_running() {
            out.steps.push(self.mapper.end_turn(end));
        }
        out.close = true;
    }

    /// Codex refused `collaborationMode` (an update took the experimental
    /// API away): never send it again in this process.
    fn plan_refused(error: &Value, params: &Value) -> bool {
        if params.get("collaborationMode").is_none() {
            return false;
        }
        let message = s(error, "message").unwrap_or_default();
        message.contains("collaborationMode")
            || message.contains("experimentalApi")
            || error.get("code").and_then(Value::as_i64) == Some(-32600)
    }

    fn on_response(&mut self, msg: &Value, out: &mut Output) {
        let Some(id) = msg.get("id").and_then(Value::as_u64) else { return };
        let Some(kind) = self.pending.remove(&id) else { return };
        let error = msg.get("error");
        let error_message =
            error.map(|e| s(e, "message").unwrap_or("The agent refused the request.").to_owned());
        let result = msg.get("result").unwrap_or(&Value::Null);
        match kind {
            Pending::Initialize { experimental } => {
                if error_message.is_some() && experimental {
                    // Without the experimental API: no plan mode (§Q4.2).
                    let id = self.next_id;
                    self.next_id += 1;
                    self.pending.insert(id, Pending::Initialize { experimental: false });
                    out.stdin.push(initialize_request_with(id, false).to_string());
                    return;
                }
                if let Some(message) = error_message {
                    return self.fail(TurnEnd::failed(code::AGENT_ERROR, message), out);
                }
                self.plan_ok = experimental;
                out.stdin.push(json!({"jsonrpc": "2.0", "method": "initialized"}).to_string());
                self.request(Pending::AccountRead, "account/read", json!({"refreshToken": false}), out);
            }
            Pending::AccountRead => {
                if error_message.is_none() && !parse_codex_account(result).signed_in {
                    return self.fail(TurnEnd::failed(code::LOGIN, "Codex is not signed in."), out);
                }
                let policy = approval_policy(self.settings.mode);
                match self.resume.clone() {
                    Some(native) => self.request(
                        Pending::Thread,
                        "thread/resume",
                        json!({"threadId": native, "cwd": self.cwd, "approvalPolicy": policy, "sandbox": SANDBOX, "excludeTurns": true}),
                        out,
                    ),
                    None => self.request(
                        Pending::Thread,
                        "thread/start",
                        json!({"cwd": self.cwd, "approvalPolicy": policy, "sandbox": SANDBOX, "approvalsReviewer": "user"}),
                        out,
                    ),
                };
            }
            Pending::Thread => {
                if let Some(message) = error_message {
                    return self.fail(TurnEnd::failed(code::AGENT_ERROR, message), out);
                }
                let Some(native) = result.get("thread").and_then(|t| s(t, "id")).map(str::to_owned) else {
                    return self
                        .fail(TurnEnd::failed(code::AGENT_ERROR, "The agent didn't start a thread."), out);
                };
                self.native_id = Some(native.clone());
                out.steps.push(Step::NativeId(native));
                self.thread_model = s(result, "model")
                    .map(|m| api::clean_cut(m.trim(), api::MAX_MODEL_ID).0)
                    .filter(|m| !m.is_empty());
                let answering = self.settings.model.clone().filter(|m| catalog::valid_model_id(m));
                if let Some(model) = answering.or_else(|| self.thread_model.clone()) {
                    out.steps.push(Step::AnsweredModel(model));
                }
                if self.interrupt_wanted {
                    // Interrupted before the turn began: nothing to run.
                    self.interrupt_wanted = false;
                    self.texts.clear();
                    out.steps.push(self.mapper.end_turn(TurnEnd::interrupted()));
                    return;
                }
                self.start_turn(out);
            }
            Pending::Skills => {
                let map: HashMap<String, String> =
                    if error_message.is_none() { skill_paths(result) } else { HashMap::new() };
                self.skills = Some(map);
                self.start_turn(out);
            }
            Pending::TurnStart => {
                let in_flight = self.in_flight.take();
                if let Some(error) = error {
                    if let Some(flight) = in_flight.filter(|f| Self::plan_refused(error, &f.params)) {
                        self.plan_ok = false;
                        if self.settings.mode == Mode::Plan {
                            if self.mapper.turn_running() {
                                let end = TurnEnd::failed(PLAN_UNAVAILABLE_CODE, PLAN_UNAVAILABLE);
                                out.steps.push(self.mapper.end_turn(end));
                            }
                            return self.start_turn(out);
                        }
                        let mut params = flight.params;
                        if let Some(map) = params.as_object_mut() {
                            map.remove("collaborationMode");
                        }
                        self.request(Pending::TurnStart, "turn/start", params.clone(), out);
                        self.in_flight = Some(InFlight { waiting: flight.waiting, params });
                        return;
                    }
                    let message = s(error, "message").unwrap_or("The agent refused the request.").to_owned();
                    if self.mapper.turn_running() {
                        out.steps.push(self.mapper.end_turn(TurnEnd::failed(code::AGENT_ERROR, message)));
                    }
                    return;
                }
                let turn = result.get("turn").and_then(|t| s(t, "id")).map(str::to_owned);
                self.mapper.set_turn_id(turn);
                if let Some(flight) = in_flight
                    && flight.waiting.steer
                    && !flight.waiting.input.id.is_empty()
                {
                    out.steps.push(Step::Delivered { id: flight.waiting.input.id });
                }
                if self.interrupt_wanted {
                    self.send_interrupt(out);
                }
            }
            Pending::Compact => {
                let flight = self.in_flight.take();
                if let Some(message) = error_message {
                    if self.mapper.turn_running() {
                        out.steps.push(self.mapper.end_turn(TurnEnd::failed(code::AGENT_ERROR, message)));
                    }
                    return self.start_turn(out);
                }
                if let Some(flight) = flight
                    && flight.waiting.steer
                    && !flight.waiting.input.id.is_empty()
                {
                    out.steps.push(Step::Delivered { id: flight.waiting.input.id });
                }
            }
            Pending::Steer => {
                let Some(waiting) = self.steering.remove(&id) else { return };
                if error.is_none() {
                    if !waiting.input.id.is_empty() {
                        out.steps.push(Step::Delivered { id: waiting.input.id });
                    }
                    return;
                }
                // The turn it was for is over (or refused it): it goes as
                // the next turn, before anything sent after it (§Q4.5).
                self.texts.push_front(Waiting { steer: true, ..waiting });
                if !self.mapper.turn_running() {
                    self.start_turn(out);
                }
            }
            Pending::Interrupt => {}
        }
    }

    /// The `decision` for an answer.
    pub fn decision(answer: &Answer, decline_ok: bool) -> &'static str {
        match answer {
            Answer::Allow => "accept",
            Answer::Deny(_) | Answer::DenySaying(_) if decline_ok => "decline",
            Answer::Deny(_) | Answer::DenySaying(_) => "cancel",
        }
    }

    fn steer(&mut self, input: UserInput, out: &mut Output) {
        let running = self.mapper.turn_running();
        match (running, self.native_id.clone(), self.mapper.turn_id().map(str::to_owned)) {
            (true, Some(native), Some(turn)) => {
                let params =
                    json!({"threadId": native, "expectedTurnId": turn, "input": self.inputs(&input)});
                let id = self.request(Pending::Steer, "turn/steer", params, out);
                self.steering.insert(id, Waiting { input, steer: true });
            }
            // The turn is starting: the message is its next one.
            (true, _, _) => self.texts.push_back(Waiting { input, steer: true }),
            (false, _, _) => {
                self.texts.push_back(Waiting { input, steer: true });
                self.start_turn(out);
            }
        }
    }
}

/// `/name …` → `name`.
fn slash_word(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    let word = rest.split_whitespace().next()?;
    (!word.is_empty() && rest.starts_with(word)).then_some(word)
}

/// The enabled skills' paths by name from `skills/list`.
fn skill_paths(result: &Value) -> HashMap<String, String> {
    catalog::codex_skill_entries_of(result).into_iter().map(|s| (s.command.name, s.path)).collect()
}

fn id_key(id: &Value) -> String {
    id.to_string()
}

impl Driver for CodexDriver {
    fn start(&mut self, out: &mut Output) {
        self.next_id = 2;
        self.pending.insert(1, Pending::Initialize { experimental: true });
        out.stdin.push(initialize_request(1).to_string());
    }

    fn on_line(&mut self, line: Value, out: &mut Output) {
        let has_method = line.get("method").is_some();
        let id = line.get("id").filter(|id| !id.is_null()).cloned();
        match (has_method, id) {
            (true, Some(id)) => match self.mapper.server_request(&line) {
                ServerRequest::Approval { spec, decline_ok } => {
                    self.asks.insert(id_key(&id), decline_ok);
                    out.approvals.push((id, *spec));
                }
                ServerRequest::Question(spec) => {
                    self.questions.insert(id_key(&id), spec.clone());
                    out.questions.push((id, spec));
                }
                ServerRequest::EmptyQuestion => {
                    out.stdin
                        .push(json!({"jsonrpc": "2.0", "id": id, "result": {"answers": {}}}).to_string());
                }
                ServerRequest::Elicitation => {
                    out.stdin.push(
                        json!({"jsonrpc": "2.0", "id": id, "result": {"action": "decline"}}).to_string(),
                    );
                    out.steps.push(Step::Notice(questions::MCP_DECLINED.to_owned()));
                }
                ServerRequest::Unsupported => {
                    out.stdin.push(
                        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "not supported"}})
                            .to_string(),
                    );
                    out.steps.push(Step::Notice("The agent asked for something G7 can't answer yet.".into()));
                }
            },
            (true, None) => {
                let steps = self.mapper.notification(&line);
                let ended = steps.iter().any(|s| matches!(s, Step::TurnEnded(_)));
                out.steps.extend(steps);
                if ended {
                    self.interrupt_sent = false;
                    self.interrupt_wanted = false;
                    self.start_turn(out);
                }
            }
            (false, Some(_)) => self.on_response(&line, out),
            (false, None) => {}
        }
    }

    fn on_cmd(&mut self, cmd: SessionCmd, out: &mut Output) {
        if self.failed {
            return;
        }
        match cmd {
            SessionCmd::Send(input) => {
                self.texts.push_back(Waiting { input, steer: false });
                if !self.mapper.turn_running() || self.mapper.turn_id().is_none() {
                    if !self.mapper.turn_running() {
                        self.mapper.turn_started();
                    }
                    self.start_turn(out);
                }
            }
            SessionCmd::Steer(input) => self.steer(input, out),
            SessionCmd::Interrupt => {
                if !self.mapper.turn_running() || self.interrupt_sent {
                    return;
                }
                if self.mapper.turn_id().is_some() {
                    self.send_interrupt(out);
                } else {
                    self.interrupt_wanted = true;
                }
            }
            SessionCmd::Close | SessionCmd::Kill => {}
        }
    }

    fn on_answer(&mut self, id: Value, answer: Answer, out: &mut Output) {
        let Some(decline_ok) = self.asks.remove(&id_key(&id)) else { return };
        let decision = Self::decision(&answer, decline_ok);
        out.stdin.push(json!({"jsonrpc": "2.0", "id": id, "result": {"decision": decision}}).to_string());
        // The user's words go as the next turn: stop this one now, without
        // the registry hearing of a stop (it waits for the turn's end).
        if matches!(answer, Answer::DenySaying(_))
            && INTERRUPT_FOR_REPLY
            && self.mapper.turn_running()
            && !self.interrupt_sent
        {
            if self.mapper.turn_id().is_some() {
                self.send_interrupt(out);
            } else {
                self.interrupt_wanted = true;
            }
        }
    }

    fn on_question_reply(&mut self, id: Value, reply: QuestionReply, out: &mut Output) {
        let Some(spec) = self.questions.remove(&id_key(&id)) else { return };
        let result = questions::codex_reply(&spec, &reply);
        out.stdin.push(json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string());
    }

    fn on_too_long(&mut self) -> Vec<Step> {
        if !self.mapper.turn_running() {
            return Vec::new();
        }
        vec![
            self.mapper
                .end_turn(TurnEnd::failed(code::AGENT_OUTPUT, "The agent wrote a line too long to read.")),
        ]
    }

    fn on_exit(&mut self, code: Option<i32>) -> Vec<Step> {
        let interrupted = self.interrupt_sent || self.interrupt_wanted;
        self.mapper.on_exit(code, interrupted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Outcome;

    fn fixture(name: &str) -> Vec<Value> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .collect()
    }

    /// Feeds the `recv` lines of a fixture to a mapper.
    fn run(name: &str) -> (Vec<Step>, Vec<ServerRequest>) {
        let mut mapper = CodexMapper::new(PathBuf::from("/work"));
        let mut steps = Vec::new();
        let mut requests = Vec::new();
        for entry in fixture(name) {
            let msg = &entry["msg"];
            match entry["dir"].as_str() {
                Some("send") if msg["method"] == "turn/start" => mapper.turn_started(),
                Some("recv") if msg.get("method").is_some() && msg.get("id").is_some() => {
                    requests.push(mapper.server_request(msg));
                }
                Some("recv") if msg.get("method").is_some() => steps.extend(mapper.notification(msg)),
                _ => {}
            }
        }
        (steps, requests)
    }

    fn end(steps: &[Step]) -> &TurnEnd {
        let ends: Vec<_> =
            steps.iter().filter_map(|s| if let Step::TurnEnded(e) = s { Some(e) } else { None }).collect();
        assert_eq!(ends.len(), 1);
        ends[0]
    }

    #[test]
    fn a_command_approval_then_done() {
        let (steps, requests) = run("codex-01-create-approve.jsonl");
        let [ServerRequest::Approval { spec, decline_ok }] = &requests[..] else { panic!("{requests:?}") };
        assert_eq!(spec.kind, ApprovalKind::Command);
        assert_eq!(spec.command.as_deref(), Some("printf 'hi.' > hello.txt\nls"));
        assert_eq!(spec.tool_key.as_deref(), Some("exec-752af8af-7490-447e-a5d0-2a11d897c800"));
        assert!(!decline_ok, "the capture offered accept, the amendment and cancel only");
        let ok = steps.iter().any(|s| {
            matches!(s, Step::ToolEnd { key, status: ToolStatus::Ok, output: Some(o) }
                if key == "exec-752af8af-7490-447e-a5d0-2a11d897c800" && o == "README.md\nhello.txt\n")
        });
        assert!(ok);
        let end = end(&steps);
        assert_eq!(end.outcome, Outcome::Done);
        assert_eq!(
            end.summary.as_deref(),
            Some(
                "Created `hello.txt` containing exactly `hi.`\n\n`ls` output:\n```text\nREADME.md\nhello.txt\n```"
            )
        );
        // Both messages are shown; only the final answer is the summary.
        let finals: Vec<_> = steps
            .iter()
            .filter_map(|s| {
                if let Step::AssistantDone { final_answer, .. } = s { Some(*final_answer) } else { None }
            })
            .collect();
        assert_eq!(finals, [false, true]);
    }

    #[test]
    fn a_declined_file_change() {
        let (steps, requests) = run("codex-02-resume-filechange-decline.jsonl");
        let [ServerRequest::Approval { spec, decline_ok }] = &requests[..] else { panic!("{requests:?}") };
        assert_eq!(spec.kind, ApprovalKind::Edit);
        assert_eq!(
            spec.diff.as_deref(),
            Some("diff --git a/notes.txt b/notes.txt\n--- /dev/null\n+++ b/notes.txt\n@@ -0,0 +1,1 @@\n+a")
        );
        assert_eq!(spec.path.as_deref(), Some("notes.txt"));
        assert_eq!(spec.title, "Edit notes.txt");
        assert!(decline_ok);
        assert!(steps.iter().any(|s| matches!(s, Step::ToolEnd { status: ToolStatus::Denied, .. })));
        assert_eq!(end(&steps).outcome, Outcome::Done);
    }

    #[test]
    fn interrupted_while_waiting() {
        let (steps, requests) = run("codex-03-resume-interrupt-while-waiting.jsonl");
        assert_eq!(requests.len(), 1);
        assert_eq!(end(&steps).outcome, Outcome::Interrupted);
    }

    fn phases(steps: &[Step]) -> Vec<(&str, Phase)> {
        steps
            .iter()
            .filter_map(|s| match s {
                Step::AssistantPhase { key, phase } => Some((key.as_str(), *phase)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn reasoning_and_phases_in_the_captures() {
        let (steps, _) = run("codex-02-resume-filechange-decline.jsonl");
        let opened: Vec<&str> = steps
            .iter()
            .filter_map(|s| if let Step::ReasoningOpen { key } = s { Some(key.as_str()) } else { None })
            .collect();
        let done: Vec<(&str, &str)> = steps
            .iter()
            .filter_map(|s| {
                if let Step::ReasoningDone { key, text } = s {
                    Some((key.as_str(), text.as_str()))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(opened.len(), 1);
        assert_eq!(done, [(opened[0], "")], "summary: [] in the capture");
        assert!(!steps.iter().any(|s| matches!(s, Step::ReasoningDelta { .. })));
        let kinds: Vec<Phase> = phases(&steps).into_iter().map(|(_, p)| p).collect();
        // Each message's phase comes when it starts and again when it ends.
        assert_eq!(kinds, [Phase::Interim, Phase::Interim, Phase::Final, Phase::Final]);

        let (steps, _) = run("codex-01-create-approve.jsonl");
        let kinds: Vec<Phase> = phases(&steps).into_iter().map(|(_, p)| p).collect();
        assert_eq!(kinds, [Phase::Interim, Phase::Interim, Phase::Final, Phase::Final]);
        assert!(!steps.iter().any(|s| matches!(s, Step::ReasoningOpen { .. })));
        let command = steps
            .iter()
            .find_map(|s| if let Step::ToolStart { tool, .. } = s { Some(tool) } else { None })
            .unwrap();
        assert_eq!(command.class, ToolClass::Command);
    }

    #[test]
    fn reasoning_summaries_and_their_deltas() {
        let mut mapper = CodexMapper::new(PathBuf::from("/w"));
        mapper.turn_started();
        let mut steps = Vec::new();
        for msg in [
            json!({"method": "item/started", "params": {"item": {"type": "reasoning", "id": "rs_1", "summary": [], "content": []}}}),
            json!({"method": "item/reasoning/summaryPartAdded", "params": {"itemId": "rs_1", "summaryIndex": 0}}),
            json!({"method": "item/reasoning/summaryTextDelta", "params": {"itemId": "rs_1", "delta": "**Plan**", "summaryIndex": 0}}),
            json!({"method": "item/reasoning/summaryPartAdded", "params": {"itemId": "rs_1", "summaryIndex": 1}}),
            json!({"method": "item/reasoning/summaryTextDelta", "params": {"itemId": "rs_1", "delta": "Read it.", "summaryIndex": 1}}),
            json!({"method": "item/reasoning/textDelta", "params": {"itemId": "rs_1", "delta": "raw"}}),
            json!({"method": "item/reasoning/summaryTextDelta", "params": {"itemId": "rs_9", "delta": "elsewhere"}}),
            json!({"method": "item/reasoning/summaryTextDelta", "params": {"itemId": "rs_1", "delta": 7}}),
            json!({"method": "item/completed", "params": {"item": {"type": "reasoning", "id": "rs_1",
                "summary": ["**Plan**", "Read it."], "content": []}}}),
            json!({"method": "item/started", "params": {"item": {"type": "reasoning", "id": "rs_2", "summary": []}}}),
            json!({"method": "item/completed", "params": {"item": {"type": "reasoning", "id": "rs_2",
                "summary": [{"type": "summary_text", "text": "Check."}, {"type": "x"}, 3, {"text": "Then fix."}]}}}),
            json!({"method": "item/completed", "params": {"item": {"type": "reasoning", "id": "rs_3"}}}),
            json!({"method": "item/started", "params": {"item": {"type": "agentMessage", "id": "m1", "text": ""}}}),
            json!({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "m1", "text": "Looking.", "phase": "later"}}}),
            json!({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "m2", "text": "Fixed."}}}),
            json!({"method": "turn/completed", "params": {"turn": {"status": "completed"}}}),
        ] {
            steps.extend(mapper.notification(&msg));
        }
        let deltas: Vec<&str> = steps
            .iter()
            .filter_map(
                |s| if let Step::ReasoningDelta { text, .. } = s { Some(text.as_str()) } else { None },
            )
            .collect();
        assert_eq!(deltas, ["**Plan**", "\n\n", "Read it."], "a part break only once there is text");
        let done: Vec<(&str, &str)> = steps
            .iter()
            .filter_map(|s| {
                if let Step::ReasoningDone { key, text } = s {
                    Some((key.as_str(), text.as_str()))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(done, [("rs_1", "**Plan**\n\nRead it."), ("rs_2", "Check.\n\nThen fix."), ("rs_3", "")]);
        // Unknown and missing phases are settled when the turn completes.
        assert_eq!(phases(&steps), [("m1", Phase::Interim), ("m2", Phase::Final)]);
        assert!(matches!(steps.last(), Some(Step::TurnEnded(e)) if e.outcome == Outcome::Done));
    }

    #[test]
    fn an_interrupted_turn_leaves_its_messages_without_a_phase() {
        let mut mapper = CodexMapper::new(PathBuf::from("/w"));
        mapper.turn_started();
        mapper.notification(&json!({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "m1", "text": "On it."}}}));
        let steps = mapper.notification(
            &json!({"method": "turn/completed", "params": {"turn": {"status": "interrupted"}}}),
        );
        assert!(phases(&steps).is_empty());
    }

    #[test]
    fn tool_classes_and_the_first_changed_file() {
        let mut mapper = CodexMapper::new(PathBuf::from("/w"));
        mapper.turn_started();
        let started = |mapper: &mut CodexMapper, item: Value| {
            let steps = mapper.notification(&json!({"method": "item/started", "params": {"item": item}}));
            match steps.into_iter().next() {
                Some(Step::ToolStart { tool, .. }) => tool,
                other => panic!("{other:?}"),
            }
        };
        for (kind, class) in [
            ("mcpToolCall", ToolClass::Mcp),
            ("webSearch", ToolClass::Web),
            ("collabAgentToolCall", ToolClass::Agent),
            ("dynamicToolCall", ToolClass::Other),
            ("imageView", ToolClass::Other),
        ] {
            assert_eq!(started(&mut mapper, json!({"type": kind, "id": kind})).class, class, "{kind}");
        }
        let two = started(
            &mut mapper,
            json!({"type": "fileChange", "id": "fc", "changes": [
                {"path": "/w/a.txt", "kind": {"type": "add"}, "diff": "x"},
                {"path": "/w/b.txt", "kind": {"type": "update"}, "diff": "y"}]}),
        );
        assert_eq!((two.class, two.path.as_deref()), (ToolClass::Edit, Some("a.txt")));
        // The request for two files names neither.
        let ServerRequest::Approval { spec, .. } = mapper.server_request(
            &json!({"method": "item/fileChange/requestApproval", "id": 3, "params": {"itemId": "fc"}}),
        ) else {
            panic!()
        };
        assert_eq!((spec.path, spec.title.as_str()), (None, "Edit files"));
    }

    /// The driver up to its first `turn/start`; the lines it wrote.
    fn driver_started() -> (CodexDriver, Vec<Value>) {
        let mut driver = CodexDriver::new(PathBuf::from("/w"), Mode::Supervised, None, "Hi".into());
        let mut out = Output::default();
        driver.start(&mut out);
        for reply in [
            json!({"id": 1, "result": {}}),
            json!({"id": 2, "result": {"account": {"type": "chatgpt"}, "requiresOpenaiAuth": true}}),
            json!({"id": 3, "result": {"thread": {"id": "th_1"}}}),
            json!({"id": 4, "result": {"turn": {"id": "tu_1"}}}),
        ] {
            driver.on_line(reply, &mut out);
        }
        let lines = out.stdin.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
        (driver, lines)
    }

    #[test]
    fn a_turn_asks_for_reasoning_summaries() {
        let (_, lines) = driver_started();
        let start = lines.iter().find(|l| l["method"] == "turn/start").unwrap();
        assert_eq!(start["params"]["summary"], REASONING_SUMMARY);
        assert_eq!(start["params"]["threadId"], "th_1");
    }

    #[test]
    fn denying_with_words_declines_then_stops_the_turn() {
        let (mut driver, _) = driver_started();
        let mut out = Output::default();
        driver.on_line(
            json!({"method": "item/fileChange/requestApproval", "id": 0, "params": {"itemId": "fc", "threadId": "th_1", "turnId": "tu_1"}}),
            &mut out,
        );
        let (id, _) = out.approvals.pop().unwrap();
        let mut out = Output::default();
        driver.on_answer(id, Answer::DenySaying("Use a temp dir".into()), &mut out);
        let lines: Vec<Value> = out.stdin.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines[0], json!({"jsonrpc": "2.0", "id": 0, "result": {"decision": "decline"}}));
        assert_eq!(lines[1]["method"], "turn/interrupt");
        assert_eq!(lines[1]["params"], json!({"threadId": "th_1", "turnId": "tu_1"}));
        assert!(!format!("{lines:?}").contains("temp dir"), "the words go as the next turn, not now");
        // A plain denial doesn't stop the turn.
        let mut out = Output::default();
        driver.on_line(
            json!({"method": "item/commandExecution/requestApproval", "id": 1, "params": {"itemId": "c", "command": "ls"}}),
            &mut out,
        );
        let (id, _) = out.approvals.pop().unwrap();
        let mut out = Output::default();
        driver.on_answer(id, Answer::Deny(crate::host::agents::DenyReason::User), &mut out);
        assert_eq!(out.stdin.len(), 1);
    }

    #[test]
    fn failures() {
        let mut mapper = CodexMapper::new(PathBuf::from("/w"));
        mapper.turn_started();
        mapper.notification(&json!({"method":"account/rateLimits/updated","params":{"rateLimits":{
            "primary":{"usedPercent":100,"resetsAt":1795055175},"secondary":{"usedPercent":15,"resetsAt":1795113266}}}}));
        let steps = mapper
            .notification(&json!({"method":"turn/completed","params":{"turn":{"status":"failed",
            "error":{"message":"You've hit your usage limit.","codexErrorInfo":"usageLimitExceeded"}}}}));
        let Step::TurnEnded(end) = &steps[0] else { panic!() };
        assert_eq!(end.error.as_ref().unwrap().code, code::USAGE_LIMIT);
        assert_eq!(end.error.as_ref().unwrap().message, "Usage limit reached|resets_ms=1795055175000");

        mapper.turn_started();
        let steps = mapper
            .notification(&json!({"method":"turn/completed","params":{"turn":{"status":"failed",
            "error":{"message":"Your login is not allowed.","codexErrorInfo":"unauthorized"}}}}));
        let Step::TurnEnded(end) = &steps[0] else { panic!() };
        assert_eq!(end.error.as_ref().unwrap().code, code::LOGIN);
        assert_eq!(end.error.as_ref().unwrap().message, "Your login is not allowed.");

        mapper.turn_started();
        let steps = mapper.notification(
            &json!({"method":"error","params":{"willRetry":true,"error":{"message":"Reconnecting"}}}),
        );
        assert_eq!(steps, [Step::Notice("Reconnecting".into())]);
        let steps = mapper
            .notification(&json!({"method":"turn/completed","params":{"turn":{"status":"failed",
            "error":{"message":"Bad","codexErrorInfo":{"httpConnectionFailed":{"httpStatusCode":502}}}}}}));
        let Step::TurnEnded(end) = &steps[0] else { panic!() };
        assert_eq!(end.error.as_ref().unwrap().code, "httpConnectionFailed");
    }

    #[test]
    fn a_turn_plan() {
        let mut mapper = CodexMapper::new(PathBuf::from("/w"));
        mapper.turn_started();
        let steps = mapper.notification(&json!({"method":"turn/plan/updated","params":{"threadId":"t","turnId":"u",
            "plan":[{"step":"Look","status":"completed"},{"step":"Change","status":"inProgress"},{"step":"Test","status":"pending"}]}}));
        let [Step::Plan(plan)] = &steps[..] else { panic!("{steps:?}") };
        let statuses: Vec<_> = plan.iter().map(|p| p.status).collect();
        assert_eq!(
            statuses,
            [
                crate::api::PlanStatus::Completed,
                crate::api::PlanStatus::InProgress,
                crate::api::PlanStatus::Pending
            ]
        );
    }

    #[test]
    fn decisions() {
        assert_eq!(CodexDriver::decision(&Answer::Allow, false), "accept");
        assert_eq!(
            CodexDriver::decision(&Answer::Deny(crate::host::agents::DenyReason::User), true),
            "decline"
        );
        assert_eq!(
            CodexDriver::decision(&Answer::Deny(crate::host::agents::DenyReason::Expired), false),
            "cancel"
        );
        assert_eq!(CodexDriver::decision(&Answer::DenySaying("x".into()), true), "decline");
        assert_eq!(CodexDriver::decision(&Answer::DenySaying("x".into()), false), "cancel");
    }

    #[test]
    fn unknown_server_requests_are_refused() {
        let mut mapper = CodexMapper::new(PathBuf::from("/w"));
        let req = json!({"method":"item/tool/call","id":7,"params":{}});
        assert_eq!(mapper.server_request(&req), ServerRequest::Unsupported);
        let req = json!({"method":"item/tool/requestUserInput","id":8,"params":{"questions":[]}});
        assert_eq!(mapper.server_request(&req), ServerRequest::EmptyQuestion);
    }

    /// The fields and values we send are ones the 0.170.0 schema has.
    #[test]
    fn our_requests_match_the_schema() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-schema");
        let read = |name: &str| {
            serde_json::from_str::<Value>(&std::fs::read_to_string(dir.join(name)).unwrap()).unwrap()
        };
        let v2 = read("v2-subset.json");
        let props = |name: &str| -> Vec<String> {
            v2["definitions"][name]["properties"].as_object().unwrap().keys().cloned().collect()
        };
        for (name, fields) in [
            ("ThreadStartParams", &["cwd", "approvalPolicy", "sandbox", "approvalsReviewer"][..]),
            ("ThreadResumeParams", &["threadId", "cwd", "approvalPolicy", "sandbox", "excludeTurns"][..]),
            ("TurnStartParams", &["threadId", "input", "summary"][..]),
            ("TurnInterruptParams", &["threadId", "turnId"][..]),
            ("InitializeParams", &["clientInfo", "capabilities"][..]),
        ] {
            let have = props(name);
            for field in fields {
                assert!(have.iter().any(|h| h == field), "{name}.{field}");
            }
        }
        // What we read from `turn/plan/updated`.
        assert!(props("TurnPlanUpdatedNotification").iter().any(|h| h == "plan"));
        for field in ["step", "status"] {
            assert!(props("TurnPlanStep").iter().any(|h| h == field), "{field}");
        }
        assert_eq!(
            v2["definitions"]["TurnPlanStepStatus"]["enum"],
            json!(["pending", "inProgress", "completed"])
        );
        let text = v2.to_string();
        let summaries = v2["definitions"]["ReasoningSummary"].to_string();
        assert!(summaries.contains(&format!("\"{REASONING_SUMMARY}\"")), "{summaries}");
        for value in [approval_policy(Mode::Supervised), approval_policy(Mode::AutoEdit), SANDBOX] {
            assert!(text.contains(&format!("\"{value}\"")), "{value}");
        }
        for file in ["CommandExecutionRequestApprovalResponse.json", "FileChangeRequestApprovalResponse.json"]
        {
            let text = read(file).to_string();
            for decision in ["accept", "decline", "cancel"] {
                assert!(text.contains(&format!("\"{decision}\"")), "{file}: {decision}");
            }
        }
    }

    fn plan_settings() -> LaunchSettings {
        LaunchSettings {
            mode: Mode::Plan,
            model: None,
            effort: None,
            mcp: true,
            default_model: Some("gpt-5.5-codex".into()),
            default_effort: Some("medium".into()),
        }
    }

    fn sent(out: &Output) -> Vec<Value> {
        out.stdin.iter().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    /// Plays a capture against the driver: its `recv` lines go in, a question
    /// is answered with `reply`; the lines the driver writes and its steps.
    fn play(
        name: &str,
        settings: LaunchSettings,
        reply: impl Fn() -> QuestionReply,
    ) -> (Vec<Step>, Vec<Value>) {
        let first = fixture(name)
            .iter()
            .find(|e| e["msg"]["method"] == "turn/start")
            .and_then(|e| e["msg"]["params"]["input"][0]["text"].as_str().map(str::to_owned))
            .unwrap();
        let mut driver =
            CodexDriver::with_settings(PathBuf::from("/work"), settings, None, UserInput::text(first));
        let mut out = Output::default();
        driver.start(&mut out);
        for entry in fixture(name) {
            if entry["dir"] != "recv" {
                continue;
            }
            driver.on_line(entry["msg"].clone(), &mut out);
            for (id, _) in std::mem::take(&mut out.questions) {
                driver.on_question_reply(id, reply(), &mut out);
            }
        }
        let lines = sent(&out);
        (out.steps, lines)
    }

    #[test]
    fn a_plan_turn_with_a_question_as_captured() {
        let answers = || {
            QuestionReply::Answered(crate::host::agents::SecretAnswers::new(vec![api::Answer {
                id: "colour".into(),
                labels: vec!["Blue".into()],
                other: None,
            }]))
        };
        let (steps, lines) = play("codex-11-plan-question.jsonl", plan_settings(), answers);
        let expected: Vec<Value> = fixture("codex-11-plan-question.jsonl")
            .into_iter()
            .filter(|e| e["dir"] == "send")
            .map(|e| e["msg"].clone())
            .collect();
        // Every line the driver wrote is the capture's (the client version aside).
        assert_eq!(lines.len(), expected.len());
        for (ours, theirs) in lines.iter().zip(&expected) {
            let mut ours = ours.clone();
            if let Some(version) = ours.pointer_mut("/params/clientInfo/version") {
                *version = json!("0.1.17");
            }
            assert_eq!(&ours, theirs);
        }
        let plan: Vec<&Step> = steps
            .iter()
            .filter(|s| matches!(s, Step::PlanOpen { .. } | Step::PlanDelta { .. } | Step::PlanDone { .. }))
            .collect();
        assert_eq!(
            plan,
            [
                &Step::PlanOpen { key: "plan_1".into() },
                &Step::PlanDelta { key: "plan_1".into(), text: "1. Add notes.txt ".into() },
                &Step::PlanDelta { key: "plan_1".into(), text: "with blue.".into() },
                &Step::PlanDone {
                    key: "plan_1".into(),
                    text: "1. Add notes.txt with blue.\n2. Done.".into()
                },
            ]
        );
        assert!(steps.contains(&Step::Context { used: 19_384, max: 475_000 }));
        assert!(steps.contains(&Step::AnsweredModel("gpt-5.5-codex".into())));
        assert!(steps.contains(&Step::Notice(questions::MCP_DECLINED.into())));
        let limits = steps.iter().find_map(|s| if let Step::Limits(l) = s { Some(l) } else { None }).unwrap();
        assert_eq!((limits[0].window.as_str(), limits[0].state), ("300", api::LimitState::Warn));
        let mcp: Vec<&Step> = steps.iter().filter(|s| matches!(s, Step::Mcp { .. })).collect();
        assert_eq!(
            mcp.last().unwrap(),
            &&Step::Mcp { names: vec!["docs".into(), "tracker".into()], connected: 1, need_login: 1 }
        );
        assert!(matches!(steps.last(), Some(Step::TurnEnded(e)) if e.outcome == Outcome::Done));
        // Not one word of the question's text is in a step: it is the host's to show.
        assert!(!format!("{steps:?}").contains("Red or blue"));
    }

    #[test]
    fn a_dismissed_question_answers_empty() {
        let (_, lines) = play("codex-11-plan-question.jsonl", plan_settings(), || QuestionReply::Dismissed);
        let reply = lines.iter().find(|l| l["id"] == 0 && l.get("result").is_some()).unwrap();
        assert_eq!(reply["result"], json!({"answers": {"colour": {"answers": []}}}));
    }

    fn started(settings: LaunchSettings, thread_result: Value) -> (CodexDriver, Output) {
        let mut driver =
            CodexDriver::with_settings(PathBuf::from("/w"), settings, None, UserInput::text("Hi"));
        let mut out = Output::default();
        driver.start(&mut out);
        for reply in [
            json!({"id": 1, "result": {}}),
            json!({"id": 2, "result": {"account": {"type": "chatgpt"}, "requiresOpenaiAuth": true}}),
            json!({"id": 3, "result": thread_result}),
        ] {
            driver.on_line(reply, &mut out);
        }
        (driver, out)
    }

    #[test]
    fn model_effort_and_mode_go_with_each_turn() {
        let settings = LaunchSettings {
            mode: Mode::Supervised,
            model: Some("gpt-5.5".into()),
            effort: Some("xhigh".into()),
            ..plan_settings()
        };
        let (_, out) = started(settings, json!({"thread": {"id": "th"}, "model": "gpt-5.5-codex"}));
        let lines = sent(&out);
        assert_eq!(lines[0]["params"]["capabilities"], json!({"experimentalApi": true}));
        let turn = &lines.iter().find(|l| l["method"] == "turn/start").unwrap()["params"];
        assert_eq!((turn["model"].as_str(), turn["effort"].as_str()), (Some("gpt-5.5"), Some("xhigh")));
        assert_eq!(
            turn["collaborationMode"],
            json!({"mode": "default", "settings": {"model": "gpt-5.5", "reasoning_effort": "xhigh", "developer_instructions": null}})
        );
        assert!(out.steps.contains(&Step::AnsweredModel("gpt-5.5".into())), "the thread's own model");
        // A model or effort we can't pass safely is left out.
        let settings = LaunchSettings {
            mode: Mode::Supervised,
            model: Some("-x".into()),
            effort: Some("max!".into()),
            ..plan_settings()
        };
        let (_, out) = started(settings, json!({"thread": {"id": "th"}, "model": "gpt-5.5-codex"}));
        let turn = sent(&out).into_iter().find(|l| l["method"] == "turn/start").unwrap()["params"].clone();
        assert!(turn.get("model").is_none() && turn.get("effort").is_none());
        assert_eq!(turn["collaborationMode"]["settings"]["model"], "gpt-5.5-codex");
        assert_eq!(turn["collaborationMode"]["settings"]["reasoning_effort"], "medium");
    }

    #[test]
    fn without_the_experimental_api_there_is_no_plan_mode() {
        let mut driver =
            CodexDriver::with_settings(PathBuf::from("/w"), plan_settings(), None, UserInput::text("Plan"));
        let mut out = Output::default();
        driver.start(&mut out);
        driver
            .on_line(json!({"id": 1, "error": {"code": -32600, "message": "unknown capability"}}), &mut out);
        let lines = sent(&out);
        assert_eq!(lines[1]["method"], "initialize");
        assert_eq!(lines[1]["params"]["capabilities"], Value::Null);
        let id = lines[1]["id"].clone();
        driver.on_line(json!({"id": id, "result": {}}), &mut out);
        let account = sent(&out).last().unwrap()["id"].clone();
        driver.on_line(json!({"id": account, "result": {"account": {"type": "chatgpt"}}}), &mut out);
        let thread = sent(&out).last().unwrap()["id"].clone();
        driver.on_line(json!({"id": thread, "result": {"thread": {"id": "th"}}}), &mut out);
        assert!(
            !sent(&out).iter().any(|l| l["method"] == "turn/start"),
            "a plan turn isn't sent without plan mode"
        );
        let Some(Step::TurnEnded(end)) = out.steps.last() else { panic!("{:?}", out.steps) };
        assert_eq!(end.error.as_ref().unwrap().code, PLAN_UNAVAILABLE_CODE);
        assert_eq!(end.error.as_ref().unwrap().message, PLAN_UNAVAILABLE);
    }

    #[test]
    fn a_refused_collaboration_mode() {
        let refusal = json!({"code": -32600, "message": "turn/start.collaborationMode requires experimentalApi capability"});
        // Plan mode: the turn fails with the reason.
        let (mut driver, mut out) = started(plan_settings(), json!({"thread": {"id": "th"}}));
        let turn = sent(&out).into_iter().find(|l| l["method"] == "turn/start").unwrap();
        assert_eq!(turn["params"]["collaborationMode"]["mode"], "plan");
        driver.on_line(json!({"id": turn["id"], "error": refusal}), &mut out);
        assert!(
            matches!(out.steps.last(), Some(Step::TurnEnded(e)) if e.error.as_ref().unwrap().code == PLAN_UNAVAILABLE_CODE)
        );
        // Another mode: sent again without it, and never again.
        let settings = LaunchSettings { mode: Mode::Supervised, ..plan_settings() };
        let (mut driver, mut out) = started(settings, json!({"thread": {"id": "th"}}));
        let turn = sent(&out).into_iter().find(|l| l["method"] == "turn/start").unwrap();
        driver.on_line(json!({"id": turn["id"], "error": refusal}), &mut out);
        let again = sent(&out).into_iter().rev().find(|l| l["method"] == "turn/start").unwrap();
        assert_ne!(again["id"], turn["id"]);
        assert!(again["params"].get("collaborationMode").is_none());
        assert_eq!(again["params"]["input"], turn["params"]["input"]);
        assert!(!out.steps.iter().any(|s| matches!(s, Step::TurnEnded(_))));
        driver.on_line(json!({"id": again["id"], "result": {"turn": {"id": "tu"}}}), &mut out);
        driver.on_line(
            json!({"method": "turn/completed", "params": {"turn": {"status": "completed"}}}),
            &mut out,
        );
        driver.on_cmd(SessionCmd::Send(UserInput::text("More")), &mut out);
        let next = sent(&out).into_iter().rev().find(|l| l["method"] == "turn/start").unwrap();
        assert!(next["params"].get("collaborationMode").is_none());
    }

    #[test]
    fn steering_the_running_turn() {
        let (mut driver, mut out) =
            started(LaunchSettings::new(Mode::Supervised), json!({"thread": {"id": "th"}}));
        let turn = sent(&out).into_iter().find(|l| l["method"] == "turn/start").unwrap();
        driver.on_line(json!({"id": turn["id"], "result": {"turn": {"id": "tu_1"}}}), &mut out);
        let mut out = Output::default();
        let image = json!({"type": "localImage", "path": "/data/images/th/a.png"});
        let input = UserInput { id: "u2".into(), text: "Use blue".into(), images: vec![image.clone()] };
        driver.on_cmd(SessionCmd::Steer(input), &mut out);
        let steer = sent(&out).pop().unwrap();
        assert_eq!(steer["method"], "turn/steer");
        assert_eq!(
            steer["params"],
            json!({"threadId": "th", "expectedTurnId": "tu_1", "input": [image, {"type": "text", "text": "Use blue", "text_elements": []}]})
        );
        driver.on_line(json!({"id": steer["id"], "result": {"turnId": "tu_1"}}), &mut out);
        assert_eq!(out.steps, [Step::Delivered { id: "u2".into() }]);
        // A mismatch: it goes as the next turn when this one ends.
        let mut out = Output::default();
        driver.on_cmd(
            SessionCmd::Steer(UserInput { id: "u3".into(), text: "Late".into(), images: Vec::new() }),
            &mut out,
        );
        let steer = sent(&out).pop().unwrap();
        driver.on_line(
            json!({"id": steer["id"], "error": {"code": -32600, "message": "turn mismatch"}}),
            &mut out,
        );
        assert!(out.steps.is_empty());
        driver.on_line(
            json!({"method": "turn/completed", "params": {"turn": {"id": "tu_1", "status": "completed"}}}),
            &mut out,
        );
        let marks: Vec<&Step> =
            out.steps.iter().filter(|s| matches!(s, Step::TurnEnded(_) | Step::TurnStarted)).collect();
        assert_eq!(marks.len(), 2);
        assert!(matches!(marks[1], Step::TurnStarted));
        let next = sent(&out).pop().unwrap();
        assert_eq!(
            (next["method"].as_str(), next["params"]["input"][0]["text"].as_str()),
            (Some("turn/start"), Some("Late"))
        );
        driver.on_line(json!({"id": next["id"], "result": {"turn": {"id": "tu_2"}}}), &mut out);
        assert_eq!(out.steps.last(), Some(&Step::Delivered { id: "u3".into() }));
    }

    #[test]
    fn compact_and_skills() {
        let (mut driver, mut out) =
            started(LaunchSettings::new(Mode::Supervised), json!({"thread": {"id": "th"}}));
        let turn = sent(&out).into_iter().find(|l| l["method"] == "turn/start").unwrap();
        driver.on_line(json!({"id": turn["id"], "result": {"turn": {"id": "tu_1"}}}), &mut out);
        driver.on_line(
            json!({"method": "turn/completed", "params": {"turn": {"status": "completed"}}}),
            &mut out,
        );
        let mut out = Output::default();
        driver.on_cmd(SessionCmd::Send(UserInput::text("/compact")), &mut out);
        let compact = sent(&out).pop().unwrap();
        assert_eq!(
            (compact["method"].as_str(), &compact["params"]),
            (Some("thread/compact/start"), &json!({"threadId": "th"}))
        );
        driver.on_line(json!({"id": compact["id"], "result": {}}), &mut out);
        driver.on_line(
            json!({"method": "thread/compacted", "params": {"threadId": "th", "turnId": "x"}}),
            &mut out,
        );
        assert_eq!(out.steps[0], Step::Notice(COMPACTED.into()));
        assert!(matches!(&out.steps[1], Step::TurnEnded(e) if e.outcome == Outcome::Done));
        // A message naming a skill: the folder's skills first, then the turn with it.
        let mut out = Output::default();
        driver.on_cmd(SessionCmd::Send(UserInput::text("/review the parser")), &mut out);
        let skills = sent(&out).pop().unwrap();
        assert_eq!(
            (skills["method"].as_str(), &skills["params"]),
            (Some("skills/list"), &json!({"cwds": ["/w"]}))
        );
        driver.on_line(
            json!({"id": skills["id"], "result": {"data": [{"cwd": "/w", "errors": [], "skills": [
                {"name": "review", "description": "d", "enabled": true, "path": "/s/review/SKILL.md", "scope": "user"},
                {"name": "off", "description": "d", "enabled": false, "path": "/s/off", "scope": "user"}]}]}}),
            &mut out,
        );
        let turn = sent(&out).pop().unwrap();
        assert_eq!(
            turn["params"]["input"],
            json!([{"type": "skill", "name": "review", "path": "/s/review/SKILL.md"},
                   {"type": "text", "text": "/review the parser", "text_elements": []}])
        );
        // Asked once per process.
        driver.on_line(json!({"id": turn["id"], "result": {"turn": {"id": "tu_3"}}}), &mut out);
        driver.on_line(
            json!({"method": "turn/completed", "params": {"turn": {"status": "completed"}}}),
            &mut out,
        );
        let mut out = Output::default();
        driver.on_cmd(SessionCmd::Send(UserInput::text("/off now")), &mut out);
        let turn = sent(&out).pop().unwrap();
        assert_eq!(turn["method"], "turn/start");
        assert_eq!(turn["params"]["input"].as_array().unwrap().len(), 1, "a disabled skill is plain text");
    }

    #[test]
    fn the_methods_we_send_are_stable_ones() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-schema");
        let stable: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("client-methods.json")).unwrap()).unwrap();
        let stable: Vec<&str> =
            stable["methods"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        for method in METHODS {
            assert!(stable.contains(&method), "{method}");
        }
        for never in [
            "account/rateLimitResetCredit/consume",
            "account/usage/read",
            "thread/queue/add",
            "collaborationMode/list",
        ] {
            assert!(!METHODS.contains(&never), "{never}");
            for file in [include_str!("codex.rs"), include_str!("catalog.rs")] {
                let code = &file[..file.find("#[cfg(test)]").unwrap()];
                assert!(!code.contains(never), "{never}");
            }
        }
        // Every method the code names in a request is on the list.
        for file in [include_str!("codex.rs"), include_str!("catalog.rs")] {
            let code = &file[..file.find("#[cfg(test)]").unwrap()];
            for (i, _) in code.match_indices("\"method\": ") {
                let rest = &code[i + 10..];
                if let Some(name) = rest.strip_prefix('"').and_then(|r| r.split('"').next())
                    && name.contains('/')
                {
                    assert!(METHODS.contains(&name) || name == "initialized", "{name}");
                }
            }
        }
        // The fields we send are stable ones, but `turn/start.collaborationMode`.
        let v2: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("v2-subset.json")).unwrap()).unwrap();
        let props = |name: &str| -> Vec<String> {
            v2["definitions"][name]["properties"].as_object().unwrap().keys().cloned().collect()
        };
        for (name, fields) in [
            ("TurnStartParams", &["threadId", "input", "summary", "model", "effort"][..]),
            ("TurnSteerParams", &["threadId", "expectedTurnId", "input"][..]),
            ("ModelListParams", &["cursor"][..]),
            ("SkillsListParams", &["cwds"][..]),
            ("ThreadCompactStartParams", &["threadId"][..]),
            ("ToolRequestUserInputResponse", &["answers"][..]),
            ("McpServerElicitationRequestResponse", &["action"][..]),
            ("InitializeCapabilities", &["experimentalApi"][..]),
        ] {
            let have = props(name);
            for field in fields {
                assert!(have.iter().any(|h| h == field), "{name}.{field}");
            }
        }
        assert!(!props("TurnStartParams").iter().any(|p| p == "collaborationMode"), "the named exception");
        let input = v2["definitions"]["UserInput"].to_string();
        for kind in ["\"localImage\"", "\"skill\"", "\"text\""] {
            assert!(input.contains(kind), "{kind}");
        }
        assert!(v2["definitions"]["McpServerElicitationAction"].to_string().contains("\"decline\""));
    }
}
