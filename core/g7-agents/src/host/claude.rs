// SPDX-License-Identifier: MPL-2.0
//! The Claude adapter (§4): `claude -p` with stream-json on stdin and
//! stdout, one process per thread, approvals through our MCP bridge.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::api::{self, ApprovalKind, Mode, Phase, ToolClass, ToolStatus};
use crate::host::agents::{
    AgentProcess, Driver, Output, SessionCmd, Step, ToolSpec, TurnEnd, code, display_path, plan_of,
    prefixed_lines,
};

/// The MCP server name and our tool, as Claude sees them.
pub const MCP_SERVER: &str = "g7";
pub const PERMISSION_TOOL: &str = "mcp__g7__approve";

/// A new v4 UUID for `--session-id`.
pub fn new_session_id() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

/// A session id as `--resume` accepts it: a UUID.
pub fn is_session_id(id: &str) -> bool {
    id.len() == 36
        && id
            .char_indices()
            .all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() })
}

/// The stdin line for a user message.
pub fn user_line(text: &str) -> String {
    json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]}})
        .to_string()
}

/// The stdin line for an interrupt.
pub fn interrupt_line(request_id: &str) -> String {
    json!({"type": "control_request", "request_id": request_id, "request": {"subtype": "interrupt"}})
        .to_string()
}

/// The arguments for one process: a new session or a resumed one.
pub fn args(mode: Mode, mcp_config: &Path, session_id: &str, resume: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-mode",
        match mode {
            Mode::Supervised => "default",
            Mode::AutoEdit => "acceptEdits",
        },
        "--strict-mcp-config",
        "--mcp-config",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    args.push(mcp_config.to_string_lossy().into_owned());
    args.extend(["--permission-prompt-tool".to_owned(), PERMISSION_TOOL.to_owned()]);
    args.extend(["--disallowedTools".to_owned(), "AskUserQuestion".to_owned()]);
    args.push(if resume { "--resume" } else { "--session-id" }.to_owned());
    args.push(session_id.to_owned());
    args
}

/// Writes the MCP config that starts our approval bridge, mode 0600.
pub fn write_mcp_config(
    path: &Path,
    bridge_exe: &Path,
    socket: &Path,
    bridge_token: &str,
) -> std::io::Result<()> {
    let config = json!({"mcpServers": {MCP_SERVER: {
        "type": "stdio",
        "command": bridge_exe.to_string_lossy(),
        "args": ["mcp-approve"],
        "env": {"G7_AGENTS_SOCKET": socket.to_string_lossy(), "G7_AGENTS_BRIDGE_TOKEN": bridge_token},
    }}});
    let _ = std::fs::remove_file(path);
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    file.write_all(config.to_string().as_bytes())?;
    file.sync_all()
}

/// What kind of step a Claude tool is.
pub fn tool_class(name: &str) -> ToolClass {
    match name {
        "Bash" => ToolClass::Command,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => ToolClass::Edit,
        "Read" => ToolClass::Read,
        "Grep" | "Glob" | "LS" => ToolClass::Search,
        "WebSearch" | "WebFetch" => ToolClass::Web,
        "Task" | "Agent" => ToolClass::Agent,
        "TodoWrite" => ToolClass::Tasks,
        _ if name.starts_with("mcp__") => ToolClass::Mcp,
        _ => ToolClass::Other,
    }
}

/// How a Claude tool call is shown and asked about.
pub fn tool_spec(name: &str, input: &Value, cwd: &Path) -> ToolSpec {
    let s = |key: &str| input.get(key).and_then(Value::as_str);
    let path_of = |key: &str| s(key).map(|p| display_path(p, cwd));
    match name {
        "Bash" => {
            let command = s("command").unwrap_or_default().to_owned();
            ToolSpec {
                name: name.into(),
                title: "Run command".into(),
                detail: Some(command.clone()),
                kind: ApprovalKind::Command,
                command: Some(command),
                diff: None,
                path: None,
                description: s("description").map(str::to_owned),
                class: ToolClass::Command,
                group: None,
            }
        }
        "Write" => {
            let path = path_of("file_path").unwrap_or_default();
            let diff = prefixed_lines(s("content").unwrap_or_default(), '+');
            edit_spec(name, path, diff)
        }
        "Edit" | "MultiEdit" => {
            let path = path_of("file_path").unwrap_or_default();
            let mut pairs: Vec<(&str, &str)> = Vec::new();
            if let Some(edits) = input.get("edits").and_then(Value::as_array) {
                for e in edits {
                    pairs.push((
                        e.get("old_string").and_then(Value::as_str).unwrap_or_default(),
                        e.get("new_string").and_then(Value::as_str).unwrap_or_default(),
                    ));
                }
            } else {
                pairs.push((s("old_string").unwrap_or_default(), s("new_string").unwrap_or_default()));
            }
            let diff = pairs
                .iter()
                .map(|(old, new)| {
                    let mut part = prefixed_lines(old, '-');
                    let added = prefixed_lines(new, '+');
                    if !part.is_empty() && !added.is_empty() {
                        part.push('\n');
                    }
                    part.push_str(&added);
                    part
                })
                .collect::<Vec<_>>()
                .join("\n");
            edit_spec(name, path, diff)
        }
        "NotebookEdit" => {
            let path = path_of("notebook_path").unwrap_or_default();
            edit_spec(name, path, prefixed_lines(s("new_source").unwrap_or_default(), '+'))
        }
        _ => ToolSpec {
            detail: Some(input.to_string()),
            path: if name == "Read" { path_of("file_path") } else { None },
            ..ToolSpec::other(name, name, tool_class(name))
        },
    }
}

/// The plan in a `TodoWrite` call's input.
pub fn todo_plan(name: &str, input: &Value) -> Option<Vec<api::PlanStep>> {
    if name != "TodoWrite" {
        return None;
    }
    let todos = input.get("todos")?.as_array()?;
    Some(plan_of(todos.iter().map(|t| {
        (
            t.get("content").and_then(Value::as_str).unwrap_or_default(),
            t.get("status").and_then(Value::as_str).unwrap_or_default(),
        )
    })))
}

fn edit_spec(name: &str, path: String, diff: String) -> ToolSpec {
    let verb = if name == "Write" { "Write" } else { "Edit" };
    ToolSpec {
        name: name.into(),
        title: format!("{verb} {path}"),
        detail: Some(diff.clone()),
        kind: ApprovalKind::Edit,
        command: None,
        diff: Some(diff),
        path: Some(path),
        description: None,
        class: ToolClass::Edit,
        group: None,
    }
}

/// Signs that the CLI refused the login (compliance §4).
const LOGIN_MARKERS: &[&str] = &[
    "authentication_failed",
    "oauth_org_not_allowed",
    "account_on_hold",
    "billing_error",
    "please run /login",
    "not logged in",
    "invalid api key",
    "only authorized for use with claude code",
    "oauth token has expired",
    "oauth token revoked",
];

/// `resets_ms=` text for a reset time in seconds or milliseconds.
pub fn usage_limit_message(resets: Option<u64>) -> String {
    match resets {
        Some(t) => {
            let ms = if t < 100_000_000_000 { t.saturating_mul(1000) } else { t };
            format!("Usage limit reached|resets_ms={ms}")
        }
        None => "Usage limit reached".to_owned(),
    }
}

/// Turns Claude's stdout lines into steps (§4.4).
#[derive(Debug)]
pub struct ClaudeMapper {
    cwd: PathBuf,
    session_id: Option<String>,
    current_message: Option<String>,
    /// Text blocks opened by the stream and not yet finished, in order.
    open_text: Vec<String>,
    synthesized: u64,
    turn_running: bool,
    interrupt_sent: bool,
    /// Set when the CLI said the usage limit stops requests; the reset time
    /// in seconds when it gave one.
    rate_limited: Option<Option<u64>>,
    assistant_error: Option<String>,
    last_text: Option<String>,
    tool_names: HashMap<String, String>,
    /// Thinking blocks opened by the stream and not yet finished: the key
    /// and the text so far. Their signatures are never kept.
    open_thinking: Vec<(String, String)>,
    /// Text items of the current message, in order.
    message_texts: Vec<String>,
    /// Text items of the turn, in order, and whether each has its phase.
    turn_texts: Vec<(String, bool)>,
}

impl ClaudeMapper {
    pub fn new(cwd: PathBuf, session_id: Option<String>) -> Self {
        Self {
            cwd,
            session_id,
            current_message: None,
            open_text: Vec::new(),
            synthesized: 0,
            turn_running: false,
            interrupt_sent: false,
            rate_limited: None,
            assistant_error: None,
            last_text: None,
            tool_names: HashMap::new(),
            open_thinking: Vec::new(),
            message_texts: Vec::new(),
            turn_texts: Vec::new(),
        }
    }

    pub fn turn_started(&mut self) {
        self.turn_running = true;
        self.interrupt_sent = false;
        self.rate_limited = None;
        self.assistant_error = None;
        self.last_text = None;
    }

    pub fn turn_running(&self) -> bool {
        self.turn_running
    }

    pub fn interrupt_sent(&mut self) {
        self.interrupt_sent = true;
    }

    fn end_turn(&mut self, end: TurnEnd) -> Step {
        self.turn_running = false;
        self.interrupt_sent = false;
        self.open_text.clear();
        self.tool_names.clear();
        self.open_thinking.clear();
        self.message_texts.clear();
        self.turn_texts.clear();
        Step::TurnEnded(end)
    }

    /// Gives a text item its phase, unless it has one.
    fn set_phase(&mut self, key: &str, phase: Phase, steps: &mut Vec<Step>) {
        if let Some((_, phased)) = self.turn_texts.iter_mut().find(|(k, phased)| k == key && !*phased) {
            *phased = true;
            steps.push(Step::AssistantPhase { key: key.to_owned(), phase });
        }
    }

    /// How a message ended places its texts: before a tool call they were
    /// said on the way; at the end of the turn its last one is the answer.
    fn message_ended(&mut self, stop_reason: Option<&str>, steps: &mut Vec<Step>) {
        let keys = self.message_texts.clone();
        let last = keys.len().saturating_sub(1);
        for (i, key) in keys.iter().enumerate() {
            match stop_reason {
                Some("tool_use") => self.set_phase(key, Phase::Interim, steps),
                Some("end_turn") if i == last => self.set_phase(key, Phase::Final, steps),
                Some("end_turn") => self.set_phase(key, Phase::Interim, steps),
                _ => {}
            }
        }
    }

    /// The turn succeeded: a text still without a phase is the answer when
    /// it is the turn's last, else said on the way.
    fn settle_phases(&mut self, steps: &mut Vec<Step>) {
        let keys: Vec<String> = self.turn_texts.iter().map(|(k, _)| k.clone()).collect();
        let last = keys.len().saturating_sub(1);
        for (i, key) in keys.iter().enumerate() {
            self.set_phase(key, if i == last { Phase::Final } else { Phase::Interim }, steps);
        }
    }

    /// A text item is done.
    fn text_done(&mut self, key: &str) {
        self.message_texts.push(key.to_owned());
        if !self.turn_texts.iter().any(|(k, _)| k == key) {
            self.turn_texts.push((key.to_owned(), false));
        }
    }

    pub fn feed(&mut self, line: &Value) -> Vec<Step> {
        let mut steps = Vec::new();
        match line.get("parent_tool_use_id") {
            None | Some(Value::Null) => {}
            Some(Value::String(parent)) if !parent.is_empty() => {
                self.subagent(parent, line, &mut steps);
                return steps;
            }
            Some(_) => return steps,
        }
        let kind = line.get("type").and_then(Value::as_str).unwrap_or_default();
        let subtype = line.get("subtype").and_then(Value::as_str).unwrap_or_default();
        match (kind, subtype) {
            ("system", "init") => {
                let ours = line
                    .get("session_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| self.session_id.as_deref().is_none_or(|s| s == id));
                if !ours {
                    tracing::warn!("Claude reported a different session id");
                }
                let bridge_up = line.get("mcp_servers").and_then(Value::as_array).is_some_and(|servers| {
                    servers.iter().any(|s| {
                        s.get("name").and_then(Value::as_str) == Some(MCP_SERVER)
                            && s.get("status").and_then(Value::as_str) == Some("connected")
                    })
                });
                if !bridge_up && self.turn_running {
                    steps.push(self.end_turn(TurnEnd::failed(
                        code::BRIDGE_FAILED,
                        "The approval service didn't start.",
                    )));
                }
            }
            ("system", "api_retry") => {
                if let Some(error) = line.get("error").and_then(Value::as_str) {
                    self.assistant_error = Some(error.to_owned());
                }
            }
            ("rate_limit_event", _) => {
                let info = line.get("rate_limit_info");
                let status = info.and_then(|i| i.get("status")).and_then(Value::as_str);
                if status == Some("rejected") {
                    self.rate_limited = Some(info.and_then(|i| i.get("resetsAt")).and_then(Value::as_u64));
                }
            }
            ("stream_event", _) => self.stream_event(line.get("event").unwrap_or(&Value::Null), &mut steps),
            ("assistant", _) => self.assistant(line, &mut steps),
            ("user", _) => self.user(line, &mut steps),
            ("result", _) if self.turn_running => {
                let end = self.result(line);
                if end.outcome == crate::api::Outcome::Done {
                    self.settle_phases(&mut steps);
                }
                steps.push(self.end_turn(end));
            }
            _ => {}
        }
        steps
    }

    /// The key of the block an event is about: `<message id>:<index>`.
    fn block_key(&self, event: &Value) -> Option<String> {
        let msg = self.current_message.as_ref()?;
        let index = event.get("index").and_then(Value::as_u64)?;
        Some(format!("{msg}:{index}"))
    }

    fn stream_event(&mut self, event: &Value, steps: &mut Vec<Step>) {
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                self.current_message =
                    event.get("message").and_then(|m| m.get("id")).and_then(Value::as_str).map(str::to_owned);
                self.message_texts.clear();
            }
            Some("content_block_start") => {
                let block = event.get("content_block").and_then(|b| b.get("type")).and_then(Value::as_str);
                let Some(key) = self.block_key(event) else { return };
                match block {
                    Some("text") => {
                        self.open_text.push(key.clone());
                        steps.push(Step::AssistantOpen { key });
                    }
                    Some("thinking" | "redacted_thinking")
                        if !self.open_thinking.iter().any(|(k, _)| *k == key) =>
                    {
                        self.open_thinking.push((key.clone(), String::new()));
                        steps.push(Step::ReasoningOpen { key });
                    }
                    _ => {}
                }
            }
            Some("content_block_delta") => {
                let delta = event.get("delta");
                let field = |name: &str| delta.and_then(|d| d.get(name)).and_then(Value::as_str);
                let Some(key) = self.block_key(event) else { return };
                match field("type") {
                    Some("text_delta") => {
                        let text = field("text").unwrap_or_default();
                        if self.open_text.contains(&key) && !text.is_empty() {
                            steps.push(Step::AssistantDelta { key, text: text.to_owned() });
                        }
                    }
                    Some("thinking_delta") => {
                        let text = field("thinking").unwrap_or_default();
                        if let Some((_, so_far)) = self.open_thinking.iter_mut().find(|(k, _)| *k == key)
                            && !text.is_empty()
                        {
                            so_far.push_str(text);
                            steps.push(Step::ReasoningDelta { key, text: text.to_owned() });
                        }
                    }
                    // `signature_delta` and anything else: not ours to keep.
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                let Some(key) = self.block_key(event) else { return };
                if let Some(pos) = self.open_thinking.iter().position(|(k, _)| *k == key) {
                    let (key, text) = self.open_thinking.remove(pos);
                    steps.push(Step::ReasoningDone { key, text });
                }
            }
            Some("message_delta") => {
                let reason = event.get("delta").and_then(|d| d.get("stop_reason")).and_then(Value::as_str);
                self.message_ended(reason, steps);
            }
            _ => {}
        }
    }

    fn assistant(&mut self, line: &Value, steps: &mut Vec<Step>) {
        if let Some(error) = line.get("error").and_then(Value::as_str) {
            self.assistant_error = Some(error.to_owned());
        }
        let Some(message) = line.get("message") else { return };
        let id = message.get("id").and_then(Value::as_str).unwrap_or("message").to_owned();
        let Some(blocks) = message.get("content").and_then(Value::as_array) else { return };
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = block.get("text").and_then(Value::as_str).unwrap_or_default().to_owned();
                    let prefix = format!("{id}:");
                    let key = match self.open_text.iter().position(|k| k.starts_with(&prefix)) {
                        Some(pos) => self.open_text.remove(pos),
                        None => {
                            self.synthesized += 1;
                            let key = format!("{id}:t{}", self.synthesized);
                            steps.push(Step::AssistantOpen { key: key.clone() });
                            key
                        }
                    };
                    if !text.trim().is_empty() {
                        self.last_text = Some(text.clone());
                    }
                    self.text_done(&key);
                    steps.push(Step::AssistantDone { key, text, final_answer: false });
                }
                Some(kind @ ("thinking" | "redacted_thinking")) => {
                    let given = if kind == "thinking" {
                        block.get("thinking").and_then(Value::as_str).unwrap_or_default()
                    } else {
                        ""
                    };
                    let prefix = format!("{id}:");
                    match self.open_thinking.iter().position(|(k, _)| k.starts_with(&prefix)) {
                        Some(pos) => {
                            let (key, so_far) = self.open_thinking.remove(pos);
                            let text = if so_far.is_empty() { given.to_owned() } else { so_far };
                            steps.push(Step::ReasoningDone { key, text });
                        }
                        // Streamed and already done: nothing more.
                        None if self.current_message.as_deref() == Some(id.as_str()) => {}
                        None => {
                            self.synthesized += 1;
                            let key = format!("{id}:r{}", self.synthesized);
                            steps.push(Step::ReasoningOpen { key: key.clone() });
                            steps.push(Step::ReasoningDone { key, text: given.to_owned() });
                        }
                    }
                }
                Some("tool_use") => {
                    let Some(key) = block.get("id").and_then(Value::as_str) else { continue };
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                    let input = block.get("input").unwrap_or(&Value::Null);
                    self.tool_names.insert(key.to_owned(), name.to_owned());
                    if let Some(plan) = todo_plan(name, input) {
                        steps.push(Step::Plan(plan));
                    }
                    steps.push(Step::ToolStart {
                        key: key.to_owned(),
                        tool: tool_spec(name, input, &self.cwd),
                    });
                }
                _ => {}
            }
        }
    }

    /// A line of a sub-agent's work (`parent_tool_use_id` names the tool
    /// it runs inside): its tool calls and their results only, as steps of
    /// that tool's group. Its text, thinking and plan stay its own.
    fn subagent(&mut self, parent: &str, line: &Value, steps: &mut Vec<Step>) {
        match line.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let blocks = line.pointer("/message/content").and_then(Value::as_array);
                for block in blocks.into_iter().flatten() {
                    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    let Some(key) = block.get("id").and_then(Value::as_str) else { continue };
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                    let input = block.get("input").unwrap_or(&Value::Null);
                    self.tool_names.insert(key.to_owned(), name.to_owned());
                    let tool =
                        ToolSpec { group: Some(parent.to_owned()), ..tool_spec(name, input, &self.cwd) };
                    steps.push(Step::ToolStart { key: key.to_owned(), tool });
                }
            }
            Some("user") => self.user(line, steps),
            _ => {}
        }
    }

    fn user(&mut self, line: &Value, steps: &mut Vec<Step>) {
        let Some(blocks) = line.get("message").and_then(|m| m.get("content")).and_then(Value::as_array)
        else {
            return;
        };
        let metas = line.get("tool_result_meta").and_then(Value::as_array);
        for (i, block) in blocks.iter().enumerate() {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(key) = block.get("tool_use_id").and_then(Value::as_str) else { continue };
            let meta = metas.and_then(|metas| {
                metas
                    .iter()
                    .find(|m| m.get("id").and_then(Value::as_str) == Some(key))
                    .or_else(|| metas.get(i))
            });
            let rejected = meta.is_some_and(|m| {
                m.get("permission_decision").and_then(|d| d.get("decision")).and_then(Value::as_str)
                    == Some("reject")
                    || m.get("non_execution_kind").is_some()
            });
            let status = if rejected {
                ToolStatus::Denied
            } else if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                ToolStatus::Error
            } else {
                ToolStatus::Ok
            };
            let output = match block.get("content") {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Array(parts)) => Some(
                    parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                _ => None,
            }
            .filter(|s| !s.is_empty());
            steps.push(Step::ToolEnd { key: key.to_owned(), status, output });
        }
    }

    fn result(&mut self, line: &Value) -> TurnEnd {
        let is_error = line.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        let result_text = line.get("result").and_then(Value::as_str);
        if !is_error {
            let summary = result_text.map(str::to_owned).or_else(|| self.last_text.clone());
            return TurnEnd::done(summary);
        }
        if self.interrupt_sent {
            return TurnEnd::interrupted();
        }
        let subtype = line.get("subtype").and_then(Value::as_str).unwrap_or("error");
        let first_error =
            line.get("errors").and_then(Value::as_array).and_then(|e| e.first()).and_then(Value::as_str);
        let message = first_error.or(result_text).unwrap_or(subtype).to_owned();
        let haystack = format!(
            "{} {} {}",
            message,
            result_text.unwrap_or_default(),
            self.assistant_error.as_deref().unwrap_or_default()
        )
        .to_lowercase();
        if LOGIN_MARKERS.iter().any(|m| haystack.contains(m)) {
            return TurnEnd::failed(code::LOGIN, message);
        }
        let limited = self.rate_limited.is_some()
            || self.assistant_error.as_deref() == Some("rate_limit")
            || haystack.contains("usage limit")
            || haystack.contains("limit reached");
        if limited {
            let resets = self.rate_limited.flatten().or_else(|| {
                // "Claude AI usage limit reached|1700000000"
                message.rsplit_once('|').and_then(|(_, t)| t.trim().parse::<u64>().ok())
            });
            return TurnEnd::failed(code::USAGE_LIMIT, usage_limit_message(resets));
        }
        TurnEnd::failed(api::cut(subtype, api::MAX_ERROR_CODE), message)
    }

    /// The process exited: ends a turn still running.
    pub fn on_exit(&mut self, code: Option<i32>) -> Vec<Step> {
        if !self.turn_running {
            return Vec::new();
        }
        let end = if self.interrupt_sent {
            TurnEnd::interrupted()
        } else {
            let text = code.map_or_else(|| "exit signal".to_owned(), |c| format!("exit {c}"));
            TurnEnd::failed(code::AGENT_EXITED, text)
        };
        vec![self.end_turn(end)]
    }
}

/// Everything one Claude process needs.
#[derive(Debug, Clone)]
pub struct Launch {
    pub bin: PathBuf,
    pub cwd: PathBuf,
    pub mode: Mode,
    pub session_id: String,
    pub resume: bool,
    pub mcp_config: PathBuf,
    pub bridge_exe: PathBuf,
    pub socket: PathBuf,
    pub bridge_token: String,
    pub env: Vec<(OsString, OsString)>,
    pub first_text: String,
}

/// Starts a Claude process for a thread.
pub fn spawn(
    launch: &Launch,
) -> std::io::Result<(AgentProcess, tokio::sync::mpsc::Receiver<crate::host::agents::OutLine>)> {
    write_mcp_config(&launch.mcp_config, &launch.bridge_exe, &launch.socket, &launch.bridge_token)?;
    let args = args(launch.mode, &launch.mcp_config, &launch.session_id, launch.resume);
    let spawned = AgentProcess::spawn(&launch.bin, &args, &launch.cwd, &launch.env);
    if spawned.is_err() {
        let _ = std::fs::remove_file(&launch.mcp_config);
    }
    spawned
}

/// The Claude side of [`crate::host::agents::run_session`].
#[derive(Debug)]
pub struct ClaudeDriver {
    mapper: ClaudeMapper,
    first_text: Option<String>,
    interrupts: u64,
}

impl ClaudeDriver {
    pub fn new(cwd: PathBuf, session_id: String, first_text: String) -> Self {
        Self { mapper: ClaudeMapper::new(cwd, Some(session_id)), first_text: Some(first_text), interrupts: 0 }
    }
}

impl Driver for ClaudeDriver {
    fn start(&mut self, out: &mut Output) {
        if let Some(text) = self.first_text.take() {
            out.stdin.push(user_line(&text));
            self.mapper.turn_started();
        }
    }

    fn on_line(&mut self, line: Value, out: &mut Output) {
        for step in self.mapper.feed(&line) {
            if matches!(&step, Step::TurnEnded(end) if end.error.as_ref().is_some_and(|e| e.code == code::BRIDGE_FAILED))
            {
                out.close = true;
            }
            out.steps.push(step);
        }
    }

    fn on_cmd(&mut self, cmd: SessionCmd, out: &mut Output) {
        match cmd {
            SessionCmd::Send(text) => {
                out.stdin.push(user_line(&text));
                self.mapper.turn_started();
            }
            SessionCmd::Interrupt => {
                if self.mapper.turn_running() {
                    self.interrupts += 1;
                    out.stdin.push(interrupt_line(&format!("g7-int-{}", self.interrupts)));
                    self.mapper.interrupt_sent();
                }
            }
            SessionCmd::Close | SessionCmd::Kill => {}
        }
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
        self.mapper.on_exit(code)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::api::Outcome;

    /// The stdout lines of a fixture, without our markers.
    pub(crate) fn fixture_lines(name: &str) -> Vec<Value> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .collect()
    }

    /// Feeds a fixture as the session would: a turn starts at each user
    /// line we wrote (or at the start when there's none), an interrupt
    /// marker marks the interrupt sent.
    fn run(name: &str) -> Vec<Step> {
        let mut mapper = ClaudeMapper::new(PathBuf::from("/work"), None);
        let lines = fixture_lines(name);
        if !lines.iter().any(|l| l.get("_stdin").is_some()) {
            mapper.turn_started();
        }
        let mut steps = Vec::new();
        for line in lines {
            if let Some(stdin) = line.get("_stdin") {
                match stdin.get("type").and_then(Value::as_str) {
                    Some("user") => mapper.turn_started(),
                    Some("control_request") if mapper.turn_running() => mapper.interrupt_sent(),
                    _ => {}
                }
                continue;
            }
            if let Some(code) = line.get("_exit") {
                steps.extend(mapper.on_exit(code.as_i64().map(|c| c as i32)));
                continue;
            }
            steps.extend(mapper.feed(&line));
        }
        steps
    }

    fn ends(steps: &[Step]) -> Vec<&TurnEnd> {
        steps.iter().filter_map(|s| if let Step::TurnEnded(e) = s { Some(e) } else { None }).collect()
    }

    fn tool_ends(steps: &[Step]) -> Vec<(&str, ToolStatus)> {
        steps
            .iter()
            .filter_map(|s| {
                if let Step::ToolEnd { key, status, .. } = s { Some((key.as_str(), *status)) } else { None }
            })
            .collect()
    }

    fn assistant_texts(steps: &[Step]) -> Vec<&str> {
        steps
            .iter()
            .filter_map(|s| if let Step::AssistantDone { text, .. } = s { Some(text.as_str()) } else { None })
            .collect()
    }

    #[test]
    fn a_read_only_turn() {
        let steps = run("claude-01-list.jsonl");
        let starts: Vec<_> = steps
            .iter()
            .filter_map(|s| if let Step::ToolStart { tool, .. } = s { Some(tool) } else { None })
            .collect();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].title, "Run command");
        assert_eq!(starts[0].detail.as_deref(), Some("ls -A"));
        assert_eq!(tool_ends(&steps), [("toolu_013PFtgkpsUM8ox8Za7sEm99", ToolStatus::Ok)]);
        assert_eq!(assistant_texts(&steps), ["There's one file here, `README.md`, plus the `.git` folder."]);
        // The streamed text arrives as deltas on the item the stream opened.
        let opened: Vec<_> = steps
            .iter()
            .filter_map(|s| if let Step::AssistantOpen { key } = s { Some(key.as_str()) } else { None })
            .collect();
        assert_eq!(opened, ["msg_011ChB76kJBmygwFBdunKr7M:0"]);
        let streamed: String = steps
            .iter()
            .filter_map(
                |s| if let Step::AssistantDelta { text, .. } = s { Some(text.as_str()) } else { None },
            )
            .collect();
        assert_eq!(streamed, assistant_texts(&steps)[0]);
        let ends = ends(&steps);
        assert_eq!(ends.len(), 1);
        assert_eq!(ends[0].outcome, Outcome::Done);
        assert_eq!(ends[0].summary.as_deref(), Some(assistant_texts(&steps)[0]));
    }

    #[test]
    fn two_turns_with_an_approved_write() {
        let steps = run("claude-02-followup-approve.jsonl");
        let ends = ends(&steps);
        assert_eq!(ends.len(), 2);
        assert!(ends.iter().all(|e| e.outcome == Outcome::Done));
        let write = steps
            .iter()
            .find_map(|s| match s {
                Step::ToolStart { tool, .. } if tool.name == "Write" => Some(tool),
                _ => None,
            })
            .unwrap();
        assert_eq!(write.title, "Write hello.txt");
        assert_eq!(write.diff.as_deref(), Some("+hi"));
        assert_eq!(write.kind, ApprovalKind::Edit);
        assert!(tool_ends(&steps).contains(&("toolu_019665aUVn5LSZP1U45iVqz1", ToolStatus::Ok)));
    }

    #[test]
    fn a_denied_command() {
        let steps = run("claude-03-resume-deny.jsonl");
        assert_eq!(tool_ends(&steps), [("toolu_01LyvrGoibjCPmTVTgii8XHP", ToolStatus::Denied)]);
        let ends = ends(&steps);
        assert_eq!(ends.len(), 1);
        assert_eq!(ends[0].outcome, Outcome::Done);
    }

    #[test]
    fn an_interrupt_while_idle_ends_nothing() {
        let steps = run("claude-04-resume-interrupt.jsonl");
        let ends = ends(&steps);
        assert_eq!(ends.len(), 2);
        assert!(ends.iter().all(|e| e.outcome == Outcome::Done));
    }

    #[test]
    fn an_interrupt_while_waiting() {
        let steps = run("claude-05-interrupt-while-waiting.jsonl");
        assert_eq!(tool_ends(&steps), [("toolu_01YKsx3mzGkW8veWFK5sKNPF", ToolStatus::Denied)]);
        let ends = ends(&steps);
        assert_eq!(ends.len(), 1, "exit 1 after an interrupted turn is not a failure");
        assert_eq!(ends[0].outcome, Outcome::Interrupted);
    }

    fn reasoning(steps: &[Step]) -> (Vec<&str>, Vec<&str>, Vec<(&str, &str)>) {
        let mut opened = Vec::new();
        let mut deltas = Vec::new();
        let mut done = Vec::new();
        for step in steps {
            match step {
                Step::ReasoningOpen { key } => opened.push(key.as_str()),
                Step::ReasoningDelta { text, .. } => deltas.push(text.as_str()),
                Step::ReasoningDone { key, text } => done.push((key.as_str(), text.as_str())),
                _ => {}
            }
        }
        (opened, deltas, done)
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
    fn thinking_in_the_captures_has_no_text() {
        for name in [
            "claude-02-followup-approve.jsonl",
            "claude-03-resume-deny.jsonl",
            "claude-04-resume-interrupt.jsonl",
        ] {
            let steps = run(name);
            let (opened, deltas, done) = reasoning(&steps);
            assert_eq!(opened.len(), 1, "{name}");
            assert!(deltas.is_empty(), "{name}: the CLI sent no thinking text");
            assert_eq!(done.len(), 1, "{name}");
            assert_eq!(done[0], (opened[0], ""), "{name}");
            // It opens before what follows it in its message.
            let open = steps.iter().position(|s| matches!(s, Step::ReasoningOpen { .. })).unwrap();
            let end = steps.iter().position(|s| matches!(s, Step::ReasoningDone { .. })).unwrap();
            assert!(open < end);
        }
        for name in ["claude-01-list.jsonl", "claude-05-interrupt-while-waiting.jsonl"] {
            let steps = run(name);
            let (opened, deltas, done) = reasoning(&steps);
            assert!(opened.is_empty() && deltas.is_empty() && done.is_empty(), "{name}");
        }
    }

    #[test]
    fn no_step_carries_a_signature() {
        for name in [
            "claude-02-followup-approve.jsonl",
            "claude-03-resume-deny.jsonl",
            "claude-04-resume-interrupt.jsonl",
        ] {
            let mut signatures = Vec::new();
            for line in fixture_lines(name) {
                let event = line.get("event");
                for sig in [
                    event.and_then(|e| e.pointer("/delta/signature")),
                    event.and_then(|e| e.pointer("/content_block/signature")),
                ]
                .into_iter()
                .flatten()
                .chain(
                    line.pointer("/message/content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|b| b.get("signature")),
                ) {
                    if let Some(sig) = sig.as_str().filter(|s| !s.is_empty()) {
                        signatures.push(sig.to_owned());
                    }
                }
            }
            assert!(!signatures.is_empty(), "{name} has signatures to look for");
            let steps = format!("{:?}", run(name));
            for sig in &signatures {
                assert!(!steps.contains(sig.as_str()), "{name}");
            }
        }
    }

    #[test]
    fn each_turns_last_text_is_its_answer() {
        let steps = run("claude-02-followup-approve.jsonl");
        let texts: Vec<&str> = steps
            .iter()
            .filter_map(|s| if let Step::AssistantDone { key, .. } = s { Some(key.as_str()) } else { None })
            .collect();
        assert_eq!(texts.len(), 2);
        assert_eq!(phases(&steps), [(texts[0], Phase::Final), (texts[1], Phase::Final)]);
        // Each phase comes after its text and before its turn ends.
        let first_phase = steps.iter().position(|s| matches!(s, Step::AssistantPhase { .. })).unwrap();
        let first_end = steps.iter().position(|s| matches!(s, Step::TurnEnded(_))).unwrap();
        let first_done = steps.iter().position(|s| matches!(s, Step::AssistantDone { .. })).unwrap();
        assert!(first_done < first_phase && first_phase < first_end);
        // An interrupted turn leaves its texts without a phase.
        assert!(phases(&run("claude-05-interrupt-while-waiting.jsonl")).is_empty());
    }

    fn stream(event: Value) -> Value {
        json!({"type": "stream_event", "event": event, "parent_tool_use_id": null, "session_id": "s"})
    }

    #[test]
    fn thinking_with_text_and_texts_said_on_the_way() {
        let mut mapper = ClaudeMapper::new(PathBuf::from("/w"), None);
        mapper.turn_started();
        let mut steps = Vec::new();
        let lines = [
            stream(json!({"type": "message_start", "message": {"id": "msg_a"}})),
            stream(
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
            ),
            stream(
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "**Look first**\n"}}),
            ),
            stream(
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Then edit."}}),
            ),
            stream(
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "c2lnbmF0dXJl"}}),
            ),
            json!({"type": "assistant", "message": {"id": "msg_a", "content": [
                {"type": "thinking", "thinking": "**Look first**\nThen edit.", "signature": "c2lnbmF0dXJl"}]}}),
            stream(json!({"type": "content_block_stop", "index": 0})),
            stream(
                json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
            ),
            stream(
                json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "I'll look."}}),
            ),
            json!({"type": "assistant", "message": {"id": "msg_a", "content": [{"type": "text", "text": "I'll look."}]}}),
            stream(json!({"type": "content_block_stop", "index": 1})),
            stream(
                json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "toolu_r", "name": "Read", "input": {}}}),
            ),
            json!({"type": "assistant", "message": {"id": "msg_a", "content": [
                {"type": "tool_use", "id": "toolu_r", "name": "Read", "input": {"file_path": "/w/src/a.rs"}}]}}),
            stream(json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}})),
            stream(json!({"type": "message_start", "message": {"id": "msg_b"}})),
            stream(
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "redacted_thinking", "data": "x"}}),
            ),
            json!({"type": "assistant", "message": {"id": "msg_b", "content": [{"type": "redacted_thinking", "data": "x"}]}}),
            stream(json!({"type": "content_block_stop", "index": 0})),
            json!({"type": "assistant", "message": {"id": "msg_b", "content": [{"type": "text", "text": "Done."}]}}),
            json!({"type": "system", "subtype": "thinking_tokens", "estimated_tokens": 5}),
            json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done."}),
        ];
        for line in &lines {
            steps.extend(mapper.feed(line));
        }
        let (opened, deltas, done) = reasoning(&steps);
        assert_eq!(opened, ["msg_a:0", "msg_b:0"]);
        assert_eq!(deltas, ["**Look first**\n", "Then edit."]);
        assert_eq!(done, [("msg_a:0", "**Look first**\nThen edit."), ("msg_b:0", "")]);
        assert!(!format!("{steps:?}").contains("c2lnbmF0dXJl"));
        // The text before the tool was said on the way; the result made the
        // last one, which had no stop reason, the answer.
        assert_eq!(phases(&steps), [("msg_a:1", Phase::Interim), ("msg_b:t1", Phase::Final)]);
        let read = steps
            .iter()
            .find_map(|s| if let Step::ToolStart { tool, .. } = s { Some(tool) } else { None })
            .unwrap();
        assert_eq!((read.class, read.path.as_deref()), (ToolClass::Read, Some("src/a.rs")));
        assert!(matches!(steps.last(), Some(Step::TurnEnded(_))));
    }

    #[test]
    fn thinking_without_the_stream_still_shows() {
        let mut mapper = ClaudeMapper::new(PathBuf::from("/w"), None);
        mapper.turn_started();
        let steps = mapper.feed(&json!({"type": "assistant", "message": {"id": "msg_c", "content": [
            {"type": "thinking", "thinking": "Short.", "signature": "zz"}, {"type": "text", "text": "Hi."}]}}));
        let (opened, _, done) = reasoning(&steps);
        assert_eq!(opened, ["msg_c:r1"]);
        assert_eq!(done, [("msg_c:r1", "Short.")]);
        // A failed turn leaves its text without a phase.
        let steps = mapper.feed(&json!({"type": "result", "subtype": "error_max_turns", "is_error": true}));
        assert!(phases(&steps).is_empty());
    }

    #[test]
    fn tool_classes() {
        for (name, class) in [
            ("Bash", ToolClass::Command),
            ("Edit", ToolClass::Edit),
            ("MultiEdit", ToolClass::Edit),
            ("Write", ToolClass::Edit),
            ("NotebookEdit", ToolClass::Edit),
            ("Read", ToolClass::Read),
            ("Grep", ToolClass::Search),
            ("Glob", ToolClass::Search),
            ("LS", ToolClass::Search),
            ("WebSearch", ToolClass::Web),
            ("WebFetch", ToolClass::Web),
            ("Task", ToolClass::Agent),
            ("Agent", ToolClass::Agent),
            ("TodoWrite", ToolClass::Tasks),
            ("mcp__github__search", ToolClass::Mcp),
            ("Skill", ToolClass::Other),
        ] {
            assert_eq!(tool_spec(name, &json!({}), Path::new("/w")).class, class, "{name}");
        }
        let write = tool_spec("Write", &json!({"file_path": "/w/a/b.txt", "content": "x"}), Path::new("/w"));
        assert_eq!(write.path.as_deref(), Some("a/b.txt"));
        let read = tool_spec("Read", &json!({"file_path": "/etc/hosts"}), Path::new("/w"));
        assert_eq!((read.path.as_deref(), read.kind), (Some("/etc/hosts"), ApprovalKind::Other));
    }

    #[test]
    fn a_subagents_text_is_skipped_and_its_tools_are_grouped() {
        let mut mapper = ClaudeMapper::new(PathBuf::from("/w"), None);
        mapper.turn_started();
        let line = json!({"type":"assistant","parent_tool_use_id":"toolu_x","message":{"id":"m","content":[
            {"type":"text","text":"inner"},{"type":"thinking","thinking":"inner"}]}});
        assert!(mapper.feed(&line).is_empty());
        let todo = json!({"type":"assistant","parent_tool_use_id":"toolu_task","message":{"id":"m2","content":[
            {"type":"tool_use","id":"toolu_todo","name":"TodoWrite","input":{"todos":[{"content":"x","status":"pending"}]}},
            {"type":"tool_use","id":"toolu_in","name":"Read","input":{"file_path":"/w/a.rs"}}]}});
        let steps = mapper.feed(&todo);
        assert!(!steps.iter().any(|s| matches!(s, Step::Plan(_))), "a sub-agent's plan isn't the thread's");
        let starts: Vec<(&str, Option<&str>, ToolClass)> = steps
            .iter()
            .filter_map(|s| match s {
                Step::ToolStart { key, tool } => Some((key.as_str(), tool.group.as_deref(), tool.class)),
                _ => None,
            })
            .collect();
        assert_eq!(
            starts,
            [
                ("toolu_todo", Some("toolu_task"), ToolClass::Tasks),
                ("toolu_in", Some("toolu_task"), ToolClass::Read)
            ]
        );
        let result = json!({"type":"user","parent_tool_use_id":"toolu_task","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"toolu_in","content":"fn a() {}"}]}});
        assert_eq!(tool_ends(&mapper.feed(&result)), [("toolu_in", ToolStatus::Ok)]);
        assert!(
            mapper
                .feed(&json!({"type":"result","parent_tool_use_id":"toolu_task","is_error":false}))
                .is_empty()
        );
        assert!(mapper.feed(&json!({"type":"assistant","parent_tool_use_id":7,"message":{}})).is_empty());
        assert!(mapper.turn_running(), "only the top-level result ends the turn");
    }

    #[test]
    fn a_missing_bridge_fails_the_turn() {
        let mut mapper = ClaudeMapper::new(PathBuf::from("/w"), None);
        mapper.turn_started();
        let steps = mapper.feed(&json!({"type":"system","subtype":"init","session_id":"s","mcp_servers":[{"name":"g7","status":"failed"}]}));
        assert!(
            matches!(&steps[..], [Step::TurnEnded(e)] if e.error.as_ref().unwrap().code == code::BRIDGE_FAILED)
        );
    }

    #[test]
    fn failures_usage_limits_and_login() {
        let mut mapper = ClaudeMapper::new(PathBuf::from("/w"), None);
        mapper.turn_started();
        mapper.feed(
            &json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1795050000}}),
        );
        let steps = mapper.feed(
            &json!({"type":"result","subtype":"success","is_error":true,"result":"You've hit your limit"}),
        );
        let Step::TurnEnded(end) = &steps[0] else { panic!() };
        assert_eq!(end.error.as_ref().unwrap().code, code::USAGE_LIMIT);
        assert_eq!(end.error.as_ref().unwrap().message, "Usage limit reached|resets_ms=1795050000000");

        mapper.turn_started();
        let steps = mapper.feed(&json!({"type":"result","subtype":"success","is_error":true,"result":"Invalid API key · Please run /login"}));
        let Step::TurnEnded(end) = &steps[0] else { panic!() };
        assert_eq!(end.error.as_ref().unwrap().code, code::LOGIN);
        assert_eq!(end.error.as_ref().unwrap().message, "Invalid API key · Please run /login");

        mapper.turn_started();
        let steps = mapper.feed(&json!({"type":"result","subtype":"error_max_turns","is_error":true,"errors":["Reached max turns"]}));
        let Step::TurnEnded(end) = &steps[0] else { panic!() };
        assert_eq!(end.error.as_ref().unwrap().code, "error_max_turns");
        assert_eq!(end.error.as_ref().unwrap().message, "Reached max turns");

        mapper.turn_started();
        let steps = mapper.on_exit(Some(3));
        let Step::TurnEnded(end) = &steps[0] else { panic!() };
        assert_eq!(end.error.as_ref().unwrap().code, code::AGENT_EXITED);
        assert_eq!(end.error.as_ref().unwrap().message, "exit 3");
    }

    #[test]
    fn a_todo_list_is_the_plan() {
        let mut mapper = ClaudeMapper::new(PathBuf::from("/w"), None);
        mapper.turn_started();
        let line = json!({"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"toolu_t","name":"TodoWrite",
            "input":{"todos":[{"content":"Read the code","status":"completed","activeForm":"Reading"},
                              {"content":"Fix the test","status":"in_progress","activeForm":"Fixing"}]}}]}});
        let steps = mapper.feed(&line);
        let Step::Plan(plan) = &steps[0] else { panic!("{steps:?}") };
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[1].status, api::PlanStatus::InProgress);
        assert!(matches!(&steps[1], Step::ToolStart { .. }));
        let bash = tool_spec("Bash", &json!({"command":"ls","description":"List files"}), Path::new("/w"));
        assert_eq!(bash.step(), "List files");
    }

    #[test]
    fn tool_specs() {
        let cwd = Path::new("/w");
        let edit =
            tool_spec("Edit", &json!({"file_path":"/w/a.rs","old_string":"x\ny","new_string":"z"}), cwd);
        assert_eq!(edit.title, "Edit a.rs");
        assert_eq!(edit.diff.as_deref(), Some("-x\n-y\n+z"));
        let multi = tool_spec(
            "MultiEdit",
            &json!({"file_path":"/w/a.rs","edits":[{"old_string":"a","new_string":"b"},{"old_string":"c","new_string":"d"}]}),
            cwd,
        );
        assert_eq!(multi.diff.as_deref(), Some("-a\n+b\n-c\n+d"));
        let nb =
            tool_spec("NotebookEdit", &json!({"notebook_path":"/x/n.ipynb","new_source":"print(1)"}), cwd);
        assert_eq!(nb.title, "Edit /x/n.ipynb");
        let other = tool_spec("WebFetch", &json!({"url":"https://example.org"}), cwd);
        assert_eq!(other.kind, ApprovalKind::Other);
        assert_eq!(other.title, "WebFetch");
        assert_eq!(other.detail.as_deref(), Some(r#"{"url":"https://example.org"}"#));
    }

    #[test]
    fn session_ids_and_args() {
        let id = new_session_id();
        assert!(is_session_id(&id));
        assert_eq!(&id[14..15], "4");
        assert!(!is_session_id("../etc"));
        let a = args(Mode::AutoEdit, Path::new("/r/mcp.json"), &id, true);
        assert!(a.windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
        assert!(a.windows(2).any(|w| w[0] == "--resume" && w[1] == id));
        assert!(!a.iter().any(|s| s == "--session-id" || s == "--bare"));
        let a = args(Mode::Supervised, Path::new("/r/mcp.json"), &id, false);
        assert!(a.windows(2).any(|w| w == ["--permission-mode", "default"]));
        assert!(a.windows(2).any(|w| w[0] == "--session-id"));
    }
}
