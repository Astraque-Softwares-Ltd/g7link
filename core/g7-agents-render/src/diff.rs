// SPDX-License-Identifier: MPL-2.0
//! Diff text into rows with line numbers and word marks.
//!
//! Accepted forms: a git unified diff (`diff --git`, `---`/`+++`, `@@`
//! hunks); a bare list of hunks; the `*** Update File:` patch form; what the
//! host sends for a Claude edit (lines that start with `+`, `-` or a space,
//! no headers); and what it sends for a Codex file change (a
//! `<path> (add|delete|update)` line before each file's content). Rows carry
//! line numbers only where a hunk header gave them; otherwise both are 0.
//! A file says when git called it binary, or when its text stops short.

use serde::{Deserialize, Serialize};
use similar::{Algorithm, ChangeTag, TextDiff};

use crate::bounds::{
    MAX_DIFF_BYTES, MAX_DIFF_LINE_UNITS, MAX_DIFF_ROWS, WORD_DIFF_MAX_UNITS, clean, cut_bytes, cut_units,
    units, units32,
};
use crate::markdown::is_zero;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiffDoc {
    pub files: Vec<DiffFile>,
    pub adds: u32,
    pub dels: u32,
    /// Rows were dropped: the text was over `MAX_DIFF_BYTES` or the rows over
    /// `MAX_DIFF_ROWS`.
    #[serde(skip_serializing_if = "crate::markdown::is_false")]
    pub more: bool,
}

#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiffFile {
    /// Empty when the diff names no file.
    pub path: String,
    pub adds: u32,
    pub dels: u32,
    pub rows: Vec<DiffRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<DiffNote>,
}

impl std::fmt::Debug for DiffFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiffFile")
            .field("adds", &self.adds)
            .field("dels", &self.dels)
            .field("rows", &self.rows)
            .field("note", &self.note)
            .finish_non_exhaustive()
    }
}

/// What a file's rows don't show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffNote {
    /// Git called it binary ("Binary files … differ", "GIT binary patch"):
    /// no rows.
    Binary,
    /// Its text stops short: it ends inside a hunk, past `MAX_DIFF_BYTES`,
    /// or past `MAX_DIFF_ROWS`. (A host that cut a patch between two hunks
    /// says so itself; a client shows either.)
    Truncated,
}

#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiffRow {
    pub kind: DiffKind,
    /// 0 = none.
    #[serde(skip_serializing_if = "is_zero")]
    pub old_no: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub new_no: u32,
    /// The line without its `+`, `-` or space; a hunk row's whole header.
    pub text: String,
    /// Changed ranges within `text`, `(start, len)` in UTF-16 units.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<(u32, u32)>,
}

impl std::fmt::Debug for DiffRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiffRow")
            .field("kind", &self.kind)
            .field("old_no", &self.old_no)
            .field("new_no", &self.new_no)
            .field("marks", &self.marks)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffKind {
    Hunk,
    #[default]
    Context,
    Add,
    Del,
}

/// How lines that are not headers are read in the current file.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Prefixed lines (`+`, `-`, space).
    Prefixed,
    /// Codex's content of an added or deleted file: every line is one.
    Raw(DiffKind),
}

struct Hunk {
    old_no: u32,
    new_no: u32,
    /// Lines the header promised; `None` for a hunk with no counts.
    left: Option<(u32, u32)>,
}

struct Parser {
    doc: DiffDoc,
    /// Rows and files kept so far, against `MAX_DIFF_ROWS`.
    budget: usize,
    /// Whether the current file is in `doc.files` (past the budget it is not).
    stored: bool,
    /// The current file came from a `diff --git` line and has no hunk yet.
    git_header: bool,
    mode: Mode,
    hunk: Option<Hunk>,
    /// Rows whose text was cut, by index in the current file.
    cut: Vec<bool>,
    cuts: Vec<Vec<bool>>,
}

/// Parses diff text (cut to `MAX_DIFF_BYTES`, cleaned) into files and rows.
pub fn diff(src: &str) -> DiffDoc {
    let over = src.len() > MAX_DIFF_BYTES;
    let text = clean(cut_bytes(src, MAX_DIFF_BYTES));
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let mut p = Parser {
        doc: DiffDoc::default(),
        budget: 0,
        stored: false,
        git_header: false,
        mode: Mode::Prefixed,
        hunk: None,
        cut: Vec::new(),
        cuts: Vec::new(),
    };
    let mut i = 0;
    while i < lines.len() {
        i += p.line(lines[i], lines.get(i + 1).copied(), lines.get(i + 2).copied());
    }
    let owed = p.hunk.as_ref().and_then(|h| h.left).is_some_and(|(old, new)| old > 0 || new > 0);
    if owed || over {
        p.note(DiffNote::Truncated);
    }
    p.end_file();
    let mut doc = p.doc;
    doc.more |= over;
    for (file, cut) in doc.files.iter_mut().zip(p.cuts.iter()) {
        mark_words(&mut file.rows, cut);
    }
    doc
}

impl Parser {
    /// Reads one line (two for a `---`/`+++` pair); returns how many it used.
    /// `next` and `after` are the two lines that follow.
    fn line(&mut self, line: &str, next: Option<&str>, after: Option<&str>) -> usize {
        if let Some(hunk) = self.hunk.as_mut()
            && let Some((old_left, new_left)) = hunk.left.as_mut()
            && (*old_left > 0 || *new_left > 0)
        {
            let kind = match line.as_bytes().first() {
                Some(b'+') => Some(DiffKind::Add),
                Some(b'-') => Some(DiffKind::Del),
                Some(b' ') | None => Some(DiffKind::Context),
                Some(b'\\') => return 1,
                _ => None,
            };
            if let Some(kind) = kind {
                match kind {
                    DiffKind::Add => *new_left = new_left.saturating_sub(1),
                    DiffKind::Del => *old_left = old_left.saturating_sub(1),
                    _ => {
                        *old_left = old_left.saturating_sub(1);
                        *new_left = new_left.saturating_sub(1);
                    }
                }
                self.row(kind, line.get(1..).unwrap_or_default());
                return 1;
            }
            // The header's counts were wrong: read the line as a header.
            self.hunk = None;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            self.start_file(git_path(rest));
            self.git_header = true;
            return 1;
        }
        // A `---`/`+++` pair is a file header after `diff --git` or before a
        // hunk; otherwise it is a removed `--` line and an added `++` one.
        let header = self.git_header || after.is_some_and(|a| a.starts_with("@@"));
        if let (true, Some(old), Some(new)) =
            (header, line.strip_prefix("--- "), next.and_then(|n| n.strip_prefix("+++ ")))
        {
            let path = match header_path(new) {
                Some(p) => p,
                None => header_path(old).unwrap_or_default(),
            };
            if self.git_header && self.stored_rows() == 0 {
                self.set_path(path);
            } else {
                self.start_file(path);
            }
            self.git_header = false;
            return 2;
        }
        if line.starts_with("@@") {
            if self.doc.files.is_empty() && !self.stored && self.budget == 0 {
                self.start_file(String::new());
            }
            self.git_header = false;
            self.mode = Mode::Prefixed;
            self.hunk = Some(match hunk_numbers(line) {
                Some((old_no, old_len, new_no, new_len)) => {
                    Hunk { old_no, new_no, left: Some((old_len, new_len)) }
                }
                None => Hunk { old_no: 0, new_no: 0, left: None },
            });
            self.push(DiffRow { kind: DiffKind::Hunk, text: line.to_owned(), ..DiffRow::default() }, false);
            return 1;
        }
        if let Some(rest) = line.strip_prefix("*** ") {
            for prefix in ["Update File: ", "Add File: ", "Delete File: "] {
                if let Some(path) = rest.strip_prefix(prefix) {
                    self.start_file(path.trim().to_owned());
                    return 1;
                }
            }
            if let Some(path) = rest.strip_prefix("Move to: ") {
                self.set_path(path.trim().to_owned());
                return 1;
            }
            if matches!(rest.trim(), "Begin Patch" | "End Patch" | "End of File") {
                return 1;
            }
        }
        if let Some((path, mode)) = codex_header(line) {
            self.start_file(path.to_owned());
            self.mode = mode;
            return 1;
        }
        if self.git_header {
            if let Some(path) = line.strip_prefix("rename to ") {
                self.set_path(path.trim().to_owned());
            } else if line == "GIT binary patch"
                || (line.starts_with("Binary files ") && line.ends_with(" differ"))
            {
                self.note(DiffNote::Binary);
            }
            // Extended headers (index, modes, similarity, binary) are not rows.
            return 1;
        }
        let (kind, text) = match self.mode {
            Mode::Raw(kind) => (kind, line),
            Mode::Prefixed => match line.as_bytes().first() {
                Some(b'+') => (DiffKind::Add, &line[1..]),
                Some(b'-') => (DiffKind::Del, &line[1..]),
                Some(b' ') => (DiffKind::Context, &line[1..]),
                Some(b'\\') => return 1,
                _ => (DiffKind::Context, line),
            },
        };
        self.row(kind, text);
        1
    }

    fn stored_rows(&self) -> usize {
        if self.stored { self.doc.files.last().map_or(0, |f| f.rows.len()) } else { 0 }
    }

    /// Notes the current file, when it is kept; a first note stays.
    fn note(&mut self, note: DiffNote) {
        if self.stored
            && let Some(file) = self.doc.files.last_mut()
        {
            file.note.get_or_insert(note);
        }
    }

    fn set_path(&mut self, path: String) {
        if self.stored
            && let Some(file) = self.doc.files.last_mut()
        {
            file.path = path;
        }
    }

    fn end_file(&mut self) {
        if self.stored {
            self.cuts.push(std::mem::take(&mut self.cut));
        }
        self.stored = false;
    }

    fn start_file(&mut self, path: String) {
        // A file opened implicitly that got nothing is replaced, not kept.
        if self.stored && self.doc.files.last().is_some_and(|f| f.path.is_empty() && f.rows.is_empty()) {
            self.doc.files.pop();
            self.cut.clear();
            self.stored = false;
            self.budget = self.budget.saturating_sub(1);
        } else {
            self.end_file();
        }
        self.mode = Mode::Prefixed;
        self.hunk = None;
        self.git_header = false;
        if self.budget < MAX_DIFF_ROWS {
            self.budget += 1;
            self.doc.files.push(DiffFile { path, ..DiffFile::default() });
            self.stored = true;
        } else {
            self.doc.more = true;
        }
    }

    fn row(&mut self, kind: DiffKind, text: &str) {
        if self.doc.files.is_empty() && !self.stored && self.budget == 0 {
            self.start_file(String::new());
        }
        let (mut old_no, mut new_no) = (0, 0);
        if let Some(hunk) = self.hunk.as_mut().filter(|h| h.left.is_some()) {
            match kind {
                DiffKind::Add => {
                    new_no = hunk.new_no;
                    hunk.new_no = hunk.new_no.saturating_add(1);
                }
                DiffKind::Del => {
                    old_no = hunk.old_no;
                    hunk.old_no = hunk.old_no.saturating_add(1);
                }
                _ => {
                    old_no = hunk.old_no;
                    new_no = hunk.new_no;
                    hunk.old_no = hunk.old_no.saturating_add(1);
                    hunk.new_no = hunk.new_no.saturating_add(1);
                }
            }
        }
        match kind {
            DiffKind::Add => {
                self.doc.adds = self.doc.adds.saturating_add(1);
                if self.stored
                    && let Some(f) = self.doc.files.last_mut()
                {
                    f.adds = f.adds.saturating_add(1);
                }
            }
            DiffKind::Del => {
                self.doc.dels = self.doc.dels.saturating_add(1);
                if self.stored
                    && let Some(f) = self.doc.files.last_mut()
                {
                    f.dels = f.dels.saturating_add(1);
                }
            }
            _ => {}
        }
        let cut = units(text) > MAX_DIFF_LINE_UNITS;
        let text = if cut { cut_units(text, MAX_DIFF_LINE_UNITS) } else { text };
        self.push(DiffRow { kind, old_no, new_no, text: text.to_owned(), marks: Vec::new() }, cut);
    }

    fn push(&mut self, row: DiffRow, cut: bool) {
        if !self.stored || self.budget >= MAX_DIFF_ROWS {
            self.note(DiffNote::Truncated);
            self.doc.more = true;
            return;
        }
        if let Some(file) = self.doc.files.last_mut() {
            self.budget += 1;
            file.rows.push(row);
            self.cut.push(cut);
        }
    }
}

/// The new path of a `diff --git a/x b/y` line.
fn git_path(rest: &str) -> String {
    let path = match rest.rfind(" b/") {
        Some(i) => &rest[i + 3..],
        None => rest.split_whitespace().last().unwrap_or_default(),
    };
    unquote(path).to_owned()
}

/// The path of a `---`/`+++` header; `None` for `/dev/null`.
fn header_path(rest: &str) -> Option<String> {
    let path = rest.split('\t').next().unwrap_or_default().trim();
    let path = unquote(path);
    if path == "/dev/null" || path.is_empty() {
        return None;
    }
    let path = path.strip_prefix("a/").or_else(|| path.strip_prefix("b/")).unwrap_or(path);
    Some(path.to_owned())
}

fn unquote(s: &str) -> &str {
    s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(s)
}

/// `@@ -a[,b] +c[,d] @@` → `(a, b, c, d)`.
fn hunk_numbers(line: &str) -> Option<(u32, u32, u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, _) = rest.split_once(" @@")?;
    let range = |s: &str| -> Option<(u32, u32)> {
        match s.split_once(',') {
            Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    let (a, b) = range(old)?;
    let (c, d) = range(new)?;
    Some((a, b, c, d))
}

/// The host's header for a Codex file change: `<path> (add|delete|update)`.
fn codex_header(line: &str) -> Option<(&str, Mode)> {
    if line.starts_with(['+', '-', ' ', '@', '\\']) {
        return None;
    }
    let (path, mode) = if let Some(p) = line.strip_suffix(" (add)") {
        (p, Mode::Raw(DiffKind::Add))
    } else if let Some(p) = line.strip_suffix(" (delete)") {
        (p, Mode::Raw(DiffKind::Del))
    } else {
        let p = line.strip_suffix(" (update)")?;
        (p, Mode::Prefixed)
    };
    (!path.trim().is_empty()).then_some((path, mode))
}

/// Number of file headers in `detail`, as [`diff`] would split it; for the
/// "and N more" of an edit's label.
pub(crate) fn file_headers(detail: &str) -> usize {
    let detail = cut_bytes(detail, MAX_DIFF_BYTES);
    let mut n = 0;
    let lines: Vec<&str> = detail.split('\n').collect();
    let mut git = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.starts_with("diff --git ") {
            n += 1;
            git = true;
        } else if line.starts_with("--- ")
            && lines.get(i).is_some_and(|l| l.starts_with("+++ "))
            && (git || lines.get(i + 1).is_some_and(|l| l.starts_with("@@")))
        {
            i += 1;
            if !git {
                n += 1;
            }
            git = false;
        } else if line.starts_with("*** Update File: ")
            || line.starts_with("*** Add File: ")
            || line.starts_with("*** Delete File: ")
            || codex_header(line).is_some()
        {
            n += 1;
            git = false;
        } else if line.starts_with("@@") {
            git = false;
        }
    }
    n
}

/// Adds word marks to each `Del` row directly followed (within its run) by
/// an `Add` row, pairing them in order.
fn mark_words(rows: &mut [DiffRow], cut: &[bool]) {
    let mut i = 0;
    while i < rows.len() {
        if rows[i].kind != DiffKind::Del {
            i += 1;
            continue;
        }
        let del_start = i;
        while i < rows.len() && rows[i].kind == DiffKind::Del {
            i += 1;
        }
        let add_start = i;
        while i < rows.len() && rows[i].kind == DiffKind::Add {
            i += 1;
        }
        let pairs = (add_start - del_start).min(i - add_start);
        for k in 0..pairs {
            let (d, a) = (del_start + k, add_start + k);
            let short = |r: usize| {
                !cut.get(r).copied().unwrap_or(true) && units(&rows[r].text) <= WORD_DIFF_MAX_UNITS
            };
            if !short(d) || !short(a) {
                continue;
            }
            let (old_marks, new_marks) = word_marks(&rows[d].text, &rows[a].text);
            rows[d].marks = old_marks;
            rows[a].marks = new_marks;
        }
    }
}

/// Changed ranges within a line: `(start, len)` in UTF-16 units.
type Marks = Vec<(u32, u32)>;

/// Changed ranges of `old` and `new`, word by word, in UTF-16 units.
pub(crate) fn word_marks(old: &str, new: &str) -> (Marks, Marks) {
    let diff = TextDiff::configure().algorithm(Algorithm::Myers).diff_words(old, new);
    let (mut old_at, mut new_at) = (0u32, 0u32);
    let (mut old_marks, mut new_marks): (Marks, Marks) = (Vec::new(), Vec::new());
    let add = |marks: &mut Marks, at: u32, len: u32| {
        if len == 0 {
            return;
        }
        match marks.last_mut() {
            Some((s, l)) if s.saturating_add(*l) == at => *l = l.saturating_add(len),
            _ => marks.push((at, len)),
        }
    };
    for op in diff.ops() {
        for change in diff.iter_changes(op) {
            let len = units32(change.value());
            match change.tag() {
                ChangeTag::Equal => {
                    old_at = old_at.saturating_add(len);
                    new_at = new_at.saturating_add(len);
                }
                ChangeTag::Delete => {
                    add(&mut old_marks, old_at, len);
                    old_at = old_at.saturating_add(len);
                }
                ChangeTag::Insert => {
                    add(&mut new_marks, new_at, len);
                    new_at = new_at.saturating_add(len);
                }
            }
        }
    }
    // Nothing in common: marking the whole line says nothing.
    if old_marks == [(0, units32(old))] && new_marks == [(0, units32(new))] {
        return (Vec::new(), Vec::new());
    }
    (old_marks, new_marks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(doc: &DiffDoc) -> Vec<(DiffKind, u32, u32, &str)> {
        doc.files
            .iter()
            .flat_map(|f| f.rows.iter().map(|r| (r.kind, r.old_no, r.new_no, r.text.as_str())))
            .collect()
    }

    #[test]
    fn git_diff_with_numbers() {
        let src = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -3,3 +3,3 @@ fn x\n ctx\n-old line\n+new line\n ctx2\n";
        let doc = diff(src);
        assert_eq!(doc.files.len(), 1);
        assert_eq!(doc.files[0].path, "src/a.rs");
        assert_eq!((doc.adds, doc.dels), (1, 1));
        assert_eq!(
            rows(&doc),
            [
                (DiffKind::Hunk, 0, 0, "@@ -3,3 +3,3 @@ fn x"),
                (DiffKind::Context, 3, 3, "ctx"),
                (DiffKind::Del, 4, 0, "old line"),
                (DiffKind::Add, 0, 4, "new line"),
                (DiffKind::Context, 5, 5, "ctx2"),
            ]
        );
        assert_eq!(doc.files[0].rows[2].marks, [(0, 3)]);
        assert_eq!(doc.files[0].rows[3].marks, [(0, 3)]);
    }

    #[test]
    fn host_prefixed_lines_have_no_numbers() {
        let doc = diff("-a\n-b\n+a\n+c");
        assert_eq!(doc.files.len(), 1);
        assert_eq!(doc.files[0].path, "");
        assert!(doc.files[0].rows.iter().all(|r| r.old_no == 0 && r.new_no == 0));
        assert_eq!((doc.adds, doc.dels), (2, 2));
    }

    #[test]
    fn codex_file_changes() {
        let doc = diff("notes.txt (add)\na\nb\nsrc/x.rs (update)\n@@ -1 +1 @@\n-x\n+y\n");
        let got: Vec<_> = doc.files.iter().map(|f| (f.path.as_str(), f.adds, f.dels)).collect();
        assert_eq!(got, [("notes.txt", 2, 0), ("src/x.rs", 1, 1)]);
        assert_eq!(file_headers("notes.txt (add)\na\nsrc/x.rs (update)\n@@ -1 +1 @@\n-x\n+y\n"), 2);
    }

    #[test]
    fn several_git_files_count_once_each() {
        let src = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\ndiff --git a/b b/b\nnew file mode 100644\n--- /dev/null\n+++ b/b\n@@ -0,0 +1 @@\n+z\n";
        let doc = diff(src);
        assert_eq!(doc.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(file_headers(src), 2);
    }

    #[test]
    fn word_marks_cover_changed_words() {
        let (o, n) = word_marks("let x = 1;", "let y = 1;");
        assert_eq!(o, [(4, 1)]);
        assert_eq!(n, [(4, 1)]);
    }
}
