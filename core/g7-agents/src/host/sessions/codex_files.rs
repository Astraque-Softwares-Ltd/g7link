// SPDX-License-Identifier: MPL-2.0
//! Codex rollout lines (`~/.codex/sessions/YYYY/MM/DD/rollout-…-<uuid>.jsonl`)
//! to items, the first line's session facts, and thread names from
//! `session_index.jsonl`.
//!
//! A rollout holds finished items only, so a Codex session never shows a
//! running tool step; open calls and turn markers give the state guess.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::api::{self, ApprovalKind, Item, ItemKind, SessionSource, SessionState, ToolStatus};
use crate::host::agents::{ToolSpec, display_path, prefixed_lines};
use crate::host::sessions::paths::{is_uuid, parse_rfc3339_ms};
use crate::host::sessions::tail::Line;
use crate::host::sessions::{MAX_OPEN_TOOLS, block_seq, first_line, is_wrapper, text_item};
use crate::host::threads::tool_item;

/// What the first line (`session_meta`) says, reduced to what is kept. The
/// creator ids and the base instructions in it are never kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexMeta {
    pub cwd: Option<String>,
    pub originator: Option<String>,
}

impl CodexMeta {
    pub fn source(&self) -> SessionSource {
        match self.originator.as_deref() {
            Some("codex-tui") => SessionSource::Terminal,
            Some("Codex Desktop") => SessionSource::DesktopApp,
            _ => SessionSource::Other,
        }
    }
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// The facts of a `session_meta` line; `None` for any other line.
pub fn parse_meta(bytes: &[u8]) -> Option<CodexMeta> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if str_of(&v, "type") != Some("session_meta") {
        return None;
    }
    let payload = v.get("payload")?;
    Some(CodexMeta {
        cwd: str_of(payload, "cwd").filter(|c| c.starts_with('/')).map(str::to_owned),
        originator: str_of(payload, "originator").map(str::to_owned),
    })
}

/// Thread names by UUID from the tail of `session_index.jsonl`; a later line wins.
pub fn index_titles(bytes: &[u8]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in bytes.split(|&b| b == b'\n') {
        let Ok(v) = serde_json::from_slice::<Value>(line) else { continue };
        let (Some(id), Some(name)) = (str_of(&v, "id"), str_of(&v, "thread_name")) else { continue };
        let name = name.trim();
        if is_uuid(id) && !name.is_empty() {
            out.insert(id.to_owned(), api::clean_cut(name, api::MAX_TITLE).0);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Turn {
    /// No turn marker seen.
    #[default]
    None,
    Started,
    Ended,
}

/// Reads one rollout, line by line.
#[derive(Debug, Default)]
pub struct CodexTail {
    cwd: PathBuf,
    turn: Turn,
    /// Calls made and not yet answered: id and name.
    open_calls: Vec<(String, String)>,
    last_was_assistant: bool,
    pub meta: Option<CodexMeta>,
    pub first_user_text: Option<String>,
    /// From the last `task_complete`.
    pub last_agent_message: Option<String>,
    pub last_seq: u64,
}

fn texts(content: Option<&Value>) -> String {
    content
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| matches!(str_of(b, "type"), Some("text" | "Text" | "input_text" | "output_text")))
                .filter_map(|b| str_of(b, "text"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// A `CommandExecution` item as a tool step.
fn command_spec(item: &Value) -> ToolSpec {
    let parsed: Option<Vec<&str>> = item
        .get("parsed_cmd")
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty())
        .and_then(|list| list.iter().map(|p| str_of(p, "cmd")).collect());
    let detail = match (parsed, item.get("command")) {
        (Some(cmds), _) => cmds.join("\n"),
        (None, Some(Value::Array(argv))) => {
            argv.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")
        }
        (None, Some(Value::String(s))) => s.clone(),
        _ => String::new(),
    };
    ToolSpec {
        name: "shell".into(),
        title: "Run command".into(),
        detail: Some(detail.clone()),
        kind: ApprovalKind::Command,
        command: Some(detail),
        diff: None,
        path: None,
        description: None,
    }
}

/// A `FileChange` item as a tool step: each file, then its diff.
fn file_spec(item: &Value, cwd: &Path) -> ToolSpec {
    let mut detail = String::new();
    if let Some(changes) = item.get("changes").and_then(Value::as_object) {
        let mut paths: Vec<&String> = changes.keys().collect();
        paths.sort();
        for path in paths {
            let change = &changes[path];
            let kind = str_of(change, "type").unwrap_or("update");
            if !detail.is_empty() {
                detail.push('\n');
            }
            detail.push_str(&format!("{} ({kind})\n", display_path(path, cwd)));
            match (str_of(change, "unified_diff"), str_of(change, "content")) {
                (Some(diff), _) => detail.push_str(diff.trim_end_matches('\n')),
                (None, Some(content)) => {
                    detail.push_str(&prefixed_lines(content, if kind == "delete" { '-' } else { '+' }));
                }
                _ => {}
            }
        }
    }
    ToolSpec {
        name: "apply_patch".into(),
        title: "Edit files".into(),
        detail: Some(detail.clone()),
        kind: ApprovalKind::Edit,
        command: None,
        diff: Some(detail),
        path: None,
        description: None,
    }
}

impl CodexTail {
    pub fn new(cwd: Option<PathBuf>) -> Self {
        Self { cwd: cwd.unwrap_or_default(), ..Self::default() }
    }

    /// Feeds one line; returns the items it made.
    pub fn feed(&mut self, line: &Line, mtime_ms: u64) -> Vec<Item> {
        let Ok(v) = serde_json::from_slice::<Value>(&line.bytes) else { return Vec::new() };
        let payload = v.get("payload").unwrap_or(&Value::Null);
        let ts = str_of(&v, "timestamp").and_then(parse_rfc3339_ms).unwrap_or(mtime_ms);
        let seq = block_seq(line.offset, 0);
        match (str_of(&v, "type").unwrap_or_default(), str_of(payload, "type").unwrap_or_default()) {
            ("session_meta", _) => {
                if let Some(meta) = parse_meta(&line.bytes) {
                    if self.cwd.as_os_str().is_empty()
                        && let Some(cwd) = &meta.cwd
                    {
                        self.cwd = PathBuf::from(cwd);
                    }
                    self.meta = Some(meta);
                }
            }
            ("event_msg", "task_started") => {
                self.turn = Turn::Started;
                self.open_calls.clear();
                self.last_was_assistant = false;
            }
            ("event_msg", "task_complete") => {
                self.turn = Turn::Ended;
                self.open_calls.clear();
                self.last_agent_message =
                    str_of(payload, "last_agent_message").filter(|m| !m.trim().is_empty()).map(str::to_owned);
            }
            ("event_msg", "turn_aborted") => {
                self.turn = Turn::Ended;
                self.open_calls.clear();
            }
            ("event_msg", "item_completed") => {
                if let Some(item) = self.item(payload.get("item").unwrap_or(&Value::Null), seq, ts) {
                    self.last_seq = self.last_seq.max(item.seq);
                    return vec![item];
                }
            }
            ("response_item", "custom_tool_call" | "function_call") => {
                if let Some(id) = str_of(payload, "call_id") {
                    if self.open_calls.len() >= MAX_OPEN_TOOLS {
                        self.open_calls.remove(0);
                    }
                    let name = str_of(payload, "name").unwrap_or("tool");
                    self.open_calls.push((id.to_owned(), name.to_owned()));
                }
            }
            ("response_item", "custom_tool_call_output" | "function_call_output") => {
                if let Some(id) = str_of(payload, "call_id") {
                    self.open_calls.retain(|(open, _)| open != id);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn item(&mut self, item: &Value, seq: u64, ts: u64) -> Option<Item> {
        let id = str_of(item, "id").unwrap_or("item");
        let status = str_of(item, "status").unwrap_or_default();
        let made = match str_of(item, "type")? {
            "UserMessage" => {
                let text = texts(item.get("content"));
                if text.trim().is_empty() || is_wrapper(&text) {
                    return None;
                }
                if self.first_user_text.is_none() {
                    self.first_user_text = Some(text.clone());
                }
                text_item(seq, id, ItemKind::User, &text, ts)
            }
            "AgentMessage" => {
                let text = texts(item.get("content"));
                if text.trim().is_empty() {
                    return None;
                }
                self.last_was_assistant = true;
                return Some(text_item(seq, id, ItemKind::Assistant, &text, ts));
            }
            "CommandExecution" => {
                let mut made = tool_item(seq, id, &command_spec(item));
                let ok = status == "completed" && item.get("exit_code").and_then(Value::as_i64) == Some(0);
                let output = str_of(item, "aggregated_output").unwrap_or_default();
                if let Some(tool) = made.tool.as_mut() {
                    tool.status = match status {
                        _ if ok => ToolStatus::Ok,
                        "declined" => ToolStatus::Denied,
                        _ => ToolStatus::Error,
                    };
                    if !output.is_empty() {
                        let (output, cut) = api::clean_tail(output, api::MAX_TOOL_OUTPUT);
                        tool.output = Some(output);
                        made.truncated |= cut;
                    }
                }
                made
            }
            "FileChange" => {
                let mut made = tool_item(seq, id, &file_spec(item, &self.cwd));
                if let Some(tool) = made.tool.as_mut() {
                    tool.status = match status {
                        "completed" => ToolStatus::Ok,
                        "declined" => ToolStatus::Denied,
                        _ => ToolStatus::Error,
                    };
                }
                made
            }
            _ => return None,
        };
        let mut made = made;
        made.ts_ms = ts;
        self.last_was_assistant = false;
        Some(made)
    }

    /// The first line of the first prompt.
    pub fn title(&self) -> Option<String> {
        self.first_user_text.as_deref().and_then(first_line)
    }

    /// The state as far as the rollout shows it, for an open session.
    pub fn guess(&self, quiet_ms: u64) -> SessionState {
        if self.turn != Turn::Started {
            return SessionState::Idle;
        }
        if self.open_calls.iter().any(|(_, name)| name.starts_with("request_user_input")) {
            return SessionState::NeedsYou;
        }
        if !self.open_calls.is_empty() && quiet_ms >= api::SESSION_QUIET_MS {
            return SessionState::NeedsYou;
        }
        SessionState::Working
    }

    /// `Run command` while an `exec` call is open, else the open call's
    /// name, else "Writing" right after a message.
    pub fn step(&self) -> Option<String> {
        if self.turn != Turn::Started {
            return None;
        }
        if self.open_calls.iter().any(|(_, name)| name == "exec") {
            return Some("Run command".to_owned());
        }
        if let Some((_, name)) = self.open_calls.last() {
            return Some(api::clean_cut(name, api::MAX_STEP).0);
        }
        self.last_was_assistant.then(|| "Writing".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::sessions::tests::feed_fixture;

    fn run(name: &str) -> (CodexTail, Vec<Item>) {
        let mut tail = CodexTail::new(None);
        let items = feed_fixture(name, |line| tail.feed(line, 7));
        (tail, items)
    }

    #[test]
    fn an_idle_rollout() {
        let (tail, items) = run("codex-idle.jsonl");
        let meta = tail.meta.as_ref().unwrap();
        assert_eq!(meta.cwd.as_deref(), Some("/home/me/code/app"));
        assert_eq!(meta.source(), SessionSource::Terminal);
        let kinds: Vec<ItemKind> = items.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, [ItemKind::User, ItemKind::Assistant, ItemKind::Tool, ItemKind::Assistant]);
        assert_eq!(
            items[0].text.as_deref(),
            Some("What does this project do?"),
            "the wrapper line was skipped"
        );
        let tool = items[2].tool.as_ref().unwrap();
        assert_eq!((tool.title.as_str(), tool.status), ("Run command", ToolStatus::Ok));
        assert_eq!(tool.detail.as_deref(), Some("cat README.md"));
        assert_eq!(tool.output.as_deref(), Some("A tiny app.\n"));
        assert_eq!(tail.guess(u64::MAX), SessionState::Idle);
        assert_eq!(tail.step(), None);
        assert_eq!(tail.last_agent_message.as_deref(), Some("It is a tiny app."));
        assert!(items.windows(2).all(|w| w[0].seq < w[1].seq));
    }

    #[test]
    fn an_open_call() {
        let (tail, _) = run("codex-open-call.jsonl");
        assert_eq!(tail.guess(0), SessionState::Working);
        assert_eq!(tail.step().as_deref(), Some("Run command"));
        assert_eq!(tail.guess(api::SESSION_QUIET_MS), SessionState::NeedsYou);
        assert_eq!(tail.meta.as_ref().unwrap().source(), SessionSource::DesktopApp);
    }

    #[test]
    fn a_question_needs_you_at_once() {
        let mut tail = CodexTail::new(None);
        let line = |offset: u64, text: &str| Line { offset, bytes: text.as_bytes().to_vec() };
        tail.feed(&line(0, r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"t"}}"#), 0);
        tail.feed(
            &line(
                90,
                r#"{"type":"response_item","payload":{"type":"function_call","call_id":"c1","name":"request_user_input_async"}}"#,
            ),
            0,
        );
        assert_eq!(tail.guess(0), SessionState::NeedsYou);
        tail.feed(&line(200, r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"x"}}"#), 0);
        assert_eq!(tail.guess(u64::MAX), SessionState::Working);
        tail.feed(&line(300, r#"{"type":"event_msg","payload":{"type":"turn_aborted","turn_id":"t","reason":"interrupted"}}"#), 0);
        assert_eq!(tail.guess(0), SessionState::Idle);
    }

    #[test]
    fn file_changes() {
        let (_, items) = run("codex-file-change.jsonl");
        let tools: Vec<_> = items.iter().filter_map(|i| i.tool.as_ref()).collect();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].status, ToolStatus::Ok);
        assert_eq!(tools[0].title, "Edit files");
        assert_eq!(
            tools[0].detail.as_deref(),
            Some("notes.txt (add)\n+first\n+second\nsrc/main.rs (update)\n@@ -1 +1 @@\n-old\n+new")
        );
        assert_eq!(tools[1].status, ToolStatus::Denied);
        assert_eq!(tools[2].status, ToolStatus::Error);
    }

    #[test]
    fn the_index_gives_names() {
        let index = br#"{"id":"11111111-2222-4333-8444-555555555555","thread_name":"Old","updated_at":"x"}
{"id":"11111111-2222-4333-8444-555555555555","thread_name":"Fix the build","updated_at":"y"}
{"id":"not-a-uuid","thread_name":"No"}
partial line"#;
        let titles = index_titles(index);
        assert_eq!(titles.len(), 1);
        assert_eq!(titles["11111111-2222-4333-8444-555555555555"], "Fix the build");
    }

    #[test]
    fn meta_keeps_only_what_is_used() {
        let line = br#"{"timestamp":"2026-11-19T09:00:00Z","type":"session_meta","payload":{"id":"x","cwd":"/w","originator":"G7 Linux","creator_user_id":"secret","base_instructions":{"text":"long"}}}"#;
        let meta = parse_meta(line).unwrap();
        assert_eq!(meta, CodexMeta { cwd: Some("/w".into()), originator: Some("G7 Linux".into()) });
        assert_eq!(meta.source(), SessionSource::Other);
        assert!(!format!("{meta:?}").contains("secret"));
        assert_eq!(parse_meta(br#"{"type":"event_msg","payload":{}}"#), None);
        let relative = br#"{"type":"session_meta","payload":{"cwd":"w"}}"#;
        assert_eq!(parse_meta(relative).unwrap().cwd, None);
    }
}
