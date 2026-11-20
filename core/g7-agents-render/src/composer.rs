// SPDX-License-Identifier: MPL-2.0
// Parts ported from t3code (MIT); see THIRD-PARTY-T3CODE.md.
//! The composer's triggers: `@` for a file of the thread's folder and `/` for
//! a command, where the cursor is; putting the chosen one in; and the
//! `@path` mentions of a text, for chips.
//!
//! Offsets are UTF-16 code units, as everywhere in the crate; one that falls
//! inside a surrogate pair counts as the start of its character, one past
//! the end as the end. Whitespace is Unicode's White_Space, which is what
//! `char::is_whitespace` tests: U+0009 to U+000D, U+0020, U+0085, U+00A0,
//! U+1680, U+2000 to U+200A, U+2028, U+2029, U+202F, U+205F and U+3000.

use serde::{Deserialize, Serialize};

use crate::bounds::{MAX_MENTIONS, MAX_TRIGGER_QUERY_UNITS, clean, units, units32};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerKind {
    /// `@` at the start of the text or after whitespace: a file or directory
    /// of the folder.
    Mention,
    /// `/` as the first character of the text: a command.
    Slash,
}

/// A trigger at the cursor.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trigger {
    pub kind: TriggerKind,
    /// Where its `@` or `/` is.
    pub start: u32,
    /// What follows it up to the cursor, cleaned.
    pub query: String,
}

impl std::fmt::Debug for Trigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Trigger")
            .field("kind", &self.kind)
            .field("start", &self.start)
            .field("query_bytes", &self.query.len())
            .finish()
    }
}

/// The byte index of UTF-16 offset `at` in `text`: the start of the
/// character it falls in, or the end.
fn byte_at(text: &str, at: u32) -> usize {
    let mut units = 0u32;
    for (i, c) in text.char_indices() {
        let next = units.saturating_add(u32::from(c.len_utf16() == 2) + 1);
        if next > at {
            return i;
        }
        units = next;
    }
    text.len()
}

/// The trigger the cursor is at, if any.
///
/// - `Slash`: the text starts with `/` and the cursor is after it, within
///   the first word. The query is the word from after the `/` up to the
///   cursor.
/// - `Mention`: an `@` at the start of the text or after whitespace starts a
///   run of characters that are not whitespace, and the cursor is at the end
///   of that run (the next character, if any, is whitespace). The query is
///   the run after the `@`.
///
/// A query over `MAX_TRIGGER_QUERY_UNITS` is no trigger.
pub fn trigger(text: &str, cursor: u32) -> Option<Trigger> {
    let at = byte_at(text, cursor);
    if text.starts_with('/') {
        let word_end = text.find(char::is_whitespace).unwrap_or(text.len());
        if at >= 1 && at <= word_end {
            return with_query(TriggerKind::Slash, 0, &text[1..at]);
        }
    }
    if text[at..].chars().next().is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    let run_start =
        text[..at].char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(i, c)| i + c.len_utf8());
    let query = text[run_start..at].strip_prefix('@')?;
    with_query(TriggerKind::Mention, units32(&text[..run_start]), query)
}

fn with_query(kind: TriggerKind, start: u32, query: &str) -> Option<Trigger> {
    if units(query) > MAX_TRIGGER_QUERY_UNITS {
        return None;
    }
    Some(Trigger { kind, start, query: clean(query).into_owned() })
}

/// `text` with the trigger's run, from its start up to `cursor`, replaced by
/// the chosen value and one space after it; with the new cursor, after that
/// space. For `/` the run is the whole first word, even when the cursor is
/// inside it.
///
/// `insert` is the bare value: a path relative to the folder, or a
/// command's name; its control and bidi characters are dropped. The `@` or
/// `/` is written by this function. A path with whitespace is written as
/// `@"the path"`. The space is the one already after the run when there is
/// one; a directory (a path ending in `/`, unquoted) gets none, so the
/// cursor stays after its `/` and the list opens again inside it.
pub fn apply(text: &str, trigger: &Trigger, cursor: u32, insert: &str) -> (String, u32) {
    let mention = trigger.kind == TriggerKind::Mention;
    let start = byte_at(text, trigger.start);
    let mut end = byte_at(text, cursor);
    if !mention {
        end = end.max(text.find(char::is_whitespace).unwrap_or(text.len()));
    }
    let end = end.max(start);
    let value: String = clean(insert).chars().filter(|c| !c.is_control()).collect();
    let quoted = mention && value.chars().any(char::is_whitespace);
    let mut piece = String::with_capacity(value.len() + 4);
    piece.push(if mention { '@' } else { '/' });
    if quoted {
        piece.push('"');
        piece.push_str(&value);
        piece.push('"');
    } else {
        piece.push_str(&value);
    }
    let directory = mention && !quoted && value.ends_with('/');
    let mut tail = &text[end..];
    if !directory {
        piece.push(' ');
        tail = tail.strip_prefix(' ').unwrap_or(tail);
    }
    let mut out = String::with_capacity(start + piece.len() + tail.len());
    out.push_str(&text[..start]);
    out.push_str(&piece);
    let cursor = units32(&out);
    out.push_str(tail);
    (out, cursor)
}

/// The `@path` mentions of `text`, as (start, length) in UTF-16 units, in
/// order, at most `MAX_MENTIONS`: an `@` at the start or after whitespace,
/// then either a quoted path (`@"the path"`, closed on the same line, not
/// empty) or one or more characters that are not whitespace.
pub fn mentions(text: &str) -> Vec<(u32, u32)> {
    let mut found = Vec::new();
    let mut at = 0usize;
    let mut units_at = 0u32;
    let mut after_space = true;
    while let Some(c) = text[at..].chars().next() {
        if found.len() >= MAX_MENTIONS {
            break;
        }
        if c == '@' && after_space {
            let rest = &text[at + 1..];
            let quoted = rest.strip_prefix('"').and_then(|q| {
                let close = q.find(['"', '\n'])?;
                (q[close..].starts_with('"') && close > 0).then_some(close + 2)
            });
            let len = quoted.unwrap_or_else(|| rest.find(char::is_whitespace).unwrap_or(rest.len()));
            if len > 0 {
                let token = &text[at..at + 1 + len];
                let n = units32(token);
                found.push((units_at, n));
                at += token.len();
                units_at = units_at.saturating_add(n);
                after_space = false;
                continue;
            }
        }
        after_space = c.is_whitespace();
        at += c.len_utf8();
        units_at = units_at.saturating_add(u32::from(c.len_utf16() == 2) + 1);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str, cursor: u32) -> Option<(TriggerKind, u32, String)> {
        trigger(text, cursor).map(|t| (t.kind, t.start, t.query))
    }

    #[test]
    fn mention_at_the_end_of_its_run() {
        assert_eq!(t("@", 1), Some((TriggerKind::Mention, 0, String::new())));
        assert_eq!(t("see @src/ma", 11), Some((TriggerKind::Mention, 4, "src/ma".into())));
        assert_eq!(t("see @src/ma now", 11), Some((TriggerKind::Mention, 4, "src/ma".into())));
        // Not at the end of the run, not after whitespace, not an @ at all.
        assert_eq!(t("see @src/ma", 8), None);
        assert_eq!(t("mail a@b.example", 16), None);
        assert_eq!(t("see src", 7), None);
        assert_eq!(t("see @src/ma", 4), None);
    }

    #[test]
    fn slash_only_in_the_first_word() {
        assert_eq!(t("/", 1), Some((TriggerKind::Slash, 0, String::new())));
        assert_eq!(t("/rev now", 4), Some((TriggerKind::Slash, 0, "rev".into())));
        assert_eq!(t("/review", 3), Some((TriggerKind::Slash, 0, "re".into())));
        assert_eq!(t("/review", 0), None);
        assert_eq!(t("/rev now", 8), None);
        assert_eq!(t("a /rev", 6), None);
        assert_eq!(t(" /rev", 5), None);
    }

    #[test]
    fn offsets_are_utf16_and_clamped() {
        // 😀 is two units: the @ is at 3.
        assert_eq!(t("😀 @x", 5), Some((TriggerKind::Mention, 3, "x".into())));
        // Inside the pair counts as before it; past the end as the end.
        assert_eq!(t("@😀", 2), None);
        assert_eq!(t("@😀", 99), Some((TriggerKind::Mention, 0, "😀".into())));
        assert_eq!(byte_at("a😀b", 2), 1);
        assert_eq!(byte_at("a😀b", 3), 5);
    }

    #[test]
    fn long_queries_are_no_trigger() {
        let at_limit = format!("@{}", "a".repeat(MAX_TRIGGER_QUERY_UNITS));
        assert!(trigger(&at_limit, units32(&at_limit)).is_some());
        let over = format!("@{}", "a".repeat(MAX_TRIGGER_QUERY_UNITS + 1));
        assert!(trigger(&over, units32(&over)).is_none());
    }

    #[test]
    fn apply_writes_the_sigil_quotes_and_one_space() {
        let tr = trigger("see @src/ma", 11).expect("a trigger");
        assert_eq!(apply("see @src/ma", &tr, 11, "src/main.rs"), ("see @src/main.rs ".into(), 17));
        assert_eq!(apply("see @src/ma now", &tr, 11, "src/main.rs"), ("see @src/main.rs now".into(), 17));
        assert_eq!(apply("see @src/ma", &tr, 11, "my docs/a b.md"), ("see @\"my docs/a b.md\" ".into(), 22));
        assert_eq!(
            apply("see @sr", &trigger("see @sr", 7).expect("a trigger"), 7, "src/"),
            ("see @src/".into(), 9)
        );
        let slash = trigger("/rev", 4).expect("a trigger");
        assert_eq!(apply("/rev", &slash, 4, "review"), ("/review ".into(), 8));
        let inside = trigger("/rexx the diff", 3).expect("a trigger");
        assert_eq!(apply("/rexx the diff", &inside, 3, "review"), ("/review the diff".into(), 8));
        assert_eq!(apply("/rev", &slash, 4, "re\u{202e}view\n"), ("/review ".into(), 8));
    }

    #[test]
    fn mentions_find_both_forms() {
        assert_eq!(mentions("see @src/main.rs and @\"a b.md\" too"), vec![(4, 12), (21, 9)]);
        assert_eq!(mentions("a@b.example @ @\"\" @\"open"), vec![(14, 3), (18, 6)]);
        assert_eq!(mentions("😀@x @y"), vec![(5, 2)]);
        assert!(mentions("@").is_empty());
        let many = "@a ".repeat(MAX_MENTIONS + 5);
        assert_eq!(mentions(&many).len(), MAX_MENTIONS);
    }
}
