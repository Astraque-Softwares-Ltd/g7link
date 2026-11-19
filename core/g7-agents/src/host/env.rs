// SPDX-License-Identifier: MPL-2.0
//! The environment an agent CLI runs with: only the listed variables of the
//! host's own environment, so a host started from inside another agent's
//! terminal doesn't make its children look like part of that session, and no
//! credential of ours is ever injected.

use std::ffi::{OsStr, OsString};

use crate::api::AgentId;

/// Copied from the host's environment when present.
const PASSED: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "PATH",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "TZ",
    "TMPDIR",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "DBUS_SESSION_BUS_ADDRESS",
    "SSH_AUTH_SOCK",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
];

/// Passed to Claude only, when the user has it set: `claude -p` then bills
/// that key, and the agent list says so (`auth: "api_key"`).
const CLAUDE_ONLY: &[&str] = &["ANTHROPIC_API_KEY"];

/// How long Claude may wait on the approval tool: longer than an approval
/// lives (2 hours), so the host's own expiry is what ends a wait.
pub const MCP_TOOL_TIMEOUT_MS: &str = "7500000";

/// The child's environment from `vars` (the host's own).
pub fn child_env(
    agent: AgentId,
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    let mut out: Vec<(OsString, OsString)> = vars
        .into_iter()
        .filter(|(key, _)| {
            let Some(key) = key.to_str() else { return false };
            PASSED.contains(&key) || (agent == AgentId::Claude && CLAUDE_ONLY.contains(&key))
        })
        .collect();
    let home = out.iter().find(|(k, _)| k == "HOME").map(|(_, v)| v.clone());
    if let Some(home) = home.filter(|h| !h.is_empty()) {
        let local_bin = std::path::Path::new(&home).join(".local/bin");
        match out.iter_mut().find(|(k, _)| k == "PATH") {
            Some((_, path)) => {
                if !std::env::split_paths(path).any(|p| p == local_bin) {
                    let mut parts: Vec<_> = std::env::split_paths(path).collect();
                    parts.push(local_bin);
                    if let Ok(joined) = std::env::join_paths(parts) {
                        *path = joined;
                    }
                }
            }
            None => out.push(("PATH".into(), local_bin.into_os_string())),
        }
    }
    if agent == AgentId::Claude {
        out.push(("MCP_TOOL_TIMEOUT".into(), MCP_TOOL_TIMEOUT_MS.into()));
    }
    out
}

/// The value of `name` in an environment list.
pub fn lookup<'a>(env: &'a [(OsString, OsString)], name: &str) -> Option<&'a OsStr> {
    env.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_os_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(list: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        list.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect()
    }

    #[test]
    fn only_the_listed_variables_pass() {
        let host = vars(&[
            ("HOME", "/home/u"),
            ("PATH", "/usr/bin"),
            ("LANG", "C.UTF-8"),
            ("CLAUDE_CODE_ENTRYPOINT", "claude-desktop"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_SESSION_ID", "x"),
            ("ANTHROPIC_API_KEY", "sk-test"),
            ("ANTHROPIC_BASE_URL", "http://proxy"),
            ("OPENAI_API_KEY", "sk-openai"),
            ("ELECTRON_RUN_AS_NODE", "1"),
            ("LD_PRELOAD", "/x.so"),
            ("CODEX_HOME", "/home/u/.codex"),
        ]);
        let codex = child_env(AgentId::Codex, host.clone());
        let names: Vec<_> = codex.iter().map(|(k, _)| k.to_str().unwrap().to_owned()).collect();
        assert_eq!(names, ["HOME", "PATH", "LANG", "CODEX_HOME"]);
        assert_eq!(lookup(&codex, "PATH").unwrap(), "/usr/bin:/home/u/.local/bin");

        let claude = child_env(AgentId::Claude, host);
        assert!(lookup(&claude, "CLAUDE_CODE_ENTRYPOINT").is_none());
        assert!(lookup(&claude, "CLAUDECODE").is_none());
        assert!(lookup(&claude, "OPENAI_API_KEY").is_none());
        assert!(lookup(&claude, "ANTHROPIC_BASE_URL").is_none());
        assert_eq!(lookup(&claude, "ANTHROPIC_API_KEY").unwrap(), "sk-test");
        assert_eq!(lookup(&claude, "MCP_TOOL_TIMEOUT").unwrap(), MCP_TOOL_TIMEOUT_MS);
    }

    #[test]
    fn local_bin_is_added_once() {
        let env = child_env(AgentId::Codex, vars(&[("HOME", "/h"), ("PATH", "/h/.local/bin:/usr/bin")]));
        assert_eq!(lookup(&env, "PATH").unwrap(), "/h/.local/bin:/usr/bin");
        let env = child_env(AgentId::Codex, vars(&[("HOME", "/h")]));
        assert_eq!(lookup(&env, "PATH").unwrap(), "/h/.local/bin");
    }
}
