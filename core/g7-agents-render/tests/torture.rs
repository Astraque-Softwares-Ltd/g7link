// SPDX-License-Identifier: MPL-2.0
//! Fixed-seed random inputs, built from Markdown and diff fragments and
//! random bytes made valid UTF-8, through every public function. Nothing
//! may panic, and every bound and invariant the clients rely on must hold.

use std::collections::HashSet;

use g7_agents_render::bounds::*;
use g7_agents_render::{
    ALIASES, Act, Block, Checkpoint, CheckpointWhy, CodeLine, Ctx, DetailOf, DiffDoc, DiffNote, ItemIn,
    ItemKind, Link, Mark, MarkKind, NoteKind, NoticeKind, Phase, PlanState, QuestionIn, QuestionState, Row,
    RowKind, Span, ThreadState, Timeline, ToolClass, ToolIn, ToolStatus, TurnFileIn, TurnFileStatus, TurnIn,
    Via, apply, diff, guess, highlight, languages, live_thought_line, markdown, mentions, output_lines,
    snippet, speakable, summary, tool_label, trigger,
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
/// Inputs between language guesses and output marks (each scans the input).
const GUESS_EVERY: usize = 3;
/// Inputs between words-to-read-aloud checks (each parses the input twice).
const SPEAK_EVERY: usize = 2;
/// Inputs per grammar in `torture_every_grammar`.
const GRAMMAR_INPUTS: usize = 2;

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
    "```kt\n",
    "```tsx\n",
    "```toml\n",
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
    for f in &doc.files {
        if f.note == Some(DiffNote::Binary) {
            assert!(f.rows.is_empty(), "{what}: rows in a binary file");
        }
    }
}

/// A guess is a language `highlight` knows; output lines cover their text
/// exactly and are cleaned.
fn check_guess_and_output(input: &str, n: usize) {
    let hints = [None, Some("rs"), Some("PY"), Some(".ts"), Some("json"), Some("x")];
    if let Some(word) = guess(input, hints[n % hints.len()]) {
        assert!(!highlight(word, "x").is_empty(), "guess {word} has no grammar");
    }
    let lines = output_lines(input);
    assert!(lines.len() <= MAX_CODE_LINES, "output lines");
    for line in &lines {
        let total: u32 = line.toks.iter().map(|t| t.len).sum();
        assert_eq!(total as usize, line.text.encode_utf16().count(), "output toks cover the line");
        assert!(clean(&line.text), "output line not cleaned");
    }
}

/// Words to read aloud: within their max, on one line, cleaned, trimmed,
/// and the same text cut shorter is never longer.
fn check_speakable(input: &str, n: usize) {
    let max = [0u32, 1, 7, 40, 200, 4_000, u32::MAX][n % 7];
    let words = speakable(input, max);
    let units = words.encode_utf16().count();
    assert!(units <= (max as usize).min(MAX_SPEAKABLE_UNITS), "speakable: {units} over {max}");
    assert!(clean(&words) && !words.contains('\n') && !words.contains('\t'), "speakable: one clean line");
    assert_eq!(words.trim(), words, "speakable: trimmed");
    if max > 1 {
        assert!(speakable(input, max / 2).encode_utf16().count() <= units, "speakable: shorter max");
    }
}

/// A snippet: on one line, cleaned, no empty part, neighbours differ, and
/// no mark at a space's edge.
fn check_snippet(input: &str, n: usize) {
    let len = input.encode_utf16().count() as u32;
    let k = n as u32;
    let marks = [(k % (len + 1), 1 + k % 7), (k.wrapping_mul(7) % (len + 2), k % 40), (u32::MAX, u32::MAX)];
    let parts = snippet(input, &marks);
    let mut units = 0;
    for (i, p) in parts.iter().enumerate() {
        assert!(!p.text.is_empty() && clean(&p.text), "snippet part");
        assert!(!p.text.contains(['\n', '\t']) && !p.text.contains("  "), "snippet on one line");
        assert!(i == 0 || parts[i - 1].mark != p.mark, "snippet parts alternate");
        if p.mark {
            assert!(!p.text.starts_with(' ') && !p.text.ends_with(' '), "a mark at a space");
        }
        units += p.text.encode_utf16().count();
    }
    assert!(units <= MAX_SNIPPET_UNITS, "snippet length");
    if let (Some(first), Some(last)) = (parts.first(), parts.last()) {
        assert!(!first.text.starts_with(' ') && !last.text.ends_with(' '), "snippet trimmed");
    }
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
                if !label.toks.is_empty() {
                    let total: u32 = label.toks.iter().map(|t| t.len).sum();
                    assert_eq!(total as usize, label.object.encode_utf16().count(), "{what}: label toks");
                }
            }
            RowKind::Thought { preview, .. } => {
                assert!(preview.encode_utf16().count() <= THOUGHT_PREVIEW_UNITS, "{what}: preview")
            }
            RowKind::Live { thought, .. } => {
                assert!(thought.encode_utf16().count() <= THOUGHT_PREVIEW_UNITS, "{what}: live thought")
            }
            RowKind::Detail { of: DetailOf::Diff { doc, .. } } => check_diff(doc, what),
            RowKind::Changes { files, .. } => {
                assert!(files.len() <= MAX_CHANGED_FILES, "{what}: changed files");
                assert!(files.iter().all(|f| clean(&f.path)), "{what}: changed file not cleaned");
            }
            RowKind::ChangesNote { note, why, count } => {
                assert!(why.is_none() || *note == NoteKind::Missing, "{what}: a reason off a missing note");
                assert_ne!(*why, Some(CheckpointWhy::Before), "{what}: a note for a turn from before");
                assert_eq!(*count > 0, *note == NoteKind::Unrecorded, "{what}: a note's count");
            }
            RowKind::User { text, from, .. } => assert!(clean(text) && clean(from), "{what}: user text"),
            RowKind::Notice { text, .. } => assert!(clean(text), "{what}: notice"),
            RowKind::Question { by, .. } => assert!(clean(by), "{what}: question by"),
            _ => {}
        }
    }
    // A plan's head is followed by exactly `shown` of its blocks, in order.
    for (k, r) in rows.iter().enumerate() {
        if let RowKind::Plan { shown, .. } = r.kind {
            for (index, next) in rows.iter().skip(k + 1).take(usize::from(shown)).enumerate() {
                assert!(
                    matches!(next.kind, RowKind::Block { index: i, .. } if usize::from(i) == index)
                        && next.item_id == r.item_id,
                    "{what}: plan blocks"
                );
            }
        }
    }
    // Actions only on a message at depth 0; an answer never rewinds.
    for r in rows {
        if r.acts.is_hidden() {
            continue;
        }
        assert_eq!(r.depth, 0, "{what}: actions in a fold");
        match &r.kind {
            RowKind::User { deny_of, via, .. } => {
                assert!(!deny_of && *via != Via::Steer, "{what}: actions on a message that started no turn")
            }
            RowKind::Block { last, .. } => {
                assert!(*last, "{what}: actions on an answer's inner block");
                assert!(
                    r.acts.rewind == Act::Hidden
                        && r.acts.edit == Act::Hidden
                        && r.acts.restore == Act::Hidden,
                    "{what}: an answer that rewinds"
                );
            }
            _ => panic!("{what}: actions on a {:?} row", r.kind),
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
    ToolClass::Plan,
];
const STATUSES: &[ToolStatus] = &[ToolStatus::Running, ToolStatus::Ok, ToolStatus::Error, ToolStatus::Denied];
const KINDS: &[ItemKind] = &[
    ItemKind::User,
    ItemKind::Assistant,
    ItemKind::Tool,
    ItemKind::Notice,
    ItemKind::Reasoning,
    ItemKind::Plan,
    ItemKind::Question,
];
const VIAS: &[Via] = &[Via::Turn, Via::Steer, Via::Queue];
const PLAN_STATES: &[PlanState] = &[PlanState::Proposed, PlanState::Implemented, PlanState::Replaced];
const QUESTION_STATES: &[QuestionState] =
    &[QuestionState::Asked, QuestionState::Answered, QuestionState::Dismissed, QuestionState::Expired];
const STATES: &[ThreadState] = &[
    ThreadState::Working,
    ThreadState::Waiting,
    ThreadState::Done,
    ThreadState::Failed,
    ThreadState::Interrupted,
];
const NOTICES: &[NoticeKind] =
    &[NoticeKind::Rewound, NoticeKind::EditFromHere, NoticeKind::FilesRestored, NoticeKind::Forked];
const CHECKPOINTS: &[Checkpoint] =
    &[Checkpoint::Pending, Checkpoint::Ready, Checkpoint::Missing, Checkpoint::NotGit, Checkpoint::Expired];
const WHYS: &[CheckpointWhy] = &[
    CheckpointWhy::Busy,
    CheckpointWhy::Slow,
    CheckpointWhy::Large,
    CheckpointWhy::Refused,
    CheckpointWhy::Off,
    CheckpointWhy::Before,
];
const FILE_STATUSES: &[TurnFileStatus] = &[
    TurnFileStatus::Added,
    TurnFileStatus::Modified,
    TurnFileStatus::Deleted,
    TurnFileStatus::Renamed,
    TurnFileStatus::Type,
    TurnFileStatus::Unrecorded,
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
            name: if rng.chance(10) { "ToolSearch" } else { "x" },
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
            via: VIAS[rng.below(VIAS.len())],
            plan_state: rng.chance(80).then(|| PLAN_STATES[rng.below(PLAN_STATES.len())]),
            question: rng.chance(80).then(|| QuestionIn {
                state: QUESTION_STATES[rng.below(QUESTION_STATES.len())],
                count: rng.below(6) as u8,
                by: if rng.chance(50) { "Pixel\u{202e}" } else { "" },
            }),
            notice: rng.chance(30).then(|| NOTICES[rng.below(NOTICES.len())]),
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
        let key = match rng.below(9) {
            0 => format!("i:{id}"),
            1 => format!("g:{id}"),
            2 => format!("t:{id}"),
            3 => format!("f:{}", rng.below(5)),
            4 => format!("c:{}:{}", rng.below(5), rng.below(3)),
            5 => format!("d:{id}:c"),
            6 => format!("p:{id}"),
            7 => format!("q:{id}"),
            _ => "live".into(),
        };
        open.push(key);
    }
    let paths: Vec<TurnFileIn<'_>> = (0..rng.below(30))
        .map(|k| TurnFileIn {
            path: texts[k % texts.len()].as_str(),
            status: FILE_STATUSES[rng.below(FILE_STATUSES.len())],
            adds: rng.below(100) as u32,
            dels: rng.below(100) as u32,
        })
        .collect();
    let turns: Vec<TurnIn<'_>> = (0..rng.below(turn as usize + 3))
        .map(|_| TurnIn {
            turn: rng.below(turn as usize + 2) as u32,
            checkpoint: CHECKPOINTS[rng.below(CHECKPOINTS.len())],
            why: rng.chance(50).then(|| WHYS[rng.below(WHYS.len())]),
            ended_ms: rng.chance(70).then(|| rng.next() % 100_000),
            files: if rng.chance(20) { u32::MAX } else { rng.below(40) as u32 },
            adds: rng.next() as u32,
            dels: rng.below(10) as u32,
            skipped: if rng.chance(70) { 0 } else { rng.below(300) as u32 },
            paths: &paths[..rng.below(paths.len() + 1)],
            can_fork: rng.chance(50),
            can_rewind: rng.chance(50),
            can_restore: rng.chance(50),
        })
        .collect();
    let ctx = Ctx {
        state: STATES[rng.below(STATES.len())],
        state_since_ms: rng.next() % 100_000,
        now_ms: rng.next() % 100_000,
        marks: &marks,
        open: &open,
        turns: &turns,
    };
    let mut timeline = Timeline::new();
    let rows = timeline.rows(&items, &ctx);
    check_rows(&rows, "timeline");
    if matches!(ctx.state, ThreadState::Working | ThreadState::Waiting) {
        assert!(rows.iter().all(|r| r.acts.is_hidden()), "timeline: actions while it works");
    }
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

/// Every grammar in the set, not only the aliased ones (contract §Q2.1),
/// on a few threads: a grammar's first use compiles its patterns, which is
/// slow unoptimised.
#[test]
fn torture_every_grammar() {
    let langs: Vec<&str> = languages().collect();
    let chunk = langs.len().div_ceil(4).max(1);
    std::thread::scope(|s| {
        for (k, part) in langs.chunks(chunk).enumerate() {
            s.spawn(move || {
                let mut rng = Rng(0x6772_616d_6d61_7273 ^ (k as u64 + 1));
                for lang in part {
                    for _ in 0..GRAMMAR_INPUTS {
                        let input = build(&mut rng, true);
                        check_lines(&highlight(lang, &input), lang);
                    }
                }
            });
        }
    });
}

/// The composer's functions on a text: total, in bounds, on character
/// boundaries, and `apply` leaves the text around the run alone.
fn check_composer(input: &str, n: usize) {
    let edges = boundaries(input);
    let len = input.encode_utf16().count() as u32;
    let cursor = (n as u32).wrapping_mul(2_654_435_761) % (len + 2);
    if let Some(t) = trigger(input, cursor) {
        assert!(edges.contains(&t.start) && t.start < len.max(1), "trigger start");
        assert!(t.query.encode_utf16().count() <= MAX_TRIGGER_QUERY_UNITS && clean(&t.query), "query");
        let (out, at) = apply(input, &t, cursor, "src/a b😀/c.rs");
        let out_edges = boundaries(&out);
        assert!(out_edges.contains(&at), "apply cursor");
        let prefix: Vec<u16> = input.encode_utf16().take(t.start as usize).collect();
        assert_eq!(
            out.encode_utf16().take(prefix.len()).collect::<Vec<_>>(),
            prefix,
            "apply keeps the start"
        );
        assert!(trigger(&out, at).is_none(), "a trigger right after apply");
    }
    let mut end = 0;
    let found = mentions(input);
    assert!(found.len() <= MAX_MENTIONS, "mentions");
    for (start, l) in found {
        assert!(l >= 2 && start >= end, "mentions order");
        end = start + l;
        assert!(edges.contains(&start) && edges.contains(&end) && end <= len, "mention bounds");
    }
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
            let lang = ALIASES[(n / HIGHLIGHT_EVERY) % ALIASES.len()];
            check_lines(&highlight(lang, &input), lang);
        }
        check_composer(&input, n);
        check_snippet(&input, n);
        if n % SPEAK_EVERY == 0 {
            check_speakable(&input, n / SPEAK_EVERY);
        }
        if n % GUESS_EVERY == 0 {
            check_guess_and_output(&input, n / GUESS_EVERY);
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
            via: Via::Turn,
            plan_state: None,
            question: None,
            notice: None,
        };
        let rows = timeline.rows(
            &[answer],
            &Ctx {
                state: ThreadState::Interrupted,
                state_since_ms: 0,
                now_ms: 0,
                marks: &[],
                open: &[],
                turns: &[],
            },
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
