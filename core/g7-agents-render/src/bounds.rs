// SPDX-License-Identifier: MPL-2.0
//! Every limit the crate puts on untrusted text, and the character rule each
//! input string goes through before anything else looks at it.
//!
//! Each constant has a test that crosses it (`tests/bounds.rs`).

use std::borrow::Cow;

/// Markdown source; longer text is cut at a character boundary.
pub const MAX_SRC_BYTES: usize = 65_536;
/// Blocks in one document; more and the whole text becomes one plain paragraph.
pub const MAX_BLOCKS: usize = 2_000;
/// List and quote nesting; deeper levels are flattened to this one.
pub const MAX_DEPTH: u8 = 16;
/// Spans in one block; more and the block keeps its text with no spans.
pub const MAX_SPANS_PER_BLOCK: usize = 2_000;
/// Body rows of a table that are kept; the rest are counted in `more_rows`.
pub const MAX_TABLE_ROWS: usize = 64;
/// Columns of a table; a wider table becomes a code block of its source.
pub const MAX_TABLE_COLS: usize = 16;
/// A link target; a longer one leaves its text plain.
pub const MAX_HREF_BYTES: usize = 2_048;
/// Code over any of these is not highlighted.
pub const MAX_CODE_BYTES: usize = 65_536;
pub const MAX_CODE_LINES: usize = 2_000;
pub const MAX_CODE_LINE_UNITS: usize = 2_000;
/// Highlighting work per call, in parser operations (no clock is read).
pub const HIGHLIGHT_STEPS: u64 = 400_000;
/// Diff text that is read; rows past `MAX_DIFF_ROWS` are dropped.
pub const MAX_DIFF_BYTES: usize = 1_048_576;
pub const MAX_DIFF_ROWS: usize = 5_000;
/// A diff line is cut to this many UTF-16 units.
pub const MAX_DIFF_LINE_UNITS: usize = 2_000;
/// Lines longer than this get no word marks.
pub const WORD_DIFF_MAX_UNITS: usize = 500;
/// Items a timeline reads (the newest ones).
pub const MAX_ITEMS_IN: usize = 2_000;
/// Rows a timeline returns (the newest ones).
pub const MAX_ROWS_OUT: usize = 20_000;
/// Fewer finished tools in a run than this make no group.
pub const GROUP_MIN: usize = 2;
/// Files a changes row lists by name.
pub const MAX_CHANGED_FILES: usize = 20;
/// A tool label's object, in UTF-16 units.
pub const LABEL_UNITS: usize = 160;
/// A thought preview and the live line's thought, in UTF-16 units.
pub const THOUGHT_PREVIEW_UNITS: usize = 240;
/// A request that arrives within this long of the last keystroke is held.
pub const TYPING_IDLE_MS: u64 = 1_500;
/// The timeline's parsed-Markdown cache.
pub const CACHE_ENTRIES: usize = 256;
pub const CACHE_BYTES: usize = 4_194_304;

/// Whether `c` is removed from every input: C0 and C1 controls except `\n`
/// and `\t`, and the bidi controls. The same set the host strips.
pub fn is_stripped_char(c: char) -> bool {
    match c {
        '\n' | '\t' => false,
        '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' => true,
        '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => true,
        _ => false,
    }
}

/// `s` without the characters [`is_stripped_char`] names. `\r\n` becomes
/// `\n` because a lone `\r` is a C0 control.
pub fn clean(s: &str) -> Cow<'_, str> {
    if s.chars().any(is_stripped_char) {
        Cow::Owned(s.chars().filter(|&c| !is_stripped_char(c)).collect())
    } else {
        Cow::Borrowed(s)
    }
}

/// The longest prefix of `s` that is at most `max` bytes and ends on a
/// character boundary.
pub(crate) fn cut_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Length of `s` in UTF-16 code units.
pub(crate) fn units(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// `units` as the `u32` the public types carry; texts are bounded far below
/// `u32::MAX`, and a longer one saturates rather than wraps.
pub(crate) fn units32(s: &str) -> u32 {
    u32::try_from(units(s)).unwrap_or(u32::MAX)
}

/// The longest prefix of `s` that is at most `max` UTF-16 units.
pub(crate) fn cut_units(s: &str, max: usize) -> &str {
    let mut n = 0;
    for (i, c) in s.char_indices() {
        n += c.len_utf16();
        if n > max {
            return &s[..i];
        }
    }
    s
}

/// `s` when it fits in `max` UTF-16 units; otherwise cut so that it and a
/// trailing `…` fit.
pub(crate) fn ellipsize(s: &str, max: usize) -> String {
    if units(s) <= max {
        return s.to_owned();
    }
    let mut out = cut_units(s, max.saturating_sub(1)).trim_end().to_owned();
    out.push('…');
    out
}

/// Runs of whitespace collapsed to one space, and trimmed.
pub(crate) fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for word in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// A count as the `u16` the public types carry, saturating.
pub(crate) fn sat16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// A count as `u32`, saturating.
pub(crate) fn sat32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_strips_controls_and_bidi_but_keeps_newlines_and_tabs() {
        assert_eq!(clean("a\r\nb\tc\u{0}\u{85}\u{202e}d\u{2066}"), "a\nb\tcd");
        assert!(matches!(clean("plain text"), Cow::Borrowed(_)));
    }

    #[test]
    fn cuts_respect_characters() {
        assert_eq!(cut_bytes("aé", 2), "a");
        assert_eq!(cut_units("a😀b", 2), "a");
        assert_eq!(cut_units("a😀b", 3), "a😀");
        assert_eq!(ellipsize("abcdef", 4), "abc…");
        assert_eq!(ellipsize("abc", 3), "abc");
        assert_eq!(units("a😀"), 3);
    }
}
