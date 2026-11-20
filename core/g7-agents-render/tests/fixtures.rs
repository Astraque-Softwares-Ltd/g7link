// SPDX-License-Identifier: MPL-2.0
//! Every fixture under `tests/fixtures/` is loaded and compared in full.
//! The launcher re-implements the rules and reads the same `timeline/`,
//! `labels/`, `thought/` and `guard/` files.

mod common;

use common::{CtxJ, ItemJ, ToolJ, blessing, first_difference, fixtures, load, save, text_input};
use g7_agents_render::{
    GuardEvent, Timeline, TypingGuard, diff, live_thought_line, markdown, summary, tool_label,
};
use serde::Deserialize;
use serde_json::{Value, json};

fn check(path: &std::path::Path, mut fixture: Value, field: &str, got: Value) {
    if blessing() {
        fixture[field] = got;
        save(path, &fixture);
        return;
    }
    if let Some(d) = first_difference(field, &fixture[field], &got) {
        panic!("{}: {d}", path.display());
    }
}

#[test]
fn timeline() {
    for path in fixtures("timeline") {
        let fixture = load(&path);
        let items: Vec<ItemJ> = serde_json::from_value(fixture["items"].clone())
            .unwrap_or_else(|e| panic!("{}: items: {e}", path.display()));
        let ctx: CtxJ = serde_json::from_value(fixture["ctx"].clone())
            .unwrap_or_else(|e| panic!("{}: ctx: {e}", path.display()));
        let items_in: Vec<_> = items.iter().map(ItemJ::as_in).collect();
        let mut timeline = Timeline::new();
        let rows = timeline.rows(&items_in, &ctx.as_ctx());
        // The second call reads the caches; it must give the same rows.
        let again = timeline.rows(&items_in, &ctx.as_ctx());
        assert_eq!(rows, again, "{}: cached rows differ", path.display());
        let keys: std::collections::HashSet<_> = rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys.len(), rows.len(), "{}: duplicate keys", path.display());
        check(&path, fixture, "rows", serde_json::to_value(&rows).expect("rows serialize"));
    }
}

#[test]
fn labels() {
    for path in fixtures("labels") {
        let fixture = load(&path);
        if fixture.get("tool").is_some() {
            let tool: ToolJ = serde_json::from_value(fixture["tool"].clone()).expect("a tool");
            let got = serde_json::to_value(tool_label(&tool.as_in())).expect("labels serialize");
            check(&path, fixture, "label", got);
        } else {
            let tools: Vec<ToolJ> = serde_json::from_value(fixture["tools"].clone()).expect("tools");
            let tools_in: Vec<_> = tools.iter().map(ToolJ::as_in).collect();
            let got = serde_json::to_value(summary(&tools_in)).expect("summaries serialize");
            check(&path, fixture, "summary", got);
        }
    }
}

#[test]
fn thought() {
    for path in fixtures("thought") {
        let fixture = load(&path);
        let input = fixture["in"].as_str().expect("in is text");
        assert_eq!(Value::String(live_thought_line(input)), fixture["out"], "{}", path.display());
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum EventJ {
    Requests { ids: Vec<String>, now_ms: u64 },
    Typed { now_ms: u64 },
    Focus,
    Blur,
    Sent,
    Idle,
}

#[test]
fn guard() {
    for path in fixtures("guard") {
        let fixture = load(&path);
        let events: Vec<EventJ> = serde_json::from_value(fixture["events"].clone()).expect("events");
        let mut guard = TypingGuard::new();
        let mut held_after = Vec::new();
        let mut idle_after = Vec::new();
        let mut known: Vec<String> = Vec::new();
        for event in &events {
            match event {
                EventJ::Requests { ids, now_ms } => {
                    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                    guard.on(GuardEvent::Requests { ids: &refs, now_ms: *now_ms });
                    known.extend(ids.iter().cloned());
                }
                EventJ::Typed { now_ms } => guard.on(GuardEvent::Typed { now_ms: *now_ms }),
                EventJ::Focus => guard.on(GuardEvent::Focus),
                EventJ::Blur => guard.on(GuardEvent::Blur),
                EventJ::Sent => guard.on(GuardEvent::Sent),
                EventJ::Idle => guard.on(GuardEvent::Idle),
            }
            let mut held: Vec<&String> = known.iter().filter(|id| guard.held(id)).collect();
            held.sort();
            held.dedup();
            held_after.push(json!(held));
            idle_after.push(json!(guard.idle_at()));
        }
        assert_eq!(Value::Array(held_after), fixture["held_after"], "{}: held", path.display());
        assert_eq!(Value::Array(idle_after), fixture["idle_at_after"], "{}: idle_at", path.display());
    }
}

/// A summary of blocks, for inputs too large to keep their output in a file.
fn shape(doc: &g7_agents_render::Doc) -> Value {
    json!({
        "plain": doc.plain,
        "blocks": doc.blocks.iter().map(|b| json!({
            "kind": b.kind,
            "text_bytes": b.text.len(),
            "spans": b.spans.len(),
            "lines": b.lines.len(),
            "depth": b.depth,
            "quote": b.quote,
            "rows": b.table.as_ref().map(|t| t.cells.len() / usize::from(t.columns.max(1))),
            "more_rows": b.table.as_ref().map(|t| t.more_rows),
        })).collect::<Vec<_>>(),
    })
}

#[test]
fn markdown_fixtures() {
    for path in fixtures("markdown") {
        let fixture = load(&path);
        let input = text_input(&fixture["in"]);
        let doc = markdown(&input);
        if fixture.get("shape").is_some() {
            check(&path, fixture, "shape", shape(&doc));
        } else {
            check(&path, fixture, "doc", serde_json::to_value(&doc).expect("docs serialize"));
        }
    }
}

#[test]
fn diff_fixtures() {
    for path in fixtures("diff") {
        let fixture = load(&path);
        let input = text_input(&fixture["in"]);
        let doc = diff(&input);
        if fixture.get("shape").is_some() {
            let got = json!({
                "files": doc.files.len(),
                "rows": doc.files.iter().map(|f| f.rows.len()).sum::<usize>(),
                "adds": doc.adds,
                "dels": doc.dels,
                "more": doc.more,
            });
            check(&path, fixture, "shape", got);
        } else {
            check(&path, fixture, "doc", serde_json::to_value(&doc).expect("diffs serialize"));
        }
    }
}
