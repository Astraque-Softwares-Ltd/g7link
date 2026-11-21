// SPDX-License-Identifier: MPL-2.0
//! What git's and gh's output looks like before any client sees it (§SC3.5):
//! escape sequences and control characters dropped, what a terminal would
//! have written over (`\r`) dropped, a URL's `user:secret@` written `…@`,
//! the home folder written `~`, and at most [`MAX_LINE_CHARS`] characters.

use crate::api::clean;

/// Characters kept of one line.
pub const MAX_LINE_CHARS: usize = 512;

/// One line of output, cleaned. `home`: the user's home folder.
pub fn line(raw: &[u8], home: Option<&str>) -> String {
    let text = String::from_utf8_lossy(raw);
    let text = text.strip_suffix('\r').unwrap_or(&text);
    let text = text.rsplit('\r').next().unwrap_or(text);
    let text = without_escapes(text);
    let text = without_userinfo(&text);
    let text = match home.filter(|h| h.len() > 1) {
        Some(home) => home_as_tilde(&text, home),
        None => text,
    };
    let text = clean(&text).replace('\t', " ");
    match text.char_indices().nth(MAX_LINE_CHARS) {
        Some((end, _)) => text[..end].to_owned(),
        None => text,
    }
}

/// Text from a remote or a forge (a title, a check's name): control
/// characters dropped, cut to `max` bytes at a character boundary.
pub fn text(raw: &str, max: usize) -> String {
    let cleaned = without_escapes(raw);
    let cleaned = clean(&cleaned).replace(['\n', '\t'], " ");
    crate::api::cut(cleaned.trim(), max).to_owned()
}

/// ANSI and other escape sequences removed: CSI (`ESC [ … final`), OSC
/// (`ESC ] … BEL` or `ESC \`), and any other two-character escape.
fn without_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Every `scheme://user[:secret]@host` written `scheme://…@host`.
pub(crate) fn without_userinfo(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        let end =
            tail.find(|c: char| c == '/' || c.is_whitespace() || c == '"' || c == '\'').unwrap_or(tail.len());
        let authority = &tail[..end];
        match authority.rfind('@') {
            Some(user_end) => {
                out.push('…');
                out.push_str(&authority[user_end..]);
            }
            None => out.push_str(authority),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

fn home_as_tilde(s: &str, home: &str) -> String {
    let home = home.strip_suffix('/').unwrap_or(home);
    let path_char = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/');
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find(home) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + home.len()..].chars().next();
        let whole = !before.is_some_and(path_char) && after.is_none_or(|c| c == '/' || !path_char(c));
        out.push_str(&rest[..at]);
        out.push_str(if whole { "~" } else { home });
        rest = &rest[at + home.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_controls_and_overwritten_text_go() {
        assert_eq!(line(b"\x1b[31merror\x1b[0m: no", None), "error: no");
        assert_eq!(line(b"\x1b]8;;https://x\x07link\x1b]8;;\x1b\\ end", None), "link end");
        assert_eq!(line(b"Counting 10%\rCounting 100%, done.\r", None), "Counting 100%, done.");
        assert_eq!(line(b"a\x00b\x7fc\x08d\te", None), "abcd e");
        assert_eq!(line(&[b'x', 0xff, b'y'], None), "x\u{fffd}y");
    }

    #[test]
    fn secrets_in_urls_and_the_home_folder_are_hidden() {
        assert_eq!(
            line(b"To https://kim:s3cret@example.test/o/r.git", None),
            "To https://\u{2026}@example.test/o/r.git"
        );
        assert_eq!(
            line(b"fatal: 'ssh://git@host:22/x' and http://h/p", None),
            "fatal: 'ssh://\u{2026}@host:22/x' and http://h/p"
        );
        assert_eq!(
            line(b"see /home/kim/repo and /home/kimberly", Some("/home/kim")),
            "see ~/repo and /home/kimberly"
        );
        assert_eq!(line(b"/home/kim", Some("/home/kim/")), "~");
        assert_eq!(line(b"/x/home/kim/a", Some("/home/kim")), "/x/home/kim/a");
    }

    #[test]
    fn a_line_is_cut_at_512_characters() {
        let long = "é".repeat(2000);
        assert_eq!(line(long.as_bytes(), None).chars().count(), MAX_LINE_CHARS);
        assert_eq!(text(" a\u{1b}[1mb\nc ", 10), "ab c");
        assert_eq!(text(&"x".repeat(300), 256).len(), 256);
    }
}
