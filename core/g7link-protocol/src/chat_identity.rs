// SPDX-License-Identifier: MPL-2.0
//! Which conversation a chat notification belongs to
//! (`docs/protocol/notifications.md` §2.3).
//!
//! A conversation is known by the ID the phone sends for it, or else by its
//! title. Chat apps write a message count into the title ("<Group> (5
//! messages)"), so the count is removed before the title is used or shown.

use crate::messages::notify_limits::CONVERSATION_ID_BYTES;

/// Most digits a count in a title has.
const COUNT_DIGITS: usize = 6;

/// The title without a leading or trailing message count.
///
/// Removes one of ` (N messages)`, ` (N message)`, ` (N new messages)`,
/// ` (N new message)`, `: N new messages`, `: N new message` at the end, or
/// `(N) ` at the start, ignoring case, where `N` is 1 to 6 ASCII digits; then
/// trims. Other brackets stay. A title that would become empty is only trimmed.
pub fn strip_count(title: &str) -> &str {
    let title = title.trim();
    // ASCII lowercasing keeps every byte offset, so the cuts below apply to `title`.
    let lower = title.to_ascii_lowercase();
    let stripped = if let Some(end) = trailing_paren_count(&lower).or_else(|| trailing_colon_count(&lower)) {
        &title[..end]
    } else if let Some(start) = leading_count(&lower) {
        &title[start..]
    } else {
        title
    };
    let stripped = stripped.trim();
    if stripped.is_empty() { title } else { stripped }
}

/// `i:<id>` when the phone sent an ID, else `t:<lowercased strip_count(title)>`.
pub fn conversation_key(id: Option<&str>, title: &str) -> String {
    match id.filter(|id| !id.is_empty()) {
        Some(id) => format!("i:{id}"),
        None => format!("t:{}", strip_count(title).to_lowercase()),
    }
}

/// Whether `id` is a conversation ID as the wire allows it: 1 to
/// [`CONVERSATION_ID_BYTES`] bytes of `[A-Za-z0-9_-]`.
pub fn is_conversation_id(id: &str) -> bool {
    (1..=CONVERSATION_ID_BYTES).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `s` without its trailing ASCII digits, if there are 1 to 6 of them.
fn without_count_end(s: &str) -> Option<&str> {
    let rest = s.trim_end_matches(|c: char| c.is_ascii_digit());
    (1..=COUNT_DIGITS).contains(&(s.len() - rest.len())).then_some(rest)
}

fn without_messages_end(s: &str) -> Option<&str> {
    s.strip_suffix("messages").or_else(|| s.strip_suffix("message"))
}

/// Where ` (N [new ]message[s])` starts in `lower`.
fn trailing_paren_count(lower: &str) -> Option<usize> {
    let rest = without_messages_end(lower.strip_suffix(')')?)?.strip_suffix(' ')?;
    let rest = rest.strip_suffix(" new").unwrap_or(rest);
    Some(without_count_end(rest)?.strip_suffix(" (")?.len())
}

/// Where `: N new message[s]` starts in `lower`.
fn trailing_colon_count(lower: &str) -> Option<usize> {
    let rest = without_messages_end(lower)?.strip_suffix(" new ")?;
    Some(without_count_end(rest)?.strip_suffix(": ")?.len())
}

/// Where the title starts after a leading `(N) ` in `lower`.
fn leading_count(lower: &str) -> Option<usize> {
    let rest = lower.strip_prefix('(')?;
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if !(1..=COUNT_DIGITS).contains(&digits) {
        return None;
    }
    let rest = rest[digits..].strip_prefix(") ")?;
    Some(lower.len() - rest.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailing_count_is_stripped() {
        for title in [
            "Book Club (5 messages)",
            "Book Club (1 message)",
            "Book Club (12 new messages)",
            "Book Club (1 new message)",
            "Book Club (5 MESSAGES)",
            "Book Club (3 New Messages)",
            "Book Club: 3 new messages",
            "Book Club: 1 new message",
            "Book Club: 999999 NEW MESSAGES",
            "(2) Book Club",
            "(123456) Book Club",
            "  Book Club (5 messages)  ",
        ] {
            assert_eq!(strip_count(title), "Book Club", "{title:?}");
        }
        assert_eq!(strip_count("Café ☕ (2 messages)"), "Café ☕");
        assert_eq!(strip_count("Book Club"), "Book Club", "nothing to strip");
        assert_eq!(strip_count("  Book Club "), "Book Club", "always trimmed");
    }

    #[test]
    fn brackets_that_are_not_counts_stay() {
        for title in [
            "Family (Mum's side)",
            "Team (2024)",
            "Team (5 photos)",
            "Team (messages)",
            "Team (1234567 messages)",
            "Team (5messages)",
            "Team (5 messages) 2",
            "Team (5 old messages)",
            "Team: 3 messages",
            "Team: new messages",
            "(2)Team",
            "(two) Team",
            "(1234567) Team",
            "Team (2)",
        ] {
            assert_eq!(strip_count(title), title, "{title:?}");
        }
        // One count is removed, once: what's left stays.
        assert_eq!(strip_count("Team (2) (5 messages)"), "Team (2)");
    }

    #[test]
    fn a_title_that_is_only_a_count_is_left_alone() {
        for title in ["(5 messages)", ": 3 new messages", "(2)", "(2) ", "5 messages"] {
            assert_eq!(strip_count(title), title.trim(), "{title:?}");
        }
    }

    #[test]
    fn a_conversation_id_outranks_the_title_in_the_key() {
        assert_eq!(conversation_key(Some("Ab_9-x"), "Book Club (5 messages)"), "i:Ab_9-x");
        assert_eq!(conversation_key(Some("Ab_9-x"), "Renamed Club"), "i:Ab_9-x");
        assert_eq!(conversation_key(None, "Book Club (5 messages)"), "t:book club");
        assert_eq!(conversation_key(None, "BOOK CLUB"), "t:book club");
        assert_eq!(conversation_key(Some(""), "Book Club"), "t:book club", "an empty ID is no ID");
    }

    #[test]
    fn conversation_ids_are_short_and_plain() {
        assert!(is_conversation_id("aZ09_-"));
        assert!(is_conversation_id(&"a".repeat(CONVERSATION_ID_BYTES)));
        assert!(!is_conversation_id(""));
        assert!(!is_conversation_id(&"a".repeat(CONVERSATION_ID_BYTES + 1)));
        for bad in ["a b", "a:b", "a/b", "a+b", "a=b", "é"] {
            assert!(!is_conversation_id(bad), "{bad:?}");
        }
    }
}
