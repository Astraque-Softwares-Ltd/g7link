// SPDX-License-Identifier: MPL-2.0
//! Search snippets on arbitrary text, with marks taken from the first bytes.

#![no_main]

use g7_agents_render::bounds::{MAX_SNIPPET_UNITS, is_stripped_char};
use g7_agents_render::snippet;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (head, body) = data.split_at(data.len().min(40));
    let marks: Vec<(u32, u32)> = head
        .chunks_exact(4)
        .map(|c| (u32::from(u16::from_le_bytes([c[0], c[1]])), u32::from(u16::from_le_bytes([c[2], c[3]]))))
        .collect();
    let text = String::from_utf8_lossy(body);
    let parts = snippet(&text, &marks);
    let mut units = 0;
    for (k, part) in parts.iter().enumerate() {
        assert!(!part.text.is_empty());
        assert!(!part.text.chars().any(|c| is_stripped_char(c) || (c.is_whitespace() && c != ' ')));
        assert!(!part.text.contains("  "));
        assert!(k == 0 || parts[k - 1].mark != part.mark);
        if part.mark {
            assert!(!part.text.starts_with(' ') && !part.text.ends_with(' '));
        }
        units += part.text.encode_utf16().count();
    }
    assert!(units <= MAX_SNIPPET_UNITS);
    if let (Some(first), Some(last)) = (parts.first(), parts.last()) {
        assert!(!first.text.starts_with(' ') && !last.text.ends_with(' '));
    }
});
