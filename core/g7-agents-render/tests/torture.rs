// SPDX-License-Identifier: MPL-2.0
//! Fixed-seed random inputs, built from Markdown and diff fragments and
//! random bytes made valid UTF-8, through every public function. Nothing
//! may panic, and every bound and invariant the clients rely on must hold.

use std::collections::HashSet;

use g7_agents_render::bounds::*;
use g7_agents_render::{
    ALIASES, Block, CodeLine, Ctx, DetailOf, DiffDoc, ItemIn, ItemKind, Link, Mark, MarkKind, Phase, Row,
    RowKind, Span, ThreadState, Timeline, ToolClass, ToolIn, ToolStatus, diff, highlight, live_thought_line,
    markdown, summary, tool_label,
};

/// Inputs per run, sized to finish well inside 20 s in a debug build
/// (contract §P2.9: lower the count, keep the checks). Measured on the
/// reference laptop: about 1 ms an input without highlighting, 35 ms with
/// it (the grammars' regular expressions run some fifty times slower
/// unoptimised), so inputs with fenced code in a known language, and the
/// `highlight` calls, are fewer.
const INPUTS: usize = 10_000;
const CODE_INPUTS: usize = 250;
const HIGHLIGHT_EVERY: usize = 2;
const TIMELINE_EVERY: usize = 10;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

const MARKDOWN: &[&str] = &[
    "# ",
    "## ",
    "###### ",
    "####### ",
    "\n",
    "\n\n",
    "  ",
    "    ",
    "\t",
    "> ",
    ">> ",
    "> [!NOTE]\n",
    "> [!WARNING]\n",
    "- ",
    "* ",
    "+ ",
    "1. ",
    "2) ",
    "10. ",
    "- [ ] ",
    "- [x] ",
    "---\n",
    "***",
    "___",
    "===\n",
    "```",
    "~~~",
    "```\n",
    "```nolang\n",
    "```mermaid\n",
    "\n```\n",
    "`",
    "``",
    "*",
    "**",
    "_",
    "__",
    "~~",
    "[",
    "]",
    "(",
    ")",
    "![",
    "](",
    "[x](https://a.example)",
    "[y](javascript:alert(1))",
    "[z](data:x)",
    "[w](mailto:a@b.example)",
    "<https://auto.example>",
    "<a@b.example>",
    "[r][1]",
    "[1]: https://def.example\n",
    "![i](https://img.example/a.png)",
    "|",
    "| a | b |\n",
    "|---|:-:|\n",
    "| 1 | 2 |\n",
    "<img src=x onerror=y>",
    "<script>",
    "</script>",
    "<div>\n",
    "</div>\n",
    "<!--",
    "-->",
    "&amp;",
    "&#x202e;",
    "\\",
    "\\*",
    "$x$",
    "$$\n",
    "[^1]",
    "word",
    "words and more",
    "fn main() { let x = 1; }",
    "😀",
    "日本語。",
    "é",
    "e\u{301}",
    "\u{a0}",
    "\u{202e}",
    "\u{2066}",
    "\u{200f}",
    "\u{0}",
    "\u{7}",
    "\u{85}",
    "\u{9b}",
    "\r\n",
    "\r",
    "\u{2028}",
    "\u{feff}",
    "\u{fffd}",
    "\\ud800",
    "%",
    "&",
    "\"",
    "'",
    "**Heading**\n",
    ". ",
    "? ",
    "! ",
    "。",
];

const DIFF: &[&str] = &[
    "diff --git a/x.rs b/x.rs\n",
    "--- a/x.rs\n",
    "+++ b/x.rs\n",
    "--- /dev/null\n",
    "+++ /dev/null\n",
    "@@ -1,3 +1,3 @@\n",
    "@@ -0,0 +1 @@\n",
    "@@ -10 +10,2 @@ fn x\n",
    "@@\n",
    "@@ garbage\n",
    "+added line\n",
    "-removed line\n",
    " context line\n",
    "+\n",
    "-\n",
    " \n",
    "\\ No newline at end of file\n",
    "index 1..2\n",
    "new file mode 100644\n",
    "rename from a\n",
    "rename to b\n",
    "*** Begin Patch\n",
    "*** Update File: a.py\n",
    "*** Add File: b.py\n",
    "*** Delete File: c.py\n",
    "*** Move to: d.py\n",
    "*** End Patch\n",
    "notes.txt (add)\n",
    "src/y.rs (update)\n",
    "old.txt (delete)\n",
    "-let a = 1;\n+let a = 2;\n",
    "+😀 é\n",
    "-\u{202e}x\n",
];

/// Fences that name a language, so their code is highlighted.
const FENCES: &[&str] = &[
    "```rust\n",
    "```py\n",
    "```ts title=a.ts\n",
    "```sh\n",
    "```diff\n",
    "```c++\n",
    "```yaml\n",
    "```md\n",
];

fn build(rng: &mut Rng, code: bool) -> String {
    let mut s = String::new();
    let parts = 1 + rng.below(60);
    for _ in 0..parts {
        match rng.below(10) {
            0 if code => s.push_str(rng.pick(FENCES)),
            0..=5 => s.push_str(rng.pick(MARKDOWN)),
            6..=7 => s.push_str(rng.pick(DIFF)),
            8 => {
                let n = rng.below(40);
                let bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
                s.push_str(&String::from_utf8_lossy(&bytes));
            }
            _ => {
                let piece = rng.pick(MARKDOWN);
                for _ in 0..rng.below(30) {
                    s.push_str(piece);
                }
            }
        }
    }
    s
}

/// The UTF-16 offsets at which `text` may be cut: every character boundary.
fn boundaries(text: &str) -> HashSet<u32> {
    let mut at = 0u32;
    let mut set = HashSet::from([0]);
    for c in text.chars() {
        at += c.len_utf16() as u32;
        set.insert(at);
    }
    set
}

fn clean(s: &str) -> bool {
    !s.chars().any(is_stripped_char)
}

fn check_spans(spans: &[Span], text: &str, what: &str) {
    assert!(spans.len() <= MAX_SPANS_PER_BLOCK, "{what}: spans");
    let edges = boundaries(text);
    let mut end = 0u32;
    for span in spans {
        assert!(span.len > 0, "{what}: empty span");
        assert!(span.start >= end, "{what}: spans overlap or are out of order");
        end = span.start + span.len;
        assert!(edges.contains(&span.start) && edges.contains(&end), "{what}: span splits a character");
        if span.style.link == Link::None {
            assert!(span.href.is_empty(), "{what}: href without a link");
        } else {
            let lower = span.href.to_ascii_lowercase();
            assert!(
                lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("mailto:"),
                "{what}: unsafe href {:?}",
                span.href
            );
            assert!(span.href.len() <= MAX_HREF_BYTES, "{what}: long href");
            assert!(clean(&span.href), "{what}: href not cleaned");
        }
        if span.style.image {
            assert_ne!(span.style.link, Link::None, "{what}: an image mark without a link");
        }
    }
}

fn check_lines(lines: &[CodeLine], what: &str) {
    assert!(lines.len() <= MAX_CODE_LINES, "{what}: code lines");
    for line in lines {
        let units: u32 = line.toks.iter().map(|t| t.len).sum();
        assert_eq!(units as usize, line.text.encode_utf16().count(), "{what}: tokens do not cover the line");
        assert!(line.toks.iter().all(|t| t.len > 0), "{what}: empty token");
        let edges = boundaries(&line.text);
        let mut at = 0;
        for t in &line.toks {
            at += t.len;
            assert!(edges.contains(&at), "{what}: token splits a character");
        }
        assert!(clean(&line.text), "{what}: code line not cleaned");
    }
}

fn check_block(b: &Block, what: &str) {
    assert!(b.depth <= MAX_DEPTH && b.quote <= MAX_DEPTH, "{what}: depth");
    assert!(b.lang.len() <= 32, "{what}: language word");
    assert!(clean(&b.text), "{what}: text not cleaned");
    check_spans(&b.spans, &b.text, what);
    check_lines(&b.lines, what);
    if let Some(t) = &b.table {
        let cols = usize::from(t.columns);
        assert!(cols <= MAX_TABLE_COLS && cols > 0, "{what}: columns");
        assert_eq!(t.align.len(), cols, "{what}: alignments");
        assert_eq!(t.cells.len() % cols, 0, "{what}: ragged table");
        assert!(t.cells.len() / cols <= MAX_TABLE_ROWS + 1, "{what}: rows");
        for c in &t.cells {
            assert!(clean(&c.text), "{what}: cell not cleaned");
            check_spans(&c.spans, &c.text, what);
        }
    }
}

fn check_diff(doc: &DiffDoc, what: &str) {
    let rows: usize = doc.files.iter().map(|f| f.rows.len()).sum();
    assert!(rows + doc.files.len() <= MAX_DIFF_ROWS, "{what}: diff rows");
    let (mut adds, mut dels) = (0u32, 0u32);
    for f in &doc.files {
        assert!(clean(&f.path), "{what}: path not cleaned");
        adds += f.adds;
        dels += f.dels;
        for r in &f.rows {
            assert!(r.text.encode_utf16().count() <= MAX_DIFF_LINE_UNITS, "{what}: diff line");
            assert!(clean(&r.text), "{what}: diff line not cleaned");
            let edges = boundaries(&r.text);
            let mut end = 0;
            for &(s, l) in &r.marks {
                assert!(l > 0 && s >= end, "{what}: marks out of order");
                end = s + l;
                assert!(edges.contains(&s) && edges.contains(&end), "{what}: mark splits a character");
            }
        }
    }
    assert!(adds <= doc.adds && dels <= doc.dels, "{what}: file counts exceed the totals");
}

fn check_rows(rows: &[Row], what: &str) {
    assert!(rows.len() <= MAX_ROWS_OUT, "{what}: rows");
    let mut keys = HashSet::new();
    for r in rows {
        assert!(keys.insert(r.key.as_str()), "{what}: duplicate key {}", r.key);
        match &r.kind {
            RowKind::Block { block, .. } => check_block(block, what),
            RowKind::Tool { label, .. } => {
                assert!(label.object.encode_utf16().count() <= LABEL_UNITS, "{what}: label");
                assert!(!label.object.contains('\n') && clean(&label.object), "{what}: label line");
            }
            RowKind::Thought { preview, .. } => {
                assert!(preview.encode_utf16().count() <= THOUGHT_PREVIEW_UNITS, "{what}: preview")
            }
            RowKind::Live { thought, .. } => {
                assert!(thought.encode_utf16().count() <= THOUGHT_PREVIEW_UNITS, "{what}: live thought")
            }
            RowKind::Detail { of: DetailOf::Diff { doc, .. } } => check_diff(doc, what),
            RowKind::Changes { files, .. } => {
                assert!(files.len() <= MAX_CHANGED_FILES, "{what}: changed files")
            }
            RowKind::User { text, from, .. } => assert!(clean(text) && clean(from), "{what}: user text"),
            RowKind::Notice { text } => assert!(clean(text), "{what}: notice"),
            _ => {}
        }
    }
    let live = rows.iter().filter(|r| r.key == "live").count();
    assert!(live <= 1, "{what}: more than one live row");
    if live == 1 {
        assert_eq!(rows.last().map(|r| r.key.as_str()), Some("live"), "{what}: the live row is last");
    }
}

const CLASSES: &[ToolClass] = &[
    ToolClass::Command,
    ToolClass::Edit,
    ToolClass::Read,
    ToolClass::Search,
    ToolClass::Web,
    ToolClass::Mcp,
    ToolClass::Agent,
    ToolClass::Tasks,
    ToolClass::Other,
];
const STATUSES: &[ToolStatus] = &[ToolStatus::Running, ToolStatus::Ok, ToolStatus::Error, ToolStatus::Denied];
const KINDS: &[ItemKind] =
    &[ItemKind::User, ItemKind::Assistant, ItemKind::Tool, ItemKind::Notice, ItemKind::Reasoning];
const STATES: &[ThreadState] = &[
    ThreadState::Working,
    ThreadState::Waiting,
    ThreadState::Done,
    ThreadState::Failed,
    ThreadState::Interrupted,
];
const MARKS: &[MarkKind] = &[
    MarkKind::Waiting,
    MarkKind::Held,
    MarkKind::Allowed,
    MarkKind::Denied,
    MarkKind::Expired,
    MarkKind::Cancelled,
    MarkKind::Rule,
];

fn random_timeline(rng: &mut Rng, texts: &[String], ids: &[String]) {
    let n = 1 + rng.below(40);
    let mut items = Vec::with_capacity(n);
    let mut turn = 0u32;
    for k in 0..n {
        let kind = KINDS[rng.below(KINDS.len())];
        if rng.chance(30) {
            turn += 1;
        }
        let text = texts[rng.below(texts.len())].as_str();
        let tool = (kind == ItemKind::Tool).then(|| ToolIn {
            name: "x",
            title: texts[rng.below(texts.len())].as_str(),
            class: CLASSES[rng.below(CLASSES.len())],
            detail: texts[rng.below(texts.len())].as_str(),
            path: if rng.chance(50) { "src/a.rs" } else { "" },
            status: STATUSES[rng.below(STATUSES.len())],
            output: if rng.chance(50) { text } else { "" },
            by_rule: rng.chance(10),
        });
        let ts = rng.next() % 100_000;
        items.push(ItemIn {
            seq: k as u64 + 1,
            // Ids repeat now and then, and look like keys.
            id: ids[rng.below(ids.len())].as_str(),
            kind,
            ts_ms: ts,
            end_ms: rng.chance(50).then(|| ts + rng.next() % 10_000),
            rev: rng.below(3) as u32,
            text,
            streaming: rng.chance(15),
            turn: if rng.chance(20) { 0 } else { turn },
            group: rng.chance(15).then(|| ids[rng.below(ids.len())].as_str()),
            phase: match rng.below(3) {
                0 => None,
                1 => Some(Phase::Interim),
                _ => Some(Phase::Final),
            },
            from: if rng.chance(20) { "Pixel" } else { "" },
            deny_of: rng.chance(10).then(|| ids[rng.below(ids.len())].as_str()),
            tool,
            truncated: rng.chance(10),
        });
    }
    let marks: Vec<Mark> = (0..rng.below(5))
        .map(|_| Mark {
            item_id: ids[rng.below(ids.len())].clone(),
            kind: MARKS[rng.below(MARKS.len())],
            by: if rng.chance(50) { "Pixel".into() } else { String::new() },
        })
        .collect();
    let mut open: Vec<String> = Vec::new();
    for _ in 0..rng.below(8) {
        let id = &ids[rng.below(ids.len())];
        let key = match rng.below(7) {
            0 => format!("i:{id}"),
            1 => format!("g:{id}"),
            2 => format!("t:{id}"),
            3 => format!("f:{}", rng.below(5)),
            4 => format!("c:{}:{}", rng.below(5), rng.below(3)),
            5 => format!("d:{id}:c"),
            _ => "live".into(),
        };
        open.push(key);
    }
    let ctx = Ctx {
        state: STATES[rng.below(STATES.len())],
        state_since_ms: rng.next() % 100_000,
        now_ms: rng.next() % 100_000,
        marks: &marks,
        open: &open,
    };
    let mut timeline = Timeline::new();
    let rows = timeline.rows(&items, &ctx);
    check_rows(&rows, "timeline");
    assert_eq!(timeline.rows(&items, &ctx), rows, "timeline: cached rows differ");
    let tools: Vec<ToolIn<'_>> = items.iter().filter_map(|i| i.tool).collect();
    let s = summary(&tools);
    assert!(s.second.is_none() || s.first.count > 0, "summary order");
}

#[test]
fn torture() {
    run(0x6737_6167_656e_7473, INPUTS, false);
}

#[test]
fn torture_with_highlighting() {
    run(0x7267_6869_6768_6c74, CODE_INPUTS, true);
}

fn run(seed: u64, inputs: usize, code: bool) {
    let mut rng = Rng(seed);
    let ids: Vec<String> =
        ["a", "b", "c", "d", "e", "f", "a:1", "b:0", "x:c", "live", "1"].map(String::from).to_vec();
    let mut recent: Vec<String> = vec![String::new()];
    let mut timeline = Timeline::new();
    for n in 0..inputs {
        let input = build(&mut rng, code);
        let doc = markdown(&input);
        assert!(doc.blocks.len() <= MAX_BLOCKS, "blocks");
        if doc.plain {
            assert_eq!(doc.blocks.len(), 1);
        }
        for b in &doc.blocks {
            check_block(b, "markdown");
            assert!(!b.open, "markdown() blocks are never open");
        }
        check_diff(&diff(&input), "diff");
        let line = live_thought_line(&input);
        assert!(line.encode_utf16().count() <= THOUGHT_PREVIEW_UNITS && clean(&line), "thought line");
        let class = CLASSES[n % CLASSES.len()];
        let label = tool_label(&ToolIn {
            name: "x",
            title: &input,
            class,
            detail: &input,
            path: "",
            status: ToolStatus::Ok,
            output: "",
            by_rule: false,
        });
        assert!(label.object.encode_utf16().count() <= LABEL_UNITS && !label.object.contains('\n'), "label");
        if code && n % HIGHLIGHT_EVERY == 0 {
            let alias = ALIASES[(n / HIGHLIGHT_EVERY) % ALIASES.len()];
            check_lines(&highlight(alias, &input), alias);
        }
        // The timeline's pieces parse like the whole text.
        let answer = ItemIn {
            seq: 1,
            id: "a",
            kind: ItemKind::Assistant,
            ts_ms: 0,
            end_ms: None,
            rev: n as u32,
            text: &input,
            streaming: false,
            turn: 1,
            group: None,
            phase: None,
            from: "",
            deny_of: None,
            tool: None,
            truncated: false,
        };
        let rows = timeline.rows(
            &[answer],
            &Ctx { state: ThreadState::Interrupted, state_since_ms: 0, now_ms: 0, marks: &[], open: &[] },
        );
        let blocks: Vec<&Block> = rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::Block { block, .. } => Some(block),
                _ => None,
            })
            .collect();
        assert_eq!(blocks, doc.blocks.iter().collect::<Vec<_>>(), "pieces differ from the whole: {input:?}");
        recent.push(input);
        if recent.len() > 16 {
            recent.remove(0);
        }
        if n % TIMELINE_EVERY == 0 {
            random_timeline(&mut rng, &recent, &ids);
        }
    }
}
