// SPDX-License-Identifier: MPL-2.0
//! The fixtures' JSON forms of the inputs, shared by the tests.
//!
//! An item is `ItemIn` with owned strings: `seq`, `id`, `kind` are required;
//! every other field may be left out (`0`, `""`, `false`, absent, `via`
//! `turn`). A tool is `ToolIn` the same way (`status` required, `class`
//! defaults to `other`); a question is `QuestionIn` (`state` defaults to
//! `asked`).
//! A context is `Ctx` with `marks`, `open` and `turns` as arrays (`turns`
//! may be left out). A turn is `TurnIn` the same way: `turn` and
//! `checkpoint` are required, `paths` is an array of `TurnFileIn` (`path`
//! and `status` required).

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use g7_agents_render::{
    Checkpoint, CheckpointWhy, Ctx, ItemIn, ItemKind, Mark, Phase, PlanState, QuestionIn, QuestionState,
    ThreadState, ToolClass, ToolIn, ToolStatus, TurnFileIn, TurnFileStatus, TurnIn, Via,
};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ItemJ {
    pub seq: u64,
    pub id: String,
    pub kind: ItemKind,
    #[serde(default)]
    pub ts_ms: u64,
    #[serde(default)]
    pub end_ms: Option<u64>,
    #[serde(default)]
    pub rev: u32,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub turn: u32,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub phase: Option<Phase>,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub deny_of: Option<String>,
    #[serde(default)]
    pub tool: Option<ToolJ>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub via: Via,
    #[serde(default)]
    pub plan_state: Option<PlanState>,
    #[serde(default)]
    pub question: Option<QuestionJ>,
    /// A word the crate doesn't know reads as absent, as on the wire.
    #[serde(default)]
    pub notice: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuestionJ {
    #[serde(default)]
    pub state: QuestionState,
    #[serde(default)]
    pub count: u8,
    #[serde(default)]
    pub by: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolJ {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub class: ToolClass,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub path: String,
    pub status: ToolStatus,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub by_rule: bool,
    #[serde(default)]
    pub reviewed: bool,
}

impl ToolJ {
    pub fn as_in(&self) -> ToolIn<'_> {
        ToolIn {
            name: &self.name,
            title: &self.title,
            class: self.class,
            detail: &self.detail,
            path: &self.path,
            status: self.status,
            output: &self.output,
            by_rule: self.by_rule,
            reviewed: self.reviewed,
        }
    }
}

impl ItemJ {
    pub fn as_in(&self) -> ItemIn<'_> {
        ItemIn {
            seq: self.seq,
            id: &self.id,
            kind: self.kind,
            ts_ms: self.ts_ms,
            end_ms: self.end_ms,
            rev: self.rev,
            text: &self.text,
            streaming: self.streaming,
            turn: self.turn,
            group: self.group.as_deref(),
            phase: self.phase,
            from: &self.from,
            deny_of: self.deny_of.as_deref(),
            tool: self.tool.as_ref().map(ToolJ::as_in),
            truncated: self.truncated,
            via: self.via,
            plan_state: self.plan_state,
            question: self.question.as_ref().map(|q| QuestionIn {
                state: q.state,
                count: q.count,
                by: &q.by,
            }),
            notice: self.notice.clone().and_then(|v| serde_json::from_value(v).ok()),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CtxJ {
    pub state: ThreadState,
    #[serde(default)]
    pub state_since_ms: u64,
    #[serde(default)]
    pub now_ms: u64,
    #[serde(default)]
    pub marks: Vec<Mark>,
    #[serde(default)]
    pub open: Vec<String>,
    #[serde(default)]
    pub turns: Vec<TurnJ>,
}

impl CtxJ {
    /// Each turn's files in borrowed form, for `turns_in`.
    pub fn files_in(&self) -> Vec<Vec<TurnFileIn<'_>>> {
        self.turns.iter().map(|t| t.paths.iter().map(TurnFileJ::as_in).collect()).collect()
    }

    /// The turns in borrowed form, for `as_ctx`.
    pub fn turns_in<'a>(&'a self, files: &'a [Vec<TurnFileIn<'a>>]) -> Vec<TurnIn<'a>> {
        self.turns.iter().zip(files).map(|(t, paths)| t.as_in(paths)).collect()
    }

    pub fn as_ctx<'a>(&'a self, turns: &'a [TurnIn<'a>]) -> Ctx<'a> {
        Ctx {
            state: self.state,
            state_since_ms: self.state_since_ms,
            now_ms: self.now_ms,
            marks: &self.marks,
            open: &self.open,
            turns,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TurnJ {
    pub turn: u32,
    pub checkpoint: Checkpoint,
    #[serde(default)]
    pub why: Option<CheckpointWhy>,
    #[serde(default)]
    pub ended_ms: Option<u64>,
    #[serde(default)]
    pub files: u32,
    #[serde(default)]
    pub adds: u32,
    #[serde(default)]
    pub dels: u32,
    #[serde(default)]
    pub skipped: u32,
    #[serde(default)]
    pub paths: Vec<TurnFileJ>,
    #[serde(default)]
    pub can_fork: bool,
    #[serde(default)]
    pub can_rewind: bool,
    #[serde(default)]
    pub can_restore: bool,
}

impl TurnJ {
    pub fn as_in<'a>(&self, paths: &'a [TurnFileIn<'a>]) -> TurnIn<'a> {
        TurnIn {
            turn: self.turn,
            checkpoint: self.checkpoint,
            why: self.why,
            ended_ms: self.ended_ms,
            files: self.files,
            adds: self.adds,
            dels: self.dels,
            skipped: self.skipped,
            paths,
            can_fork: self.can_fork,
            can_rewind: self.can_rewind,
            can_restore: self.can_restore,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TurnFileJ {
    pub path: String,
    pub status: TurnFileStatus,
    #[serde(default)]
    pub adds: u32,
    #[serde(default)]
    pub dels: u32,
}

impl TurnFileJ {
    pub fn as_in(&self) -> TurnFileIn<'_> {
        TurnFileIn { path: &self.path, status: self.status, adds: self.adds, dels: self.dels }
    }
}

pub fn fixtures(dir: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(dir);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no fixtures in {}", root.display());
    files
}

pub fn load(path: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Set `G7_RENDER_BLESS=1` to write what the crate produces into the
/// fixtures instead of comparing; review the result by hand.
pub fn blessing() -> bool {
    std::env::var_os("G7_RENDER_BLESS").is_some()
}

/// Writes a fixture: one top-level field per line, an array's elements one
/// per line, with control and bidi characters escaped so the file shows
/// what it holds.
pub fn save(path: &Path, value: &serde_json::Value) {
    let compact = |v: &serde_json::Value| serde_json::to_string(v).expect("JSON values serialize");
    let text = match value {
        serde_json::Value::Object(map) => {
            let fields: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    let v = match v {
                        serde_json::Value::Array(items) if !items.is_empty() => {
                            let lines: Vec<String> =
                                items.iter().map(|i| format!("    {}", compact(i))).collect();
                            format!("[\n{}\n  ]", lines.join(",\n"))
                        }
                        other => compact(other),
                    };
                    format!("  {}: {v}", compact(&serde_json::Value::String(k.clone())))
                })
                .collect();
            format!("{{\n{}\n}}", fields.join(",\n"))
        }
        other => compact(other),
    };
    let mut out = String::with_capacity(text.len() + 1);
    for c in text.chars() {
        if g7_agents_render::bounds::is_stripped_char(c) || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}')
        {
            out.push_str(&format!("\\u{:04x}", c as u32));
        } else {
            out.push(c);
        }
    }
    out.push('\n');
    std::fs::write(path, out).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

/// An input that is either a string or `{"repeat", "times", "before", "after"}`
/// (for inputs too large to keep in a file).
pub fn text_input(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(o) => {
            let s = |k: &str| o.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
            let times = o.get("times").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
            format!("{}{}{}", s("before"), s("repeat").repeat(times), s("after"))
        }
        _ => panic!("an input is a string or a repeat object"),
    }
}

/// The first place two JSON values differ, for a readable failure.
pub fn first_difference(path: &str, want: &serde_json::Value, got: &serde_json::Value) -> Option<String> {
    use serde_json::Value;
    match (want, got) {
        (Value::Object(a), Value::Object(b)) => {
            for k in a.keys().chain(b.keys()) {
                let (x, y) = (a.get(k).unwrap_or(&Value::Null), b.get(k).unwrap_or(&Value::Null));
                if let Some(d) = first_difference(&format!("{path}.{k}"), x, y) {
                    return Some(d);
                }
            }
            None
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
                if let Some(d) = first_difference(&format!("{path}[{i}]"), x, y) {
                    return Some(d);
                }
            }
            (a.len() != b.len()).then(|| format!("{path}: {} items expected, {} produced", a.len(), b.len()))
        }
        _ => (want != got).then(|| format!("{path}: expected {want}, produced {got}")),
    }
}
