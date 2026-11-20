// SPDX-License-Identifier: MPL-2.0
//! Markdown, highlighting and the thought line on arbitrary text.

#![no_main]

use g7_agents_render::{ALIASES, Link, highlight, live_thought_line, markdown};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let doc = markdown(&text);
    assert!(doc.blocks.len() <= g7_agents_render::bounds::MAX_BLOCKS);
    for block in &doc.blocks {
        let units = block.text.encode_utf16().count() as u32;
        for span in &block.spans {
            assert!(span.len > 0 && span.start + span.len <= units);
            if span.style.link != Link::None {
                let href = span.href.to_ascii_lowercase();
                assert!(
                    href.starts_with("http:") || href.starts_with("https:") || href.starts_with("mailto:")
                );
            }
        }
        for line in &block.lines {
            let toks: u32 = line.toks.iter().map(|t| t.len).sum();
            assert_eq!(toks as usize, line.text.encode_utf16().count());
        }
    }
    let alias = ALIASES[data.first().copied().unwrap_or(0) as usize % ALIASES.len()];
    for line in highlight(alias, &text) {
        let toks: u32 = line.toks.iter().map(|t| t.len).sum();
        assert_eq!(toks as usize, line.text.encode_utf16().count());
    }
    let thought = live_thought_line(&text);
    assert!(thought.encode_utf16().count() <= g7_agents_render::bounds::THOUGHT_PREVIEW_UNITS);
});
