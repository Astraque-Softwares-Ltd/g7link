// SPDX-License-Identifier: MPL-2.0
//! The Codex adapter (§5): `codex app-server` over stdio JSON-RPC, one
//! process per thread.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::api::{ApprovalKind, Mode, ToolStatus};
use crate::host::agents::{
    Answer, ApprovalSpec, Driver, Output, SessionCmd, Step, ToolSpec, TurnEnd, code, display_path,
    parse_codex_account, plan_of,
};
use crate::host::claude::usage_limit_message;

/// How we name ourselves to Codex (`clientInfo`, a documented field).
pub const CLIENT_NAME: &str = "G7 Linux";

pub fn initialize_request(id: u64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": {
        "clientInfo": {"name": CLIENT_NAME, "title": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION")},
        "capabilities": null,
    }})
}

/// The approval policy for a mode.
pub fn approval_policy(mode: Mode) -> &'static str {
    match mode {
        Mode::Supervised => "untrusted",
        Mode::AutoEdit => "on-request",
    }
}

pub const SANDBOX: &str = "workspace-write";

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
    }
}

fn file_tool(item: &Value, cwd: &Path) -> ToolSpec {
    let changes = item.get("changes").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut detail = String::new();
    let mut paths = Vec::new();
    for change in &changes {
        let path = display_path(s(change, "path").unwrap_or_default(), cwd);
        let kind = change.get("kind").and_then(|k| s(k, "type")).unwrap_or("update");
        if !detail.is_empty() {
            detail.push('\n');
        }
        detail.push_str(&format!("{path} ({kind})\n"));
        detail.push_str(s(change, "diff").unwrap_or_default());
        paths.push(path);
    }
    let path = if paths.len() == 1 { paths.pop() } else { None };
    ToolSpec {
        name: "apply_patch".into(),
        title: "Edit files".into(),
        detail: Some(detail.clone()),
        kind: ApprovalKind::Edit,
        command: None,
        diff: Some(detail),
        path,
        description: None,
    }
}

/// A request Codex sent us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerRequest {
    Approval { spec: Box<ApprovalSpec>, decline_ok: bool },
    Unsupported,
}

/// Turns Codex notifications and server requests into steps (§5.4, §5.5).
#[derive(Debug)]
pub struct CodexMapper {
    cwd: PathBuf,
    /// File changes by item id: their approvals carry no diff.
    file_changes: HashMap<String, ToolSpec>,
    last_final: Option<String>,
    last_text: Option<String>,
    turn_id: Option<String>,
    turn_running: bool,
    rate_limits: Option<Value>,
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
        Step::TurnEnded(end)
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
                    "agentMessage" => steps.push(Step::AssistantOpen { key: id }),
                    "commandExecution" => steps.push(Step::ToolStart { key: id, tool: command_tool(item) }),
                    "fileChange" => {
                        let tool = file_tool(item, &self.cwd);
                        self.file_changes.insert(id.clone(), tool.clone());
                        steps.push(Step::ToolStart { key: id, tool });
                    }
                    kind @ ("mcpToolCall"
                    | "dynamicToolCall"
                    | "webSearch"
                    | "imageView"
                    | "collabAgentToolCall") => {
                        let title = s(item, "tool").unwrap_or(kind).to_owned();
                        steps.push(Step::ToolStart {
                            key: id,
                            tool: ToolSpec {
                                name: kind.to_owned(),
                                title,
                                detail: None,
                                kind: ApprovalKind::Other,
                                command: None,
                                diff: None,
                                path: None,
                                description: None,
                            },
                        });
                    }
                    _ => {}
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
                        let final_answer = s(item, "phase") == Some("final_answer");
                        if final_answer {
                            self.last_final = Some(text.clone());
                        }
                        if !text.trim().is_empty() {
                            self.last_text = Some(text.clone());
                        }
                        steps.push(Step::AssistantDone { key: id, text, final_answer });
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
            "account/rateLimits/updated" => self.rate_limits = params.get("rateLimits").cloned(),
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
                let tool =
                    item_id.as_ref().and_then(|id| self.file_changes.get(id)).cloned().unwrap_or(ToolSpec {
                        name: "apply_patch".into(),
                        title: "Edit files".into(),
                        detail: None,
                        kind: ApprovalKind::Edit,
                        command: None,
                        diff: None,
                        path: None,
                        description: None,
                    });
                let mut spec = ApprovalSpec::from_tool(&tool, item_id);
                if let Some(path) = &tool.path {
                    spec.title = format!("Edit {path}");
                }
                spec.reason = reason;
                ServerRequest::Approval { spec: Box::new(spec), decline_ok }
            }
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
    Initialize,
    AccountRead,
    Thread,
    TurnStart,
    Interrupt,
}

/// The Codex side of [`crate::host::agents::run_session`].
#[derive(Debug)]
pub struct CodexDriver {
    mapper: CodexMapper,
    cwd: String,
    mode: Mode,
    resume: Option<String>,
    native_id: Option<String>,
    texts: VecDeque<String>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    /// Server requests waiting for the user: whether `decline` may be sent.
    asks: HashMap<String, bool>,
    interrupt_wanted: bool,
    interrupt_sent: bool,
    failed: bool,
}

impl CodexDriver {
    pub fn new(cwd: PathBuf, mode: Mode, resume: Option<String>, first_text: String) -> Self {
        let mut mapper = CodexMapper::new(cwd.clone());
        mapper.turn_started();
        Self {
            mapper,
            cwd: cwd.to_string_lossy().into_owned(),
            mode,
            resume,
            native_id: None,
            texts: VecDeque::from([first_text]),
            next_id: 1,
            pending: HashMap::new(),
            asks: HashMap::new(),
            interrupt_wanted: false,
            interrupt_sent: false,
            failed: false,
        }
    }

    fn request(&mut self, kind: Pending, method: &str, params: Value, out: &mut Output) {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, kind);
        out.stdin.push(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string());
    }

    fn start_turn(&mut self, out: &mut Output) {
        let Some(native) = self.native_id.clone() else { return };
        if self.pending.values().any(|p| *p == Pending::TurnStart) {
            return;
        }
        let Some(text) = self.texts.pop_front() else { return };
        if !self.mapper.turn_running() {
            self.mapper.turn_started();
        }
        self.interrupt_sent = false;
        let params = json!({"threadId": native, "input": [{"type": "text", "text": text}]});
        self.request(Pending::TurnStart, "turn/start", params, out);
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

    fn on_response(&mut self, msg: &Value, out: &mut Output) {
        let Some(id) = msg.get("id").and_then(Value::as_u64) else { return };
        let Some(kind) = self.pending.remove(&id) else { return };
        let error_message =
            msg.get("error").map(|e| s(e, "message").unwrap_or("The agent refused the request.").to_owned());
        let result = msg.get("result").unwrap_or(&Value::Null);
        match kind {
            Pending::Initialize => {
                if let Some(message) = error_message {
                    return self.fail(TurnEnd::failed(code::AGENT_ERROR, message), out);
                }
                out.stdin.push(json!({"jsonrpc": "2.0", "method": "initialized"}).to_string());
                self.request(Pending::AccountRead, "account/read", json!({"refreshToken": false}), out);
            }
            Pending::AccountRead => {
                if error_message.is_none() && !parse_codex_account(result).signed_in {
                    return self.fail(TurnEnd::failed(code::LOGIN, "Codex is not signed in."), out);
                }
                let policy = approval_policy(self.mode);
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
                }
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
                if self.interrupt_wanted {
                    // Interrupted before the turn began: nothing to run.
                    self.interrupt_wanted = false;
                    self.texts.clear();
                    out.steps.push(self.mapper.end_turn(TurnEnd::interrupted()));
                    return;
                }
                self.start_turn(out);
            }
            Pending::TurnStart => {
                if let Some(message) = error_message {
                    let end = TurnEnd::failed(code::AGENT_ERROR, message);
                    if self.mapper.turn_running() {
                        out.steps.push(self.mapper.end_turn(end));
                    }
                    return;
                }
                let turn = result.get("turn").and_then(|t| s(t, "id")).map(str::to_owned);
                self.mapper.set_turn_id(turn);
                if self.interrupt_wanted {
                    self.send_interrupt(out);
                }
            }
            Pending::Interrupt => {}
        }
    }

    /// The `decision` for an answer.
    pub fn decision(answer: Answer, decline_ok: bool) -> &'static str {
        match answer {
            Answer::Allow => "accept",
            Answer::Deny(_) if decline_ok => "decline",
            Answer::Deny(_) => "cancel",
        }
    }
}

fn id_key(id: &Value) -> String {
    id.to_string()
}

impl Driver for CodexDriver {
    fn start(&mut self, out: &mut Output) {
        self.next_id = 2;
        self.pending.insert(1, Pending::Initialize);
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
        match cmd {
            SessionCmd::Send(text) => {
                if self.failed {
                    return;
                }
                self.texts.push_back(text);
                if !self.mapper.turn_running() || self.mapper.turn_id().is_none() {
                    if !self.mapper.turn_running() {
                        self.mapper.turn_started();
                    }
                    self.start_turn(out);
                }
            }
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
        let decision = Self::decision(answer, decline_ok);
        out.stdin.push(json!({"jsonrpc": "2.0", "id": id, "result": {"decision": decision}}).to_string());
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
        assert_eq!(spec.diff.as_deref(), Some("notes.txt (add)\na\n"));
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
        assert_eq!(CodexDriver::decision(Answer::Allow, false), "accept");
        assert_eq!(
            CodexDriver::decision(Answer::Deny(crate::host::agents::DenyReason::User), true),
            "decline"
        );
        assert_eq!(
            CodexDriver::decision(Answer::Deny(crate::host::agents::DenyReason::Expired), false),
            "cancel"
        );
    }

    #[test]
    fn unknown_server_requests_are_refused() {
        let mut mapper = CodexMapper::new(PathBuf::from("/w"));
        let req = json!({"method":"item/tool/requestUserInput","id":7,"params":{}});
        assert_eq!(mapper.server_request(&req), ServerRequest::Unsupported);
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
            ("TurnStartParams", &["threadId", "input"][..]),
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
}
