// SPDX-License-Identifier: MPL-2.0
// Parts ported from t3code (MIT); see THIRD-PARTY-T3CODE.md.
//! Markdown that is still arriving.
//!
//! The text is cut at blank lines that close every open block, so each
//! piece parses exactly as it does inside the whole text; a finished piece
//! is parsed and highlighted once and kept. Only the last piece is parsed
//! again as text arrives.
//!
//! In the last piece, the blocks that may still change are marked `open`:
//! every block whose source reaches into the last complete line or the line
//! still being written, which may underline it as a heading, turn it into a
//! table or continue it (`- - -` is three nested list items until its last
//! `-` makes it a rule). Every other block is final: it never changes while
//! the text grows. Open code blocks are not highlighted.

use crate::bounds::{MAX_BLOCKS, MAX_SRC_BYTES, clean, cut_bytes};
use crate::highlight::highlight;
use crate::markdown::{Block, BlockKind, parse, plain_doc};

/// Finished pieces of a text kept between updates.
#[derive(Debug, Default, Clone)]
pub(crate) struct StreamCache {
    /// The text the pieces cover.
    prefix: String,
    /// Each finished piece: where it ends in `prefix`, and its blocks.
    pieces: Vec<(usize, Vec<Block>)>,
}

/// The blocks of `src` (cut and cleaned here) and whether it fell back to
/// one plain paragraph. `streaming` marks the blocks that may still change;
/// `cache` is read and replaced.
pub(crate) fn blocks(src: &str, streaming: bool, cache: &mut StreamCache) -> (Vec<Block>, bool) {
    let text = clean(cut_bytes(src, MAX_SRC_BYTES));
    let text: &str = &text;
    let Some(starts) = split(text) else {
        *cache = StreamCache::default();
        return whole(text, streaming);
    };
    let reuse = text.starts_with(cache.prefix.as_str());
    let old = std::mem::take(cache);
    let mut kept = old.pieces.into_iter().peekable();
    let mut out: Vec<Block> = Vec::new();
    let mut piece_start = 0;
    let bounds: Vec<usize> = starts.iter().copied().chain(std::iter::once(text.len())).collect();
    for (k, &end) in bounds.iter().enumerate() {
        let last = k + 1 == bounds.len();
        let piece = &text[piece_start..end];
        if !last {
            let cached = if reuse { kept.next_if(|(e, _)| *e == end) } else { None };
            let blocks = match cached {
                Some((_, blocks)) => blocks,
                None => {
                    let parsed = parse(piece, false);
                    if parsed.plain {
                        *cache = StreamCache::default();
                        return fallback(text, streaming);
                    }
                    let mut blocks = parsed.blocks;
                    fill_highlight(&mut blocks);
                    blocks
                }
            };
            out.extend(blocks.iter().cloned());
            cache.pieces.push((end, blocks));
        } else {
            let parsed = parse(piece, false);
            if parsed.plain {
                *cache = StreamCache::default();
                return fallback(text, streaming);
            }
            let mut blocks = parsed.blocks;
            if streaming {
                mark_open(&mut blocks, &parsed.starts, &parsed.ends, piece_start, text);
            }
            fill_highlight(&mut blocks);
            out.extend(blocks);
        }
        piece_start = end;
        if out.len() > MAX_BLOCKS {
            *cache = StreamCache::default();
            return fallback(text, streaming);
        }
    }
    cache.prefix = text[..starts.last().copied().unwrap_or(0)].to_owned();
    (out, false)
}

/// Marks the blocks that the text still arriving may change: every block
/// whose source reaches into the last complete line or the line being
/// written (a list item's marker block reaches to the item's end), the code
/// block a blank line may not have ended, and the last block when only
/// blank lines or quote markers follow it.
/// `base` is where the parsed piece starts in `text`.
fn mark_open(blocks: &mut [Block], starts: &[usize], ends: &[usize], base: usize, text: &str) {
    let line = text.rfind('\n').map_or(0, |i| i + 1);
    let prev = if line == 0 { 0 } else { text[..line - 1].rfind('\n').map_or(0, |i| i + 1) };
    let prev_blank = line > 0 && blank(&text[prev..line - 1]);
    let mut from = ends.iter().position(|&e| base + e > prev).unwrap_or(blocks.len());
    if prev_blank {
        let last_before = starts.iter().rposition(|&s| base + s < line);
        if let Some(k) = last_before.filter(|&k| blocks.get(k).is_some_and(|b| b.kind == BlockKind::Code)) {
            from = from.min(k);
        }
    }
    // The last block is open while nothing but blank lines (or bare quote
    // markers) follows it; a definition being written after it makes no
    // block of its own and leaves it final.
    if let Some(&end) = ends.last() {
        let quiet = |line: &str| line.bytes().all(|b| matches!(b, b' ' | b'\t' | b'>'));
        if text.get(base + end..).is_none_or(|rest| rest.split('\n').all(quiet)) {
            from = from.min(blocks.len().saturating_sub(1));
        }
    }
    for b in blocks.iter_mut().skip(from) {
        b.open = true;
    }
}

/// The whole text parsed in one go (it has syntax that ties blocks together).
fn whole(text: &str, streaming: bool) -> (Vec<Block>, bool) {
    let parsed = parse(text, false);
    if parsed.plain {
        return fallback(text, streaming);
    }
    let mut blocks = parsed.blocks;
    if streaming {
        mark_open(&mut blocks, &parsed.starts, &parsed.ends, 0, text);
    }
    fill_highlight(&mut blocks);
    (blocks, false)
}

fn fallback(text: &str, streaming: bool) -> (Vec<Block>, bool) {
    let mut blocks = plain_doc(text).blocks;
    for b in &mut blocks {
        b.open = streaming;
    }
    (blocks, true)
}

/// Highlights the code blocks that are not open.
fn fill_highlight(blocks: &mut [Block]) {
    for b in blocks.iter_mut().filter(|b| b.kind == BlockKind::Code && !b.open && b.lines.is_empty()) {
        b.lines = highlight(&b.lang, &b.text);
    }
}

/// Where a line may still tie a block to others: link definitions and
/// footnotes (anywhere), HTML blocks that run across blank lines, container
/// directives and display math.
fn document_wide(text: &str) -> bool {
    if text.contains("[^") {
        return true;
    }
    text.split('\n').any(|line| {
        let indent = line.bytes().take_while(|&b| b == b' ').count();
        if indent > 3 {
            return false;
        }
        let rest = &line[indent..];
        if let Some(label) = rest.strip_prefix('[')
            && let Some(close) = label.find(']')
            && close > 0
            && label[close + 1..].starts_with(':')
        {
            return true;
        }
        if let Some(after) = rest.strip_prefix('<')
            && after.starts_with(|c: char| c.is_ascii_alphabetic() || matches!(c, '!' | '?' | '/'))
        {
            return true;
        }
        rest.starts_with(":::") || rest.starts_with("$$")
    })
}

/// A fence opener: `^ {0,3}(`{3,}(?![^`\n]*`)|~{3,})`, as (indent, run).
fn fence(line: &str) -> Option<(usize, &str)> {
    let indent = line.bytes().take_while(|&b| b == b' ').count();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let ch = match rest.as_bytes().first()? {
        b'`' => '`',
        b'~' => '~',
        _ => return None,
    };
    let run = rest.bytes().take_while(|&b| b == ch as u8).count();
    if run < 3 {
        return None;
    }
    if ch == '`' && rest[run..].contains('`') {
        return None;
    }
    Some((indent, &rest[..run]))
}

/// After a blank line, these still continue the block before: an indented
/// line or a list item. `^(?:[ \t]|(?:[-*+]|\d{1,9}[.)])(?:[ \t]|$))`.
fn continues(line: &str) -> bool {
    let b = line.as_bytes();
    match b.first() {
        Some(b' ' | b'\t') => true,
        Some(b'-' | b'*' | b'+') => matches!(b.get(1), None | Some(b' ' | b'\t')),
        Some(d) if d.is_ascii_digit() => {
            let digits = b.iter().take_while(|x| x.is_ascii_digit()).count();
            digits <= 9
                && matches!(b.get(digits), Some(b'.' | b')'))
                && matches!(b.get(digits + 1), None | Some(b' ' | b'\t'))
        }
        _ => false,
    }
}

fn blank(line: &str) -> bool {
    line.bytes().all(|b| b == b' ' || b == b'\t')
}

/// The offsets where a new piece starts (after 0), or `None` when the text
/// must be parsed whole. A piece starts at a complete line that follows a
/// blank line outside any fence and does not continue the block before.
pub(crate) fn split(text: &str) -> Option<Vec<usize>> {
    if document_wide(text) {
        return None;
    }
    let mut starts = Vec::new();
    let mut piece_start = 0;
    let mut line_start = 0;
    let mut open_fence: Option<(usize, String)> = None;
    let mut after_blank = false;
    for line in text.split('\n') {
        let complete = line_start + line.len() < text.len();
        if let Some((indent, run)) = &open_fence {
            // A line left of an indented fence may end the list item holding
            // it, which is not tracked: nothing after it is cut.
            if line.as_bytes().iter().take(*indent).any(|&b| b != b' ' && b != b'\t') {
                break;
            }
            let closes = fence(line).is_some_and(|(_, r)| {
                r.as_bytes()[0] == run.as_bytes()[0]
                    && r.len() >= run.len()
                    && line.trim_matches([' ', '\t']) == r
            });
            if closes {
                open_fence = None;
            }
        } else if blank(line) {
            after_blank = true;
        } else {
            if after_blank && line_start > piece_start && !continues(line) && complete {
                starts.push(line_start);
                piece_start = line_start;
            }
            after_blank = false;
            open_fence = fence(line).map(|(indent, run)| (indent, run.to_owned()));
        }
        line_start += line.len() + 1;
    }
    Some(starts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::markdown;

    #[test]
    fn splits_at_blank_lines_that_close_everything() {
        assert_eq!(split("a\n\nb\n"), Some(vec![3]));
        assert_eq!(split("a\n\n- b\n"), Some(vec![]));
        assert_eq!(split("a\n\n  b\n"), Some(vec![]));
        assert_eq!(split("```\na\n\nb\n```\n\nc\n"), Some(vec![14]));
        assert_eq!(split("a\n\nb"), Some(vec![]), "an unfinished line is never cut before");
        assert_eq!(split("a\n\n<div>\n\nb\n"), None);
        assert_eq!(split("a\u{a0}\n\u{a0}\nb\n"), Some(vec![]), "only spaces and tabs make a line blank");
    }

    #[test]
    fn pieces_parse_like_the_whole() {
        let text =
            "# T\n\npara *one*\n\n1. a\n\n2. b\n\n```rust\nfn x() {}\n\n```\n\n| a |\n|---|\n| 1 |\n\nend\n";
        let mut cache = StreamCache::default();
        let (blocks, plain) = blocks(text, false, &mut cache);
        assert!(!plain);
        assert_eq!(blocks, markdown(text).blocks);
    }

    #[test]
    fn open_blocks_are_the_unfinished_line_and_the_one_before() {
        let mut cache = StreamCache::default();
        let (b, _) = blocks("one\n\ntwo\nthr", true, &mut cache);
        let open: Vec<_> = b.iter().map(|b| b.open).collect();
        assert_eq!(open, [false, true]);
        let (b, _) = blocks("one\n\ntw", true, &mut cache);
        let open: Vec<_> = b.iter().map(|b| b.open).collect();
        assert_eq!(open, [false, true], "a blank line closes a paragraph");
        let (b, _) = blocks("    code\n\n  ", true, &mut cache);
        assert!(b.iter().all(|b| b.open), "an indented code block may go on");
        let (b, _) = blocks("x\n- -", true, &mut cache);
        assert!(b.iter().all(|b| b.open));
    }
}
