// SPDX-License-Identifier: MPL-2.0
//! What each CLI says it offers (§Q4.1): models, effort levels, commands
//! and usage windows, read from the CLI itself and never from a vendor.
//!
//! Claude answers a `control_request` `initialize` on its control channel
//! before any message is sent; only its `models` and `commands` are read,
//! everything else in that answer (the account among it) is skipped by the
//! parser without being kept. Codex lists models with `model/list`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::api::{self, AgentId, Can, CommandKind, Limit, LimitState, ModelInfo};
use crate::host::agents::{AgentProcess, ClaudeAuth, OutLine, parse_codex_account};

/// The id of the probe's one control request.
pub const PROBE_REQUEST_ID: &str = "g7-init-1";
/// How long Claude has to answer `initialize`.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a Codex probe may take in all.
const CODEX_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// `model/list` pages followed at most.
const MAX_MODEL_PAGES: usize = 5;
/// A Codex window at or over this share is close to its limit.
const CODEX_WARN_PCT: u8 = 80;
/// The model Claude names for a reply it made itself (a slash command).
pub const SYNTHETIC_MODEL: &str = "<synthetic>";

// ---- Checks on what a client asks for (§Q7) ----

/// A model id that may be passed to a CLI as one argument: 1 to 64 bytes,
/// no whitespace or control character, not starting with `-`.
pub fn valid_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= api::MAX_MODEL_ID
        && !id.starts_with('-')
        && !id.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The fixed CLI word for an effort level; `None` for anything else, so a
/// client's string is never passed on.
pub fn effort_word(effort: &str) -> Option<&'static str> {
    const WORDS: [&str; 8] = ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"];
    WORDS.into_iter().find(|w| *w == effort)
}

/// What an agent can do (§Q4.1). Codex plans only when its probe could turn
/// on the experimental API; Codex has no MCP switch (its `config` override
/// leaves the user's servers loaded: smoke §Q12 #7).
pub fn can(agent: AgentId, plan_ok: bool) -> Vec<Can> {
    match agent {
        AgentId::Claude => {
            vec![Can::Plan, Can::Questions, Can::Steer, Can::Images, Can::McpSwitch, Can::Commands]
        }
        AgentId::Codex => {
            let mut can = vec![Can::Questions, Can::Steer, Can::Images, Can::Commands];
            if plan_ok {
                can.insert(0, Can::Plan);
            }
            can
        }
    }
}

// ---- Claude ----

/// What Claude's `initialize` answer gave.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeCatalog {
    pub models: Vec<ModelInfo>,
    /// The CLI's own list, unfiltered (F's `commands.rs` filters it).
    pub commands: Vec<api::Command>,
}

/// A stdout line of the probe, read only as far as the answer we wait for.
#[derive(Deserialize)]
struct ProbeLine {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    response: Option<ProbeResponse>,
}

#[derive(Deserialize)]
struct ProbeResponse {
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    response: Option<InitializeAnswer>,
}

/// Exactly the two fields we read. Every other key of the answer (the
/// account, the pid, …) is skipped by the parser without being parsed into
/// anything.
#[derive(Deserialize)]
struct InitializeAnswer {
    #[serde(default)]
    models: Option<Value>,
    #[serde(default)]
    commands: Option<Value>,
}

/// The control request the probe writes.
pub fn initialize_line() -> String {
    crate::host::claude::initialize_line(PROBE_REQUEST_ID)
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn note(text: Option<&str>, max: usize) -> Option<String> {
    text.map(|t| api::clean_cut(t.trim(), max).0).filter(|t| !t.is_empty())
}

/// One of Claude's models.
fn claude_model(entry: &Value) -> Option<ModelInfo> {
    let id = str_of(entry, "value")?;
    if !valid_model_id(id) {
        return None;
    }
    let efforts = if entry.get("supportsEffort").and_then(Value::as_bool) == Some(true) {
        entry
            .get("supportedEffortLevels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(effort_word)
            .map(str::to_owned)
            .collect()
    } else {
        Vec::new()
    };
    ModelInfo {
        id: id.to_owned(),
        name: str_of(entry, "displayName").unwrap_or(id).to_owned(),
        note: str_of(entry, "description").map(str::to_owned),
        default: id == "default",
        efforts,
        default_effort: None,
        images: true,
        short: !id.chars().any(|c| c.is_ascii_digit()),
        // Filled by `exact_names` once the whole list is read.
        exact: None,
    }
    .sanitized()
}

/// Each model's exact name (§3.2 of the new-thread contract): an alias's is
/// the display name of the entry its `resolvedModel` names, else a name made
/// from that id; a full id's is its own display name. A description is free
/// text and never read for a version.
fn exact_names(models: &mut [ModelInfo], resolved: &[Option<String>]) {
    let names: HashMap<String, String> =
        models.iter().filter(|m| !m.short && !m.default).map(|m| (m.id.clone(), m.name.clone())).collect();
    for (model, resolved) in models.iter_mut().zip(resolved) {
        model.exact = match resolved {
            Some(id) => {
                Some(names.get(id).cloned().unwrap_or_else(|| name_from_id(id).unwrap_or_else(|| id.clone())))
            }
            None if !model.short && !model.default => Some(model.name.clone()),
            None => None,
        };
    }
}

/// "Sonnet 4.6" from `claude-sonnet-4-6` (or `claude-sonnet-4-6-20260101`):
/// lowercase letters, one or two numbers, an optional eight-digit date.
/// `None` for any other shape.
fn name_from_id(id: &str) -> Option<String> {
    let mut parts = id.strip_prefix("claude-")?.split('-');
    let family = parts.next().filter(|f| !f.is_empty() && f.chars().all(|c| c.is_ascii_lowercase()))?;
    let rest: Vec<&str> = parts.collect();
    let rest = match rest.last() {
        Some(date) if date.len() == 8 && date.chars().all(|c| c.is_ascii_digit()) => &rest[..rest.len() - 1],
        _ => &rest[..],
    };
    let numbers = (1..=2).contains(&rest.len())
        && rest.iter().all(|n| (1..=2).contains(&n.len()) && n.chars().all(|c| c.is_ascii_digit()));
    if !numbers {
        return None;
    }
    let mut chars = family.chars();
    let first = chars.next()?.to_ascii_uppercase();
    Some(format!("{first}{} {}", chars.as_str(), rest.join(".")))
}

/// One of Claude's slash commands; `None` without a usable name.
fn claude_command(entry: &Value) -> Option<api::Command> {
    let name = str_of(entry, "name")?.trim().trim_start_matches('/');
    if name.is_empty()
        || name.len() > api::MAX_COMMAND_NAME
        || name.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    Some(api::Command {
        name: name.to_owned(),
        note: note(str_of(entry, "description"), api::MAX_COMMAND_NOTE),
        hint: note(str_of(entry, "argumentHint"), api::MAX_COMMAND_HINT),
        kind: CommandKind::Command,
    })
}

/// Reads the probe's answer from one stdout line: `Some` when the line is
/// the `control_response` to [`PROBE_REQUEST_ID`] (empty lists when it
/// failed or had an unexpected shape).
pub fn parse_claude_answer(line: &[u8]) -> Option<ClaudeCatalog> {
    let parsed: ProbeLine = serde_json::from_slice(line).ok()?;
    if parsed.kind.as_deref() != Some("control_response") {
        return None;
    }
    let response = parsed.response?;
    if response.request_id.as_deref() != Some(PROBE_REQUEST_ID) {
        return None;
    }
    let Some(answer) = response.response.filter(|_| response.subtype.as_deref() == Some("success")) else {
        return Some(ClaudeCatalog::default());
    };
    let mut models: Vec<ModelInfo> = Vec::new();
    let mut resolved: Vec<Option<String>> = Vec::new();
    let entries = |list: &Option<Value>| list.as_ref().and_then(Value::as_array).cloned().unwrap_or_default();
    for entry in entries(&answer.models) {
        if models.len() >= api::MAX_MODELS {
            break;
        }
        if let Some(model) = claude_model(&entry)
            && !models.iter().any(|m| m.id == model.id)
        {
            resolved.push(str_of(&entry, "resolvedModel").filter(|r| valid_model_id(r)).map(str::to_owned));
            models.push(model);
        }
    }
    exact_names(&mut models, &resolved);
    let mut commands: Vec<api::Command> = Vec::new();
    for command in entries(&answer.commands) {
        if commands.len() >= api::MAX_COMMANDS {
            break;
        }
        if let Some(command) = claude_command(&command)
            && !commands.iter().any(|c| c.name == command.name)
        {
            commands.push(command);
        }
    }
    Some(ClaudeCatalog { models, commands })
}

/// The probe's arguments: no session kept, none of the user's MCP servers
/// (an empty config), and nothing that would make a model call.
pub fn claude_probe_args(empty_mcp_config: &Path) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
        "--strict-mcp-config",
        "--mcp-config",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    args.push(empty_mcp_config.to_string_lossy().into_owned());
    args
}

/// Asks Claude for its models and commands (§Q4.1): one `initialize`
/// control request and no message, so no model is called. `dir` is the
/// host's own directory: the probe runs there and its empty MCP config is
/// written there for the run. `None` when the CLI didn't answer.
pub async fn probe_claude(
    bin: &Path,
    child_env: &[(OsString, OsString)],
    dir: &Path,
) -> Option<ClaudeCatalog> {
    let config = dir.join(format!("probe-mcp-{}-{:08x}.json", std::process::id(), rand::random::<u32>()));
    write_private(&config, br#"{"mcpServers":{}}"#).ok()?;
    let spawned = AgentProcess::spawn(bin, &claude_probe_args(&config), dir, child_env);
    let catalog = match spawned {
        Ok((mut process, mut lines)) => {
            let catalog = if process.write_line(&initialize_line()).await {
                tokio::time::timeout(ANSWER_TIMEOUT, claude_answer(&mut lines)).await.ok().flatten()
            } else {
                None
            };
            // No `user` line is ever written: closing stdin ends it.
            process.finish().await;
            catalog
        }
        Err(e) => {
            tracing::debug!("couldn't start the Claude probe: {}", e.kind());
            None
        }
    };
    let _ = std::fs::remove_file(&config);
    catalog
}

async fn claude_answer(lines: &mut mpsc::Receiver<OutLine>) -> Option<ClaudeCatalog> {
    while let Some(line) = lines.recv().await {
        match line {
            OutLine::Line(bytes) => {
                if let Some(catalog) = parse_claude_answer(&bytes) {
                    return Some(catalog);
                }
            }
            OutLine::TooLong => return None,
        }
    }
    None
}

/// Writes `bytes` to a new file only this user can read.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::remove_file(path);
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Claude's usage windows from a `rate_limit_event`'s `rate_limit_info`
/// (§Q4.8): one per `unifiedWindows` entry, else one from `rateLimitType`.
/// Nothing about overage or the account is read.
pub fn claude_limits(info: &Value) -> Vec<Limit> {
    let top_status = str_of(info, "status");
    let mut limits = Vec::new();
    if let Some(windows) = info.get("unifiedWindows").and_then(Value::as_object) {
        for (name, window) in windows.iter().take(api::MAX_LIMITS) {
            let status = str_of(window, "status").or(top_status);
            let warned = window.get("surpassedThreshold").is_some_and(|t| !t.is_null());
            // Where the window has no utilization of its own, the top level's
            // is for the window `rateLimitType` names.
            let top = (str_of(info, "rateLimitType") == Some(name.as_str())).then_some(info);
            let utilization = window.get("utilization").or_else(|| top.and_then(|t| t.get("utilization")));
            let resets = window.get("resetsAt").or_else(|| top.and_then(|t| t.get("resetsAt")));
            if let Some(limit) = claude_limit(name, utilization, resets, status, warned) {
                limits.push(limit);
            }
        }
    }
    if limits.is_empty()
        && let Some(name) = str_of(info, "rateLimitType")
    {
        let warned = info.get("surpassedThreshold").is_some_and(|t| !t.is_null());
        limits.extend(claude_limit(name, info.get("utilization"), info.get("resetsAt"), top_status, warned));
    }
    limits
}

fn claude_limit(
    window: &str,
    utilization: Option<&Value>,
    resets: Option<&Value>,
    status: Option<&str>,
    warned: bool,
) -> Option<Limit> {
    let window = api::clean_cut(window.trim(), api::MAX_LIMIT_WINDOW).0;
    if window.is_empty() {
        return None;
    }
    let used_pct = utilization.and_then(Value::as_f64).map(|u| percent(u * 100.0));
    let status = status.unwrap_or_default().to_ascii_lowercase();
    let state = if status.contains("rejected") {
        LimitState::Reached
    } else if status.contains("warning") || warned {
        LimitState::Warn
    } else {
        LimitState::Ok
    };
    Some(Limit { window, used_pct, resets_ms: resets.and_then(seconds_to_ms), state })
}

/// A share as a whole percent, 0 to 100.
fn percent(value: f64) -> u8 {
    if value.is_nan() {
        return 0;
    }
    // In range after the clamp, so the cast is exact.
    value.round().clamp(0.0, 100.0) as u8
}

/// A Unix time in seconds (or already in milliseconds) as milliseconds.
fn seconds_to_ms(value: &Value) -> Option<u64> {
    let t =
        value.as_u64().or_else(|| value.as_f64().filter(|f| *f >= 0.0 && f.is_finite()).map(|f| f as u64))?;
    Some(if t < 100_000_000_000 { t.saturating_mul(1000) } else { t })
}

// ---- Codex ----

/// What a Codex probe learned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexProbe {
    /// `account/read`, reduced to what we keep; `None` when it didn't answer.
    pub auth: Option<ClaudeAuth>,
    pub models: Vec<ModelInfo>,
    pub limits: Vec<Limit>,
    /// `initialize` with `experimentalApi` was accepted (plan mode).
    pub plan_ok: bool,
}

/// One of Codex's models, unless it is hidden.
fn codex_model(entry: &Value) -> Option<ModelInfo> {
    if entry.get("hidden").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let id = str_of(entry, "id").or_else(|| str_of(entry, "model"))?;
    if !valid_model_id(id) {
        return None;
    }
    let efforts = entry
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| str_of(e, "reasoningEffort"))
        .filter_map(effort_word)
        .map(str::to_owned)
        .collect();
    let images = entry
        .get("inputModalities")
        .and_then(Value::as_array)
        .is_none_or(|m| m.iter().any(|m| m.as_str() == Some("image")));
    ModelInfo {
        id: id.to_owned(),
        name: str_of(entry, "displayName").unwrap_or(id).to_owned(),
        note: str_of(entry, "description").map(str::to_owned),
        default: entry.get("isDefault").and_then(Value::as_bool) == Some(true),
        efforts,
        default_effort: str_of(entry, "defaultReasoningEffort").and_then(effort_word).map(str::to_owned),
        images,
        // Set by `add_codex_models` once the order is known.
        short: false,
        // Codex names each model exactly ("GPT-5.5 Codex").
        exact: Some(str_of(entry, "displayName").unwrap_or(id).to_owned()),
    }
    .sanitized()
}

/// The first three models that aren't the default are offered inline, as
/// Claude's aliases are.
fn mark_short(models: &mut [ModelInfo]) {
    let mut left = 3;
    for model in models.iter_mut() {
        model.short = !model.default && left > 0;
        if model.short {
            left -= 1;
        }
    }
}

/// Adds the models of one `model/list` page; its `nextCursor`.
pub fn add_codex_models(models: &mut Vec<ModelInfo>, page: &Value) -> Option<String> {
    let mut full = false;
    for entry in page.get("data").and_then(Value::as_array).into_iter().flatten() {
        if models.len() >= api::MAX_MODELS {
            full = true;
            break;
        }
        if let Some(model) = codex_model(entry)
            && !models.iter().any(|m| m.id == model.id)
        {
            models.push(model);
        }
    }
    mark_short(models);
    if full {
        return None;
    }
    str_of(page, "nextCursor").filter(|c| !c.is_empty()).map(str::to_owned)
}

/// Codex's usage windows from a `rateLimits` snapshot (§Q4.8). Only the
/// windows are read; credits, plan and account fields are not.
pub fn codex_limits(snapshot: &Value) -> Vec<Limit> {
    let reached = snapshot.get("rateLimitReachedType").is_some_and(|t| !t.is_null());
    let windows: Vec<(&str, &Value)> = ["primary", "secondary"]
        .into_iter()
        .filter_map(|key| snapshot.get(key).filter(|w| w.is_object()).map(|w| (key, w)))
        .collect();
    let full = |w: &Value| w.get("usedPercent").and_then(Value::as_f64).is_some_and(|p| p >= 100.0);
    let any_full = windows.iter().any(|(_, w)| full(w));
    windows
        .into_iter()
        .map(|(key, w)| {
            let used_pct = w.get("usedPercent").and_then(Value::as_f64).map(percent);
            let window = match w.get("windowDurationMins").and_then(Value::as_u64) {
                Some(mins) => mins.to_string(),
                None => key.to_owned(),
            };
            let state = if full(w) || (reached && !any_full) {
                LimitState::Reached
            } else if used_pct.is_some_and(|p| p >= CODEX_WARN_PCT) {
                LimitState::Warn
            } else {
                LimitState::Ok
            };
            Limit { window, used_pct, resets_ms: w.get("resetsAt").and_then(seconds_to_ms), state }
        })
        .collect()
}

/// Merges a sparse `account/rateLimits/updated` into what is known.
pub fn merge_limits(known: &mut Vec<Limit>, update: Vec<Limit>) {
    for limit in update {
        match known.iter().position(|l| l.window == limit.window) {
            Some(i) => known[i] = limit,
            None if known.len() < api::MAX_LIMITS => known.push(limit),
            None => {}
        }
    }
}

/// One of Codex's skills: its `commands.list` entry, and the path of its
/// file, which Codex's `{"type": "skill"}` input names. The path stays in
/// the host (never in a client's list, never sent to a phone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexSkill {
    pub command: api::Command,
    pub path: String,
}

/// Codex's enabled skills for a folder from `skills/list` (§Q4.7), with
/// their paths. A skill without a usable name or an absolute path is left out.
pub fn codex_skill_entries_of(result: &Value) -> Vec<CodexSkill> {
    let mut skills_out: Vec<CodexSkill> = Vec::new();
    let skills = result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("skills").and_then(Value::as_array))
        .flatten();
    for skill in skills {
        if skills_out.len() >= api::MAX_COMMANDS {
            break;
        }
        if skill.get("enabled").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let Some(name) = str_of(skill, "name").map(str::trim) else { continue };
        if name.is_empty()
            || name.len() > api::MAX_COMMAND_NAME
            || name.chars().any(|c| c.is_whitespace() || c.is_control())
            || skills_out.iter().any(|s| s.command.name == name)
        {
            continue;
        }
        let Some(path) = str_of(skill, "path").filter(|p| {
            Path::new(p).is_absolute() && p.len() <= api::MAX_PATH && !p.chars().any(char::is_control)
        }) else {
            continue;
        };
        let text = str_of(skill, "shortDescription")
            .filter(|t| !t.trim().is_empty())
            .or(str_of(skill, "description"));
        skills_out.push(CodexSkill {
            command: api::Command {
                name: name.to_owned(),
                note: note(text, api::MAX_COMMAND_NOTE),
                hint: None,
                kind: CommandKind::Skill,
            },
            path: path.to_owned(),
        });
    }
    skills_out
}

/// [`codex_skill_entries_of`] without the paths: what a client's list shows.
pub fn codex_skills_of(result: &Value) -> Vec<api::Command> {
    codex_skill_entries_of(result).into_iter().map(|s| s.command).collect()
}

/// Codex's own command G7 offers besides its skills.
pub const CODEX_COMPACT: &str = "compact";

/// A short-lived `codex app-server` conversation: requests in order, one at a time.
struct CodexConversation {
    process: AgentProcess,
    lines: mpsc::Receiver<OutLine>,
    next_id: u64,
}

impl CodexConversation {
    fn start(bin: &Path, child_env: &[(OsString, OsString)]) -> Option<Self> {
        let cwd = crate::host::env::lookup(child_env, "HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| PathBuf::from("/"));
        match AgentProcess::spawn(bin, &["app-server".to_owned()], &cwd, child_env) {
            Ok((process, lines)) => Some(Self { process, lines, next_id: 1 }),
            Err(e) => {
                tracing::debug!("couldn't start a Codex probe: {}", e.kind());
                None
            }
        }
    }

    /// Sends a request and waits for its response; server requests are
    /// refused and notifications skipped meanwhile. `Err` with the error
    /// object when Codex refused it; `None` when the process went away.
    async fn call(&mut self, method: &str, params: Value) -> Option<Result<Value, Value>> {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        if !self.process.write_line(&line).await {
            return None;
        }
        while let Some(line) = self.lines.recv().await {
            let OutLine::Line(bytes) = line else { return None };
            let Ok(msg) = serde_json::from_slice::<Value>(&bytes) else { continue };
            let msg_id = msg.get("id").filter(|i| !i.is_null());
            match (msg.get("method").is_some(), msg_id) {
                (true, Some(request)) => {
                    let refusal = json!({"jsonrpc": "2.0", "id": request, "error": {"code": -32601, "message": "not supported"}});
                    self.process.write_line(&refusal.to_string()).await;
                }
                (false, Some(got)) if got.as_u64() == Some(id) => {
                    return Some(match msg.get("error") {
                        Some(error) => Err(error.clone()),
                        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                    });
                }
                _ => {}
            }
        }
        None
    }

    async fn notify(&mut self, method: &str) {
        self.process.write_line(&json!({"jsonrpc": "2.0", "method": method}).to_string()).await;
    }

    /// `initialize` with `experimentalApi`, else without; whether the first was accepted.
    async fn initialize(&mut self) -> Option<bool> {
        let params = |experimental: bool| {
            crate::host::codex::initialize_request_with(0, experimental)["params"].clone()
        };
        let plan_ok = match self.call("initialize", params(true)).await? {
            Ok(_) => true,
            Err(_) => {
                self.call("initialize", params(false)).await?.ok()?;
                false
            }
        };
        self.notify("initialized").await;
        Some(plan_ok)
    }
}

/// Probes Codex (§Q4.1) on one short-lived app-server: `initialize`,
/// `account/read`, `model/list` (up to five pages), `account/rateLimits/read`.
/// `None` when it didn't get as far as `initialize`.
pub async fn probe_codex(bin: &Path, child_env: &[(OsString, OsString)]) -> Option<CodexProbe> {
    let mut conversation = CodexConversation::start(bin, child_env)?;
    let probe =
        tokio::time::timeout(CODEX_PROBE_TIMEOUT, codex_probe_steps(&mut conversation)).await.ok().flatten();
    conversation.process.finish().await;
    probe
}

async fn codex_probe_steps(c: &mut CodexConversation) -> Option<CodexProbe> {
    let plan_ok = c.initialize().await?;
    let auth = match c.call("account/read", json!({"refreshToken": false})).await {
        Some(Ok(result)) => Some(parse_codex_account(&result)),
        _ => None,
    };
    let mut models = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_MODEL_PAGES {
        let params = match &cursor {
            Some(cursor) => json!({"cursor": cursor}),
            None => json!({}),
        };
        let Some(Ok(page)) = c.call("model/list", params).await else { break };
        cursor = add_codex_models(&mut models, &page);
        if cursor.is_none() {
            break;
        }
    }
    let limits = match c.call("account/rateLimits/read", Value::Null).await {
        Some(Ok(result)) => result.get("rateLimits").map(codex_limits).unwrap_or_default(),
        _ => Vec::new(),
    };
    Some(CodexProbe { auth, models, limits, plan_ok })
}

/// Codex's enabled skills for `folder` (§Q4.7), with their paths, from a
/// short-lived app-server.
pub async fn codex_skill_entries(
    bin: &Path,
    child_env: &[(OsString, OsString)],
    folder: &Path,
) -> Vec<CodexSkill> {
    let Some(mut conversation) = CodexConversation::start(bin, child_env) else { return Vec::new() };
    let folder = folder.to_string_lossy().into_owned();
    let work = async {
        conversation.initialize().await?;
        match conversation.call("skills/list", json!({"cwds": [folder]})).await? {
            Ok(result) => Some(codex_skill_entries_of(&result)),
            Err(_) => None,
        }
    };
    let skills = tokio::time::timeout(CODEX_PROBE_TIMEOUT, work).await.ok().flatten().unwrap_or_default();
    conversation.process.finish().await;
    skills
}

/// [`codex_skill_entries`] without the paths.
pub async fn codex_skills(
    bin: &Path,
    child_env: &[(OsString, OsString)],
    folder: &Path,
) -> Vec<api::Command> {
    codex_skill_entries(bin, child_env, folder).await.into_iter().map(|s| s.command).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn model_ids_and_efforts_a_client_may_name() {
        for ok in ["opus", "claude-opus-5", "gpt-5.1-codex", "a"] {
            assert!(valid_model_id(ok), "{ok}");
        }
        for bad in ["", "-p", "--model", "a b", "a\tb", "a\u{7}", "x\u{2028}y", &"m".repeat(65)] {
            assert!(!valid_model_id(bad), "{bad:?}");
        }
        assert!(valid_model_id(&"m".repeat(64)));
        assert_eq!(effort_word("xhigh"), Some("xhigh"));
        assert_eq!(effort_word("ultra"), Some("ultra"));
        for bad in ["", "High", "high ", "--effort", "extreme"] {
            assert_eq!(effort_word(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn what_each_agent_can_do() {
        assert_eq!(can(AgentId::Claude, false).len(), 6);
        assert!(can(AgentId::Claude, false).contains(&Can::McpSwitch));
        assert!(!can(AgentId::Codex, true).contains(&Can::McpSwitch));
        assert!(can(AgentId::Codex, true).contains(&Can::Plan));
        assert!(!can(AgentId::Codex, false).contains(&Can::Plan));
        assert!(can(AgentId::Codex, false).contains(&Can::Steer));
    }

    #[test]
    fn claudes_answer_gives_models_and_commands_only() {
        let text = fixture("claude-10-initialize.jsonl");
        let mut found = None;
        for line in text.lines() {
            if let Some(catalog) = parse_claude_answer(line.as_bytes()) {
                found = Some(catalog);
                break;
            }
        }
        let catalog = found.expect("the answer");
        let ids: Vec<&str> = catalog.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["default", "opus", "sonnet", "haiku", "claude-opus-5", "claude-sonnet-4-6"]);
        let default = &catalog.models[0];
        assert!(default.default && default.short && default.images);
        assert_eq!(default.name, "Default (recommended)");
        assert_eq!(default.efforts, ["low", "medium", "high", "xhigh", "max"]);
        let full = catalog.models.iter().find(|m| m.id == "claude-opus-5").unwrap();
        assert!(!full.short && !full.default);
        // No effort support: no levels, even when some are listed.
        assert!(catalog.models.iter().find(|m| m.id == "haiku").unwrap().efforts.is_empty());
        // A level we don't know is dropped, not passed on.
        assert_eq!(catalog.models.iter().find(|m| m.id == "sonnet").unwrap().efforts, ["low", "high"]);
        // Exact names: an alias's from the entry its `resolvedModel` names,
        // else from that id's shape; a full id's is its own.
        let exact: Vec<Option<&str>> = catalog.models.iter().map(|m| m.exact.as_deref()).collect();
        assert_eq!(
            exact,
            [
                Some("Opus 5"),
                Some("Opus 5"),
                Some("Sonnet 5"),
                Some("Haiku 5"),
                Some("Opus 5"),
                Some("Sonnet 4.6")
            ]
        );
        let names: Vec<&str> = catalog.commands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["compact", "context", "review", "login"]);
        assert_eq!(catalog.commands[2].hint.as_deref(), Some("[pr-number]"));
        assert!(catalog.commands.iter().all(|c| c.kind == CommandKind::Command));
        // The account in the fixture has a wrong type throughout; it is
        // never read, so nothing of it is anywhere in what we keep.
        let kept = format!("{catalog:?}");
        for secret in ["someone@example.org", "Example Org", "4242"] {
            assert!(!kept.contains(secret), "{secret}");
        }
    }

    #[test]
    fn other_lines_and_failed_answers() {
        assert_eq!(parse_claude_answer(br#"{"type":"system","subtype":"hook_started"}"#), None);
        assert_eq!(
            parse_claude_answer(
                br#"{"type":"control_response","response":{"request_id":"other","subtype":"success"}}"#
            ),
            None
        );
        assert_eq!(parse_claude_answer(b"not json"), None);
        let failed = br#"{"type":"control_response","response":{"request_id":"g7-init-1","subtype":"error","error":"no"}}"#;
        assert_eq!(parse_claude_answer(failed), Some(ClaudeCatalog::default()));
        // An unexpected shape: an empty catalog, not a failure to answer.
        let odd = br#"{"type":"control_response","response":{"request_id":"g7-init-1","subtype":"success","response":{"models":"many","account":7}}}"#;
        assert_eq!(parse_claude_answer(odd), Some(ClaudeCatalog::default()));
        let mixed = br#"{"type":"control_response","response":{"request_id":"g7-init-1","subtype":"success","response":{"models":[3,{"value":"opus"},{"value":"-x"},{"value":"opus"}],"commands":[{"name":"/a b"},{"name":"/ok"}]}}}"#;
        let catalog = parse_claude_answer(mixed).unwrap();
        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.commands.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["ok"]);
    }

    #[test]
    fn the_probe_runs_without_the_users_servers_or_a_session() {
        let args = claude_probe_args(Path::new("/run/g7/probe-mcp.json"));
        for flag in ["--no-session-persistence", "--strict-mcp-config", "--input-format", "--output-format"] {
            assert!(args.iter().any(|a| a == flag), "{flag}");
        }
        assert!(args.windows(2).any(|w| w == ["--mcp-config", "/run/g7/probe-mcp.json"]));
        for never in ["--bare", "--session-id", "--resume", "--model", "--permission-prompt-tool"] {
            assert!(!args.iter().any(|a| a == never), "{never}");
        }
        let line: Value = serde_json::from_str(&initialize_line()).unwrap();
        assert_eq!(line["request"]["subtype"], "initialize");
        assert_eq!(line["request_id"], PROBE_REQUEST_ID);
    }

    /// A stand-in for `claude` that answers the probe from a fixture.
    #[tokio::test]
    async fn the_probe_against_a_stand_in() {
        let dir = tempfile::tempdir().unwrap();
        let answer = fixture("claude-10-initialize.jsonl");
        let answer_path = dir.path().join("answer.jsonl");
        std::fs::write(&answer_path, answer).unwrap();
        let log = dir.path().join("stdin.log");
        let script = dir.path().join("claude");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nIFS= read -r line\nprintf '%s\\n' \"$line\" > '{}'\ncat '{}'\ncat > /dev/null\n",
                log.display(),
                answer_path.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let env = vec![(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))];
        let catalog = probe_claude(&script, &env, dir.path()).await.unwrap();
        assert_eq!(catalog.models.len(), 6);
        let written: Value = serde_json::from_str(std::fs::read_to_string(&log).unwrap().trim()).unwrap();
        assert_eq!(written["request"]["subtype"], "initialize");
        // The empty config is gone after the run.
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("probe-mcp"))
            .collect();
        assert!(left.is_empty());
        // A CLI that never answers: no catalog, and the probe still ends.
        let silent = dir.path().join("silent");
        std::fs::write(&silent, "#!/bin/sh\ncat > /dev/null\n").unwrap();
        std::fs::set_permissions(&silent, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        assert_eq!(probe_claude(&silent, &env, dir.path()).await, None);
    }

    #[test]
    fn claude_limits_from_the_capture_and_the_spike_shape() {
        let text = fixture("claude-02-followup-approve.jsonl");
        let event: Value = text
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find(|l| l["type"] == "rate_limit_event")
            .unwrap();
        let limits = claude_limits(&event["rate_limit_info"]);
        assert_eq!(
            limits,
            [Limit {
                window: "seven_day".into(),
                used_pct: Some(91),
                resets_ms: Some(1_795_057_200_000),
                state: LimitState::Warn
            }]
        );
        let spike = json!({"status": "allowed", "resetsAt": 1795050000u64, "rateLimitType": "five_hour",
            "unifiedWindows": {"five_hour": {"utilization": 0.41, "resetsAt": 1795050000u64, "status": "allowed"},
                               "seven_day": {"resetsAt": 1795500000u64, "status": "allowed"}}});
        let limits = claude_limits(&spike);
        assert_eq!(limits.len(), 2);
        assert_eq!(
            (limits[0].window.as_str(), limits[0].used_pct, limits[0].state),
            ("five_hour", Some(41), LimitState::Ok)
        );
        assert_eq!((limits[1].used_pct, limits[1].resets_ms), (None, Some(1_795_500_000_000)));
        // Only the top level: one window from `rateLimitType`.
        let rejected = json!({"status": "rejected", "resetsAt": 1795050000u64, "rateLimitType": "five_hour", "utilization": 1.0});
        assert_eq!(
            claude_limits(&rejected),
            [Limit {
                window: "five_hour".into(),
                used_pct: Some(100),
                resets_ms: Some(1_795_050_000_000),
                state: LimitState::Reached
            }]
        );
        // A status word we haven't seen is ok; nothing without a window name.
        assert_eq!(
            claude_limits(&json!({"status": "allowed_soon", "rateLimitType": "x"}))[0].state,
            LimitState::Ok
        );
        assert!(claude_limits(&json!({"status": "rejected"})).is_empty());
    }

    #[test]
    fn exact_names_come_from_resolved_ids_never_descriptions() {
        assert_eq!(name_from_id("claude-opus-5").as_deref(), Some("Opus 5"));
        assert_eq!(name_from_id("claude-sonnet-4-6").as_deref(), Some("Sonnet 4.6"));
        assert_eq!(name_from_id("claude-sonnet-4-6-20260101").as_deref(), Some("Sonnet 4.6"));
        assert_eq!(name_from_id("claude-fable-5-2").as_deref(), Some("Fable 5.2"));
        for odd in
            ["claude-opus", "claude-Opus-5", "claude-opus-5-6-7", "claude-opus-123", "gpt-5", "claude--5", ""]
        {
            assert_eq!(name_from_id(odd), None, "{odd}");
        }
        let model = |id: &str| ModelInfo {
            id: id.into(),
            name: id.into(),
            note: Some("Opus 4.9 for most work".into()),
            default: id == "default",
            efforts: Vec::new(),
            default_effort: None,
            images: true,
            short: !id.chars().any(|c| c.is_ascii_digit()),
            exact: None,
        };
        let mut models = vec![model("default"), model("opus"), model("odd"), model("claude-x-1")];
        exact_names(&mut models, &[None, Some("claude-opus-9".into()), Some("weird-id".into()), None]);
        let exact: Vec<Option<&str>> = models.iter().map(|m| m.exact.as_deref()).collect();
        // No resolvedModel: nothing guessed, the description is never read.
        assert_eq!(exact, [None, Some("Opus 9"), Some("weird-id"), Some("claude-x-1")]);
    }

    #[test]
    fn codex_models_pages_and_limits() {
        let text = fixture("codex-10-model-list.jsonl");
        let pages: Vec<Value> = text
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|e| e["dir"] == "recv" && e["msg"]["result"]["data"].is_array())
            .map(|e| e["msg"]["result"].clone())
            .collect();
        assert_eq!(pages.len(), 2);
        let mut models = Vec::new();
        assert_eq!(add_codex_models(&mut models, &pages[0]).as_deref(), Some("page-2"));
        assert_eq!(add_codex_models(&mut models, &pages[1]), None);
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gpt-5.5-codex", "gpt-5.5", "gpt-5.4-mini"], "the hidden one is left out");
        let first = &models[0];
        assert!(first.default && first.images && !first.short);
        assert_eq!(first.efforts, ["low", "medium", "high", "xhigh", "max", "ultra"]);
        assert_eq!(first.default_effort.as_deref(), Some("medium"));
        assert!(!models[2].images, "text only");
        assert!(models.iter().all(|m| m.exact.as_deref() == Some(m.name.as_str())), "Codex names are exact");
        let short: Vec<bool> = models.iter().map(|m| m.short).collect();
        assert_eq!(short, [false, true, true], "the ones after the default are offered inline");
        let mut many: Vec<ModelInfo> =
            (0..6).map(|n| ModelInfo { id: format!("m{n}"), default: n == 1, ..models[1].clone() }).collect();
        mark_short(&mut many);
        let short: Vec<bool> = many.iter().map(|m| m.short).collect();
        assert_eq!(short, [true, false, true, true, false, false], "three at most");

        let snapshot = json!({"primary": {"usedPercent": 41, "windowDurationMins": 300, "resetsAt": 1795055175},
            "secondary": {"usedPercent": 85, "windowDurationMins": 10080, "resetsAt": 1795113266},
            "planType": "plus", "credits": {"hasCredits": true}, "rateLimitReachedType": null});
        let limits = codex_limits(&snapshot);
        assert_eq!(
            limits,
            [
                Limit {
                    window: "300".into(),
                    used_pct: Some(41),
                    resets_ms: Some(1_795_055_175_000),
                    state: LimitState::Ok
                },
                Limit {
                    window: "10080".into(),
                    used_pct: Some(85),
                    resets_ms: Some(1_795_113_266_000),
                    state: LimitState::Warn
                },
            ]
        );
        let reached = json!({"primary": {"usedPercent": 100, "windowDurationMins": 300}, "secondary": {"usedPercent": 20},
            "rateLimitReachedType": "rate_limit_reached"});
        let states: Vec<LimitState> = codex_limits(&reached).into_iter().map(|l| l.state).collect();
        assert_eq!(states, [LimitState::Reached, LimitState::Ok]);
        assert_eq!(codex_limits(&reached)[1].window, "secondary");

        let mut known = limits;
        merge_limits(
            &mut known,
            codex_limits(&json!({"primary": {"usedPercent": 50, "windowDurationMins": 300}})),
        );
        assert_eq!(known.len(), 2);
        assert_eq!(known[0].used_pct, Some(50));
    }

    #[test]
    fn codex_skills_enabled_only() {
        let result = json!({"data": [{"cwd": "/w", "errors": [], "skills": [
            {"name": "review", "description": "Long text", "shortDescription": "Review a change", "enabled": true, "path": "/s/r", "scope": "user"},
            {"name": "off", "description": "x", "enabled": false, "path": "/s/o", "scope": "user"},
            {"name": "two words", "description": "x", "enabled": true, "path": "/s/t", "scope": "repo"},
            {"name": "plain", "description": "Plain one", "enabled": true, "path": "/s/p", "scope": "repo"}]}]});
        let entries = codex_skill_entries_of(&result);
        let paths: Vec<&str> = entries.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, ["/s/r", "/s/p"]);
        let no_path = json!({"data": [{"skills": [{"name": "x", "description": "d", "enabled": true, "path": "rel/x"}]}]});
        assert!(codex_skill_entries_of(&no_path).is_empty(), "a relative path is no path");
        let skills = codex_skills_of(&result);
        let got: Vec<(&str, Option<&str>)> =
            skills.iter().map(|s| (s.name.as_str(), s.note.as_deref())).collect();
        assert_eq!(got, [("review", Some("Review a change")), ("plain", Some("Plain one"))]);
        assert!(skills.iter().all(|s| s.kind == CommandKind::Skill));
    }
}
