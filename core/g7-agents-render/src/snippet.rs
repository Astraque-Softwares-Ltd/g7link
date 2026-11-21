// SPDX-License-Identifier: MPL-2.0
//! A search hit's snippet into one line of text parts, with the matches the
//! host found marked.
//!
//! The text is shown as it is (no Markdown), on one line: characters every
//! input loses are dropped, each run of whitespace becomes one space, and
//! the ends are trimmed. A mark covers every character it touches, so it
//! never splits one, and it never starts or ends on a space.

use serde::{Deserialize, Serialize};

use crate::bounds::{MAX_SNIPPET_MARKS, MAX_SNIPPET_UNITS, cut_units, is_stripped_char};
use crate::markdown::is_false;

/// A piece of a snippet; neighbouring parts differ in `mark`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetPart {
    pub text: String,
    /// A match of the search.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mark: bool,
}

impl std::fmt::Debug for SnippetPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnippetPart").field("mark", &self.mark).finish_non_exhaustive()
    }
}

/// `text` (a hit's snippet) as parts, with `marks` (`(start, len)` in UTF-16
/// units of `text`, as the host sends them) marked. The first
/// `MAX_SNIPPET_MARKS` marks are read, in any order; empty ones and the
/// parts past the text are ignored, and overlapping ones merge.
pub fn snippet(text: &str, marks: &[(u32, u32)]) -> Vec<SnippetPart> {
    let text = cut_units(text, MAX_SNIPPET_UNITS);
    let ranges: Vec<(u32, u32)> = marks
        .iter()
        .take(MAX_SNIPPET_MARKS)
        .filter(|&&(_, len)| len > 0)
        .map(|&(start, len)| (start, start.saturating_add(len)))
        .collect();
    // Each kept character, with whether a mark touches it.
    let mut chars: Vec<(char, bool)> = Vec::with_capacity(text.len());
    let mut at = 0u32;
    for c in text.chars() {
        let width = u32::try_from(c.len_utf16()).unwrap_or(2);
        let marked = ranges.iter().any(|&(start, end)| start < at + width && at < end);
        at += width;
        if is_stripped_char(c) {
            continue;
        }
        if c.is_whitespace() {
            match chars.last_mut() {
                // A run is one space, marked when a mark touches any of it.
                Some((' ', was)) => *was |= marked,
                Some(_) => chars.push((' ', marked)),
                None => {}
            }
            continue;
        }
        chars.push((c, marked));
    }
    if chars.last().is_some_and(|&(c, _)| c == ' ') {
        chars.pop();
    }
    // A space stays marked only inside a mark: both its neighbours are.
    for k in 1..chars.len().saturating_sub(1) {
        if chars[k] == (' ', true) && !(chars[k - 1].1 && chars[k + 1].1) {
            chars[k].1 = false;
        }
    }
    let mut parts: Vec<SnippetPart> = Vec::new();
    for (c, mark) in chars {
        match parts.last_mut() {
            Some(part) if part.mark == mark => part.text.push(c),
            _ => parts.push(SnippetPart { text: c.to_string(), mark }),
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(parts: &[SnippetPart]) -> String {
        parts.iter().map(|p| if p.mark { format!("[{}]", p.text) } else { p.text.clone() }).collect()
    }

    #[test]
    fn marks_the_matches() {
        assert_eq!(shown(&snippet("the retry loop", &[(4, 5)])), "the [retry] loop");
        assert_eq!(shown(&snippet("retry", &[(0, 5)])), "[retry]");
        assert_eq!(shown(&snippet("a b", &[])), "a b");
        assert!(snippet("", &[(0, 3)]).is_empty());
    }

    #[test]
    fn a_mark_never_splits_a_character_or_ends_on_a_space() {
        // 😀 is two units: a mark on its second half takes it whole.
        assert_eq!(shown(&snippet("a😀b", &[(2, 1)])), "a[😀]b");
        assert_eq!(shown(&snippet("foo  bar", &[(0, 4)])), "[foo] bar");
        assert_eq!(shown(&snippet("foo \n bar", &[(0, 9)])), "[foo bar]");
    }
}
