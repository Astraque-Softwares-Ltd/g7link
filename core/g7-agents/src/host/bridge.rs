// SPDX-License-Identifier: MPL-2.0
//! `g7-agents mcp-approve` (§4.5): the stdio MCP server Claude calls through
//! `--permission-prompt-tool`. Each `tools/call` becomes a `bridge.ask` on
//! the host socket, and the user's answer goes back to Claude.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use crate::api::{self, BridgeAskResult, BridgeBehavior, ClientKind, HelloParams};
use crate::client::{Client, ClientError, SOCKET_ENV};
use crate::lines::{self, LineRead};

pub const TOKEN_ENV: &str = "G7_AGENTS_BRIDGE_TOKEN";
/// `arguments` larger than this are denied without asking.
pub const MAX_ARGUMENTS: usize = 200_000;
/// A line from Claude, at most.
const MAX_IN_LINE: usize = 16 * 1024 * 1024;

pub const DENY_USER: &str = "The user denied this.";
pub const DENY_NO_HOST: &str = "The approval service is not running.";
pub const DENY_TOO_LARGE: &str = "Too large to review.";

/// What to do with one line from Claude.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Reply(Value),
    Ask { id: Value, tool_name: String, input: Value, tool_use_id: String },
    Cancel { tool_use_id: String },
    Nothing,
}

/// The tool result Claude reads: one text block whose text is JSON.
pub fn decision_reply(id: &Value, behavior: BridgeBehavior, message: Option<&str>) -> Value {
    let body = match behavior {
        BridgeBehavior::Allow => json!({"behavior": "allow"}),
        BridgeBehavior::Deny => json!({"behavior": "deny", "message": message.unwrap_or(DENY_USER)}),
    };
    json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": body.to_string()}]}})
}

fn error_reply(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The protocol, without I/O.
#[derive(Debug, Default)]
pub struct BridgeCore {
    /// `tools/call` request ids waiting for the user, and their tool use ids.
    asking: HashMap<String, String>,
    cancelled: HashSet<String>,
}

fn key(id: &Value) -> String {
    id.to_string()
}

impl BridgeCore {
    pub fn handle(&mut self, msg: &Value) -> Action {
        let id = msg.get("id").filter(|id| !id.is_null());
        let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = msg.get("params").unwrap_or(&Value::Null);
        let Some(id) = id else {
            if method == "notifications/cancelled"
                && let Some(request) = params.get("requestId")
            {
                let request = key(request);
                if let Some(tool_use_id) = self.asking.remove(&request) {
                    self.cancelled.insert(request);
                    return Action::Cancel { tool_use_id };
                }
            }
            return Action::Nothing;
        };
        match method {
            "initialize" => {
                let version = params.get("protocolVersion").cloned().unwrap_or(json!("2025-06-18"));
                Action::Reply(json!({"jsonrpc": "2.0", "id": id, "result": {
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "g7", "version": env!("CARGO_PKG_VERSION")},
                }}))
            }
            "tools/list" => Action::Reply(json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{
                "name": "approve",
                "description": "Ask the user to approve a tool call.",
                "inputSchema": {"type": "object", "properties": {
                    "tool_name": {"type": "string"},
                    "input": {"type": "object"},
                    "tool_use_id": {"type": "string"},
                }, "required": ["tool_name", "input"]},
            }]}})),
            "ping" => Action::Reply(json!({"jsonrpc": "2.0", "id": id, "result": {}})),
            "tools/call" => {
                if params.get("name").and_then(Value::as_str) != Some("approve") {
                    return Action::Reply(error_reply(id, -32602, "unknown tool"));
                }
                let args = params.get("arguments").unwrap_or(&Value::Null);
                if args.to_string().len() > MAX_ARGUMENTS {
                    return Action::Reply(decision_reply(id, BridgeBehavior::Deny, Some(DENY_TOO_LARGE)));
                }
                let tool_name = args.get("tool_name").and_then(Value::as_str).unwrap_or_default();
                let tool_name = api::cut(tool_name, api::MAX_BRIDGE_TOOL_NAME).to_owned();
                let tool_use_id = args
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        params
                            .get("_meta")
                            .and_then(|m| m.get("claudecode/toolUseId"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or_default();
                let tool_use_id = api::cut(tool_use_id, api::MAX_BRIDGE_TOOL_USE_ID).to_owned();
                let input = args.get("input").cloned().unwrap_or_else(|| json!({}));
                self.asking.insert(key(id), tool_use_id.clone());
                Action::Ask { id: id.clone(), tool_name, input, tool_use_id }
            }
            _ => Action::Reply(error_reply(id, -32601, "method not found")),
        }
    }

    /// The host answered: the line for Claude, unless the request was
    /// cancelled meanwhile.
    pub fn answered(&mut self, id: &Value, answer: Result<BridgeAskResult, ()>) -> Option<Value> {
        let k = key(id);
        if self.cancelled.remove(&k) {
            return None;
        }
        self.asking.remove(&k);
        Some(match answer {
            Ok(result) => decision_reply(id, result.behavior, result.message.as_deref()),
            Err(()) => decision_reply(id, BridgeBehavior::Deny, Some(DENY_NO_HOST)),
        })
    }
}

async fn write_line(out: &Mutex<tokio::io::Stdout>, value: &Value) {
    let mut line = value.to_string();
    line.push('\n');
    let mut out = out.lock().await;
    let _ = out.write_all(line.as_bytes()).await;
    let _ = out.flush().await;
}

/// Runs the bridge on stdin/stdout until Claude closes stdin.
pub async fn run() -> std::io::Result<()> {
    let socket = std::env::var_os(SOCKET_ENV).map(PathBuf::from);
    let token = std::env::var(TOKEN_ENV).ok();
    let client = match (socket, token) {
        (Some(socket), Some(token)) => {
            let hello = HelloParams {
                client: ClientKind::Bridge,
                version: api::API_VERSION,
                bridge_token: Some(token),
            };
            match Client::connect(&socket, hello).await {
                Ok((client, _events)) => Some(client),
                Err(e) => {
                    tracing::warn!("the approval bridge couldn't reach the agent host: {e}");
                    None
                }
            }
        }
        _ => None,
    };
    let core = Arc::new(std::sync::Mutex::new(BridgeCore::default()));
    let out = Arc::new(Mutex::new(tokio::io::stdout()));
    let mut reader = BufReader::new(tokio::io::stdin());
    let mut buf = Vec::new();
    loop {
        match lines::read_line(&mut reader, &mut buf, MAX_IN_LINE).await? {
            LineRead::Line => {}
            LineRead::Eof | LineRead::TooLong => break,
        }
        let Ok(msg) = serde_json::from_slice::<Value>(&buf) else { continue };
        let action = core.lock().unwrap_or_else(|e| e.into_inner()).handle(&msg);
        match action {
            Action::Nothing => {}
            Action::Reply(reply) => write_line(&out, &reply).await,
            Action::Ask { id, tool_name, input, tool_use_id } => {
                let client = client.clone();
                let core = core.clone();
                let out = out.clone();
                tokio::spawn(async move {
                    let answer = match client {
                        Some(client) => {
                            let params =
                                json!({"tool_name": tool_name, "input": input, "tool_use_id": tool_use_id});
                            match client.call_value(api::method::BRIDGE_ASK, params, None).await {
                                Ok(value) => serde_json::from_value::<BridgeAskResult>(value).map_err(|_| ()),
                                Err(ClientError::Remote(e)) => {
                                    tracing::warn!("the agent host refused an approval: {}", e.code);
                                    Err(())
                                }
                                Err(_) => Err(()),
                            }
                        }
                        None => Err(()),
                    };
                    let reply = core.lock().unwrap_or_else(|e| e.into_inner()).answered(&id, answer);
                    if let Some(reply) = reply {
                        write_line(&out, &reply).await;
                    }
                });
            }
            Action::Cancel { tool_use_id } => {
                // Inline, so a cancel just before Claude closes stdin still arrives.
                if let Some(client) = &client {
                    let params = json!({"tool_use_id": tool_use_id});
                    let _ = client
                        .call_value(api::method::BRIDGE_CANCEL, params, Some(crate::client::REQUEST_TIMEOUT))
                        .await;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses the text block of a tool result, so `{"behavior": "allow"}`
    /// and `{"behavior":"allow"}` compare equal.
    fn normalize(mut v: Value) -> Value {
        if let Some(text) = v.pointer_mut("/result/content/0/text")
            && let Some(s) = text.as_str()
            && let Ok(parsed) = serde_json::from_str::<Value>(s)
        {
            *text = parsed;
        }
        if let Some(version) = v.pointer_mut("/result/serverInfo/version") {
            *version = json!("*");
        }
        v
    }

    #[test]
    fn replays_the_capture() {
        let log = include_str!("../../tests/fixtures/claude-mcp-approve.log");
        let mut entries: Vec<(bool, Value)> = Vec::new();
        for line in log.lines() {
            if let Some(rest) = line.strip_prefix("IN  ") {
                entries.push((true, serde_json::from_str(rest).unwrap()));
            } else if let Some(rest) = line.strip_prefix("OUT ") {
                entries.push((false, serde_json::from_str(rest).unwrap()));
            }
        }
        let mut core = BridgeCore::default();
        let mut produced: Vec<Value> = Vec::new();
        let mut expected: Vec<Value> = Vec::new();
        let mut asks = Vec::new();
        let mut cancels = Vec::new();
        for (i, (incoming, msg)) in entries.iter().enumerate() {
            if !incoming {
                expected.push(msg.clone());
                continue;
            }
            match core.handle(msg) {
                Action::Reply(reply) => produced.push(reply),
                Action::Ask { id, tool_name, tool_use_id, .. } => {
                    asks.push((tool_name, tool_use_id));
                    // The stub host answers as the capture did, if it did.
                    if let Some((false, out)) = entries.get(i + 1) {
                        let text = out.pointer("/result/content/0/text").and_then(Value::as_str).unwrap();
                        let answer: BridgeAskResult = serde_json::from_str(text).unwrap();
                        produced.extend(core.answered(&id, Ok(answer)));
                    }
                }
                Action::Cancel { tool_use_id } => cancels.push(tool_use_id),
                Action::Nothing => {}
            }
        }
        assert_eq!(produced.len(), expected.len());
        for (ours, theirs) in produced.into_iter().zip(expected) {
            if theirs.pointer("/result/tools").is_some() {
                // The capture's stub listed no tools; ours lists `approve`.
                assert_eq!(ours.pointer("/result/tools/0/name"), Some(&json!("approve")));
                assert_eq!(ours["id"], theirs["id"]);
                continue;
            }
            assert_eq!(normalize(ours), normalize(theirs));
        }
        assert_eq!(asks.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["Write", "Bash", "Bash"]);
        assert_eq!(cancels, ["toolu_01YKsx3mzGkW8veWFK5sKNPF"]);
        // An answer that arrives after the cancel is not sent.
        assert_eq!(
            core.answered(&json!(2), Ok(BridgeAskResult { behavior: BridgeBehavior::Deny, message: None })),
            None
        );
    }

    #[test]
    fn oversized_arguments_are_denied_unasked() {
        let mut core = BridgeCore::default();
        let big = "x".repeat(MAX_ARGUMENTS + 1);
        let msg = json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"approve","arguments":{"tool_name":"Write","input":{"content":big}}}});
        let Action::Reply(reply) = core.handle(&msg) else { panic!() };
        let text = reply.pointer("/result/content/0/text").and_then(Value::as_str).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(text).unwrap(),
            json!({"behavior":"deny","message":DENY_TOO_LARGE})
        );
    }

    #[test]
    fn host_unreachable_denies() {
        let mut core = BridgeCore::default();
        let msg = json!({"jsonrpc":"2.0","id":"a","method":"tools/call","params":{"name":"approve","arguments":{"tool_name":"Bash","input":{}}}});
        assert!(matches!(core.handle(&msg), Action::Ask { .. }));
        let reply = core.answered(&json!("a"), Err(())).unwrap();
        assert_eq!(reply["id"], json!("a"));
        assert!(reply.pointer("/result/content/0/text").unwrap().as_str().unwrap().contains(DENY_NO_HOST));
        assert_eq!(
            core.handle(&json!({"jsonrpc":"2.0","id":9,"method":"ping"})),
            Action::Reply(json!({"jsonrpc":"2.0","id":9,"result":{}}))
        );
        assert!(
            matches!(core.handle(&json!({"jsonrpc":"2.0","id":10,"method":"resources/list"})), Action::Reply(r) if r["error"]["code"] == -32601)
        );
    }
}
