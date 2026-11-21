// SPDX-License-Identifier: MPL-2.0
//! Command output into lines marked where they report an error, a warning
//! or a pass, with `path:line:col` places split into folder, file and
//! position; and a path on its own split the same way.
//!
//! The tests are literal line prefixes, a few infixes and suffixes, and a
//! scan for places at word boundaries: no regular expressions, so the
//! launcher's port marks the same runs. Output that guesses as a diff or
//! JSON is highlighted as that instead.

use crate::bounds::{
    MAX_CODE_BYTES, MAX_CODE_LINE_UNITS, MAX_CODE_LINES, MAX_PLACES_PER_LINE, clean, units, units32,
};
use crate::detect::guess;
use crate::highlight::{CodeLine, Tok, TokClass, highlight};

/// A line starting with one of these (after its indent) reports an error,
const ERROR_STARTS: &[&str] = &[
    "error",
    "Error",
    "ERROR",
    "fatal:",
    "fatal error",
    "FATAL",
    "FAIL",
    "--- FAIL",
    "BUILD FAILED",
    "Traceback ",
    "E   ",
    "e: ",
    "panicked",
    "test result: FAILED",
];
/// as does one holding one of these,
const ERROR_HAS: &[&str] = &[" panicked at ", ": error:", ": error ", " - error ", ": fatal error:"];
/// or ending with one of these.
const ERROR_ENDS: &[&str] = &["FAILED", "FAILED."];
const WARN_STARTS: &[&str] =
    &["warning", "Warning", "WARNING", "WARN", "warn:", "w: ", "npm WARN", "DeprecationWarning"];
const WARN_HAS: &[&str] = &[": warning:", ": warning ", " - warning "];
const OK_STARTS: &[&str] = &[
    "ok ",
    "ok\t",
    "PASS ",
    "PASS:",
    "PASSED",
    "test result: ok",
    "BUILD SUCCESSFUL",
    "Successfully ",
    "✓ ",
];
const OK_ENDS: &[&str] = &[" ok", " PASSED"];

/// The lines of command output with their marked runs: Error, Warn or Ok
/// for a whole line that reports one, Dir, File and Pos for each
/// `path:line[:col]` in it, Plain for the rest. A diff or JSON (by
/// [`guess`]) is highlighted as that. Empty when nothing is marked or the
/// output is over a bound of [`highlight`]: the client shows it plain.
pub fn output_lines(text: &str) -> Vec<CodeLine> {
    if text.len() > MAX_CODE_BYTES {
        return Vec::new();
    }
    let text = clean(text);
    if let Some(lang @ ("diff" | "json")) = guess(&text, None) {
        return highlight(lang, &text);
    }
    let mut texts: Vec<&str> = text.split('\n').collect();
    if texts.len() > 1 && texts.last().is_some_and(|l| l.is_empty()) {
        texts.pop();
    }
    if texts.len() > MAX_CODE_LINES || texts.iter().any(|l| units(l) > MAX_CODE_LINE_UNITS) {
        return Vec::new();
    }
    let lines: Vec<CodeLine> =
        texts.iter().map(|t| CodeLine { text: (*t).to_owned(), toks: line_toks(t) }).collect();
    let marked = lines.iter().any(|l| l.toks.iter().any(|t| t.class != TokClass::Plain));
    if marked { lines } else { Vec::new() }
}

/// What a whole line reports, if anything; an error outranks a warning,
/// which outranks a pass.
fn level(line: &str) -> Option<TokClass> {
    let line = line.trim_start();
    let trimmed = line.trim_end();
    let any = |starts: &[&str], has: &[&str], ends: &[&str]| {
        starts.iter().any(|s| line.starts_with(s))
            || has.iter().any(|s| line.contains(s))
            || ends.iter().any(|s| trimmed.ends_with(s))
    };
    if any(ERROR_STARTS, ERROR_HAS, ERROR_ENDS) {
        Some(TokClass::Error)
    } else if any(WARN_STARTS, WARN_HAS, &[]) {
        Some(TokClass::Warn)
    } else if any(OK_STARTS, &[], OK_ENDS) {
        Some(TokClass::Ok)
    } else {
        None
    }
}

/// A line's runs: its level's class, with places split out.
fn line_toks(line: &str) -> Vec<Tok> {
    let base = level(line).unwrap_or(TokClass::Plain);
    let mut toks: Vec<Tok> = Vec::new();
    let mut at = 0;
    for (start, end) in places(line) {
        push(&mut toks, &line[at..start], base);
        for (len, class) in path_runs(&line[start..end]) {
            push_units(&mut toks, len, class);
        }
        at = end;
    }
    push(&mut toks, &line[at..], base);
    toks
}

fn push(toks: &mut Vec<Tok>, piece: &str, class: TokClass) {
    push_units(toks, units32(piece), class);
}

fn push_units(toks: &mut Vec<Tok>, len: u32, class: TokClass) {
    if len == 0 {
        return;
    }
    match toks.last_mut() {
        Some(last) if last.class == class => last.len = last.len.saturating_add(len),
        _ => toks.push(Tok { len, class }),
    }
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '\\' | '-' | '@' | '+' | '~')
}

/// What may come right before a place: the line's start, a space, or an
/// opening bracket or quote.
fn opens_place(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | '[' | '<' | '"' | '\'' | '`' | ',' | '=')
}

/// Byte ranges of `path:line[:col]` in `line`, at word boundaries: the path
/// has a letter and a `/` or `.`, and the line and column are digits.
fn places(line: &str) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while out.len() < MAX_PLACES_PER_LINE {
        let Some(colon) = line[from..].find(':').map(|i| from + i) else { break };
        from = colon + 1;
        let digits = |at: usize| bytes[at..].iter().take_while(|b| b.is_ascii_digit()).count();
        let n = digits(colon + 1);
        if n == 0 {
            continue;
        }
        let start =
            line[..colon].char_indices().rev().take_while(|&(_, c)| is_path_char(c)).last().map(|(i, _)| i);
        let Some(start) = start else { continue };
        if line[..start].chars().next_back().is_some_and(|c| !opens_place(c)) {
            continue;
        }
        let path = &line[start..colon];
        if !path.chars().any(|c| c.is_ascii_alphabetic()) || !path.contains(['/', '.', '\\']) {
            continue;
        }
        let mut end = colon + 1 + n;
        if bytes.get(end) == Some(&b':') && digits(end + 1) > 0 {
            end += 1 + digits(end + 1);
        }
        if bytes.get(end).is_some_and(u8::is_ascii_alphanumeric) {
            continue;
        }
        out.push((start, end));
        from = end;
    }
    out
}

/// A path split into its folder (Dir, through the last separator), its
/// name (File) and a trailing `:line[:col]` (Pos), as `(units, class)`.
pub(crate) fn path_runs(path: &str) -> Vec<(u32, TokClass)> {
    let mut name_end = path.len();
    // A trailing `:digits` or `:digits:digits` is the position.
    let mut tail = path;
    for _ in 0..2 {
        match tail.rsplit_once(':') {
            Some((head, digits)) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
                name_end = head.len();
                tail = head;
            }
            _ => break,
        }
    }
    let dir_end = path[..name_end].rfind(['/', '\\']).map_or(0, |i| i + 1);
    [(0, dir_end, TokClass::Dir), (dir_end, name_end, TokClass::File), (name_end, path.len(), TokClass::Pos)]
        .into_iter()
        .filter(|(a, b, _)| b > a)
        .map(|(a, b, class)| (units32(&path[a..b]), class))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(line: &str) -> Vec<(String, TokClass)> {
        let units: Vec<u16> = line.encode_utf16().collect();
        let mut at = 0usize;
        line_toks(line)
            .iter()
            .map(|t| {
                let end = at + t.len as usize;
                let s = String::from_utf16_lossy(&units[at..end]);
                at = end;
                (s, t.class)
            })
            .collect()
    }

    #[test]
    fn places_split_and_levels_mark_lines() {
        assert_eq!(
            runs("  --> src/main.rs:3:5"),
            [
                ("  --> ".into(), TokClass::Plain),
                ("src/".into(), TokClass::Dir),
                ("main.rs".into(), TokClass::File),
                (":3:5".into(), TokClass::Pos),
            ]
        );
        assert_eq!(runs("error[E0425]: x"), [("error[E0425]: x".into(), TokClass::Error)]);
        assert_eq!(runs("test a ... ok"), [("test a ... ok".into(), TokClass::Ok)]);
        assert_eq!(
            runs("see http://host:8080/x at 12:30"),
            [("see http://host:8080/x at 12:30".into(), TokClass::Plain)]
        );
    }

    #[test]
    fn path_runs_split() {
        assert_eq!(path_runs("a/b.rs:1:2"), [(2, TokClass::Dir), (4, TokClass::File), (4, TokClass::Pos)]);
        assert_eq!(path_runs("README.md"), [(9, TokClass::File)]);
        assert_eq!(path_runs("src/"), [(4, TokClass::Dir)]);
    }
}
