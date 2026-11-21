// SPDX-License-Identifier: MPL-2.0
//! Claude transcript lines (`~/.claude/projects/<folder>/<uuid>.jsonl`) to
//! items, and the facts the state guess needs when Claude's own status file
//! isn't there.
//!
//! Only `user`, `assistant`, `system` and `custom-title` lines are looked
//! at; every other type (requests, copies of messages, bookkeeping) is
//! ignored by its `type` alone. Side-chain and meta lines are skipped.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use crate::api::{self, Item, ItemKind, SessionState, ToolStatus};
use crate::host::claude;
use crate::host::sessions::paths::parse_rfc3339_ms;
use crate::host::sessions::tail::Line;
use crate::host::sessions::{MAX_BLOCKS, MAX_OPEN_TOOLS, block_seq, first_line, is_wrapper, text_item};
use crate::host::threads::tool_item;

/// Tools whose call, left open and quiet, is most likely a permission prompt.
const ASKING_TOOLS: &[&str] = &["Bash", "Write", "Edit", "MultiEdit", "NotebookEdit"];

/// The last conversational line seen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Last {
    #[default]
    Nothing,
    User,
    ToolResult,
    Assistant {
        end_turn: bool,
        /// The line's tool call: its id and tool name.
        tool: Option<(String, String)>,
        text: bool,
    },
    StopHook {
        after_end_turn: bool,
    },
    ApiError,
}

/// Reads one Claude transcript, line by line.
#[derive(Debug, Default)]
pub struct ClaudeTail {
    /// For showing paths relative to the working directory.
    cwd: PathBuf,
    /// Tool steps waiting for their result, by tool use id, with how each is shown as a step.
    open_tools: HashMap<String, (Item, String)>,
    last: Last,
    pub custom_title: Option<String>,
    pub first_user_text: Option<String>,
    /// The `cwd` of the last conversational line.
    pub line_cwd: Option<String>,
    pub last_assistant_text: Option<String>,
    pub last_seq: u64,
    /// Where the first prompt was typed (`entrypoint`: `cli`, `claude-desktop`, …).
    pub entrypoint: Option<String>,
}

/// An `entrypoint` is kept up to this many bytes.
const MAX_ENTRYPOINT: usize = 32;

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

impl ClaudeTail {
    pub fn new(cwd: Option<PathBuf>) -> Self {
        Self { cwd: cwd.unwrap_or_default(), ..Self::default() }
    }

    /// Feeds one line; returns the items it made or changed.
    pub fn feed(&mut self, line: &Line, mtime_ms: u64) -> Vec<Item> {
        let Ok(v) = serde_json::from_slice::<Value>(&line.bytes) else { return Vec::new() };
        self.feed_value(&v, line.offset, mtime_ms)
    }

    /// [`ClaudeTail::feed`] of a line already parsed, which starts at `offset`.
    pub fn feed_value(&mut self, v: &Value, offset: u64, mtime_ms: u64) -> Vec<Item> {
        let kind = str_of(v, "type").unwrap_or_default();
        if kind == "custom-title" {
            if let Some(title) = str_of(v, "customTitle").map(str::trim).filter(|t| !t.is_empty()) {
                self.custom_title = Some(api::clean_cut(title, api::MAX_TITLE).0);
            }
            return Vec::new();
        }
        if !matches!(kind, "user" | "assistant" | "system") {
            return Vec::new();
        }
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || v.get("isMeta").and_then(Value::as_bool) == Some(true)
        {
            return Vec::new();
        }
        let ts = str_of(v, "timestamp").and_then(parse_rfc3339_ms).unwrap_or(mtime_ms);
        if let Some(cwd) = str_of(v, "cwd").filter(|c| c.starts_with('/')) {
            self.line_cwd = Some(cwd.to_owned());
            if self.cwd.as_os_str().is_empty() {
                self.cwd = PathBuf::from(cwd);
            }
        }
        let items = match kind {
            "user" => self.user(v, offset, ts),
            "assistant" => self.assistant(v, offset, ts),
            _ => {
                self.system(v);
                Vec::new()
            }
        };
        if let Some(seq) = items.iter().map(|i| i.seq).max() {
            self.last_seq = self.last_seq.max(seq);
        }
        items
    }

    fn user(&mut self, v: &Value, offset: u64, ts: u64) -> Vec<Item> {
        let mut out = Vec::new();
        let content = v.get("message").and_then(|m| m.get("content"));
        let text = match content {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Array(blocks)) => {
                let parts: Vec<&str> = blocks
                    .iter()
                    .filter(|b| str_of(b, "type") == Some("text"))
                    .filter_map(|b| str_of(b, "text"))
                    .collect();
                (!parts.is_empty()).then(|| parts.join("\n"))
            }
            _ => None,
        };
        if let Some(text) = text.filter(|t| !t.trim().is_empty() && !is_wrapper(t)) {
            if self.first_user_text.is_none() {
                self.first_user_text = Some(text.clone());
                self.entrypoint = str_of(v, "entrypoint").map(|e| api::cut(e, MAX_ENTRYPOINT).to_owned());
            }
            let uuid = str_of(v, "uuid").unwrap_or("line");
            out.push(text_item(block_seq(offset, 0), &format!("u-{uuid}"), ItemKind::User, &text, ts));
            self.last = Last::User;
        }
        let rejected =
            v.get("permissionDecision").and_then(|d| str_of(d, "decision")).is_some_and(|d| d == "reject");
        let blocks = content.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        for block in blocks.iter().filter(|b| str_of(b, "type") == Some("tool_result")) {
            self.last = Last::ToolResult;
            let Some((mut item, _)) = str_of(block, "tool_use_id").and_then(|id| self.open_tools.remove(id))
            else {
                continue;
            };
            let status = if rejected {
                ToolStatus::Denied
            } else if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                ToolStatus::Error
            } else {
                ToolStatus::Ok
            };
            let output = match block.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(parts)) => {
                    parts.iter().filter_map(|p| str_of(p, "text")).collect::<Vec<_>>().join("\n")
                }
                _ => String::new(),
            };
            item.rev += 1;
            if let Some(tool) = item.tool.as_mut() {
                tool.status = status;
                if !output.is_empty() {
                    let (output, cut) = api::clean_tail(&output, api::MAX_TOOL_OUTPUT);
                    tool.output = Some(output);
                    item.truncated |= cut;
                }
            }
            out.push(item);
        }
        out
    }

    fn assistant(&mut self, v: &Value, offset: u64, ts: u64) -> Vec<Item> {
        let mut out = Vec::new();
        let Some(message) = v.get("message") else { return out };
        let message_id = str_of(message, "id").unwrap_or("message");
        let block_index = v.get("apiBlockIndex").and_then(Value::as_u64);
        let end_turn = str_of(message, "stop_reason") == Some("end_turn");
        let mut tool = None;
        let mut text = false;
        let blocks = message.get("content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        for (i, block) in blocks.iter().enumerate().take(MAX_BLOCKS) {
            let seq = block_seq(offset, i);
            match str_of(block, "type") {
                Some("text") => {
                    let body = str_of(block, "text").unwrap_or_default();
                    if body.trim().is_empty() {
                        continue;
                    }
                    let id = format!("{message_id}:{}", block_index.unwrap_or(i as u64));
                    out.push(text_item(seq, &id, ItemKind::Assistant, body, ts));
                    self.last_assistant_text = Some(body.to_owned());
                    text = true;
                }
                Some("tool_use") => {
                    let Some(id) = str_of(block, "id") else { continue };
                    let name = str_of(block, "name").unwrap_or("tool");
                    let spec = claude::tool_spec(name, block.get("input").unwrap_or(&Value::Null), &self.cwd);
                    let mut item = tool_item(seq, id, &spec);
                    item.ts_ms = ts;
                    if self.open_tools.len() >= MAX_OPEN_TOOLS
                        && let Some(oldest) =
                            self.open_tools.iter().min_by_key(|(_, (i, _))| i.seq).map(|(k, _)| k.clone())
                    {
                        self.open_tools.remove(&oldest);
                    }
                    self.open_tools.insert(id.to_owned(), (item.clone(), spec.step()));
                    out.push(item);
                    tool = Some((id.to_owned(), name.to_owned()));
                }
                _ => {}
            }
        }
        self.last = Last::Assistant { end_turn, tool, text };
        out
    }

    fn system(&mut self, v: &Value) {
        match str_of(v, "subtype") {
            Some("stop_hook_summary") => {
                let after_end_turn = matches!(
                    self.last,
                    Last::Assistant { end_turn: true, .. } | Last::StopHook { after_end_turn: true }
                );
                self.last = Last::StopHook { after_end_turn };
            }
            Some("api_error") => self.last = Last::ApiError,
            _ => {}
        }
    }

    /// The state as far as the tail shows it, for an open session without a
    /// usable status file. `quiet_ms`: how long the file hasn't changed.
    pub fn guess(&self, quiet_ms: u64) -> SessionState {
        match &self.last {
            Last::Assistant { tool: Some((id, name)), .. }
                if self.open_tools.contains_key(id)
                    && ASKING_TOOLS.contains(&name.as_str())
                    && quiet_ms >= api::SESSION_QUIET_MS =>
            {
                SessionState::NeedsYou
            }
            Last::Assistant { end_turn: true, .. } | Last::StopHook { after_end_turn: true } => {
                SessionState::Idle
            }
            _ => SessionState::Working,
        }
    }

    /// What the session is doing while it works: the newest running tool
    /// (its description, else its title), else "Writing" right after text.
    pub fn step(&self) -> Option<String> {
        if let Some((_, step)) = self.open_tools.values().max_by_key(|(i, _)| i.seq) {
            return Some(step.clone());
        }
        matches!(self.last, Last::Assistant { text: true, end_turn: false, .. }).then(|| "Writing".to_owned())
    }

    /// Tool steps still waiting for a result.
    pub fn open_tool_count(&self) -> usize {
        self.open_tools.len()
    }

    /// The title the transcript gives: the last custom title, else the first
    /// line of the first prompt.
    pub fn title(&self) -> Option<String> {
        self.custom_title.clone().or_else(|| self.first_user_text.as_deref().and_then(first_line))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::sessions::tests::{feed_fixture, fixture_lines, fixture_path};

    fn run(name: &str) -> (ClaudeTail, Vec<Item>) {
        let mut tail = ClaudeTail::new(None);
        let items = feed_fixture(name, |line| tail.feed(line, 7));
        (tail, items)
    }

    #[test]
    fn an_idle_session() {
        let (tail, items) = run("claude-idle.jsonl");
        let kinds: Vec<ItemKind> = items.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, [ItemKind::User, ItemKind::Tool, ItemKind::Assistant]);
        assert_eq!(items[0].id, "u-a0000000-0000-4000-8000-000000000001");
        assert_eq!(items[0].text.as_deref(), Some("List the files in this folder."));
        assert_eq!(items[0].ts_ms, 1_795_000_000_000 + 2_000);
        let tool = items[1].tool.as_ref().unwrap();
        assert_eq!((tool.title.as_str(), tool.status), ("Run command", ToolStatus::Ok));
        assert_eq!(tool.detail.as_deref(), Some("ls"));
        assert_eq!(tool.output.as_deref(), Some("README.md\nsrc"));
        assert_eq!(items[1].rev, 1, "the result updated the step");
        assert_eq!(items[2].id, "msg_02:0");
        assert_eq!(tail.guess(0), SessionState::Idle);
        assert_eq!(tail.step(), None);
        assert_eq!(tail.custom_title.as_deref(), Some("Tidy the readme"));
        assert_eq!(tail.first_user_text.as_deref(), Some("List the files in this folder."));
        assert_eq!(tail.line_cwd.as_deref(), Some("/home/me/code/app"));
        assert!(tail.last_assistant_text.as_deref().unwrap().starts_with("There are two"));
    }

    #[test]
    fn an_open_tool_call_and_bookkeeping_lines() {
        let (tail, items) = run("claude-open-tool.jsonl");
        assert!(
            items.iter().all(|i| matches!(i.kind, ItemKind::User | ItemKind::Assistant | ItemKind::Tool))
        );
        let last = items.last().unwrap();
        let tool = last.tool.as_ref().unwrap();
        assert_eq!((tool.name.as_str(), tool.status), ("Write", ToolStatus::Running));
        assert_eq!(tool.title, "Write notes.txt", "shown relative to the working directory");
        assert_eq!(tail.open_tool_count(), 1);
        assert_eq!(tail.guess(1000), SessionState::Working, "not quiet long enough");
        assert_eq!(tail.guess(api::SESSION_QUIET_MS), SessionState::NeedsYou);
        assert_eq!(tail.step().as_deref(), Some("Write notes.txt"));
        // Bookkeeping lines after the call didn't become items or change the state.
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn a_denied_call() {
        let (tail, items) = run("claude-denied.jsonl");
        let tool = items.iter().rev().find_map(|i| i.tool.as_ref()).unwrap();
        assert_eq!(tool.status, ToolStatus::Denied);
        let error = items.iter().filter_map(|i| i.tool.as_ref()).find(|t| t.status == ToolStatus::Error);
        assert!(error.is_some(), "an error result without a rejection is an error");
        assert_eq!(tail.guess(0), SessionState::Working, "a result came back: Claude goes on");
    }

    #[test]
    fn noise_makes_nothing() {
        // The fixture's bookkeeping lines with one 300 KB line of filler among them.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("noise.jsonl");
        let fixture = std::fs::read_to_string(fixture_path("claude-noise.jsonl")).unwrap();
        let (head, rest) = fixture.split_at(fixture.find('\n').unwrap() + 1);
        let filler = format!("{{\"type\":\"api-request-blob\",\"blob\":\"{}\"}}\n", "f".repeat(300 * 1024));
        std::fs::write(&path, format!("{head}{filler}{rest}")).unwrap();
        let window = crate::host::sessions::tail::read_window(&path, None, 0).unwrap();
        let mut tail = ClaudeTail::new(None);
        let mut items = Vec::new();
        for line in &window.lines {
            items.extend(tail.feed(line, 7));
        }
        assert!(window.lines.len() >= 7, "the window grew past the long line");
        assert!(items.is_empty());
        assert_eq!(tail.guess(u64::MAX), SessionState::Working);
        assert_eq!(tail.first_user_text, None);
    }

    #[test]
    fn wrappers_side_chains_and_dirty_text() {
        let mut tail = ClaudeTail::new(Some(PathBuf::from("/w")));
        let line = |offset: u64, text: &str| Line { offset, bytes: text.as_bytes().to_vec() };
        let made = tail.feed(
            &line(0, r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#),
            1,
        );
        assert!(made.is_empty(), "a command wrapper isn't shown");
        let made = tail.feed(
            &line(100, r#"{"type":"assistant","isSidechain":true,"message":{"id":"m","content":[{"type":"text","text":"inner"}]}}"#),
            1,
        );
        assert!(made.is_empty());
        let made = tail.feed(
            &line(200, r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"meta"}}"#),
            1,
        );
        assert!(made.is_empty());
        let long = "é".repeat(api::MAX_ITEM_TEXT);
        let dirty = format!(
            r#"{{"type":"user","uuid":"u2","message":{{"role":"user","content":"a\u001b[31mb\u202ec {long}"}}}}"#
        );
        let made = tail.feed(&line(300, &dirty), 1);
        assert_eq!(made.len(), 1);
        let text = made[0].text.as_deref().unwrap();
        assert!(text.starts_with("a[31mbc "), "control and bidi characters are stripped");
        assert!(made[0].truncated && text.len() <= api::MAX_ITEM_TEXT);
        assert_eq!(made[0].seq, 300 << 6);
        assert_eq!(made[0].ts_ms, 1, "no timestamp: the file's time");
    }

    #[test]
    fn seqs_are_unique_increasing_and_stable() {
        let lines = fixture_lines("claude-idle.jsonl");
        let read = || {
            let mut tail = ClaudeTail::new(None);
            let mut seqs = Vec::new();
            for line in &lines {
                for item in tail.feed(line, 0) {
                    if item.rev == 0 {
                        seqs.push(item.seq);
                    }
                }
            }
            seqs
        };
        let first = read();
        assert!(first.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(first, read());
    }
}
