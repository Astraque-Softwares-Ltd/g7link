// SPDX-License-Identifier: MPL-2.0
//! A message streamed one character at a time ends with the same rows as
//! the whole message, and a block that was shown as final never changes
//! while the rest arrives.

use std::collections::HashMap;

use g7_agents_render::{Block, Ctx, ItemIn, ItemKind, Phase, Row, RowKind, ThreadState, Timeline, markdown};

const TEXTS: &[&str] = &[
    "I found it.\n\nThe parser drops the last line.\n\n- one\n- two\n  - nested\n\nDone.",
    "# Title\n\nSetext\n------\n\nAnd a paragraph\nthat goes on\n===\n\n* * *\n\n- - -\n\nend",
    "| a | b |\n|---|:-:|\n| 1 | 2 |\n| 3 | 4 |\n\nAfter the table.\n\ntext\n| x |\n|---|\n| y |",
    "```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\nThen\n\n    indented\n\n    more indented\n\nback",
    "> quote\nlazy line\n\n> [!WARNING]\n> Careful **here**\n>\n> - item\n\n1. a\n2. b\n\n   para in b\n\n10. ten",
    "See [docs](https://x.example) and [ref][1].\n\n[1]: https://y.example\n\nMore *em* and `code` ~~gone~~.",
    "<details>\n<summary>x</summary>\n\nhidden\n\n</details>\n\nafter html",
    "- [ ] todo\n- [x] done\n\n~~~\nunclosed tilde fence\n\nstill code",
    "日本語の文。次の文！\n\n😀 **bold 😀** text\u{a0}\n\u{a0}\nnbsp lines",
    "1. one\n\n1. two\n\n1) three\n\n- a\n\n+ b\n\n* c",
    "Text\n```\ncode right after text\n```\nmore text\n\n    \n\nfinal",
    "a  \nb\\\nc\n\n***bold em*** and **unclosed",
];

fn assistant<'a>(text: &'a str, streaming: bool, rev: u32) -> [ItemIn<'a>; 2] {
    let user = ItemIn {
        seq: 1,
        id: "u1",
        kind: ItemKind::User,
        ts_ms: 0,
        end_ms: None,
        rev: 0,
        text: "Go",
        streaming: false,
        turn: 1,
        group: None,
        phase: None,
        from: "",
        deny_of: None,
        tool: None,
        truncated: false,
    };
    let answer = ItemIn {
        seq: 2,
        id: "a1",
        kind: ItemKind::Assistant,
        text,
        streaming,
        rev,
        phase: Some(Phase::Final),
        ..user
    };
    [user, answer]
}

fn ctx(state: ThreadState) -> Ctx<'static> {
    Ctx { state, state_since_ms: 0, now_ms: 0, marks: &[], open: &[] }
}

fn blocks(rows: &[Row]) -> Vec<Block> {
    rows.iter()
        .filter_map(|r| match &r.kind {
            RowKind::Block { block, .. } => Some(block.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn streamed_text_ends_like_the_whole_and_final_blocks_never_change() {
    for text in TEXTS {
        let mut timeline = Timeline::new();
        let mut before: HashMap<String, Row> = HashMap::new();
        let ends: Vec<usize> = text.char_indices().map(|(i, c)| i + c.len_utf8()).collect();
        for end in ends {
            let items = assistant(&text[..end], true, 0);
            let rows = timeline.rows(&items, &ctx(ThreadState::Working));
            let now: HashMap<String, Row> = rows.iter().map(|r| (r.key.clone(), r.clone())).collect();
            for (key, row) in &before {
                if let RowKind::Block { block, .. } = &row.kind
                    && !block.open
                {
                    // `last` may move on as blocks are added; the block may not change.
                    let still = now.get(key).and_then(|r| match &r.kind {
                        RowKind::Block { block, .. } => Some(block),
                        _ => None,
                    });
                    assert_eq!(still, Some(block), "{text:?}: final block {key} changed at byte {end}");
                }
            }
            let open: Vec<bool> = blocks(&rows).iter().map(|b| b.open).collect();
            // A link definition makes no block: after one, the last block
            // may be a final one. Otherwise the last block is open.
            let no_block_yet = text[..end].lines().any(|l| l.starts_with('['));
            assert!(
                open.last().is_none_or(|o| *o) || no_block_yet,
                "{text:?} at {end}: the last block is open"
            );
            before = now;
        }
        let done = timeline.rows(&assistant(text, false, 1), &ctx(ThreadState::Done));
        let fresh = Timeline::new().rows(&assistant(text, false, 1), &ctx(ThreadState::Done));
        assert_eq!(done, fresh, "{text:?}: the streamed thread ends with the rows of the whole");
        assert_eq!(blocks(&done), markdown(text).blocks, "{text:?}: pieces parse like the whole");
    }
}
