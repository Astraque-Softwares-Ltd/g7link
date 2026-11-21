// SPDX-License-Identifier: MPL-2.0
// Parts ported from t3code (MIT); see THIRD-PARTY-T3CODE.md.
//! What a tool line and a group line say, as data: the client words them.
//! And the one-line preview of a thought.

use serde::{Deserialize, Serialize};

use crate::bounds::units32;
use crate::bounds::{
    LABEL_UNITS, MAX_SRC_BYTES, THOUGHT_PREVIEW_UNITS, clean, collapse_whitespace, cut_bytes, ellipsize,
    sat16,
};
use crate::diff::file_headers;
use crate::highlight::{Tok, TokClass, highlight};
use crate::markdown::is_zero;
use crate::output::path_runs;
use crate::timeline::{ToolClass, ToolIn, is_shown_tool};

/// The object of a tool line ("Ran <object>"), one line.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolLabel {
    pub class: ToolClass,
    /// At most `LABEL_UNITS` UTF-16 units, cut with `…`.
    pub object: String,
    /// Further files of a multi-file edit ("and N more").
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: u16,
    /// Classed runs over `object`, covering it exactly; empty for plain. A
    /// command is highlighted as shell; an edited or read path is Dir, File
    /// and Pos; a search term is String; a web address is its host as File
    /// and the rest as Dir.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub toks: Vec<Tok>,
}

impl std::fmt::Debug for ToolLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolLabel")
            .field("class", &self.class)
            .field("more", &self.more)
            .finish_non_exhaustive()
    }
}

/// A group line: its first one or two clauses and how many other actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub first: Clause,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second: Option<Clause>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub others: u16,
}

/// "Ran 3 commands": a class and a count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clause {
    pub class: ToolClass,
    pub count: u16,
}

/// The label of a tool line.
pub fn tool_label(tool: &ToolIn<'_>) -> ToolLabel {
    let title = first_line(tool.title);
    // The object, and whether it is the tool's own (its command, term or
    // path) rather than its title: only its own is classed.
    let (object, own) = match tool.class {
        ToolClass::Command => own_or(first_line(tool.detail), title),
        ToolClass::Search | ToolClass::Web => match search_term(tool.detail) {
            Some(term) => own_or(first_line(&term), title),
            None if tool.detail.trim_start().starts_with('{') => (title, false),
            None => own_or(first_line(tool.detail), title),
        },
        ToolClass::Edit | ToolClass::Read => {
            let path = first_line(tool.path);
            if !path.is_empty() {
                (path, true)
            } else {
                match path_in(&title) {
                    Some(path) => (path, true),
                    None => (title, false),
                }
            }
        }
        ToolClass::Mcp | ToolClass::Agent | ToolClass::Tasks | ToolClass::Other | ToolClass::Plan => {
            (title, false)
        }
    };
    let more =
        if tool.class == ToolClass::Edit { sat16(file_headers(tool.detail).saturating_sub(1)) } else { 0 };
    let object = ellipsize(&object, LABEL_UNITS);
    let toks = if own { label_toks(tool.class, &object) } else { Vec::new() };
    ToolLabel { class: tool.class, object, more, toks }
}

/// `own` when it is not empty, else the title.
fn own_or(own: String, title: String) -> (String, bool) {
    if own.is_empty() { (title, false) } else { (own, true) }
}

/// The classed runs of a label's object.
fn label_toks(class: ToolClass, object: &str) -> Vec<Tok> {
    if object.is_empty() {
        return Vec::new();
    }
    let runs: Vec<(u32, TokClass)> = match class {
        ToolClass::Command => {
            let mut lines = highlight("bash", object);
            return if lines.len() == 1 { lines.swap_remove(0).toks } else { Vec::new() };
        }
        ToolClass::Edit | ToolClass::Read => path_runs(object),
        ToolClass::Web if object.contains("://") => web_runs(object),
        ToolClass::Search | ToolClass::Web => vec![(units32(object), TokClass::String)],
        ToolClass::Mcp | ToolClass::Agent | ToolClass::Tasks | ToolClass::Other | ToolClass::Plan => {
            return Vec::new();
        }
    };
    runs.into_iter().map(|(len, class)| Tok { len, class }).collect()
}

/// A web address: its host as File, what comes before and after as Dir.
fn web_runs(url: &str) -> Vec<(u32, TokClass)> {
    let host_start = url.find("://").map_or(0, |i| i + 3);
    let host_end = url[host_start..].find(['/', '?', '#']).map_or(url.len(), |i| host_start + i);
    [
        (0, host_start, TokClass::Dir),
        (host_start, host_end, TokClass::File),
        (host_end, url.len(), TokClass::Dir),
    ]
    .into_iter()
    .filter(|(a, b, _)| b > a)
    .map(|(a, b, class)| (units32(&url[a..b]), class))
    .collect()
}

/// The first line of `s`, cleaned, with whitespace collapsed.
fn first_line(s: &str) -> String {
    let s = cut_bytes(s, MAX_SRC_BYTES);
    let s = clean(s);
    let line = s.split('\n').find(|l| !l.trim().is_empty()).unwrap_or_default();
    collapse_whitespace(line)
}

/// Keys of a search or fetch tool's input that name what it looked for, in
/// order (after t3code's `searchToolLabelParts`), then where.
const SEARCH_KEYS: &[&str] = &[
    "pattern",
    "query",
    "searchTerm",
    "regex",
    "grep",
    "needle",
    "glob",
    "globPattern",
    "glob_pattern",
    "include",
    "filePattern",
    "file_pattern",
    "url",
    "path",
    "target_directory",
    "targetDirectory",
    "directory",
];

/// What a search looked for, when its detail is the tool's input as JSON
/// (the host passes it through for search and web tools).
fn search_term(detail: &str) -> Option<String> {
    let detail = cut_bytes(detail, MAX_SRC_BYTES);
    if !detail.trim_start().starts_with('{') {
        return None;
    }
    SEARCH_KEYS.iter().find_map(|key| json_string(detail, key).filter(|v| !v.trim().is_empty()))
}

/// The string value of the first `"key": "…"` in JSON text, unescaped. A
/// small reader rather than a JSON parser: the label only needs one value.
fn json_string(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut from = 0;
    while let Some(at) = text.get(from..).and_then(|t| t.find(&needle)) {
        let start = from + at;
        from = start + needle.len();
        if text[..start].ends_with('\\') {
            continue;
        }
        let rest = text[from..].trim_start();
        let Some(rest) = rest.strip_prefix(':') else { continue };
        let Some(rest) = rest.trim_start().strip_prefix('"') else { continue };
        return Some(json_unescape(rest));
    }
    None
}

/// A JSON string body up to its closing quote, with escapes decoded; a
/// broken escape becomes U+FFFD.
fn json_unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    let hex = |chars: &mut std::str::Chars<'_>| -> Option<u32> {
        let h: String = chars.by_ref().take(4).collect();
        if h.len() == 4 { u32::from_str_radix(&h, 16).ok() } else { None }
    };
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r' | 'b' | 'f') => {}
                Some('u') => {
                    let hi = hex(&mut chars);
                    let c = match hi {
                        Some(hi @ 0xd800..=0xdbff) => {
                            let mut look = chars.clone();
                            let lo = (look.next() == Some('\\') && look.next() == Some('u'))
                                .then(|| hex(&mut look))
                                .flatten()
                                .filter(|lo| (0xdc00..=0xdfff).contains(lo));
                            match lo {
                                Some(lo) => {
                                    chars = look;
                                    char::from_u32(0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00))
                                }
                                None => None,
                            }
                        }
                        Some(v) => char::from_u32(v),
                        None => None,
                    };
                    out.push(c.unwrap_or('\u{fffd}'));
                }
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    out
}

/// The first word of `title` that looks like a path: it has a slash, or a
/// dot with something on both sides.
fn path_in(title: &str) -> Option<String> {
    title
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | ',' | ':' | ';' | '(' | ')')))
        .find(|w| {
            w.contains('/')
                || w.contains('\\')
                || w.char_indices().any(|(i, c)| c == '.' && i > 0 && i + 1 < w.len())
        })
        .map(str::to_owned)
}

/// The summary of a group's tools: per class, with edits and reads counted
/// as distinct paths; in the order command, edit, read, search, web, then
/// everything else as one "other" tally. Task lists, the plan file and tool
/// searches are not counted.
pub fn summary(tools: &[ToolIn<'_>]) -> Summary {
    let mut counts = [0usize; 6];
    let mut edits: Vec<&str> = Vec::new();
    let mut reads: Vec<&str> = Vec::new();
    for tool in tools {
        if !is_shown_tool(tool) {
            continue;
        }
        let slot = match tool.class {
            ToolClass::Command => 0,
            ToolClass::Edit => {
                if tool.path.is_empty() || !edits.contains(&tool.path) {
                    if !tool.path.is_empty() {
                        edits.push(tool.path);
                    }
                    counts[1] += 1;
                }
                continue;
            }
            ToolClass::Read => {
                if tool.path.is_empty() || !reads.contains(&tool.path) {
                    if !tool.path.is_empty() {
                        reads.push(tool.path);
                    }
                    counts[2] += 1;
                }
                continue;
            }
            ToolClass::Search => 3,
            ToolClass::Web => 4,
            ToolClass::Tasks | ToolClass::Plan => continue,
            ToolClass::Mcp | ToolClass::Agent | ToolClass::Other => 5,
        };
        counts[slot] += 1;
    }
    let classes = [
        ToolClass::Command,
        ToolClass::Edit,
        ToolClass::Read,
        ToolClass::Search,
        ToolClass::Web,
        ToolClass::Other,
    ];
    let mut clauses = classes
        .into_iter()
        .zip(counts)
        .filter(|(_, n)| *n > 0)
        .map(|(class, n)| Clause { class, count: sat16(n) });
    let first = clauses.next().unwrap_or(Clause { class: ToolClass::Other, count: 0 });
    let second = clauses.next();
    let others = sat16(clauses.map(|c| usize::from(c.count)).sum());
    Summary { first, second, others }
}

/// JavaScript's `\s` (and what `String.prototype.trim` removes).
fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// JavaScript's `\w`.
fn js_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Where `^` matches with JavaScript's `m` flag: after a line terminator.
fn js_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// The line the live row and a thought's preview show: a leading
/// `**Heading**` line if there is one, else the first sentence, with link,
/// list, heading and emphasis marks stripped and whitespace collapsed; cut
/// to `THOUGHT_PREVIEW_UNITS`. A port of t3code's `liveThoughtLine`.
pub fn live_thought_line(markdown: &str) -> String {
    let src = clean(cut_bytes(markdown, MAX_SRC_BYTES));
    let heading = bold_heading(&src);
    let text = strip_links(heading.unwrap_or(&src));
    let text = strip_line_marks(&text);
    let text = strip_code_marks(&text);
    let text = strip_emphasis(&text);
    let text = collapse_js_space(&text);
    if heading.is_some() {
        return ellipsize(&text, THOUGHT_PREVIEW_UNITS);
    }
    ellipsize(first_sentence(&text), THOUGHT_PREVIEW_UNITS)
}

/// `/^\s*\*\*([^*\r\n]+)\*\*[ \t]*\r?(?:\n|$)/`, group 1.
fn bold_heading(s: &str) -> Option<&str> {
    let rest = s.trim_start_matches(js_space).strip_prefix("**")?;
    let end = rest.find(['*', '\r', '\n'])?;
    if end == 0 {
        return None;
    }
    let inner = &rest[..end];
    let after = rest[end..].strip_prefix("**")?.trim_start_matches([' ', '\t']);
    let after = after.strip_prefix('\r').unwrap_or(after);
    (after.is_empty() || after.starts_with('\n')).then_some(inner)
}

/// `.replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")`.
fn strip_links(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    // The next `]` and `)` at or after a position only move forward as the
    // scan does, so each is searched for once per stretch.
    let mut next_bracket: Option<Option<usize>> = None;
    let mut next_paren: Option<Option<usize>> = None;
    let find_from = |cache: &mut Option<Option<usize>>, from: usize, byte: u8| -> Option<usize> {
        if let Some(Some(at)) = *cache
            && at >= from
        {
            return Some(at);
        }
        if let Some(None) = *cache {
            return None;
        }
        let found = b.get(from..).and_then(|rest| rest.iter().position(|&x| x == byte)).map(|p| p + from);
        *cache = Some(found);
        found
    };
    let mut i = 0;
    while i < s.len() {
        let open = if b[i] == b'[' {
            Some(i)
        } else if b[i] == b'!' && b.get(i + 1) == Some(&b'[') {
            Some(i + 1)
        } else {
            None
        };
        if let Some(open) = open
            && let Some(close) = find_from(&mut next_bracket, open + 1, b']')
            && b.get(close + 1) == Some(&b'(')
            && let Some(paren) = find_from(&mut next_paren, close + 2, b')')
        {
            out.push_str(&s[open + 1..close]);
            i = paren + 1;
            continue;
        }
        let c = s[i..].chars().next().unwrap_or_default();
        out.push(c);
        i += c.len_utf8().max(1);
    }
    out
}

/// `.replace(/^[ \t]*(?:#{1,6}|[-*+]|\d+\.)[ \t]+/gm, "")`.
fn strip_line_marks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut line_start = true;
    let mut i = 0;
    while i < s.len() {
        if line_start {
            line_start = false;
            if let Some(len) = line_mark(&s[i..]) {
                i += len;
                continue;
            }
        }
        let c = s[i..].chars().next().unwrap_or_default();
        out.push(c);
        i += c.len_utf8().max(1);
        line_start = js_line_break(c);
    }
    out
}

/// The length of a list or heading mark at the start of `s`, with the
/// blanks around it.
fn line_mark(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut k = 0;
    while matches!(b.get(k), Some(b' ' | b'\t')) {
        k += 1;
    }
    let mark = match b.get(k)? {
        b'#' => {
            let run = b[k..].iter().take_while(|&&x| x == b'#').count();
            (run <= 6).then_some(run)?
        }
        b'-' | b'*' | b'+' => 1,
        d if d.is_ascii_digit() => {
            let run = b[k..].iter().take_while(|x| x.is_ascii_digit()).count();
            (b.get(k + run) == Some(&b'.')).then_some(run + 1)?
        }
        _ => return None,
    };
    k += mark;
    let blanks = b[k..].iter().take_while(|&&x| x == b' ' || x == b'\t').count();
    (blanks > 0).then_some(k + blanks)
}

/// `.replace(/`+|\*\*|~~/g, "")`.
fn strip_code_marks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' => {
                while chars.peek() == Some(&'`') {
                    chars.next();
                }
            }
            '*' | '~' if chars.peek() == Some(&c) => {
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

/// `.replace(/(^|[^\w*])[*_]([^*_\n]+)[*_](?![\w*])/g, "$1$2")`.
fn strip_emphasis(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let marker = |c: Option<&char>| matches!(c, Some('*' | '_'));
    // A match of `[*_]([^*_\n]+)[*_](?![\w*])` from `at`: the end of group 2.
    let tail = |at: usize| -> Option<usize> {
        if !marker(chars.get(at)) {
            return None;
        }
        let inner = chars.get(at + 1..)?.iter().position(|&c| matches!(c, '*' | '_' | '\n'))?;
        let close = at + 1 + inner;
        if inner == 0 || chars.get(close) == Some(&'\n') {
            return None;
        }
        match chars.get(close + 1) {
            Some(&c) if js_word(c) || c == '*' => None,
            _ => Some(close),
        }
    };
    let mut i = 0;
    while i < chars.len() {
        if i == 0
            && let Some(close) = tail(0)
        {
            out.extend(&chars[1..close]);
            i = close + 1;
            continue;
        }
        let c = chars[i];
        if !js_word(c)
            && c != '*'
            && let Some(close) = tail(i + 1)
        {
            out.push(c);
            out.extend(&chars[i + 2..close]);
            i = close + 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// `.replace(/\s+/g, " ").trim()`.
fn collapse_js_space(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if js_space(c) {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    out
}

/// Up to the end of the first sentence:
/// `/[.?!]["'”’)]?(?=\s)|[。！？]["'”’）」』]?/`.
fn first_sentence(s: &str) -> &str {
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let next = it.peek().map(|&(_, n)| n);
        let after = |n: usize| s[n..].chars().next();
        match c {
            '.' | '?' | '!' => {
                let end = i + c.len_utf8();
                if let Some(q) = next.filter(|q| matches!(q, '"' | '\'' | '”' | '’' | ')')) {
                    let q_end = end + q.len_utf8();
                    if after(q_end).is_some_and(js_space) {
                        return &s[..q_end];
                    }
                }
                if next.is_some_and(js_space) {
                    return &s[..end];
                }
            }
            '。' | '！' | '？' => {
                let end = i + c.len_utf8();
                if let Some(q) = next.filter(|q| matches!(q, '"' | '\'' | '”' | '’' | '）' | '」' | '』'))
                {
                    return &s[..end + q.len_utf8()];
                }
                return &s[..end];
            }
            _ => {}
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::ToolStatus;

    fn tool<'a>(class: ToolClass, title: &'a str, detail: &'a str, path: &'a str) -> ToolIn<'a> {
        ToolIn { name: "", title, class, detail, path, status: ToolStatus::Ok, output: "", by_rule: false }
    }

    #[test]
    fn labels_by_class() {
        let l = tool_label(&tool(ToolClass::Command, "Bash", "cargo   test\n  --all", ""));
        assert_eq!(l.object, "cargo test");
        let l = tool_label(&tool(ToolClass::Edit, "Edit src/main.rs", "", ""));
        assert_eq!(l.object, "src/main.rs");
        let l = tool_label(&tool(ToolClass::Read, "Read", "", "README.md"));
        assert_eq!(l.object, "README.md");
        let l = tool_label(&tool(ToolClass::Mcp, "mcp__x__y", "", ""));
        assert_eq!(l.object, "mcp__x__y");
        let long = "x".repeat(400);
        let l = tool_label(&tool(ToolClass::Command, "", &long, ""));
        assert_eq!(l.object.chars().count(), LABEL_UNITS);
        assert!(l.object.ends_with('…'));
    }

    #[test]
    fn search_labels_never_show_json() {
        let l =
            tool_label(&tool(ToolClass::Search, "Grep", r#"{"pattern":"fn \"x\"\u00e9","path":"src"}"#, ""));
        assert_eq!(l.object, "fn \"x\"é");
        let l =
            tool_label(&tool(ToolClass::Web, "WebFetch", r#"{"url":"https://x.example","prompt":"p"}"#, ""));
        assert_eq!(l.object, "https://x.example");
        let l = tool_label(&tool(ToolClass::Search, "LS", r#"{"path":"/work/src"}"#, ""));
        assert_eq!(l.object, "/work/src");
        let l = tool_label(&tool(ToolClass::Web, "WebSearch", r#"{"other":1}"#, ""));
        assert_eq!(l.object, "WebSearch");
        assert_eq!(json_unescape(r#"\ud83d\ude00 \ud800x""#), "😀 \u{fffd}x");
    }

    #[test]
    fn summary_counts_distinct_paths_and_skips_tasks() {
        let tools = [
            tool(ToolClass::Edit, "", "", "a"),
            tool(ToolClass::Edit, "", "", "a"),
            tool(ToolClass::Edit, "", "", "b"),
            tool(ToolClass::Command, "", "ls", ""),
            tool(ToolClass::Tasks, "", "", ""),
            tool(ToolClass::Mcp, "", "", ""),
            tool(ToolClass::Agent, "", "", ""),
            tool(ToolClass::Web, "", "", ""),
        ];
        let s = summary(&tools);
        assert_eq!(s.first, Clause { class: ToolClass::Command, count: 1 });
        assert_eq!(s.second, Some(Clause { class: ToolClass::Edit, count: 2 }));
        assert_eq!(s.others, 3);
    }

    #[test]
    fn thought_lines() {
        assert_eq!(live_thought_line("**Planning the fix**\nThen more."), "Planning the fix");
        assert_eq!(live_thought_line("I will read [the file](x). Then edit it."), "I will read the file.");
        assert_eq!(live_thought_line("- first *item* here"), "first item here");
        assert_eq!(live_thought_line("考える。次に"), "考える。");
        assert_eq!(live_thought_line("No end"), "No end");
    }
}
