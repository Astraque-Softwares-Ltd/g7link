// SPDX-License-Identifier: MPL-2.0
//! A message's Markdown as words to read aloud: one line of plain text, cut
//! at the end of a sentence.
//!
//! Code blocks, tables, rules, HTML and struck-out text say nothing; a link
//! or an image says its text, inline code what it shows. Each paragraph, heading and list
//! item is a sentence of its own: one that doesn't end in a stop gets a
//! full stop before the next, so a voice pauses between them.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use crate::bounds::{
    MAX_SPEAKABLE_UNITS, MAX_SRC_BYTES, clean, collapse_whitespace, cut_bytes, cut_units, units,
};
use crate::markdown::options;

/// Ends a sentence when white space or the text's end follows it (and any
/// closing quotes or brackets after it).
const STOPS: &[char] = &['.', '!', '?', '…', '؟', '।'];
/// Ends a sentence whatever follows.
const WIDE_STOPS: &[char] = &['。', '！', '？'];
/// Lets a voice pause: a block that ends in one needs no full stop.
const PAUSES: &[char] = &[':', ';', ',', '：', '；', '，', '、'];
/// May follow a stop and still end its sentence.
const CLOSERS: &[char] = &['"', '\'', '”', '’', ')', ']', '»', '」', '』', '）'];

/// `text` (Markdown) as at most `max_chars` UTF-16 units of words to read
/// aloud (at most [`MAX_SPEAKABLE_UNITS`]), cut after the last sentence that
/// fits. When not even one sentence fits, it is cut after the last word that
/// does, with a `…`. Empty when nothing in it is words.
pub fn speakable(text: &str, max_chars: u32) -> String {
    let max = usize::try_from(max_chars).unwrap_or(usize::MAX).min(MAX_SPEAKABLE_UNITS);
    if max == 0 {
        return String::new();
    }
    let src = clean(cut_bytes(text, MAX_SRC_BYTES));
    let mut words = Words::default();
    // Inside a code block or a table: nothing is read.
    let mut skip = 0u32;
    // Inside struck-out text: its words are not read.
    let mut struck = 0u32;
    for event in Parser::new_ext(&src, options()) {
        match event {
            Event::Start(Tag::CodeBlock(_) | Tag::Table(_) | Tag::MetadataBlock(_)) => {
                words.flush();
                skip += 1;
            }
            Event::End(TagEnd::CodeBlock | TagEnd::Table | TagEnd::MetadataBlock(_)) => {
                skip = skip.saturating_sub(1);
            }
            _ if skip > 0 => {}
            Event::Start(Tag::Strikethrough) => struck += 1,
            Event::End(TagEnd::Strikethrough) => struck = struck.saturating_sub(1),
            Event::Text(t) | Event::Code(t) | Event::InlineMath(t) | Event::DisplayMath(t) => {
                if struck == 0 {
                    words.block.push_str(&clean(&t));
                }
            }
            // A tag says nothing, but `a<br>b` is still two words.
            Event::SoftBreak | Event::HardBreak | Event::InlineHtml(_) => words.block.push(' '),
            Event::Start(tag) if !is_inline(&tag) => words.flush(),
            Event::End(tag) if !is_inline_end(tag) => words.flush(),
            Event::Rule => words.flush(),
            // HTML blocks, footnote references, task boxes and inline tags say nothing.
            _ => {}
        }
        if words.units > max {
            break;
        }
    }
    words.flush();
    cut_at_sentence(&words.out, max)
}

/// The words so far, and the block being read.
#[derive(Default)]
struct Words {
    out: String,
    /// `out`'s length in UTF-16 units.
    units: usize,
    block: String,
}

impl Words {
    /// Ends the block being read: its words join the rest as a sentence.
    fn flush(&mut self) {
        let block = collapse_whitespace(&self.block);
        self.block.clear();
        if block.is_empty() {
            return;
        }
        if !self.out.is_empty() {
            if !ends_with_pause(&self.out) {
                self.out.push('.');
                self.units += 1;
            }
            self.out.push(' ');
            self.units += 1;
        }
        self.units += units(&block);
        self.out.push_str(&block);
    }
}

fn is_inline(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. }
    )
}

fn is_inline_end(tag: TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::Link
            | TagEnd::Image
    )
}

/// Whether `s` ends in a stop or a pause, perhaps with closers after it.
fn ends_with_pause(s: &str) -> bool {
    s.trim_end_matches(CLOSERS)
        .chars()
        .next_back()
        .is_some_and(|c| STOPS.contains(&c) || WIDE_STOPS.contains(&c) || PAUSES.contains(&c))
}

/// `s` when it fits in `max` UTF-16 units; otherwise its longest start that
/// fits and ends a sentence, or failing that, ends a word, with a `…`.
fn cut_at_sentence(s: &str, max: usize) -> String {
    if units(s) <= max {
        return s.to_owned();
    }
    let head = cut_units(s, max);
    let chars: Vec<(usize, char)> = head.char_indices().collect();
    let mut end = None;
    for (k, &(at, c)) in chars.iter().enumerate() {
        let wide = WIDE_STOPS.contains(&c);
        if !wide && !STOPS.contains(&c) {
            continue;
        }
        let mut after = at + c.len_utf8();
        for &(at, closer) in chars[k + 1..].iter().take_while(|(_, c)| CLOSERS.contains(c)) {
            after = at + closer.len_utf8();
        }
        if wide || s[after..].chars().next().is_none_or(char::is_whitespace) {
            end = Some(after);
        }
    }
    if let Some(end) = end {
        return s[..end].to_owned();
    }
    // One sentence longer than `max`: its words that fit, and the `…`.
    let head = cut_units(s, max - 1);
    let whole = s[head.len()..].starts_with(char::is_whitespace);
    let head = match head.rfind(char::is_whitespace) {
        Some(space) if !whole => &head[..space],
        _ => head,
    };
    let head = head.trim_end_matches(|c: char| c.is_whitespace() || PAUSES.contains(&c));
    if head.is_empty() {
        return String::new();
    }
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_become_sentences() {
        assert_eq!(speakable("# Done\n\nIt works.", 200), "Done. It works.");
        assert_eq!(speakable("Changes:\n\n- one\n- two", 200), "Changes: one. two");
        assert_eq!(speakable("It said \"yes.\"\n\nThen it stopped", 200), "It said \"yes.\" Then it stopped");
    }

    #[test]
    fn a_cut_ends_a_sentence_or_a_word() {
        assert_eq!(speakable("One. Two three four.", 12), "One.");
        assert_eq!(speakable("One two three four.", 12), "One two…");
        assert_eq!(speakable("Version 1.2 is out. Yes", 18), "Version 1.2 is…");
        assert_eq!(speakable("完成了。还有更多的内容", 5), "完成了。");
        assert_eq!(speakable("Supercalifragilistic", 6), "Super…");
        assert_eq!(speakable("Hello, world", 7), "Hello…");
        assert_eq!(speakable("Hello", 1), "");
        assert_eq!(speakable("Hello", 0), "");
    }
}
