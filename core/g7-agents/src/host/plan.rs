// SPDX-License-Identifier: MPL-2.0
//! Plan mode (§Q4.3): Claude's `ExitPlanMode` through the approval bridge,
//! the plan file Claude writes on the way, and Codex's plan items.

use std::path::Path;

use serde_json::Value;

use crate::api::{self, BridgeAskResult};

/// The tool Claude calls with its plan; it reaches our approval bridge.
pub const EXIT_PLAN_TOOL: &str = "ExitPlanMode";
/// What Claude is told after its plan was taken: the turn then ends by itself.
pub const EXIT_PLAN_REPLY: &str = "The plan was saved for the user to review. Stop here and wait for them.";
/// The thread's summary when a turn ended with a plan.
pub const PLAN_READY: &str = "Plan ready";

/// The plan in an `ExitPlanMode` call's input, cleaned and cut to
/// [`api::MAX_PLAN_ITEM`] bytes; `None` when it has none.
pub fn claude_plan(input: &Value) -> Option<String> {
    let plan = input.get("plan").and_then(Value::as_str)?.trim();
    (!plan.is_empty()).then(|| api::clean_cut(plan, api::MAX_PLAN_ITEM).0)
}

/// The bridge's answer to `ExitPlanMode`: a denial, so Claude stops and
/// waits; no approval is created for it.
pub fn claude_exit_reply() -> BridgeAskResult {
    BridgeAskResult::deny(EXIT_PLAN_REPLY)
}

/// A Claude tool call that writes its plan file in plan mode: a `Write` or
/// an edit whose file is outside the thread's folder. Shown as class
/// `plan` (no row; never sent to a phone).
pub fn is_plan_file(tool_name: &str, input: &Value, cwd: &Path) -> bool {
    if !matches!(tool_name, "Write" | "Edit" | "MultiEdit") {
        return false;
    }
    let Some(path) = input.get("file_path").and_then(Value::as_str).map(Path::new) else { return false };
    // Relative paths and paths with `..` are taken as inside: only a plain
    // absolute path elsewhere is the plan file.
    path.is_absolute()
        && !path.components().any(|c| matches!(c, std::path::Component::ParentDir))
        && !path.starts_with(cwd)
}

/// A Codex plan item's final text (`item/completed`, `type: "plan"`).
pub fn codex_plan_text(item: &Value) -> String {
    let text = item.get("text").and_then(Value::as_str).unwrap_or_default();
    api::clean_cut(text, api::MAX_PLAN_ITEM).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claudes_plan_is_taken_and_denied() {
        let input = json!({"plan": "## Plan\n1. Add notes.txt\u{202e}\n2. Say hello", "planFilePath": "/home/u/.claude/plans/x.md"});
        assert_eq!(claude_plan(&input).as_deref(), Some("## Plan\n1. Add notes.txt\n2. Say hello"));
        assert_eq!(claude_plan(&json!({"plan": "  "})), None);
        assert_eq!(claude_plan(&json!({})), None);
        let long = json!({"plan": "x".repeat(api::MAX_PLAN_ITEM + 10)});
        assert_eq!(claude_plan(&long).unwrap().len(), api::MAX_PLAN_ITEM);
        let reply = claude_exit_reply();
        assert_eq!(reply.behavior, api::BridgeBehavior::Deny);
        assert_eq!(reply.message.as_deref(), Some(EXIT_PLAN_REPLY));
        assert_eq!(reply.updated_input, None);
    }

    #[test]
    fn the_plan_file_is_the_write_outside_the_folder() {
        let cwd = Path::new("/w/repo");
        let write = |path: &str| json!({"file_path": path, "content": "plan"});
        assert!(is_plan_file("Write", &write("/home/u/.claude/plans/quiet-river.md"), cwd));
        assert!(is_plan_file("Edit", &write("/home/u/.claude/plans/quiet-river.md"), cwd));
        assert!(!is_plan_file("Write", &write("/w/repo/notes.md"), cwd));
        assert!(!is_plan_file("Write", &write("notes.md"), cwd));
        assert!(!is_plan_file("Write", &write("/w/repo/../other.md"), cwd));
        assert!(!is_plan_file("Read", &write("/home/u/.claude/plans/x.md"), cwd));
        assert!(!is_plan_file("Write", &json!({}), cwd));
        assert_eq!(codex_plan_text(&json!({"type": "plan", "text": "1. Do it"})), "1. Do it");
        assert_eq!(codex_plan_text(&json!({"type": "plan"})), "");
    }
}
