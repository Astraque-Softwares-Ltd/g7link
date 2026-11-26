// SPDX-License-Identifier: MPL-2.0
//! One test per bound of contract §P2.3 (and phase 2's plan card and
//! composer, phase 3's changes and snippets): just under it nothing gives,
//! just over it the stated fallback happens.

use g7_agents_render::bounds::*;
use g7_agents_render::{
    BlockKind, Checkpoint, Ctx, GuardEvent, Hints, ItemIn, ItemKind, Link, NoteKind, Phase, Row, RowKind,
    ThreadState, Timeline, TokClass, ToolClass, ToolIn, ToolStatus, TurnFileIn, TurnFileStatus, TurnIn,
    TypingGuard, Via, changed_files, diff, guess, highlight, live_thought_line, markdown, markdown_with,
    mentions, output_lines, snippet, speakable, tool_label, trigger,
};

fn item<'a>(seq: u64, id: &'a str, kind: ItemKind, text: &'a str) -> ItemIn<'a> {
    ItemIn {
        seq,
        id,
        kind,
        ts_ms: seq,
        end_ms: None,
        rev: 0,
        text,
        streaming: false,
        turn: 1,
        group: None,
        phase: None,
        from: "",
        deny_of: None,
        tool: None,
        truncated: false,
        via: Via::Turn,
        plan_state: None,
        question: None,
        notice: None,
    }
}

fn tool<'a>(class: ToolClass, detail: &'a str, path: &'a str) -> ToolIn<'a> {
    ToolIn {
        name: "",
        title: "",
        class,
        detail,
        path,
        status: ToolStatus::Ok,
        output: "",
        by_rule: false,
        reviewed: false,
    }
}

fn ctx(state: ThreadState) -> Ctx<'static> {
    Ctx { state, state_since_ms: 0, now_ms: 0, marks: &[], open: &[], turns: &[] }
}

#[test]
fn max_src_bytes() {
    let at = "a".repeat(MAX_SRC_BYTES);
    assert_eq!(markdown(&at).blocks[0].text.len(), MAX_SRC_BYTES);
    let over = format!("{at}bbb");
    let doc = markdown(&over);
    assert_eq!(doc.blocks[0].text, at);
    assert!(!doc.plain);
    // A character across the cut is dropped whole.
    let straddle = format!("{}é", "a".repeat(MAX_SRC_BYTES - 1));
    assert_eq!(markdown(&straddle).blocks[0].text.len(), MAX_SRC_BYTES - 1);
}

#[test]
fn max_blocks() {
    let at = "a\n\n".repeat(MAX_BLOCKS);
    let doc = markdown(&at);
    assert_eq!(doc.blocks.len(), MAX_BLOCKS);
    assert!(!doc.plain);
    let over = "a\n\n".repeat(MAX_BLOCKS + 1);
    let doc = markdown(&over);
    assert!(doc.plain);
    assert_eq!(doc.blocks.len(), 1);
    assert_eq!(doc.blocks[0].text, over);
    assert!(doc.blocks[0].spans.is_empty());
}

#[test]
fn max_depth() {
    let quotes = |n: usize| format!("{} x", ">".repeat(n));
    assert_eq!(markdown(&quotes(usize::from(MAX_DEPTH))).blocks[0].quote, MAX_DEPTH);
    assert_eq!(markdown(&quotes(usize::from(MAX_DEPTH) + 5)).blocks[0].quote, MAX_DEPTH);
    let lists = |n: usize| (0..n).map(|k| format!("{}- l{k}\n", "  ".repeat(k))).collect::<String>();
    let doc = markdown(&lists(usize::from(MAX_DEPTH) + 1));
    assert_eq!(doc.blocks.last().map(|b| b.depth), Some(MAX_DEPTH));
    let doc = markdown(&lists(usize::from(MAX_DEPTH) + 4));
    assert!(doc.blocks.iter().all(|b| b.depth <= MAX_DEPTH));
    assert_eq!(doc.blocks.last().map(|b| b.depth), Some(MAX_DEPTH));
}

#[test]
fn max_spans_per_block() {
    let at = "*a* ".repeat(MAX_SPANS_PER_BLOCK);
    assert_eq!(markdown(&at).blocks[0].spans.len(), MAX_SPANS_PER_BLOCK);
    let over = "*a* ".repeat(MAX_SPANS_PER_BLOCK + 1);
    let doc = markdown(&over);
    assert!(doc.blocks[0].spans.is_empty());
    assert_eq!(doc.blocks[0].text, "a ".repeat(MAX_SPANS_PER_BLOCK + 1).trim_end());
}

#[test]
fn max_table_rows_and_cols() {
    let table = |rows: usize| format!("| a | b |\n|---|---|\n{}", "| 1 | 2 |\n".repeat(rows));
    let t = markdown(&table(MAX_TABLE_ROWS)).blocks[0].table.clone().expect("a table");
    assert_eq!((t.cells.len(), t.more_rows), (2 * (MAX_TABLE_ROWS + 1), 0));
    let t = markdown(&table(MAX_TABLE_ROWS + 1)).blocks[0].table.clone().expect("a table");
    assert_eq!((t.cells.len(), t.more_rows), (2 * (MAX_TABLE_ROWS + 1), 1));
    let wide = |cols: usize| format!("|{}\n|{}\n", "h|".repeat(cols), "-|".repeat(cols));
    assert_eq!(markdown(&wide(MAX_TABLE_COLS)).blocks[0].kind, BlockKind::Table);
    let doc = markdown(&wide(MAX_TABLE_COLS + 1));
    assert_eq!(doc.blocks[0].kind, BlockKind::Code);
    assert_eq!(doc.blocks[0].text, wide(MAX_TABLE_COLS + 1).trim_end());
}

#[test]
fn max_href_bytes() {
    let link = |len: usize| {
        let href = format!("https://x.example/{}", "a".repeat(len - "https://x.example/".len()));
        markdown(&format!("[t]({href})")).blocks[0].spans.first().map(|s| s.style.link)
    };
    assert_eq!(link(MAX_HREF_BYTES), Some(Link::Web));
    assert_eq!(link(MAX_HREF_BYTES + 1), None);
}

#[test]
fn max_code_bytes_lines_and_line_units() {
    // Plain text: the bound is what is tested, not a grammar's speed.
    let line = "a".repeat(1_000);
    let mut at = vec![line.as_str(); MAX_CODE_BYTES / 1_001].join("\n");
    at.push('\n');
    at.push_str(&"b".repeat(MAX_CODE_BYTES - at.len()));
    assert_eq!(at.len(), MAX_CODE_BYTES);
    assert!(!highlight("txt", &at).is_empty());
    assert!(highlight("txt", &format!("{at}b")).is_empty());

    let lines = |n: usize| vec!["x"; n].join("\n");
    assert_eq!(highlight("txt", &lines(MAX_CODE_LINES)).len(), MAX_CODE_LINES);
    assert!(highlight("txt", &lines(MAX_CODE_LINES + 1)).is_empty());

    // Units, not bytes: an astral character is two.
    let wide = |units: usize| format!("{}{}", "😀".repeat(units / 2), "a".repeat(units % 2));
    assert_eq!(highlight("txt", &wide(MAX_CODE_LINE_UNITS)).len(), 1);
    assert!(highlight("txt", &wide(MAX_CODE_LINE_UNITS + 1)).is_empty());
}

#[test]
fn highlight_steps() {
    let short = "let x = 1;\n".repeat(20);
    let lines = highlight("rust", &short);
    assert!(lines.iter().all(|l| l.toks.iter().any(|t| t.class == TokClass::Keyword)));
    // About 400 steps a line here: the budget runs out part of the way.
    let long = "let x = 1;\n".repeat(MAX_CODE_LINES);
    let lines = highlight("rust", &long);
    assert_eq!(lines.len(), MAX_CODE_LINES);
    let coloured = lines.iter().take_while(|l| l.toks.iter().any(|t| t.class == TokClass::Keyword)).count();
    assert!(coloured > 100 && coloured < MAX_CODE_LINES, "{coloured} lines coloured");
    assert!(lines[coloured..].iter().all(|l| l.toks.iter().all(|t| t.class == TokClass::Plain)));
    for l in &lines {
        assert_eq!(l.toks.iter().map(|t| t.len as usize).sum::<usize>(), l.text.len());
    }
}

#[test]
fn max_diff_bytes() {
    // Long lines, so the row bound is not reached first.
    let line = format!("+{}\n", "x".repeat(2_000));
    let fit = MAX_DIFF_BYTES / line.len();
    let doc = diff(&line.repeat(fit));
    assert!(!doc.more);
    assert_eq!(doc.adds as usize, fit);
    let doc = diff(&line.repeat(fit + 50));
    assert!(doc.more);
    assert_eq!(doc.adds as usize, fit + 1, "lines past the cut are not read; the cut one is");
}

#[test]
fn max_diff_rows() {
    // A file and its rows share the budget.
    let at = "+x\n".repeat(MAX_DIFF_ROWS - 1);
    let doc = diff(&at);
    assert!(!doc.more);
    assert_eq!(doc.files[0].rows.len(), MAX_DIFF_ROWS - 1);
    let over = "+x\n".repeat(MAX_DIFF_ROWS);
    let doc = diff(&over);
    assert!(doc.more);
    assert_eq!(doc.files[0].rows.len(), MAX_DIFF_ROWS - 1);
    assert_eq!(doc.adds as usize, MAX_DIFF_ROWS, "dropped rows are still counted");
}

#[test]
fn max_diff_line_units() {
    let at = "a".repeat(MAX_DIFF_LINE_UNITS);
    let doc = diff(&format!("-{at}\n+{at}b"));
    assert_eq!(doc.files[0].rows[0].text.len(), MAX_DIFF_LINE_UNITS);
    assert_eq!(doc.files[0].rows[1].text.len(), MAX_DIFF_LINE_UNITS);
    assert!(doc.files[0].rows[1].marks.is_empty(), "a cut line has no marks");
    let astral = "😀".repeat(MAX_DIFF_LINE_UNITS / 2 + 1);
    let doc = diff(&format!("+{astral}"));
    assert_eq!(doc.files[0].rows[0].text.chars().count(), MAX_DIFF_LINE_UNITS / 2);
}

#[test]
fn word_diff_max_units() {
    let pair = |units: usize| {
        let body = " w".repeat(units / 2);
        let old = format!("{}{}", &body[..units - 3], "old");
        let new = format!("{}{}", &body[..units - 3], "new");
        diff(&format!("-{old}\n+{new}")).files[0].rows[0].marks.clone()
    };
    assert_eq!(pair(WORD_DIFF_MAX_UNITS), [(u32::try_from(WORD_DIFF_MAX_UNITS - 3).unwrap(), 3)]);
    assert!(pair(WORD_DIFF_MAX_UNITS + 1).is_empty());
}

#[test]
fn max_items_in() {
    let ids: Vec<String> = (0..=MAX_ITEMS_IN).map(|k| format!("n{k}")).collect();
    let items: Vec<ItemIn<'_>> =
        ids.iter().enumerate().map(|(k, id)| item(k as u64 + 1, id, ItemKind::Notice, "x")).collect();
    let rows = Timeline::new().rows(&items[1..], &ctx(ThreadState::Done));
    assert_eq!(rows.len(), MAX_ITEMS_IN);
    assert_eq!(rows[0].key, "i:n1");
    let rows = Timeline::new().rows(&items, &ctx(ThreadState::Done));
    assert_eq!(rows.len(), MAX_ITEMS_IN);
    assert_eq!(rows[0].key, "i:n1", "the oldest item is dropped");
}

#[test]
fn max_rows_out() {
    let text = "a\n\n".repeat(MAX_BLOCKS);
    let ids: Vec<String> = (0..11).map(|k| format!("a{k}")).collect();
    let mut items = vec![item(1, "u", ItemKind::User, "go")];
    for (k, id) in ids.iter().enumerate() {
        items.push(item(k as u64 + 2, id, ItemKind::Assistant, &text));
    }
    let rows = Timeline::new().rows(&items[..10], &ctx(ThreadState::Interrupted));
    assert_eq!(rows.len(), 1 + 9 * MAX_BLOCKS);
    let rows = Timeline::new().rows(&items, &ctx(ThreadState::Interrupted));
    assert_eq!(rows.len(), MAX_ROWS_OUT);
    assert_eq!(rows.last().map(|r| r.key.as_str()), Some(format!("b:a10:{}", MAX_BLOCKS - 1).as_str()));
    assert!(rows.iter().all(|r| r.key != "i:u"), "the oldest rows are dropped");
}

#[test]
fn group_min() {
    let mut a = item(2, "t1", ItemKind::Tool, "");
    a.tool = Some(tool(ToolClass::Command, "ls", ""));
    let mut b = item(3, "t2", ItemKind::Tool, "");
    b.tool = Some(tool(ToolClass::Command, "pwd", ""));
    let user = item(1, "u", ItemKind::User, "go");
    let kinds = |items: &[ItemIn<'_>]| -> Vec<&'static str> {
        Timeline::new().rows(items, &ctx(ThreadState::Interrupted)).iter().map(|r| r.kind.name()).collect()
    };
    assert_eq!(GROUP_MIN, 2);
    assert_eq!(kinds(&[user, a]), ["user", "tool"]);
    assert_eq!(kinds(&[user, a, b]), ["user", "group"]);
}

#[test]
fn max_changed_files() {
    let paths: Vec<String> = (0..=MAX_CHANGED_FILES).map(|k| format!("f{k:02}")).collect();
    let ids: Vec<String> = (0..=MAX_CHANGED_FILES).map(|k| format!("e{k}")).collect();
    let items: Vec<ItemIn<'_>> = paths
        .iter()
        .zip(&ids)
        .enumerate()
        .map(|(k, (p, id))| {
            let mut it = item(k as u64 + 1, id, ItemKind::Tool, "");
            it.tool = Some(tool(ToolClass::Edit, "+x", p));
            it
        })
        .collect();
    let (files, adds, _, more) = changed_files(&items[..MAX_CHANGED_FILES]);
    assert_eq!((files.len(), more), (MAX_CHANGED_FILES, 0));
    let (files, all_adds, _, more) = changed_files(&items);
    assert_eq!((files.len(), more), (MAX_CHANGED_FILES, 1));
    assert_eq!((adds, all_adds), (20, 21), "totals count every file");
    assert_eq!(files.last().map(|f| f.path.as_str()), Some("f19"));
}

#[test]
fn label_units() {
    let at = "x".repeat(LABEL_UNITS);
    assert_eq!(tool_label(&tool(ToolClass::Command, &at, "")).object, at);
    let over = "x".repeat(LABEL_UNITS + 1);
    let object = tool_label(&tool(ToolClass::Command, &over, "")).object;
    assert_eq!(object.encode_utf16().count(), LABEL_UNITS);
    assert!(object.ends_with('…'));
}

#[test]
fn thought_preview_units() {
    let at = "x".repeat(THOUGHT_PREVIEW_UNITS);
    assert_eq!(live_thought_line(&at), at);
    let over = "x".repeat(THOUGHT_PREVIEW_UNITS + 1);
    let line = live_thought_line(&over);
    assert_eq!(line.encode_utf16().count(), THOUGHT_PREVIEW_UNITS);
    assert!(line.ends_with('…'));
    // The thought row and the live line are both cut.
    let mut r = item(2, "r", ItemKind::Reasoning, &over);
    r.phase = None;
    let rows = Timeline::new().rows(&[item(1, "u", ItemKind::User, "go"), r], &ctx(ThreadState::Working));
    for row in &rows {
        match &row.kind {
            RowKind::Thought { preview, .. } => assert_eq!(preview, &line),
            RowKind::Live { thought, .. } => assert_eq!(thought, &line),
            _ => {}
        }
    }
}

#[test]
fn typing_idle_ms() {
    let mut g = TypingGuard::new();
    g.on(GuardEvent::Focus);
    g.on(GuardEvent::Typed { now_ms: 10_000 });
    g.on(GuardEvent::Requests { ids: &["a"], now_ms: 10_000 + TYPING_IDLE_MS - 1 });
    g.on(GuardEvent::Requests { ids: &["a", "b"], now_ms: 10_000 + TYPING_IDLE_MS });
    assert!(g.held("a"));
    assert!(!g.held("b"));
    assert_eq!(g.idle_at(), Some(10_000 + TYPING_IDLE_MS));
}

fn answers(n: usize, text: &str) -> (Vec<String>, String) {
    ((0..n).map(|k| format!("a{k}")).collect(), text.to_owned())
}

#[test]
fn cache_entries_and_bytes() {
    let (ids, text) = answers(CACHE_ENTRIES + 10, "short answer");
    let mut items = Vec::new();
    for (k, id) in ids.iter().enumerate() {
        let mut a = item(k as u64 + 1, id, ItemKind::Assistant, &text);
        a.phase = Some(Phase::Interim);
        items.push(a);
    }
    let mut timeline = Timeline::new();
    let first = timeline.rows(&items, &ctx(ThreadState::Working));
    assert!(timeline.cached().0 <= CACHE_ENTRIES);
    assert_eq!(timeline.rows(&items, &ctx(ThreadState::Working)), first, "eviction never changes rows");

    // Large texts: the byte bound evicts first.
    let big = "word ".repeat(12_000);
    let (ids, _) = answers(200, "");
    let items: Vec<ItemIn<'_>> =
        ids.iter().enumerate().map(|(k, id)| item(k as u64 + 1, id, ItemKind::Assistant, &big)).collect();
    let mut timeline = Timeline::new();
    let rows: Vec<Row> = timeline.rows(&items, &ctx(ThreadState::Interrupted));
    let (entries, bytes) = timeline.cached();
    assert!(bytes <= CACHE_BYTES, "{bytes}");
    assert!(entries < 200);
    assert_eq!(rows.len(), 200);
}

/// The plan card's (shown, more) for a plan of `lines` one-line paragraphs.
fn plan_card(lines: usize, open: bool) -> (u16, u16) {
    let text: String = (0..lines).map(|k| format!("Step {k}.\n\n")).collect();
    let plan = item(1, "p1", ItemKind::Plan, &text);
    let keys = if open { vec!["p:p1".to_owned()] } else { Vec::new() };
    let rows = Timeline::new().rows(
        &[plan],
        &Ctx {
            state: ThreadState::Working,
            state_since_ms: 0,
            now_ms: 0,
            marks: &[],
            open: &keys,
            turns: &[],
        },
    );
    match rows[0].kind {
        RowKind::Plan { shown, more, .. } => (shown, more),
        _ => panic!("a plan row first"),
    }
}

#[test]
fn plan_fold_lines() {
    let at = PLAN_FOLD_LINES;
    assert_eq!(plan_card(at, false), (at as u16, 0));
    assert_eq!(plan_card(at + 1, false), (at as u16, 1));
    assert_eq!(plan_card(at + 1, true), (at as u16 + 1, 0));
    // A first block longer than the card still shows.
    let code = format!("```\n{}```", "x\n".repeat(at + 5));
    let plan = item(1, "p1", ItemKind::Plan, &code);
    let rows = Timeline::new().rows(&[plan], &ctx(ThreadState::Working));
    assert!(matches!(rows[0].kind, RowKind::Plan { shown: 1, more: 0, .. }));
}

#[test]
fn max_trigger_query_units() {
    let at = format!("see @{}", "😀".repeat(MAX_TRIGGER_QUERY_UNITS / 2));
    let cursor = at.encode_utf16().count() as u32;
    assert_eq!(trigger(&at, cursor).map(|t| t.query.encode_utf16().count()), Some(MAX_TRIGGER_QUERY_UNITS));
    let over = format!("{at}a");
    assert!(trigger(&over, cursor + 1).is_none());
    let slash = format!("/{}", "a".repeat(MAX_TRIGGER_QUERY_UNITS + 1));
    assert!(trigger(&slash, MAX_TRIGGER_QUERY_UNITS as u32 + 1).is_some());
    assert!(trigger(&slash, MAX_TRIGGER_QUERY_UNITS as u32 + 2).is_none());
}

#[test]
fn max_mentions() {
    assert_eq!(mentions(&"@a ".repeat(MAX_MENTIONS)).len(), MAX_MENTIONS);
    let over = "@a ".repeat(MAX_MENTIONS + 1);
    let found = mentions(&over);
    assert_eq!(found.len(), MAX_MENTIONS);
    assert_eq!(found.last(), Some(&(3 * (MAX_MENTIONS as u32 - 1), 2)));
}

/// A finished turn of one message and one answer, with the host's word on it.
fn turn_rows(turn: &TurnIn<'_>, now_ms: u64) -> Vec<Row> {
    let mut answer = item(2, "a", ItemKind::Assistant, "Done.");
    answer.end_ms = Some(3);
    let items = [item(1, "u", ItemKind::User, "Go"), answer];
    let ctx = Ctx { now_ms, turns: std::slice::from_ref(turn), ..ctx(ThreadState::Done) };
    Timeline::new().rows(&items, &ctx)
}

fn ready_turn<'a>(files: u32, paths: &'a [TurnFileIn<'a>]) -> TurnIn<'a> {
    TurnIn {
        turn: 1,
        checkpoint: Checkpoint::Ready,
        why: None,
        ended_ms: Some(5_000),
        files,
        adds: 1,
        dels: 1,
        skipped: 0,
        paths,
        can_fork: true,
        can_rewind: true,
        can_restore: true,
    }
}

#[test]
fn max_changed_files_recorded() {
    let names: Vec<String> = (0..=MAX_CHANGED_FILES).map(|k| format!("f{k:02}")).collect();
    let paths: Vec<TurnFileIn<'_>> = names
        .iter()
        .map(|p| TurnFileIn { path: p, status: TurnFileStatus::Modified, adds: 1, dels: 0 })
        .collect();
    let changes = |turn: &TurnIn<'_>| {
        turn_rows(turn, 9_000).into_iter().find_map(|r| match r.kind {
            RowKind::Changes { files, more, recorded: true, .. } => Some((files.len(), more)),
            _ => None,
        })
    };
    let at = ready_turn(MAX_CHANGED_FILES as u32, &paths[..MAX_CHANGED_FILES]);
    assert_eq!(changes(&at), Some((MAX_CHANGED_FILES, 0)));
    let over = ready_turn(MAX_CHANGED_FILES as u32 + 1, &paths);
    assert_eq!(changes(&over), Some((MAX_CHANGED_FILES, 1)));
}

#[test]
fn recording_after_ms() {
    let pending = TurnIn { checkpoint: Checkpoint::Pending, ..ready_turn(0, &[]) };
    let recording = |now_ms: u64| {
        turn_rows(&pending, now_ms)
            .iter()
            .any(|r| matches!(r.kind, RowKind::ChangesNote { note: NoteKind::Recording, .. }))
    };
    assert!(!recording(5_000 + RECORDING_AFTER_MS - 1));
    assert!(recording(5_000 + RECORDING_AFTER_MS));
    // A clock behind the turn's end shows nothing.
    assert!(!recording(1_000));
}

#[test]
fn max_snippet_units() {
    let units =
        |text: &str| -> usize { snippet(text, &[]).iter().map(|p| p.text.encode_utf16().count()).sum() };
    let at = "a".repeat(MAX_SNIPPET_UNITS);
    assert_eq!(units(&at), MAX_SNIPPET_UNITS);
    assert_eq!(units(&format!("{at}b")), MAX_SNIPPET_UNITS);
    // A character across the cut is dropped whole.
    let straddle = format!("{}😀", "a".repeat(MAX_SNIPPET_UNITS - 1));
    assert_eq!(units(&straddle), MAX_SNIPPET_UNITS - 1);
}

#[test]
fn max_snippet_marks() {
    let text = "x ".repeat(MAX_SNIPPET_MARKS + 1);
    let marks: Vec<(u32, u32)> = (0..=MAX_SNIPPET_MARKS as u32).map(|k| (2 * k, 1)).collect();
    let marked = |marks: &[(u32, u32)]| snippet(&text, marks).iter().filter(|p| p.mark).count();
    assert_eq!(marked(&marks[..MAX_SNIPPET_MARKS]), MAX_SNIPPET_MARKS);
    assert_eq!(marked(&marks), MAX_SNIPPET_MARKS);
}

/// One line that is Rust on its own (5 points).
const RUST_LINE: &str = "fn main() { println!(\"hi\"); }";

#[test]
fn max_detect_lines() {
    let at = format!("{}{RUST_LINE}", "word\n".repeat(MAX_DETECT_LINES - 1));
    assert_eq!(guess(&at, None), Some("rust"));
    let over = format!("{}{RUST_LINE}", "word\n".repeat(MAX_DETECT_LINES));
    assert_eq!(guess(&over, None), None);
    // Blank lines are not counted.
    let blanks = format!("{}{RUST_LINE}", "\n \n".repeat(MAX_DETECT_LINES));
    assert_eq!(guess(&blanks, None), Some("rust"));
}

#[test]
fn max_detect_bytes() {
    let filler = |n: usize| format!("{}\n", "x".repeat(n - 1));
    let at = format!("{}{RUST_LINE}", filler(MAX_DETECT_BYTES - RUST_LINE.len()));
    assert_eq!(guess(&at, None), Some("rust"));
    let over = format!("{}{RUST_LINE}", filler(MAX_DETECT_BYTES));
    assert_eq!(guess(&over, None), None);
}

#[test]
fn max_places_per_line() {
    let places = |n: usize| {
        let line = "see src/a.rs:1 ".repeat(n);
        output_lines(&format!("error: x\n{line}"))
            .get(1)
            .map_or(0, |l| l.toks.iter().filter(|t| t.class == TokClass::Pos).count())
    };
    assert_eq!(places(MAX_PLACES_PER_LINE), MAX_PLACES_PER_LINE);
    assert_eq!(places(MAX_PLACES_PER_LINE + 1), MAX_PLACES_PER_LINE);
}

#[test]
fn max_path_hints() {
    let names: Vec<String> = (0..=MAX_PATH_HINTS).map(|k| format!("src/f{k}.rs")).collect();
    let paths: Vec<&str> = names.iter().map(String::as_str).collect();
    let marked = |k: usize| {
        let doc = markdown_with(&format!("see `f{k}.rs`"), Hints { ext: None, paths: &paths });
        doc.blocks[0].spans.iter().any(|s| s.style.path)
    };
    assert!(marked(MAX_PATH_HINTS - 1));
    assert!(!marked(MAX_PATH_HINTS));
}

#[test]
fn max_speakable_units() {
    let words = "word ".repeat(MAX_SPEAKABLE_UNITS);
    let at = speakable(&words, u32::try_from(MAX_SPEAKABLE_UNITS).unwrap());
    let over = speakable(&words, u32::MAX);
    assert_eq!(at, over, "a larger max reads as the bound");
    assert!(over.encode_utf16().count() <= MAX_SPEAKABLE_UNITS);
    assert!(over.encode_utf16().count() > MAX_SPEAKABLE_UNITS - 10, "{}", over.len());
    assert!(over.ends_with("word…"));
    let short = "Done.".to_owned();
    assert_eq!(speakable(&short, u32::MAX), short);
}
