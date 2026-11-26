// SPDX-License-Identifier: MPL-2.0
//! The Claude adapter (§4): `claude -p` with stream-json on stdin and
//! stdout, one process per thread, approvals through our MCP bridge.
//!
//! Phase 2 (§Q4): the thread's mode, model and effort are flags of the
//! process (a change restarts it with `--resume`); a message sent while a
//! turn runs is written at once and Claude takes it in at its next step;
//! `--replay-user-messages` tells us when it did.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::api::{self, ApprovalKind, Mode, Phase, ToolClass, ToolStatus};
use crate::host::agents::{
    AgentProcess, Driver, LaunchSettings, Output, SessionCmd, Step, ToolSpec, TurnEnd, UserInput, code,
    display_path, plan_of, prefixed_lines,
};
use crate::host::{catalog, plan, questions};

/// The MCP server name and our tool, as Claude sees them.
pub const MCP_SERVER: &str = "g7";
pub const PERMISSION_TOOL: &str = "mcp__g7__approve";

/// The only control requests the host ever sends Claude (§Q0.2): Stop, and
/// the catalog probe's question. Nothing that changes a setting.
pub const CONTROL_SUBTYPES: [&str; 2] = ["interrupt", "initialize"];

/// Messages written and not yet taken in, remembered at most.
const MAX_PENDING: usize = 32;

/// The notice when Claude compacted the conversation.
pub const COMPACTED: &str = "Compacted the conversation.";

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

/// The stdin line for a user message of text alone.
pub fn user_line(text: &str) -> String {
    json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]}})
        .to_string()
}

/// The stdin line for a message (§Q4.5): its images, as the host built
/// them (`image` content blocks), before the text.
pub fn input_line(input: &UserInput) -> String {
    let mut content: Vec<Value> = input.images.clone();
    content.push(json!({"type": "text", "text": input.text}));
    json!({"type": "user", "message": {"role": "user", "content": content}}).to_string()
}

/// The stdin line for an interrupt.
pub fn interrupt_line(request_id: &str) -> String {
    json!({"type": "control_request", "request_id": request_id, "request": {"subtype": "interrupt"}})
        .to_string()
}

/// The stdin line that asks Claude for its models and commands (the
/// catalog probe, §Q4.1).
pub fn initialize_line(request_id: &str) -> String {
    json!({"type": "control_request", "request_id": request_id, "request": {"subtype": "initialize"}})
        .to_string()
}

/// Our reply to a control request the CLI sent us, and whether it was an
/// MCP server asking for input (elicitation), which phase 2 declines with
/// a notice (§Q4.4). Anything else is refused, so no turn waits on it.
/// [unverified: the CLI's request and response shapes for these.]
pub fn cli_request_reply(line: &Value) -> Option<(String, bool)> {
    let id = line.get("request_id").and_then(Value::as_str)?;
    let subtype = line.pointer("/request/subtype").and_then(Value::as_str).unwrap_or_default();
    let elicitation = subtype.contains("elicitation");
    let response = if elicitation {
        json!({"subtype": "success", "request_id": id, "response": {"action": "decline"}})
    } else {
        json!({"subtype": "error", "request_id": id, "error": "Not supported"})
    };
    Some((json!({"type": "control_response", "response": response}).to_string(), elicitation))
}

/// The permission mode flag's value. `ask` is Claude's "Manual", passed as
/// `default`: older CLIs don't know `manual`. `bypassPermissions` needs no
/// `--allow-dangerously-skip-permissions` when the process starts in it,
/// fresh or resumed (spike 2); a mode never changes on a live process.
pub fn permission_mode(mode: Mode) -> &'static str {
    match mode {
        Mode::Ask => "default",
        Mode::AcceptEdits => "acceptEdits",
        Mode::Auto => "auto",
        Mode::Plan => "plan",
        Mode::Full => "bypassPermissions",
    }
}

/// The arguments for one process (§Q4.2): a new session or a resumed one,
/// with the thread's mode, model and effort, and the user's MCP servers
/// unless the folder's switch is off. A model or effort that isn't one we
/// can pass safely is left out (the CLI's default applies).
pub fn args(settings: &LaunchSettings, mcp_config: &Path, session_id: &str, resume: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--replay-user-messages",
        "--permission-mode",
        permission_mode(settings.mode),
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    if let Some(model) = settings.model.as_deref().filter(|m| catalog::valid_model_id(m)) {
        args.extend(["--model".to_owned(), model.to_owned()]);
    }
    if let Some(effort) = settings.effort.as_deref().and_then(catalog::effort_word) {
        args.extend(["--effort".to_owned(), effort.to_owned()]);
    }
    if !settings.mcp {
        args.push("--strict-mcp-config".to_owned());
    }
    args.push("--mcp-config".to_owned());
    args.push(mcp_config.to_string_lossy().into_owned());
    args.extend(["--permission-prompt-tool".to_owned(), PERMISSION_TOOL.to_owned()]);
    args.push(if resume { "--resume" } else { "--session-id" }.to_owned());
    args.push(session_id.to_owned());
    args
}

/// The arguments for a process that continues a copy of a session
/// (§U6.2): `resume` is the session's id, or the absolute path of a cut
/// copy of its transcript. `--fork-session` only ever goes with `--resume`.
pub fn fork_args(settings: &LaunchSettings, mcp_config: &Path, resume: &str) -> Vec<String> {
    let mut args = args(settings, mcp_config, resume, true);
    args.push("--fork-session".to_owned());
    args
}

/// The arguments of the check that Claude can open a copy (reconcile §5):
/// the catalog probe's flags with the copy to open, no MCP server and no
/// message. Nothing is written while no message is sent [spike S-3.6, 3.7].
pub fn check_args(empty_mcp_config: &Path, resume: &str) -> Vec<String> {
    let mut args = catalog::claude_probe_args(empty_mcp_config);
    args.retain(|a| a != "--no-session-persistence");
    args.extend(["--resume".to_owned(), resume.to_owned(), "--fork-session".to_owned()]);
    args
}

/// What the check's answer says: `Some(true)` the copy opened (Claude
/// answered `request_id`), `Some(false)` it didn't (a `result` came first,
/// or the answer was an error), `None` not yet [spike S-3.7, 3.8, 3.9].
pub fn check_answer(line: &Value, request_id: &str) -> Option<bool> {
    match line.get("type").and_then(Value::as_str) {
        Some("control_response") => {
            let response = line.get("response")?;
            if response.get("request_id").and_then(Value::as_str) != Some(request_id) {
                return None;
            }
            Some(response.get("subtype").and_then(Value::as_str) == Some("success"))
        }
        Some("result") => Some(false),
        _ => None,
    }
}

/// A cut copy of a session's transcript (§U6.2 step 2): a 0600 file named
/// `<16 hex>.jsonl` in the host's runtime directory, removed when this is
/// dropped. It is never parsed beyond the lines' `uuid` and never logged.
#[derive(Debug)]
pub struct CutFile {
    path: PathBuf,
}

impl CutFile {
    /// A new, empty cut file in `dir`, and the file to write it through.
    /// Never an existing file, nor a link (`create_new` is `O_EXCL`).
    pub fn create(dir: &Path) -> std::io::Result<(CutFile, std::fs::File)> {
        let name: u64 = rand::random();
        let path = dir.join(format!("{name:016x}{CUT_SUFFIX}"));
        let file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        Ok((CutFile { path }, file))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for CutFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

const CUT_SUFFIX: &str = ".jsonl";

/// A name [`CutFile::create`] makes.
pub fn is_cut_name(name: &str) -> bool {
    name.strip_suffix(CUT_SUFFIX).is_some_and(|hex| {
        hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// How the next process of a thread starts from a copy of a session.
#[derive(Debug)]
pub struct ForkStart {
    /// The session the copy is made from.
    pub from: String,
    /// What `--resume` gets: `from`, or the path of `cut`.
    pub resume: String,
    /// Removed once Claude reports the copy's id, or when the process ends.
    pub cut: Option<CutFile>,
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

/// `mcp__<server>__<tool>` → `("<server>", "<tool>")`.
pub fn mcp_parts(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    (!server.is_empty() && !tool.is_empty()).then_some((server, tool))
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
        _ => {
            let detail = input.to_string();
            let title = match mcp_parts(name) {
                Some((server, tool)) => format!("{server}: {tool}"),
                None => name.to_owned(),
            };
            ToolSpec {
                detail: Some(api::cut(&detail, api::MAX_TOOL_DETAIL).to_owned()),
                path: if name == "Read" { path_of("file_path") } else { None },
                ..ToolSpec::other(name, &title, tool_class(name))
            }
        }
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

/// The user's MCP servers in a `system/init` line, ours left out: how many
/// are connected, how many need a sign-in, and their names.
pub fn mcp_of(init: &Value) -> Option<(u16, u16, Vec<String>)> {
    let servers = init.get("mcp_servers")?.as_array()?;
    let mut connected = 0u16;
    let mut need_login = 0u16;
    let mut names = Vec::new();
    for server in servers {
        let Some(name) = server.get("name").and_then(Value::as_str) else { continue };
        if name == MCP_SERVER {
            continue;
        }
        match server.get("status").and_then(Value::as_str) {
            Some("connected") => connected = connected.saturating_add(1),
            Some("needs-auth") => need_login = need_login.saturating_add(1),
            _ => {}
        }
        let name = api::clean_cut(name.trim(), api::MAX_MCP_NAME).0;
        if !name.is_empty() && names.len() < api::MAX_MCP_NAMES && !names.contains(&name) {
            names.push(name);
        }
    }
    Some((connected, need_login, names))
}

/// The keys of `system/init` the host uses (the folder's commands and the
/// mode and model Claude runs with); its paths, tools and the rest stay here.
pub fn init_summary(init: &Value) -> Value {
    const KEPT: [&str; 5] =
        ["slash_commands", "terminal_slash_commands", "skills", "permissionMode", "model"];
    let kept: serde_json::Map<String, Value> =
        KEPT.iter().filter_map(|k| init.get(*k).map(|v| ((*k).to_owned(), v.clone()))).collect();
    Value::Object(kept)
}

/// The context a turn's `result` reports (§Q4.8): the last model call's
/// input, cache and output tokens, against the answering model's window.
pub fn context_of(result: &Value, model: Option<&str>) -> Option<(u64, u64)> {
    let last = result.pointer("/usage/iterations")?.as_array()?.last()?;
    let n = |key: &str| last.get(key).and_then(Value::as_u64).unwrap_or(0);
    let used = n("input_tokens")
        .saturating_add(n("cache_read_input_tokens"))
        .saturating_add(n("cache_creation_input_tokens"))
        .saturating_add(n("output_tokens"));
    let usage = result.get("modelUsage")?.as_object()?;
    let window = |v: &Value| v.get("contextWindow").and_then(Value::as_u64);
    // The usage key may carry a suffix (`claude-opus-5-5[1m]`).
    let named = model.and_then(|m| {
        usage.get(m).or_else(|| usage.iter().find(|(k, _)| k.starts_with(&format!("{m}["))).map(|(_, v)| v))
    });
    let max = match named {
        Some(entry) => window(entry)?,
        None => usage.values().filter_map(window).max()?,
    };
    (used > 0 && max > 0).then_some((used, max))
}

/// A message written to Claude and not yet taken in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Sent {
    /// Its user item's id.
    id: String,
    steer: bool,
    /// It starts a turn: its echo's `uuid` is the turn's mark.
    starts: bool,
}

/// Turns Claude's stdout lines into steps (§4.4).
#[derive(Debug)]
pub struct ClaudeMapper {
    cwd: PathBuf,
    session_id: Option<String>,
    /// The process's mode (a change restarts the process).
    mode: Mode,
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
    /// Messages written and not yet echoed back (`--replay-user-messages`).
    pending: VecDeque<Sent>,
    /// The model that answered last, as reported.
    answered_model: Option<String>,
    /// The model of this turn's last real answer (not `<synthetic>`).
    turn_model: Option<String>,
    /// What was last reported of the MCP servers.
    mcp: Option<(u16, u16, Vec<String>)>,
    /// Tool calls shown as items of their own (the plan, a question): no
    /// tool step for them or their results.
    hidden: HashSet<String>,
    /// The process continues a copy of this session (`--fork-session`):
    /// the copy's id comes with its first `system/init`.
    forking: Option<String>,
}

impl ClaudeMapper {
    pub fn new(cwd: PathBuf, session_id: Option<String>) -> Self {
        Self {
            cwd,
            session_id,
            mode: Mode::Ask,
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
            pending: VecDeque::new(),
            answered_model: None,
            turn_model: None,
            mcp: None,
            hidden: HashSet::new(),
            forking: None,
        }
    }

    /// The process continues a copy of the session `from`: the first
    /// `system/init` names the copy ([`Step::NativeId`]).
    pub fn set_forking(&mut self, from: &str) {
        self.session_id = None;
        self.forking = Some(from.to_owned());
    }

    /// The process's mode: in plan mode the plan file's write is marked.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    pub fn turn_started(&mut self) {
        self.turn_running = true;
        self.interrupt_sent = false;
        self.rate_limited = None;
        self.assistant_error = None;
        self.last_text = None;
        self.turn_model = None;
    }

    pub fn turn_running(&self) -> bool {
        self.turn_running
    }

    pub fn interrupt_sent(&mut self) {
        self.interrupt_sent = true;
    }

    /// A message was written to stdin: `steer` when it went to steer (into
    /// a running turn, or starting one when none runs).
    pub fn wrote(&mut self, id: &str, steer: bool) {
        if self.pending.len() >= MAX_PENDING {
            self.pending.pop_front();
        }
        let starts = !steer || !self.turn_running;
        self.pending.push_back(Sent { id: id.to_owned(), steer, starts });
    }

    fn end_turn(&mut self, end: TurnEnd) -> Step {
        self.turn_running = false;
        self.interrupt_sent = false;
        self.open_text.clear();
        self.tool_names.clear();
        self.open_thinking.clear();
        self.message_texts.clear();
        self.turn_texts.clear();
        self.hidden.clear();
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
                let reported = line.get("session_id").and_then(Value::as_str);
                if let Some(from) = self.forking.take() {
                    match reported.filter(|id| is_session_id(id) && *id != from) {
                        Some(id) => {
                            self.session_id = Some(id.to_owned());
                            steps.push(Step::NativeId(id.to_owned()));
                        }
                        None => {
                            tracing::warn!("Claude didn't report a copy of the session");
                            self.forking = Some(from);
                        }
                    }
                } else if !reported.is_some_and(|id| self.session_id.as_deref().is_none_or(|s| s == id)) {
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
                    return steps;
                }
                if let Some(mcp) = mcp_of(line)
                    && self.mcp.as_ref() != Some(&mcp)
                {
                    self.mcp = Some(mcp.clone());
                    let (connected, need_login, names) = mcp;
                    steps.push(Step::Mcp { names, connected, need_login });
                }
                steps.push(Step::Init(init_summary(line)));
            }
            ("system", "api_retry") => {
                if let Some(error) = line.get("error").and_then(Value::as_str) {
                    self.assistant_error = Some(error.to_owned());
                }
            }
            ("system", "status") => self.status(line, &mut steps),
            ("rate_limit_event", _) => {
                let info = line.get("rate_limit_info");
                let status = info.and_then(|i| i.get("status")).and_then(Value::as_str);
                if status == Some("rejected") {
                    self.rate_limited = Some(info.and_then(|i| i.get("resetsAt")).and_then(Value::as_u64));
                }
                let limits = info.map(catalog::claude_limits).unwrap_or_default();
                if !limits.is_empty() {
                    steps.push(Step::Limits(limits));
                }
            }
            ("stream_event", _) => self.stream_event(line.get("event").unwrap_or(&Value::Null), &mut steps),
            ("assistant", _) => self.assistant(line, &mut steps),
            ("user", _) if line.get("isReplay").and_then(Value::as_bool) == Some(true) => {
                self.replayed(line, &mut steps);
            }
            ("user", _) => self.user(line, &mut steps),
            ("result", _) if self.turn_running => {
                if let Some((used, max)) = context_of(line, self.turn_model.as_deref()) {
                    steps.push(Step::Context { used, max });
                }
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

    /// Claude took in a message we wrote. After its `result` that starts a
    /// turn of its own (a steer that came too late for the last one).
    fn replayed(&mut self, line: &Value, steps: &mut Vec<Step>) {
        let Some(sent) = self.pending.pop_front() else { return };
        let starts = sent.starts || !self.turn_running;
        if !self.turn_running {
            self.turn_started();
            steps.push(Step::TurnStarted);
        }
        // The echo's `uuid` is its transcript line's [spike S-3.3]: a cut
        // made before it leaves this turn out (§U6.2).
        if starts && let Some(mark) = line.get("uuid").and_then(Value::as_str).filter(|m| is_session_id(m)) {
            steps.push(Step::TurnMark(mark.to_owned()));
        }
        if sent.steer && !sent.id.is_empty() {
            steps.push(Step::Delivered { id: sent.id });
        }
    }

    /// `system/status`: a compaction's outcome is a notice.
    fn status(&mut self, line: &Value, steps: &mut Vec<Step>) {
        let error =
            line.get("compact_error").and_then(Value::as_str).map(str::trim).filter(|e| !e.is_empty());
        if let Some(error) = error {
            steps.push(Step::Notice(api::clean_cut(error, api::MAX_ERROR_MESSAGE).0));
            return;
        }
        let result = line.get("compact_result").filter(|r| !r.is_null());
        let failed = result.is_some_and(|r| {
            r.as_bool() == Some(false) || matches!(r.as_str(), Some("failed" | "error" | "failure"))
        });
        if result.is_some() && !failed {
            steps.push(Step::Notice(COMPACTED.to_owned()));
        }
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

    /// The answering model; a change is reported.
    fn answered_by(&mut self, model: &str, steps: &mut Vec<Step>) {
        let model = api::clean_cut(model.trim(), api::MAX_MODEL_ID).0;
        if model.is_empty() {
            return;
        }
        self.turn_model = Some(model.clone());
        if self.answered_model.as_deref() != Some(model.as_str()) {
            self.answered_model = Some(model.clone());
            steps.push(Step::AnsweredModel(model));
        }
    }

    fn assistant(&mut self, line: &Value, steps: &mut Vec<Step>) {
        if let Some(error) = line.get("error").and_then(Value::as_str) {
            self.assistant_error = Some(error.to_owned());
        }
        let Some(message) = line.get("message") else { return };
        let model = message.get("model").and_then(Value::as_str);
        let Some(blocks) = message.get("content").and_then(Value::as_array) else { return };
        if model == Some(catalog::SYNTHETIC_MODEL) {
            // Answered by the CLI itself (a slash command): a notice, never
            // the model that answered.
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("text")
                    && let Some(text) =
                        block.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty())
                {
                    steps.push(Step::Notice(text.to_owned()));
                }
            }
            return;
        }
        if let Some(model) = model {
            self.answered_by(model, steps);
        }
        let id = message.get("id").and_then(Value::as_str).unwrap_or("message").to_owned();
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
                    if name == plan::EXIT_PLAN_TOOL || name == questions::ASK_TOOL {
                        // The bridge makes its plan or question item.
                        self.hidden.insert(key.to_owned());
                        continue;
                    }
                    self.tool_names.insert(key.to_owned(), name.to_owned());
                    if let Some(plan) = todo_plan(name, input) {
                        steps.push(Step::Plan(plan));
                    }
                    let mut tool = tool_spec(name, input, &self.cwd);
                    if self.mode == Mode::Plan && plan::is_plan_file(name, input, &self.cwd) {
                        tool.class = ToolClass::Plan;
                    }
                    steps.push(Step::ToolStart { key: key.to_owned(), tool });
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
            if self.hidden.contains(key) {
                continue;
            }
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
            // Auto: what Claude's classifier let through without asking (spike 1).
            let decision = meta.and_then(|m| m.get("permission_decision"));
            if !rejected
                && decision.and_then(|d| d.get("reason_type")).and_then(Value::as_str) == Some("classifier")
                && decision.and_then(|d| d.get("decision")).and_then(Value::as_str) == Some("accept")
            {
                let review =
                    api::ToolReview { status: api::ReviewStatus::Approved, risk: None, rationale: None };
                steps.push(Step::ToolReview { key: Some(key.to_owned()), review });
            }
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
    /// Used by [`spawn`]; [`spawn_with`] takes the full settings.
    pub mode: Mode,
    pub session_id: String,
    pub resume: bool,
    pub mcp_config: PathBuf,
    pub bridge_exe: PathBuf,
    pub socket: PathBuf,
    pub bridge_token: String,
    pub env: Vec<(OsString, OsString)>,
    pub first_text: String,
    /// The process continues a copy of a session (§U6.2): `session_id`
    /// and `resume` are then not used. Start it with [`spawn_session`].
    pub fork: Option<ForkFrom>,
}

/// A thread's pending copy, for [`Launch::fork`]: made with
/// [`crate::host::fork::launch_fork`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkFrom {
    /// [`crate::host::fork::LaunchHint::encode`].
    pub hint: String,
    /// Where Claude keeps its transcripts (`$CLAUDE_CONFIG_DIR` or `~/.claude`).
    pub claude_home: PathBuf,
    /// Where the cut copy goes: `cut/` in the host's runtime directory.
    pub cut_dir: PathBuf,
}

/// Starts a Claude process for a thread with only its mode set.
pub fn spawn(
    launch: &Launch,
) -> std::io::Result<(AgentProcess, tokio::sync::mpsc::Receiver<crate::host::agents::OutLine>)> {
    spawn_with(launch, &LaunchSettings::new(launch.mode))
}

/// Starts a Claude process that continues a copy of a session (§U6.2):
/// `launch.session_id` is what `--resume` gets ([`ForkStart::resume`]).
pub fn spawn_fork(
    launch: &Launch,
    settings: &LaunchSettings,
) -> std::io::Result<(AgentProcess, tokio::sync::mpsc::Receiver<crate::host::agents::OutLine>)> {
    write_mcp_config(&launch.mcp_config, &launch.bridge_exe, &launch.socket, &launch.bridge_token)?;
    let args = fork_args(settings, &launch.mcp_config, &launch.session_id);
    let spawned = AgentProcess::spawn(&launch.bin, &args, &launch.cwd, &launch.env);
    if spawned.is_err() {
        let _ = std::fs::remove_file(&launch.mcp_config);
    }
    spawned
}

/// Starts a Claude process for a thread and its driver, sending `first`
/// (§Q4.2). With [`Launch::fork`] the process continues a copy of a session
/// (§U6.2): the cut copy is made first when the copy ends at a mark, the
/// process runs with `--resume <session or cut> --fork-session`, and the
/// copy's id comes with Claude's first `system/init` ([`Step::NativeId`]).
/// A copy that can't be made is an error and starts nothing (its message:
/// what went wrong, for the user).
pub fn spawn_session(
    launch: &Launch,
    settings: &LaunchSettings,
    first: UserInput,
) -> std::io::Result<(AgentProcess, tokio::sync::mpsc::Receiver<crate::host::agents::OutLine>, ClaudeDriver)>
{
    let Some(from) = &launch.fork else {
        let (process, lines) = spawn_with(launch, settings)?;
        let driver =
            ClaudeDriver::with_settings(launch.cwd.clone(), launch.session_id.clone(), settings, first);
        return Ok((process, lines, driver));
    };
    let start = crate::host::fork::claude_fork_start(from, &launch.cwd)?;
    let forked = Launch { session_id: start.resume.clone(), resume: true, fork: None, ..launch.clone() };
    let (process, lines) = spawn_fork(&forked, settings)?;
    Ok((process, lines, ClaudeDriver::forking(launch.cwd.clone(), settings, first, start)))
}

/// Starts a Claude process for a thread with its settings (§Q4.2). Not
/// for a [`Launch::fork`]: that needs [`spawn_session`].
pub fn spawn_with(
    launch: &Launch,
    settings: &LaunchSettings,
) -> std::io::Result<(AgentProcess, tokio::sync::mpsc::Receiver<crate::host::agents::OutLine>)> {
    if launch.fork.is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a copy starts with spawn_session",
        ));
    }
    write_mcp_config(&launch.mcp_config, &launch.bridge_exe, &launch.socket, &launch.bridge_token)?;
    let args = args(settings, &launch.mcp_config, &launch.session_id, launch.resume);
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
    first: Option<UserInput>,
    interrupts: u64,
    /// The cut copy this process was started from, until Claude has read it.
    cut: Option<CutFile>,
}

impl ClaudeDriver {
    /// A driver whose first message is text alone, in a supervised process.
    pub fn new(cwd: PathBuf, session_id: String, first_text: String) -> Self {
        Self::with_settings(cwd, session_id, &LaunchSettings::new(Mode::Ask), UserInput::text(first_text))
    }

    /// A driver for a process started with `settings`, sending `first`.
    pub fn with_settings(
        cwd: PathBuf,
        session_id: String,
        settings: &LaunchSettings,
        first: UserInput,
    ) -> Self {
        let mut mapper = ClaudeMapper::new(cwd, Some(session_id));
        mapper.set_mode(settings.mode);
        Self { mapper, first: Some(first), interrupts: 0, cut: None }
    }

    /// A driver for a process started with [`spawn_fork`] from `start`: the
    /// copy's id is reported when Claude names it, and the cut file, if
    /// any, is removed then or when the process ends (§U6.2 step 4).
    pub fn forking(cwd: PathBuf, settings: &LaunchSettings, first: UserInput, start: ForkStart) -> Self {
        let mut mapper = ClaudeMapper::new(cwd, None);
        mapper.set_mode(settings.mode);
        mapper.set_forking(&start.from);
        Self { mapper, first: Some(first), interrupts: 0, cut: start.cut }
    }

    /// Writes a message; `steer` when it goes into the running turn.
    fn write(&mut self, input: &UserInput, steer: bool, out: &mut Output) {
        out.stdin.push(input_line(input));
        self.mapper.wrote(&input.id, steer);
    }
}

impl Driver for ClaudeDriver {
    fn start(&mut self, out: &mut Output) {
        if let Some(first) = self.first.take() {
            self.write(&first, false, out);
            self.mapper.turn_started();
        }
    }

    fn on_line(&mut self, line: Value, out: &mut Output) {
        if self.cut.is_some()
            && line.get("type").and_then(Value::as_str) == Some("system")
            && line.get("subtype").and_then(Value::as_str) == Some("init")
        {
            // Claude has loaded the copy and named it: the cut is not needed.
            self.cut = None;
        }
        if line.get("type").and_then(Value::as_str) == Some("control_request") {
            if let Some((reply, elicitation)) = cli_request_reply(&line) {
                out.stdin.push(reply);
                if elicitation {
                    out.steps.push(Step::Notice(crate::host::questions::MCP_DECLINED.to_owned()));
                }
            }
            return;
        }
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
            SessionCmd::Send(input) => {
                self.write(&input, false, out);
                self.mapper.turn_started();
            }
            SessionCmd::Steer(input) => {
                // Written now: Claude takes it in at its next step (§Q4.5).
                // With no turn running it starts one.
                let running = self.mapper.turn_running();
                self.write(&input, true, out);
                if !running {
                    self.mapper.turn_started();
                    out.steps.push(Step::TurnStarted);
                }
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
        self.cut = None;
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
    fn what_auto_let_through_is_marked_on_its_step() {
        let mut mapper = ClaudeMapper::new(PathBuf::from("/w"), None);
        mapper.turn_started();
        let tools = json!({"type":"assistant","message":{"id":"m","content":[
            {"type":"tool_use","id":"toolu_curl","name":"Bash","input":{"command":"curl -sI https://example.com"}},
            {"type":"tool_use","id":"toolu_w","name":"Write","input":{"file_path":"/w/a.txt","content":"a"}},
            {"type":"tool_use","id":"toolu_q","name":"Bash","input":{"command":"ls"}}]}});
        mapper.feed(&tools);
        // Spike 1's shapes: the classifier, the mode itself, the user through the bridge.
        let decision = |id: &str, source: &str, reason: Option<&str>| {
            let mut d = json!({"decision": "accept", "source": source});
            if let Some(reason) = reason {
                d["reason_type"] = json!(reason);
            }
            json!({"id": id, "permission_decision": d})
        };
        let results = json!({"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"toolu_curl","content":"HTTP/2 200"},
            {"type":"tool_result","tool_use_id":"toolu_w","content":"ok"},
            {"type":"tool_result","tool_use_id":"toolu_q","content":"a.txt"}]},
            "tool_result_meta":[decision("toolu_curl", "config", Some("classifier")),
                decision("toolu_w", "config", Some("mode")), decision("toolu_q", "user_temporary", None)]});
        let steps = mapper.feed(&results);
        let reviewed: Vec<&str> = steps
            .iter()
            .filter_map(|s| match s {
                Step::ToolReview { key: Some(key), review }
                    if review.status == api::ReviewStatus::Approved =>
                {
                    Some(key.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(reviewed, ["toolu_curl"], "only the classifier's");
        let review = steps.iter().position(|s| matches!(s, Step::ToolReview { .. })).unwrap();
        let end =
            steps.iter().position(|s| matches!(s, Step::ToolEnd { key, .. } if key == "toolu_curl")).unwrap();
        assert!(review < end, "marked while the step still runs");
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
        let a = args(&LaunchSettings::new(Mode::AcceptEdits), Path::new("/r/mcp.json"), &id, true);
        assert!(a.windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
        assert!(a.windows(2).any(|w| w[0] == "--resume" && w[1] == id));
        assert!(!a.iter().any(|s| s == "--session-id" || s == "--bare"));
        let a = args(&LaunchSettings::new(Mode::Ask), Path::new("/r/mcp.json"), &id, false);
        assert!(a.windows(2).any(|w| w == ["--permission-mode", "default"]));
        assert!(a.windows(2).any(|w| w[0] == "--session-id"));
    }

    fn settings(mode: Mode, model: Option<&str>, effort: Option<&str>, mcp: bool) -> LaunchSettings {
        LaunchSettings {
            mode,
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
            mcp,
            default_model: None,
            default_effort: None,
        }
    }

    /// Every flag the host may pass Claude (§Q4.2, §U6.2).
    const DOCUMENTED: [&str; 15] = [
        "-p",
        "--input-format",
        "--output-format",
        "--verbose",
        "--include-partial-messages",
        "--replay-user-messages",
        "--permission-mode",
        "--model",
        "--effort",
        "--strict-mcp-config",
        "--mcp-config",
        "--permission-prompt-tool",
        "--resume",
        "--session-id",
        "--fork-session",
    ];

    #[test]
    fn launch_flags_for_every_setting() {
        let id = new_session_id();
        for mode in Mode::ALL {
            for model in [None, Some("opus"), Some("claude-opus-5"), Some("-p"), Some("a b"), Some("x\ny")] {
                for effort in [None, Some("low"), Some("xhigh"), Some("--bare"), Some("High")] {
                    for mcp in [true, false] {
                        for resume in [true, false] {
                            let a = args(
                                &settings(mode, model, effort, mcp),
                                Path::new("/r/m.json"),
                                &id,
                                resume,
                            );
                            for flag in a.iter().filter(|a| a.starts_with('-')) {
                                assert!(DOCUMENTED.contains(&flag.as_str()), "{flag} in {a:?}");
                            }
                            let value =
                                |flag: &str| a.windows(2).find(|w| w[0] == flag).map(|w| w[1].as_str());
                            assert_eq!(value("--permission-mode"), Some(permission_mode(mode)));
                            assert_eq!(value("--model"), model.filter(|m| catalog::valid_model_id(m)));
                            assert_eq!(value("--effort"), effort.and_then(catalog::effort_word));
                            assert_eq!(a.iter().any(|a| a == "--strict-mcp-config"), !mcp);
                            assert_eq!(value("--mcp-config"), Some("/r/m.json"), "our bridge, always");
                            assert_eq!(value("--permission-prompt-tool"), Some(PERMISSION_TOOL));
                            assert!(a.iter().any(|a| a == "--replay-user-messages"));
                            assert_eq!(
                                value(if resume { "--resume" } else { "--session-id" }),
                                Some(id.as_str())
                            );
                            for never in [
                                "--bare",
                                "--disallowedTools",
                                "--dangerously-skip-permissions",
                                "--allow-dangerously-skip-permissions",
                            ] {
                                assert!(!a.iter().any(|a| a == never), "{never}");
                            }
                            assert!(!a.iter().any(|a| a == "--fork-session"), "only a fork forks");
                            let f =
                                fork_args(&settings(mode, model, effort, mcp), Path::new("/r/m.json"), &id);
                            for flag in f.iter().filter(|a| a.starts_with('-')) {
                                assert!(DOCUMENTED.contains(&flag.as_str()), "{flag} in {f:?}");
                            }
                            assert_eq!(
                                &f[..f.len() - 1],
                                &args(&settings(mode, model, effort, mcp), Path::new("/r/m.json"), &id, true)
                                    [..]
                            );
                            assert_eq!(f.last().map(String::as_str), Some("--fork-session"));
                            assert!(!f.iter().any(|a| a == "--session-id" || a == "--bare"));
                        }
                    }
                }
            }
        }
        let flags: Vec<&str> = Mode::ALL.iter().map(|m| permission_mode(*m)).collect();
        assert_eq!(flags, ["default", "acceptEdits", "auto", "plan", "bypassPermissions"]);
    }

    #[test]
    fn the_only_control_requests_are_interrupt_and_initialize() {
        let source = include_str!("claude.rs");
        let code = &source[..source.find("#[cfg(test)]").unwrap()];
        let pattern = "\"request\": {\"subtype\": \"";
        let found: Vec<&str> = code
            .match_indices(pattern)
            .map(|(i, _)| {
                let rest = &code[i + pattern.len()..];
                &rest[..rest.find('"').unwrap()]
            })
            .collect();
        assert_eq!(found, CONTROL_SUBTYPES);
        for word in [
            "set_model",
            "set_permission_mode",
            "apply_flag_settings",
            "set_max_thinking_tokens",
            "mcp_status",
        ] {
            assert!(!code.contains(word), "{word}");
        }
        for line in [interrupt_line("a"), initialize_line("b"), crate::host::catalog::initialize_line()] {
            let v: Value = serde_json::from_str(&line).unwrap();
            assert!(CONTROL_SUBTYPES.contains(&v["request"]["subtype"].as_str().unwrap()));
        }
        let catalog = include_str!("catalog.rs");
        let catalog_code = &catalog[..catalog.find("#[cfg(test)]").unwrap()];
        assert!(!catalog_code.contains("\"subtype\": "), "the probe's request is built here only");
    }

    /// Drives a fixture through the driver: `_stdin` lines are the host's
    /// messages (`_steer`: sent while it worked), the first one starts it.
    fn drive(name: &str, mode: Mode) -> (Vec<Step>, Vec<Value>) {
        let mut driver: Option<ClaudeDriver> = None;
        let mut out = Output::default();
        for line in fixture_lines(name) {
            if let Some(stdin) = line.get("_stdin") {
                let input = UserInput {
                    id: line["_id"].as_str().unwrap_or_default().to_owned(),
                    text: stdin
                        .pointer("/message/content/0/text")
                        .and_then(Value::as_str)
                        .unwrap()
                        .to_owned(),
                    images: Vec::new(),
                };
                match driver.as_mut() {
                    None => {
                        let mut d = ClaudeDriver::with_settings(
                            PathBuf::from("/work"),
                            "11111111-2222-4333-8444-555555555555".into(),
                            &settings(mode, None, None, true),
                            input,
                        );
                        d.start(&mut out);
                        driver = Some(d);
                    }
                    Some(d) if line["_steer"] == true => d.on_cmd(SessionCmd::Steer(input), &mut out),
                    Some(d) => d.on_cmd(SessionCmd::Send(input), &mut out),
                }
                continue;
            }
            driver.as_mut().unwrap().on_line(line, &mut out);
        }
        let stdin = out.stdin.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
        (out.steps, stdin)
    }

    #[test]
    fn plan_mode_hides_the_plan_file_and_the_plan_call() {
        let (steps, _) = drive("claude-11-plan.jsonl", Mode::Plan);
        let starts: Vec<(&str, ToolClass)> = steps
            .iter()
            .filter_map(|s| {
                if let Step::ToolStart { key, tool } = s { Some((key.as_str(), tool.class)) } else { None }
            })
            .collect();
        assert_eq!(
            starts,
            [("toolu_w", ToolClass::Plan)],
            "ExitPlanMode makes no step; the plan item comes from the bridge"
        );
        assert_eq!(tool_ends(&steps), [("toolu_w", ToolStatus::Ok)]);
        let (mcp, models): (Vec<_>, Vec<_>) = (
            steps.iter().filter(|s| matches!(s, Step::Mcp { .. })).collect(),
            steps
                .iter()
                .filter_map(|s| if let Step::AnsweredModel(m) = s { Some(m.as_str()) } else { None })
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            mcp,
            [&Step::Mcp {
                names: vec!["github".into(), "linear".into(), "docs".into()],
                connected: 1,
                need_login: 1
            }]
        );
        assert_eq!(
            models,
            ["claude-opus-5-5", "claude-sonnet-5-5"],
            "plan mode picked another model on the way"
        );
        let init =
            steps.iter().find_map(|s| if let Step::Init(init) = s { Some(init) } else { None }).unwrap();
        assert_eq!(init["terminal_slash_commands"][0], "login");
        assert_eq!(init["permissionMode"], "plan");
        assert!(
            init.get("cwd").is_none() && init.get("mcp_servers").is_none() && init.get("tools").is_none()
        );
        let context = steps
            .iter()
            .find_map(|s| if let Step::Context { used, max } = s { Some((*used, *max)) } else { None });
        assert_eq!(context, Some((2080, 1_000_000)), "the last call's tokens, the answering model's window");
        assert_eq!(ends(&steps).len(), 1);
        // In another mode the same write is an edit.
        let (steps, _) = drive("claude-11-plan.jsonl", Mode::Ask);
        assert!(
            steps.iter().any(|s| matches!(s, Step::ToolStart { tool, .. } if tool.class == ToolClass::Edit))
        );
    }

    #[test]
    fn a_question_call_makes_no_step() {
        let (steps, _) = drive("claude-12-question.jsonl", Mode::Ask);
        assert!(!steps.iter().any(|s| matches!(s, Step::ToolStart { .. } | Step::ToolEnd { .. })));
        assert_eq!(assistant_texts(&steps), ["Blue."]);
        assert_eq!(ends(&steps)[0].outcome, Outcome::Done);
    }

    #[test]
    fn a_steer_is_delivered_in_its_turn_or_starts_the_next() {
        let (steps, stdin) = drive("claude-13-steer.jsonl", Mode::Ask);
        assert_eq!(stdin.len(), 3, "each message written at once");
        assert!(stdin.iter().all(|l| l["type"] == "user"));
        let marks: Vec<String> = steps
            .iter()
            .filter_map(|s| match s {
                Step::Delivered { id } => Some(format!("delivered {id}")),
                Step::TurnStarted => Some("started".to_owned()),
                Step::TurnEnded(_) => Some("ended".to_owned()),
                _ => None,
            })
            .collect();
        assert_eq!(marks, ["delivered u2", "ended", "started", "delivered u3", "ended"]);
        assert_eq!(assistant_texts(&steps), ["Pineapple, done.", "Pineapple."]);
        assert!(ends(&steps).iter().all(|e| e.outcome == Outcome::Done));
    }

    #[test]
    fn each_turn_is_marked_by_the_echo_that_started_it() {
        let (steps, _) = drive("claude-13-steer.jsonl", Mode::Ask);
        let marks: Vec<String> = steps
            .iter()
            .filter_map(|s| match s {
                Step::TurnMark(m) => Some(format!("mark {m}")),
                Step::Delivered { id } => Some(format!("delivered {id}")),
                Step::TurnStarted => Some("started".to_owned()),
                Step::TurnEnded(_) => Some("ended".to_owned()),
                _ => None,
            })
            .collect();
        // The first message's echo marks turn 1; a steer inside it makes no
        // mark; one taken in after the result starts and marks turn 2.
        assert_eq!(
            marks,
            [
                "mark 13000000-0000-4000-8000-000000000001",
                "delivered u2",
                "ended",
                "started",
                "mark 13000000-0000-4000-8000-000000000003",
                "delivered u3",
                "ended"
            ]
        );
        // An echo without a uuid, or with one that isn't, marks nothing.
        let mut driver = ClaudeDriver::new(PathBuf::from("/w"), new_session_id(), "Hi".into());
        let mut out = Output::default();
        driver.start(&mut out);
        driver.on_line(
            json!({"type": "user", "isReplay": true, "uuid": "../x", "message": {"content": []}}),
            &mut out,
        );
        assert!(!out.steps.iter().any(|s| matches!(s, Step::TurnMark(_))));
        // A steer while idle starts its turn and marks it.
        driver.on_line(json!({"type": "result", "subtype": "success", "is_error": false}), &mut out);
        let mut out = Output::default();
        driver.on_cmd(
            SessionCmd::Steer(UserInput { id: "u9".into(), text: "More".into(), images: Vec::new() }),
            &mut out,
        );
        let mark = new_session_id();
        driver.on_line(
            json!({"type": "user", "isReplay": true, "uuid": mark, "message": {"content": []}}),
            &mut out,
        );
        assert_eq!(out.steps, [Step::TurnStarted, Step::TurnMark(mark), Step::Delivered { id: "u9".into() }]);
    }

    #[test]
    fn a_steer_while_idle_starts_a_turn() {
        let mut driver = ClaudeDriver::new(PathBuf::from("/w"), new_session_id(), "Hi".into());
        let mut out = Output::default();
        driver.start(&mut out);
        let echo = json!({"type": "user", "isReplay": true, "message": {"role": "user", "content": []}});
        driver.on_line(echo.clone(), &mut out);
        driver.on_line(
            json!({"type": "result", "subtype": "success", "is_error": false, "result": "Hi."}),
            &mut out,
        );
        assert!(!out.steps.iter().any(|s| matches!(s, Step::Delivered { .. } | Step::TurnStarted)));
        let mut out = Output::default();
        let input = UserInput { id: "u9".into(), text: "More".into(), images: Vec::new() };
        driver.on_cmd(SessionCmd::Steer(input), &mut out);
        assert_eq!(out.steps, [Step::TurnStarted]);
        assert_eq!(out.stdin.len(), 1);
        let mut out = Output::default();
        driver.on_line(echo.clone(), &mut out);
        assert_eq!(out.steps, [Step::Delivered { id: "u9".into() }]);
        // An echo nobody waits for is ignored.
        let mut out = Output::default();
        driver.on_line(echo, &mut out);
        assert!(out.steps.is_empty());
    }

    #[test]
    fn slash_commands_answered_by_the_cli_are_notices() {
        let (steps, _) = drive("claude-14-slash.jsonl", Mode::Ask);
        let notices: Vec<&str> = steps
            .iter()
            .filter_map(|s| if let Step::Notice(t) = s { Some(t.as_str()) } else { None })
            .collect();
        assert_eq!(
            notices,
            [
                "Context: 12k of 1M tokens",
                "Not enough messages to compact.",
                "Error: Not enough messages to compact.",
                COMPACTED,
                "Compacted."
            ]
        );
        assert!(assistant_texts(&steps).is_empty());
        assert!(!steps.iter().any(|s| matches!(s, Step::AnsweredModel(_) | Step::Context { .. })));
        assert_eq!(ends(&steps).len(), 3);
    }

    #[test]
    fn images_go_before_the_text() {
        let block = json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}});
        let input = UserInput { id: "u".into(), text: "What colour?".into(), images: vec![block.clone()] };
        let line: Value = serde_json::from_str(&input_line(&input)).unwrap();
        assert_eq!(line["message"]["content"], json!([block, {"type": "text", "text": "What colour?"}]));
        assert_eq!(
            serde_json::from_str::<Value>(&user_line("x")).unwrap()["message"]["content"][0]["text"],
            "x"
        );
    }

    #[test]
    fn context_limits_and_mcp_from_the_captures() {
        let steps = run("claude-01-list.jsonl");
        assert!(steps.contains(&Step::Context { used: 40_932, max: 1_000_000 }));
        assert!(steps.contains(&Step::AnsweredModel("claude-opus-5-5".into())));
        // Our own bridge is never one of the user's servers.
        assert!(steps.contains(&Step::Mcp { names: Vec::new(), connected: 0, need_login: 0 }));
        let steps = run("claude-02-followup-approve.jsonl");
        let limits: Vec<&Vec<api::Limit>> =
            steps.iter().filter_map(|s| if let Step::Limits(l) = s { Some(l) } else { None }).collect();
        assert!(!limits.is_empty());
        assert_eq!(limits[0][0].window, "seven_day");
        assert_eq!(limits[0][0].state, api::LimitState::Warn);
        assert_eq!(
            steps.iter().filter(|s| matches!(s, Step::AnsweredModel(_))).count(),
            1,
            "the same model twice is reported once"
        );
        assert_eq!(context_of(&json!({"usage": {"iterations": []}, "modelUsage": {}}), None), None);
        let two = json!({"usage": {"iterations": [{"input_tokens": 5}]},
            "modelUsage": {"a": {"contextWindow": 200000}, "b": {"contextWindow": 1000000}}});
        assert_eq!(context_of(&two, Some("a")), Some((5, 200_000)));
        assert_eq!(context_of(&two, Some("zzz")), Some((5, 1_000_000)));
    }

    #[test]
    fn mcp_tools_are_named_by_server() {
        let spec = tool_spec("mcp__github__search_issues", &json!({"q": "x".repeat(5000)}), Path::new("/w"));
        assert_eq!(
            (spec.title.as_str(), spec.class, spec.kind),
            ("github: search_issues", ToolClass::Mcp, ApprovalKind::Other)
        );
        assert_eq!(spec.detail.unwrap().len(), api::MAX_TOOL_DETAIL);
        assert_eq!(mcp_parts("mcp__a__b__c"), Some(("a", "b__c")));
        assert_eq!(mcp_parts("mcp__x"), None);
        assert_eq!(tool_spec("ToolSearch", &json!({}), Path::new("/w")).class, ToolClass::Other);
    }

    #[test]
    fn the_clis_own_requests_are_declined() {
        let mut driver = ClaudeDriver::new(PathBuf::from("/w"), new_session_id(), "Hi".into());
        let mut out = Output::default();
        driver.start(&mut out);
        let mut out = Output::default();
        driver.on_line(
            json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "elicitation", "mcp_server_name": "x", "message": "Your token?"}}),
            &mut out,
        );
        let reply: Value = serde_json::from_str(&out.stdin[0]).unwrap();
        assert_eq!(
            reply["response"],
            json!({"subtype": "success", "request_id": "r1", "response": {"action": "decline"}})
        );
        assert_eq!(out.steps, [Step::Notice(crate::host::questions::MCP_DECLINED.into())]);
        assert!(!format!("{:?}", out.steps).contains("token"));
        let mut out = Output::default();
        driver.on_line(
            json!({"type": "control_request", "request_id": "r2", "request": {"subtype": "hook_callback"}}),
            &mut out,
        );
        let reply: Value = serde_json::from_str(&out.stdin[0]).unwrap();
        assert_eq!(reply["response"]["subtype"], "error");
        assert!(out.steps.is_empty());
    }

    #[test]
    fn a_copy_resumes_by_id_or_by_the_cuts_path() {
        let cut = "/run/user/1000/g7-agents/cut/0123456789abcdef.jsonl";
        let a = fork_args(&LaunchSettings::new(Mode::Ask), Path::new("/r/m.json"), cut);
        assert!(a.windows(2).any(|w| w == ["--resume", cut]));
        assert_eq!(a.iter().filter(|a| *a == "--resume").count(), 1);
        // The check: the probe's flags, the copy, nothing that writes or asks.
        let c = check_args(Path::new("/r/empty.json"), cut);
        assert_eq!(c[c.len() - 3..], ["--resume", cut, "--fork-session"]);
        assert!(c.windows(2).any(|w| w == ["--mcp-config", "/r/empty.json"]));
        assert!(c.iter().any(|a| a == "--strict-mcp-config"));
        for flag in c.iter().filter(|a| a.starts_with('-')) {
            assert!(DOCUMENTED.contains(&flag.as_str()), "{flag} in {c:?}");
        }
        for never in
            ["--bare", "--session-id", "--permission-prompt-tool", "--no-session-persistence", "--model"]
        {
            assert!(!c.iter().any(|a| a == never), "{never}");
        }
    }

    #[test]
    fn a_copy_is_started_only_by_spawn_session() {
        let launch = Launch {
            bin: PathBuf::from("/nonexistent/claude"),
            cwd: PathBuf::from("/w"),
            mode: Mode::Ask,
            session_id: new_session_id(),
            resume: false,
            mcp_config: PathBuf::from("/nonexistent/mcp.json"),
            bridge_exe: PathBuf::from("/nonexistent/g7-agents"),
            socket: PathBuf::from("/nonexistent/host.sock"),
            bridge_token: "t".into(),
            env: Vec::new(),
            first_text: "Hi".into(),
            fork: Some(ForkFrom {
                hint: "{}".into(),
                claude_home: PathBuf::from("/nonexistent"),
                cut_dir: PathBuf::from("/nonexistent/cut"),
            }),
        };
        let e = spawn_with(&launch, &LaunchSettings::new(Mode::Ask)).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
        // A hint that isn't one starts nothing.
        let e = spawn_session(&launch, &LaunchSettings::new(Mode::Ask), UserInput::text("Hi")).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn the_checks_answer() {
        let answer = |subtype: &str, id: &str| json!({"type": "control_response", "response": {"subtype": subtype, "request_id": id, "response": {}}});
        assert_eq!(check_answer(&answer("success", "c1"), "c1"), Some(true));
        assert_eq!(check_answer(&answer("error", "c1"), "c1"), Some(false));
        assert_eq!(check_answer(&answer("success", "other"), "c1"), None);
        // A copy Claude can't read [spike S-3.8, 3.9].
        let failed = json!({"type": "result", "subtype": "error_during_execution", "is_error": true,
            "num_turns": 0, "errors": ["No conversation found"], "session_id": new_session_id()});
        assert_eq!(check_answer(&failed, "c1"), Some(false));
        assert_eq!(check_answer(&json!({"type": "system", "subtype": "hook_started"}), "c1"), None);
        assert_eq!(check_answer(&json!({"type": "control_response"}), "c1"), None);
    }

    #[test]
    fn a_cut_file_is_private_and_goes_when_dropped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (cut, mut file) = CutFile::create(dir.path()).unwrap();
        file.write_all(b"{}\n").unwrap();
        let name = cut.path().file_name().unwrap().to_str().unwrap().to_owned();
        assert!(is_cut_name(&name), "{name}");
        assert_eq!(std::fs::metadata(cut.path()).unwrap().permissions().mode() & 0o777, 0o600);
        let path = cut.path().to_owned();
        drop(cut);
        assert!(!path.exists());
        assert!(is_cut_name("0123456789abcdef.jsonl"));
        for name in [
            "0123456789ABCDEF.jsonl",
            "0123456789abcde.jsonl",
            "../0123456789abcdef.jsonl",
            "x.jsonl",
            "0123456789abcdef.json",
        ] {
            assert!(!is_cut_name(name), "{name}");
        }
    }

    fn init(session: &str) -> Value {
        json!({"type": "system", "subtype": "init", "session_id": session,
            "mcp_servers": [{"name": MCP_SERVER, "status": "connected"}]})
    }

    #[test]
    fn a_copy_is_named_by_its_first_init() {
        let from = new_session_id();
        let copy = new_session_id();
        let mut mapper = ClaudeMapper::new(PathBuf::from("/w"), Some(from.clone()));
        mapper.set_forking(&from);
        mapper.turn_started();
        // Claude kept the source's id: not a copy, nothing reported.
        assert!(!mapper.feed(&init(&from)).iter().any(|s| matches!(s, Step::NativeId(_))));
        let steps = mapper.feed(&init(&copy));
        assert_eq!(
            steps.iter().filter(|s| matches!(s, Step::NativeId(_))).collect::<Vec<_>>(),
            [&Step::NativeId(copy.clone())]
        );
        assert!(matches!(steps[0], Step::NativeId(_)), "before anything that can end the turn");
        // Once.
        assert!(!mapper.feed(&init(&copy)).iter().any(|s| matches!(s, Step::NativeId(_))));
        // A thread that isn't forking never reports an id.
        let mut plain = ClaudeMapper::new(PathBuf::from("/w"), Some(from.clone()));
        assert!(!plain.feed(&init(&copy)).iter().any(|s| matches!(s, Step::NativeId(_))));
        // Not a session id: not taken.
        let mut odd = ClaudeMapper::new(PathBuf::from("/w"), None);
        odd.set_forking(&from);
        assert!(!odd.feed(&init("../x")).iter().any(|s| matches!(s, Step::NativeId(_))));
    }

    fn forking_driver(dir: &Path) -> (ClaudeDriver, PathBuf) {
        let (cut, _) = CutFile::create(dir).unwrap();
        let path = cut.path().to_owned();
        let from = new_session_id();
        let start = ForkStart { resume: path.to_string_lossy().into_owned(), from, cut: Some(cut) };
        let driver = ClaudeDriver::forking(
            PathBuf::from("/w"),
            &LaunchSettings::new(Mode::Ask),
            UserInput::text("Hi"),
            start,
        );
        (driver, path)
    }

    #[test]
    fn the_cut_goes_once_claude_names_the_copy_or_the_process_ends() {
        let dir = tempfile::tempdir().unwrap();
        let (mut driver, path) = forking_driver(dir.path());
        let mut out = Output::default();
        driver.start(&mut out);
        assert_eq!(out.stdin.len(), 1, "the first message goes at once");
        driver.on_line(json!({"type": "system", "subtype": "hook_started"}), &mut out);
        assert!(path.exists(), "kept until Claude names the copy");
        let copy = new_session_id();
        driver.on_line(init(&copy), &mut out);
        assert!(!path.exists());
        assert!(out.steps.contains(&Step::NativeId(copy)));

        // Claude couldn't open it: it ends with an error and exits.
        let (mut driver, path) = forking_driver(dir.path());
        let mut out = Output::default();
        driver.start(&mut out);
        driver.on_line(json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "errors": ["No conversation found"]}), &mut out);
        assert!(path.exists());
        let steps = driver.on_exit(Some(1));
        assert!(!path.exists());
        assert!(steps.is_empty(), "the result already ended the turn");
        assert!(matches!(out.steps.last(), Some(Step::TurnEnded(end)) if end.outcome == Outcome::Failed));

        // The session ends without either: dropping the driver removes it.
        let (driver, path) = forking_driver(dir.path());
        drop(driver);
        assert!(!path.exists());
    }
}
