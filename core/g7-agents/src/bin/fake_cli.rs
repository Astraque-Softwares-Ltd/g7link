// SPDX-License-Identifier: MPL-2.0
//! A stand-in for `claude` and `codex` in tests (§10.1). It replays a
//! captured fixture named by `$FAKE_CLI_SCRIPT`, waiting for the lines the
//! host writes where the capture had them, and drives the real approval
//! bridge where Claude did.
//!
//! `g7-agents-fake-cli claude <args…>` and `g7-agents-fake-cli codex <args…>`.
//! `$FAKE_CLI_LOG`, when set, gets a line per run with the arguments and
//! the working directory; for Claude a line per answer the approval bridge
//! gave; for Codex a line per request method, resumed thread, `turn/start`
//! summary setting and approval decision received.
//! `$FAKE_CLI_RESUME_SCRIPT` is the Claude fixture for `--resume` runs.
//!
//! Claude's `--resume <id or path> --fork-session` logs `fork <id or path>`
//! and answers as the copy: every `session_id` it prints is the fork's
//! (`$FAKE_CLI_FORK_ID`, else a fixed one). A transcript path that is
//! missing, or holds no line with a `uuid`, is refused as the CLI refuses
//! it (spike S-3.8, S-3.9): one `result` with `error_during_execution`,
//! nothing read from stdin, exit 1.
//!
//! Modes (plan §1.6, as the spikes saw them): Claude run with
//! `--permission-mode bypassPermissions` or `auto` asks the bridge only
//! for `AskUserQuestion`, logging `settled <tool>` for each step the
//! capture had approved. Codex logs `permissions <method> <approvalPolicy>
//! <sandbox> <approvalsReviewer>` for `thread/start`, `/resume` and `/fork`;
//! told `never` it sends none of the capture's approval requests, and told
//! `auto_review` it sends `item/autoApprovalReview/started` and `/completed`
//! (approved, risk low) for each instead, logging `settled <method>`.
//!
//! The host's catalog probes are answered without a script: Claude's
//! `initialize` control request (a run with `--no-session-persistence`)
//! from `$FAKE_CLI_INITIALIZE` (default: `claude-10-initialize.jsonl` beside
//! the script), and Codex's `account/read`, `model/list`,
//! `account/rateLimits/read` and `skills/list` wherever they arrive. Neither
//! is logged as a run or a method: the log lines are `probe <what>`.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

const AUTH_STATUS: &str = include_str!("../../tests/fixtures/claude-auth-status.json");

/// Stdin, read on a thread so waiting can look ahead.
struct Input {
    rx: mpsc::Receiver<String>,
    queue: VecDeque<Value>,
    closed: bool,
}

impl Input {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self { rx, queue: VecDeque::new(), closed: false }
    }

    fn read_more(&mut self) -> bool {
        match self.rx.recv() {
            Ok(line) => {
                if let Ok(value) = serde_json::from_str(&line) {
                    self.queue.push_back(value);
                }
                true
            }
            Err(_) => {
                self.closed = true;
                false
            }
        }
    }

    /// The first line matching `pred`, waiting for it. `None` at EOF.
    fn take(&mut self, pred: impl Fn(&Value) -> bool) -> Option<Value> {
        loop {
            if let Some(pos) = self.queue.iter().position(&pred) {
                return self.queue.remove(pos);
            }
            if self.closed || !self.read_more() {
                return None;
            }
        }
    }

    fn wait_eof(&mut self) {
        while !self.closed && self.read_more() {}
    }
}

fn print(value: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

fn load(path: &str) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn log_line(line: &str) {
    if let Some(path) = std::env::var_os("FAKE_CLI_LOG")
        && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

fn log_args(args: &[String]) {
    log_line(&format!("args {}", args.join(" ")));
    if let Ok(dir) = std::env::current_dir() {
        log_line(&format!("cwd {}", dir.display()));
    }
}

// ---- Claude ----

/// The approval bridge Claude would have started, from `--mcp-config`.
struct Bridge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Bridge {
    fn start(config_path: &str) -> Option<Bridge> {
        let config: Value = serde_json::from_str(&std::fs::read_to_string(config_path).ok()?).ok()?;
        let server = config.pointer("/mcpServers/g7")?;
        let mut command = Command::new(server.get("command")?.as_str()?);
        for arg in server.get("args").and_then(Value::as_array).into_iter().flatten() {
            command.arg(arg.as_str()?);
        }
        for (key, value) in server.get("env").and_then(Value::as_object).into_iter().flatten() {
            command.env(key, value.as_str()?);
        }
        let mut child =
            command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().ok()?;
        let stdin = child.stdin.take()?;
        let stdout = BufReader::new(child.stdout.take()?);
        let mut bridge = Bridge { child, stdin, stdout };
        bridge.send(&json!({"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fake","version":"0"}},"jsonrpc":"2.0","id":0}));
        bridge.reply()?;
        bridge.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        bridge.send(&json!({"method":"tools/list","jsonrpc":"2.0","id":1}));
        bridge.reply()?;
        Some(bridge)
    }

    fn send(&mut self, value: &Value) {
        let _ = writeln!(self.stdin, "{value}");
        let _ = self.stdin.flush();
    }

    fn reply(&mut self) -> Option<Value> {
        let mut line = String::new();
        self.stdout.read_line(&mut line).ok()?;
        serde_json::from_str(&line).ok()
    }

    fn call(&mut self, tool: &(String, Value), tool_use_id: &str) {
        self.send(&json!({"method":"tools/call","params":{"name":"approve","arguments":{
            "tool_name": tool.0, "input": tool.1, "tool_use_id": tool_use_id},
            "_meta":{"claudecode/toolUseId": tool_use_id, "progressToken": 2}},"jsonrpc":"2.0","id":2}));
    }

    fn finish(mut self) {
        drop(self.stdin);
        let _ = self.child.wait();
    }
}

fn needs_approval(line: &Value) -> Option<(String, bool)> {
    if line.get("type").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let meta = line.get("tool_result_meta")?.get(0)?;
    let id = meta.get("id").and_then(Value::as_str).map(str::to_owned).or_else(|| {
        line.pointer("/message/content/0/tool_use_id").and_then(Value::as_str).map(str::to_owned)
    })?;
    let source = meta.pointer("/permission_decision/source").and_then(Value::as_str);
    if meta.get("non_execution_kind").and_then(Value::as_str) == Some("user-rejected") {
        return Some((id, true));
    }
    matches!(source, Some("user_temporary" | "user_reject")).then_some((id, false))
}

/// The fixture next to the script, or `$name` when set.
fn sibling(env: &str, name: &str) -> Option<String> {
    if let Ok(path) = std::env::var(env) {
        return Some(path);
    }
    let script = std::env::var("FAKE_CLI_SCRIPT").ok()?;
    let dir = std::path::Path::new(&script).parent()?;
    Some(dir.join(name).to_string_lossy().into_owned())
}

/// Claude's catalog probe: answers `initialize` with the captured answer
/// (its `request_id` made ours), writes nothing else, ends at EOF.
fn claude_probe(args: &[String]) -> ExitCode {
    log_line("probe claude");
    if args.iter().any(|a| a == "--session-id" || a == "--resume") {
        eprintln!("fake claude: a probe with a session");
        return ExitCode::from(2);
    }
    let answer =
        sibling("FAKE_CLI_INITIALIZE", "claude-10-initialize.jsonl").map(|p| load(&p)).unwrap_or_default();
    let mut input = Input::new();
    while let Some(request) = input.take(|v| v.get("type").and_then(Value::as_str) == Some("control_request"))
    {
        let subtype = request.pointer("/request/subtype").and_then(Value::as_str).unwrap_or_default();
        log_line(&format!("probe control {subtype}"));
        let id = request.get("request_id").cloned().unwrap_or(Value::Null);
        for line in &answer {
            let mut line = line.clone();
            if line.get("type").and_then(Value::as_str) == Some("control_response") {
                line["response"]["request_id"] = id.clone();
            }
            print(&line);
        }
    }
    if input.queue.iter().any(|v| v.get("type").and_then(Value::as_str) == Some("user")) {
        eprintln!("fake claude: the probe was sent a message");
        return ExitCode::from(4);
    }
    ExitCode::SUCCESS
}

/// The fork's session id when `--fork-session` is given.
const FORK_ID: &str = "f0f0f0f0-0000-4000-8000-00000000f0c5";

/// A transcript copy Claude would load: a file with at least one line that
/// has a `uuid`.
fn loads_as_transcript(path: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| {
        text.lines().any(|l| serde_json::from_str::<Value>(l).is_ok_and(|v| v.get("uuid").is_some()))
    })
}

/// `--resume … --fork-session`: the fork's id, or the refusal already
/// printed (`Err` holds the exit code).
fn claude_fork(args: &[String]) -> Result<Option<String>, ExitCode> {
    if !args.iter().any(|a| a == "--fork-session") {
        return Ok(None);
    }
    let source = args.windows(2).find(|w| w[0] == "--resume").map(|w| w[1].clone()).unwrap_or_default();
    log_line(&format!("fork {source}"));
    let id = std::env::var("FAKE_CLI_FORK_ID").unwrap_or_else(|_| FORK_ID.to_owned());
    if source.starts_with('/') && !loads_as_transcript(&source) {
        print(&json!({
            "type": "result", "subtype": "error_during_execution", "is_error": true, "num_turns": 0,
            "session_id": id, "errors": ["No conversation found to continue from that transcript file."]
        }));
        eprintln!("No conversation found.");
        return Err(ExitCode::from(1));
    }
    Ok(Some(id))
}

fn claude(args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "--version") {
        println!("2.1.310 (Claude Code)");
        return ExitCode::SUCCESS;
    }
    if args.first().map(String::as_str) == Some("auth") {
        print!("{AUTH_STATUS}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--no-session-persistence") {
        return claude_probe(args);
    }
    log_args(args);
    let resume = args.iter().any(|a| a == "--resume");
    if !resume && !args.iter().any(|a| a == "--session-id") {
        eprintln!("fake claude: no --session-id or --resume");
        return ExitCode::from(2);
    }
    let fork = match claude_fork(args) {
        Ok(fork) => fork,
        Err(code) => return code,
    };
    let mcp_config =
        args.windows(2).find(|w| w[0] == "--mcp-config").map(|w| w[1].clone()).unwrap_or_default();
    let script = match (resume, std::env::var("FAKE_CLI_RESUME_SCRIPT")) {
        (true, Ok(path)) => path,
        _ => std::env::var("FAKE_CLI_SCRIPT").unwrap_or_default(),
    };
    let lines = load(&script);
    // Bypass and Auto ask nothing but questions (spikes 1 and 2).
    let mode = args.windows(2).find(|w| w[0] == "--permission-mode").map(|w| w[1].as_str());
    let asks_questions_only = matches!(mode, Some("bypassPermissions" | "auto"));
    let mut input = Input::new();
    let mut tools: HashMap<String, (String, Value)> = HashMap::new();
    let mut asked: Vec<String> = Vec::new();
    let mut exit_code = 0u8;
    for (i, line) in lines.iter().enumerate() {
        if let Some(code) = line.get("_exit").and_then(Value::as_u64) {
            exit_code = u8::try_from(code).unwrap_or(1);
            continue;
        }
        if line.get("_signal").is_some() {
            continue;
        }
        if let Some(stdin) = line.get("_stdin") {
            let kind = stdin.get("type").and_then(Value::as_str).unwrap_or_default().to_owned();
            // An interrupt while a request is waiting: ask first, then wait
            // for the interrupt, then cancel the request as Claude does.
            let held = if kind == "control_request" {
                lines[i + 1..]
                    .iter()
                    .take_while(|l| l.get("_stdin").is_none())
                    .filter_map(needs_approval)
                    .find(|(id, rejected)| *rejected && !asked.contains(id))
                    .map(|(id, _)| id)
            } else {
                None
            };
            let bridge = held.as_ref().and_then(|id| {
                let tool = tools.get(id)?.clone();
                let mut bridge = Bridge::start(&mcp_config)?;
                bridge.call(&tool, id);
                asked.push(id.clone());
                Some(bridge)
            });
            if input.take(|v| v.get("type").and_then(Value::as_str) == Some(kind.as_str())).is_none() {
                return ExitCode::from(exit_code);
            }
            if let Some(mut bridge) = bridge {
                bridge.send(&json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":"AbortError: remote-cancel"}}));
                std::thread::sleep(Duration::from_millis(200));
                bridge.finish();
            }
            continue;
        }
        if line.get("type").and_then(Value::as_str) == Some("assistant") {
            for block in line.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                if block.get("type").and_then(Value::as_str) == Some("tool_use")
                    && let (Some(id), Some(name)) =
                        (block.get("id").and_then(Value::as_str), block.get("name").and_then(Value::as_str))
                {
                    tools.insert(
                        id.to_owned(),
                        (name.to_owned(), block.get("input").cloned().unwrap_or(json!({}))),
                    );
                }
            }
        }
        if let Some((id, false)) = needs_approval(line)
            && !asked.contains(&id)
            && let Some(tool) = tools.get(&id).cloned()
        {
            asked.push(id.clone());
            if asks_questions_only && tool.0 != "AskUserQuestion" {
                log_line(&format!("settled {}", tool.0));
            } else if let Some(mut bridge) = Bridge::start(&mcp_config) {
                bridge.call(&tool, &id);
                if let Some(text) = bridge.reply().as_ref().and_then(|r| r.pointer("/result/content/0/text"))
                {
                    log_line(&format!("bridge {}", text.as_str().unwrap_or_default()));
                }
                bridge.finish();
            }
        }
        match (&fork, line.get("session_id")) {
            (Some(id), Some(_)) => {
                let mut line = line.clone();
                line["session_id"] = json!(id);
                print(&line);
            }
            _ => print(line),
        }
    }
    input.wait_eof();
    ExitCode::from(exit_code)
}

// ---- Codex ----

/// What the host's probe asks outside any script, and its canned answer.
fn codex_probe_answer(method: &str, params: &Value) -> Option<Value> {
    Some(match method {
        "account/read" => {
            json!({"account": {"type": "chatgpt", "planType": "plus"}, "requiresOpenaiAuth": true})
        }
        "model/list" => json!({"data": [{
            "id": "gpt-5.5-codex", "model": "gpt-5.5-codex", "displayName": "GPT-5.5 Codex",
            "description": "For coding", "hidden": false, "isDefault": true,
            "supportedReasoningEfforts": [{"reasoningEffort": "low"}, {"reasoningEffort": "medium"}, {"reasoningEffort": "high"}],
            "defaultReasoningEffort": "medium", "inputModalities": ["text", "image"]
        }], "nextCursor": null}),
        "account/rateLimits/read" => json!({"rateLimits": {
            "primary": {"usedPercent": 41, "windowDurationMins": 300, "resetsAt": 1_795_040_000},
            "secondary": null
        }}),
        "skills/list" => {
            let cwd = params.pointer("/cwds/0").cloned().unwrap_or(Value::Null);
            // G7's own names and `compact` are the host's to leave out.
            let skill = |name: &str| {
                json!({"name": name, "description": "A skill", "shortDescription": "A skill",
                    "path": format!("/skills/{name}/SKILL.md"), "scope": "user", "enabled": true})
            };
            json!({"data": [{"cwd": cwd, "skills": [skill("release-notes"), skill("model"), skill("compact")],
                "errors": []}]})
        }
        _ => return None,
    })
}

/// Answers a probe request in place; `true` when it was one.
fn codex_probe_reply(msg: &Value) -> bool {
    let (Some(method), Some(id)) = (msg.get("method").and_then(Value::as_str), msg.get("id")) else {
        return false;
    };
    let Some(result) = codex_probe_answer(method, msg.get("params").unwrap_or(&Value::Null)) else {
        return false;
    };
    log_line(&format!("probe {method}"));
    print(&json!({"id": id, "result": result}));
    true
}

fn codex(args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "--version") {
        println!("codex-cli 0.170.0");
        return ExitCode::SUCCESS;
    }
    log_args(args);
    if args.first().map(String::as_str) != Some("app-server") {
        return ExitCode::from(2);
    }
    let entries = load(&std::env::var("FAKE_CLI_SCRIPT").unwrap_or_default());
    let mut input = Input::new();
    // The fixture's request ids → the ids the host used.
    let mut ids: HashMap<String, Value> = HashMap::new();
    // Told `never`: Some(false); told `auto_review`: Some(true).
    let mut settles: Option<bool> = None;
    // The capture's approval requests Codex settled itself, by id.
    let mut settled: Vec<Value> = Vec::new();
    let same = |a: &str, b: &str| a == b || (a.starts_with("thread/") && b.starts_with("thread/"));
    for entry in &entries {
        let msg = entry.get("msg").cloned().unwrap_or(Value::Null);
        match entry.get("dir").and_then(Value::as_str) {
            Some("send") => {
                if let Some(method) = msg.get("method").and_then(Value::as_str) {
                    // The host's probe asks things no script has: answer
                    // them and wait on.
                    let got = loop {
                        let Some(got) = input.take(|v| v.get("method").is_some()) else {
                            return ExitCode::SUCCESS;
                        };
                        let got_method = got.get("method").and_then(Value::as_str).unwrap_or_default();
                        if got_method != method && codex_probe_reply(&got) {
                            continue;
                        }
                        break got;
                    };
                    let got_method = got.get("method").and_then(Value::as_str).unwrap_or_default();
                    log_line(&format!("method {got_method}"));
                    if got_method == "thread/resume"
                        && let Some(id) = got.pointer("/params/threadId").and_then(Value::as_str)
                    {
                        log_line(&format!("resume {id}"));
                    }
                    if got_method == "turn/start"
                        && let Some(summary) = got.pointer("/params/summary").and_then(Value::as_str)
                    {
                        log_line(&format!("summary {summary}"));
                    }
                    if matches!(got_method, "thread/start" | "thread/resume" | "thread/fork") {
                        let field = |key: &str| {
                            got.pointer(&format!("/params/{key}"))
                                .and_then(Value::as_str)
                                .unwrap_or("-")
                                .to_owned()
                        };
                        let (policy, reviewer) = (field("approvalPolicy"), field("approvalsReviewer"));
                        log_line(&format!(
                            "permissions {got_method} {policy} {} {reviewer}",
                            field("sandbox")
                        ));
                        settles = match (policy.as_str(), reviewer.as_str()) {
                            ("never", _) => Some(false),
                            (_, "auto_review") => Some(true),
                            _ => None,
                        };
                    }
                    // thread/start and thread/resume answer alike: a script
                    // that started a thread also serves a resume.
                    if !same(method, got_method) {
                        eprintln!("fake codex: expected {method}, got {got_method}");
                        return ExitCode::from(3);
                    }
                    if let (Some(theirs), Some(ours)) = (msg.get("id"), got.get("id")) {
                        ids.insert(theirs.to_string(), ours.clone());
                    }
                } else if let Some(id) = msg.get("id") {
                    if settled.contains(id) {
                        continue;
                    }
                    let id = id.clone();
                    let Some(reply) = input.take(|v| v.get("method").is_none() && v.get("id") == Some(&id))
                    else {
                        return ExitCode::SUCCESS;
                    };
                    let decision =
                        reply.pointer("/result/decision").and_then(Value::as_str).unwrap_or("error");
                    log_line(&format!("decision {decision}"));
                }
            }
            Some("recv") => {
                let method = msg.get("method").and_then(Value::as_str).unwrap_or_default().to_owned();
                if let (Some(review), Some(id)) = (settles, msg.get("id"))
                    && method.ends_with("/requestApproval")
                {
                    settled.push(id.clone());
                    log_line(&format!("settled {method}"));
                    if review {
                        let params = msg.get("params").cloned().unwrap_or(Value::Null);
                        let target = params.get("itemId").cloned().unwrap_or(Value::Null);
                        for (what, status) in [("started", "inProgress"), ("completed", "approved")] {
                            print(&json!({"method": format!("item/autoApprovalReview/{what}"), "params": {
                                "threadId": params.get("threadId"), "turnId": params.get("turnId"),
                                "reviewId": "review-1", "targetItemId": target,
                                "review": {"status": status, "riskLevel": "low", "userAuthorization": "high",
                                    "rationale": "The user asked for exactly this."},
                                "action": {"type": "command", "source": "unifiedExec"}}}));
                        }
                    }
                    continue;
                }
                let mut msg = msg;
                if msg.get("method").is_none()
                    && let Some(id) = msg.get("id").map(Value::to_string)
                    && let Some(ours) = ids.get(&id)
                {
                    msg["id"] = ours.clone();
                }
                print(&msg);
            }
            _ => {}
        }
    }
    // The script is done; a probe may still be asking.
    while let Some(got) = input.take(|v| v.get("method").is_some() && v.get("id").is_some()) {
        codex_probe_reply(&got);
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: g7-agents-fake-cli claude|codex <args…>");
        return ExitCode::from(2);
    }
    let which = args.remove(0);
    match which.as_str() {
        "claude" => claude(&args),
        "codex" => codex(&args),
        _ => ExitCode::from(2),
    }
}
