// SPDX-License-Identifier: GPL-3.0-or-later
//! The open view's rows as the Agents page's list model shows them: each
//! `g7_agents_render::Row` flattened into role values, and the changes that
//! turn the rows on screen into new ones, by key.
//!
//! Agent text reaches QML in two forms only: as plain text (shown with
//! `Text.PlainText`), and inside `Text.StyledText` markup built here, where
//! every character of the text is escaped first and the only tags are the
//! ones this file writes (`<b>`, `<i>`, `<s>`, `<font color>`, `<a href="N">`
//! with `N` a number, `<br>`). A link's target never enters the markup: the
//! page asks for it by its number ([`ModelRow::link`]).

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use g7_agents_render::{
    Act, Acts, Align, Block, BlockKind, CheckpointWhy, CodeLine, DetailOf, DiffDoc, DiffKind, DiffNote, Link,
    LiveLabel, MarkKind, Marker, NoteKind, NoticeKind, Phase, PlanState, QuestionState, Row, RowKind,
    RowStatus, Span, Summary, Tok, TokClass, ToolClass, Via, bounds::is_stripped_char,
};
use serde_json::{Value, json};

/// Diff rows an opened edit shows inline; the rest is counted in `more`.
pub(crate) const INLINE_DIFF_ROWS: usize = 400;
/// Links one row may hold.
const MAX_LINKS: usize = 256;

/// The model's roles, in order (Qt::UserRole + index). A role a row's kind
/// doesn't use has its type's empty value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Role {
    Key,
    Kind,
    /// Which delegate draws it: the kind, with `code` and `table` for those blocks.
    View,
    Turn,
    Depth,
    ItemId,
    Text,
    Styled,
    BlockKind,
    Level,
    Quote,
    Marker,
    MarkerNumber,
    Lang,
    Open,
    Last,
    Phase,
    Table,
    Label,
    Status,
    HasDetail,
    HasOutput,
    IsOpen,
    Summary,
    Count,
    Failed,
    WorkedMs,
    Hidden,
    Files,
    Adds,
    Dels,
    More,
    Diff,
    DetailKind,
    Edit,
    Edits,
    Cut,
    Ms,
    MarkKind,
    By,
    LiveLabel,
    SinceMs,
    From,
    DenyOf,
    Streaming,
    HasText,
    WhenMs,
    Truncated,
    Wrapped,
    Lines,
    /// A row of a plan's card: "head", "body" (its blocks) or "foot".
    PlanPart,
    /// A user message sent while the agent worked: "steer" or "queue".
    Via,
    /// The images sent with a user message.
    Images,
    /// A `changes` row from the turn's restore point: it opens the diff.
    Recorded,
    /// A `changes_note` row's note: "recording", "missing", "expired" or "unrecorded".
    Note,
    /// Why a restore point is missing ("busy", "slow", "large", "refused", "off").
    Why,
    /// A `notice` row the page acts on: "rewound", "edit_from_here",
    /// "files_restored" or "forked".
    Notice,
    /// This notice carries Undo: the newest rewind's, while it can be undone.
    Undo,
    /// The message's actions, each "" (hidden), "on", "latest_only",
    /// "no_restore_point" or "not_git".
    ActFork,
    ActRewind,
    ActEdit,
    ActRestore,
}

impl Role {
    pub(crate) const ALL: [Role; 62] = [
        Role::Key,
        Role::Kind,
        Role::View,
        Role::Turn,
        Role::Depth,
        Role::ItemId,
        Role::Text,
        Role::Styled,
        Role::BlockKind,
        Role::Level,
        Role::Quote,
        Role::Marker,
        Role::MarkerNumber,
        Role::Lang,
        Role::Open,
        Role::Last,
        Role::Phase,
        Role::Table,
        Role::Label,
        Role::Status,
        Role::HasDetail,
        Role::HasOutput,
        Role::IsOpen,
        Role::Summary,
        Role::Count,
        Role::Failed,
        Role::WorkedMs,
        Role::Hidden,
        Role::Files,
        Role::Adds,
        Role::Dels,
        Role::More,
        Role::Diff,
        Role::DetailKind,
        Role::Edit,
        Role::Edits,
        Role::Cut,
        Role::Ms,
        Role::MarkKind,
        Role::By,
        Role::LiveLabel,
        Role::SinceMs,
        Role::From,
        Role::DenyOf,
        Role::Streaming,
        Role::HasText,
        Role::WhenMs,
        Role::Truncated,
        Role::Wrapped,
        Role::Lines,
        Role::PlanPart,
        Role::Via,
        Role::Images,
        Role::Recorded,
        Role::Note,
        Role::Why,
        Role::Notice,
        Role::Undo,
        Role::ActFork,
        Role::ActRewind,
        Role::ActEdit,
        Role::ActRestore,
    ];

    pub(crate) fn index(self) -> usize {
        self as usize
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Role::Key => "key",
            Role::Kind => "kind",
            Role::View => "view",
            Role::Turn => "turn",
            Role::Depth => "depth",
            Role::ItemId => "itemId",
            Role::Text => "text",
            Role::Styled => "styled",
            Role::BlockKind => "blockKind",
            Role::Level => "level",
            Role::Quote => "quote",
            Role::Marker => "marker",
            Role::MarkerNumber => "markerNumber",
            Role::Lang => "lang",
            Role::Open => "open",
            Role::Last => "last",
            Role::Phase => "phase",
            Role::Table => "table",
            Role::Label => "label",
            Role::Status => "status",
            Role::HasDetail => "hasDetail",
            Role::HasOutput => "hasOutput",
            Role::IsOpen => "isOpen",
            Role::Summary => "summary",
            Role::Count => "count",
            Role::Failed => "failed",
            Role::WorkedMs => "workedMs",
            Role::Hidden => "hidden",
            Role::Files => "files",
            Role::Adds => "adds",
            Role::Dels => "dels",
            Role::More => "more",
            Role::Diff => "diff",
            Role::DetailKind => "detailKind",
            Role::Edit => "edit",
            Role::Edits => "edits",
            Role::Cut => "cut",
            Role::Ms => "ms",
            Role::MarkKind => "markKind",
            Role::By => "by",
            Role::LiveLabel => "liveLabel",
            Role::SinceMs => "sinceMs",
            Role::From => "from",
            Role::DenyOf => "denyOf",
            Role::Streaming => "streaming",
            Role::HasText => "hasText",
            Role::WhenMs => "whenMs",
            Role::Truncated => "truncated",
            Role::Wrapped => "wrapped",
            Role::Lines => "lines",
            Role::PlanPart => "planPart",
            Role::Via => "via",
            Role::Images => "images",
            Role::Recorded => "recorded",
            Role::Note => "note",
            Role::Why => "why",
            Role::Notice => "notice",
            Role::Undo => "undo",
            Role::ActFork => "actFork",
            Role::ActRewind => "actRewind",
            Role::ActEdit => "actEdit",
            Role::ActRestore => "actRestore",
        }
    }

    /// The role's empty value, which also gives its type.
    fn empty(self) -> RoleValue {
        match self {
            Role::Turn
            | Role::Depth
            | Role::Level
            | Role::Quote
            | Role::MarkerNumber
            | Role::Count
            | Role::Hidden
            | Role::Adds
            | Role::Dels
            | Role::More
            | Role::Edit
            | Role::Edits
            | Role::Lines
            | Role::Images => RoleValue::Int(0),
            Role::WorkedMs | Role::Ms | Role::SinceMs | Role::WhenMs => RoleValue::Real(0.0),
            Role::Open
            | Role::Last
            | Role::HasDetail
            | Role::HasOutput
            | Role::IsOpen
            | Role::Failed
            | Role::Cut
            | Role::DenyOf
            | Role::Streaming
            | Role::HasText
            | Role::Truncated
            | Role::Wrapped
            | Role::Recorded
            | Role::Undo => RoleValue::Flag(false),
            _ => RoleValue::Text(String::new()),
        }
    }
}

/// One role's value.
#[derive(Clone, PartialEq)]
pub(crate) enum RoleValue {
    Text(String),
    Int(i32),
    Real(f64),
    Flag(bool),
}

/// Never prints text.
impl std::fmt::Debug for RoleValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoleValue::Text(t) => write!(f, "Text({} bytes)", t.len()),
            RoleValue::Int(n) => write!(f, "Int({n})"),
            RoleValue::Real(n) => write!(f, "Real({n})"),
            RoleValue::Flag(b) => write!(f, "Flag({b})"),
        }
    }
}

/// A link in a row's markup: `<a href="N">` is `links[N]`.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LinkRef {
    /// As the render crate validated it (`http`, `https`, `mailto`).
    pub href: String,
    /// The text the link was shown as.
    pub text: String,
    /// The visible text is the target itself: it opens without asking.
    pub shown: bool,
}

impl std::fmt::Debug for LinkRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkRef").field("shown", &self.shown).finish_non_exhaustive()
    }
}

/// A row as the model shows it.
#[derive(Clone, PartialEq)]
pub(crate) struct ModelRow {
    values: Vec<RoleValue>,
    links: Vec<LinkRef>,
    /// What Copy puts on the clipboard for this row ("" for nothing).
    copy: String,
}

/// Never prints text: the key and kind only.
impl std::fmt::Debug for ModelRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRow").field("key", &self.key()).field("kind", &self.text(Role::Kind)).finish()
    }
}

impl ModelRow {
    fn new() -> Self {
        Self { values: Role::ALL.iter().map(|r| r.empty()).collect(), links: Vec::new(), copy: String::new() }
    }

    pub(crate) fn get(&self, role: Role) -> &RoleValue {
        &self.values[role.index()]
    }

    fn set(&mut self, role: Role, value: RoleValue) {
        debug_assert_eq!(std::mem::discriminant(&value), std::mem::discriminant(&role.empty()));
        self.values[role.index()] = value;
    }

    fn set_text(&mut self, role: Role, text: impl Into<String>) {
        self.set(role, RoleValue::Text(text.into()));
    }

    fn set_int(&mut self, role: Role, n: impl TryInto<i32>) {
        self.set(role, RoleValue::Int(n.try_into().unwrap_or(i32::MAX)));
    }

    fn set_real(&mut self, role: Role, n: u64) {
        self.set(role, RoleValue::Real(n as f64));
    }

    fn set_flag(&mut self, role: Role, b: bool) {
        self.set(role, RoleValue::Flag(b));
    }

    pub(crate) fn text(&self, role: Role) -> &str {
        match self.get(role) {
            RoleValue::Text(t) => t,
            _ => "",
        }
    }

    pub(crate) fn key(&self) -> &str {
        self.text(Role::Key)
    }

    pub(crate) fn link(&self, index: usize) -> Option<&LinkRef> {
        self.links.get(index)
    }

    pub(crate) fn copy_text(&self) -> &str {
        &self.copy
    }

    /// The roles whose values differ from `other`'s.
    pub(crate) fn changed_roles(&self, other: &ModelRow) -> Vec<Role> {
        Role::ALL.iter().copied().filter(|r| self.get(*r) != other.get(*r)).collect()
    }
}

// ---- Words ----

fn class_word(class: ToolClass) -> &'static str {
    match class {
        ToolClass::Command => "command",
        ToolClass::Edit => "edit",
        ToolClass::Read => "read",
        ToolClass::Search => "search",
        ToolClass::Web => "web",
        ToolClass::Mcp => "mcp",
        ToolClass::Agent => "agent",
        ToolClass::Tasks => "tasks",
        ToolClass::Other => "other",
        ToolClass::Plan => "plan",
    }
}

fn status_word(status: RowStatus) -> &'static str {
    match status {
        RowStatus::Running => "running",
        RowStatus::Asking => "asking",
        RowStatus::Ok => "ok",
        RowStatus::Failed => "failed",
        RowStatus::Denied => "denied",
        RowStatus::Stopped => "stopped",
    }
}

fn mark_word(kind: MarkKind) -> &'static str {
    match kind {
        MarkKind::Waiting => "waiting",
        MarkKind::Held => "held",
        MarkKind::Allowed => "allowed",
        MarkKind::Denied => "denied",
        MarkKind::Expired => "expired",
        MarkKind::Cancelled => "cancelled",
        MarkKind::Rule => "rule",
        MarkKind::Reviewed => "reviewed",
    }
}

pub(crate) fn block_word(kind: BlockKind) -> (&'static str, u8) {
    match kind {
        BlockKind::Paragraph => ("paragraph", 0),
        BlockKind::Heading(level) => ("heading", level),
        BlockKind::ListItem => ("item", 0),
        BlockKind::Code => ("code", 0),
        BlockKind::Table => ("table", 0),
        BlockKind::Rule => ("rule", 0),
        BlockKind::Alert(kind) => (
            match kind {
                g7_agents_render::AlertKind::Note => "note",
                g7_agents_render::AlertKind::Tip => "tip",
                g7_agents_render::AlertKind::Important => "important",
                g7_agents_render::AlertKind::Warning => "warning",
                g7_agents_render::AlertKind::Caution => "caution",
            },
            0,
        ),
    }
}

pub(crate) fn marker_word(marker: Marker) -> (&'static str, u32) {
    match marker {
        Marker::None => ("", 0),
        Marker::Bullet => ("bullet", 0),
        Marker::Number(n) => ("number", n),
        Marker::Todo => ("todo", 0),
        Marker::Done => ("done", 0),
    }
}

fn align_word(align: Align) -> &'static str {
    match align {
        Align::None => "",
        Align::Left => "left",
        Align::Center => "center",
        Align::Right => "right",
    }
}

fn summary_json(summary: &Summary) -> Value {
    let clause = |c: &g7_agents_render::Clause| json!({ "class": class_word(c.class), "count": c.count });
    json!({
        "first": clause(&summary.first),
        "second": summary.second.as_ref().map(clause),
        "others": summary.others,
    })
}

// ---- Markup ----

/// Colours for code, as the page's palette gives them (`#rrggbb`), by token
/// class, with each class's style; the colour of inline code; and the
/// grounds of a diff's changed words. Empty: plain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CodePalette {
    classes: HashMap<TokClass, String>,
    italic: HashSet<TokClass>,
    bold: HashSet<TokClass>,
    inline: String,
    add: String,
    del: String,
    add_word: String,
    del_word: String,
}

const CLASS_NAMES: [(TokClass, &str); 21] = [
    (TokClass::Plain, "plain"),
    (TokClass::Comment, "comment"),
    (TokClass::Keyword, "keyword"),
    (TokClass::String, "string"),
    (TokClass::Number, "number"),
    (TokClass::Constant, "constant"),
    (TokClass::Type, "type"),
    (TokClass::Function, "function"),
    (TokClass::Variable, "variable"),
    (TokClass::Operator, "operator"),
    (TokClass::Punctuation, "punctuation"),
    (TokClass::Tag, "tag"),
    (TokClass::Attribute, "attribute"),
    (TokClass::Inserted, "inserted"),
    (TokClass::Deleted, "deleted"),
    (TokClass::Dir, "dir"),
    (TokClass::File, "file"),
    (TokClass::Pos, "pos"),
    (TokClass::Error, "error"),
    (TokClass::Warn, "warn"),
    (TokClass::Ok, "ok"),
];

/// `#rgb`, `#rrggbb` or `#aarrggbb`, the forms QML's `color` prints.
fn is_colour(s: &str) -> bool {
    s.strip_prefix('#')
        .is_some_and(|hex| matches!(hex.len(), 3 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

impl CodePalette {
    /// From the page's JSON (`{ "keyword": "#…", …, "inline": "#…",
    /// "add": "#…", "del": "#…", "addWord": "#…", "delWord": "#…",
    /// "styles": { "comment": "italic", "keyword": "semibold" } }`); a value
    /// that isn't a colour, or a style it doesn't know, is left out.
    pub(crate) fn from_json(json: &str) -> Self {
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(json) else { return Self::default() };
        let colour = |name: &str| {
            map.get(name)
                .and_then(Value::as_str)
                .filter(|c| is_colour(c))
                .map(str::to_owned)
                .unwrap_or_default()
        };
        let classes = CLASS_NAMES
            .iter()
            .filter_map(|(class, name)| Some((*class, colour(name))).filter(|(_, c)| !c.is_empty()))
            .collect();
        let styled = |style: &str| -> HashSet<TokClass> {
            let Some(Value::Object(styles)) = map.get("styles") else { return HashSet::new() };
            CLASS_NAMES
                .iter()
                .filter(|(_, name)| styles.get(*name).and_then(Value::as_str) == Some(style))
                .map(|(class, _)| *class)
                .collect()
        };
        Self {
            classes,
            italic: styled("italic"),
            bold: styled("semibold"),
            inline: colour("inline"),
            add: colour("add"),
            del: colour("del"),
            add_word: colour("addWord"),
            del_word: colour("delWord"),
        }
    }

    fn of(&self, class: TokClass) -> Option<&str> {
        self.classes.get(&class).map(String::as_str)
    }
}

/// Appends `c` escaped for `Text.StyledText`. In code, spaces and tabs keep
/// their width. Control characters are dropped.
fn push_escaped(out: &mut String, c: char, code: bool) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        '\n' => out.push_str("<br>"),
        ' ' if code => out.push_str("&nbsp;"),
        '\t' if code => out.push_str("&nbsp;&nbsp;&nbsp;&nbsp;"),
        '\t' => out.push(' '),
        c if is_stripped_char(c) => {}
        c => out.push(c),
    }
}

/// Agent text for one line of the page (a question, an option): every
/// character escaped, `inline code` coloured as an answer's is, and nothing
/// else read as Markdown. Cheap enough for the GUI thread: no grammar is
/// ever loaded for it.
pub(crate) fn inline_markup(text: &str, palette: &CodePalette) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    // Backticks pair up; one left over is text.
    let pairs = text.matches('`').count() / 2 * 2;
    let mut seen = 0;
    for c in text.chars() {
        if c == '`' && seen < pairs {
            seen += 1;
            if !palette.inline.is_empty() {
                if seen % 2 == 1 {
                    out.push_str(&format!("<font color=\"{}\">", palette.inline));
                } else {
                    out.push_str("</font>");
                }
            }
            continue;
        }
        push_escaped(&mut out, c, false);
    }
    out
}

/// `text` escaped, every character.
pub(crate) fn escaped(text: &str, code: bool) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        push_escaped(&mut out, c, code);
    }
    out
}

/// The tags a span opens, outermost first; and the matching closing tags.
fn span_tags(span: &Span, link: Option<usize>, palette: &CodePalette) -> (String, String) {
    let mut open = String::new();
    let mut close = Vec::new();
    if let Some(n) = link {
        open.push_str(&format!("<a href=\"{n}\">"));
        close.push("</a>");
    }
    if span.style.strong {
        open.push_str("<b>");
        close.push("</b>");
    }
    if span.style.em {
        open.push_str("<i>");
        close.push("</i>");
    }
    if span.style.strike {
        open.push_str("<s>");
        close.push("</s>");
    }
    // A path is coloured part by part (`styled` writes it); other code is inline code.
    if span.style.code && !is_path(span, link, palette) && !palette.inline.is_empty() {
        open.push_str(&format!("<font color=\"{}\">", palette.inline));
        close.push("</font>");
    }
    close.reverse();
    (open, close.concat())
}

/// Inline code that names a file the thread touched, drawn as a path: the
/// folder part muted and the name in ink (not in a link, and only with a palette).
fn is_path(span: &Span, link: Option<usize>, palette: &CodePalette) -> bool {
    span.style.code && span.style.path && link.is_none() && palette.of(TokClass::File).is_some()
}

/// `text` with its spans (UTF-16 ranges, sorted, not overlapping) as
/// markup. Links are added to `links`, numbered in the markup by position.
/// A span that is out of order or out of range is left plain.
pub(crate) fn styled(text: &str, spans: &[Span], links: &mut Vec<LinkRef>, palette: &CodePalette) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    let mut spans = spans.iter().peekable();
    // The open span: its end and closing tags.
    let mut current: Option<(u32, String)> = None;
    let mut link_text: Option<String> = None;
    // The text of an open path span, written whole when it closes.
    let mut path: Option<String> = None;
    let mut pos: u32 = 0;
    for c in text.chars() {
        if let Some((end, close)) = &current
            && pos >= *end
        {
            if let Some(p) = path.take() {
                path_markup(&p, palette, &mut out);
            }
            out.push_str(close);
            if let (Some(text), Some(link)) = (link_text.take(), links.last_mut()) {
                link.text = text;
            }
            current = None;
        }
        if current.is_none() {
            // Skip spans that start before here (out of order) or are empty.
            while let Some(span) = spans.peek() {
                if span.start < pos || span.len == 0 {
                    spans.next();
                } else {
                    break;
                }
            }
            if let Some(span) = spans.peek()
                && span.start <= pos
            {
                let span = spans.next().expect("peeked");
                let link = (!span.style.link.is_none() && !span.href.is_empty() && links.len() < MAX_LINKS)
                    .then(|| {
                        links.push(LinkRef {
                            href: span.href.clone(),
                            text: String::new(),
                            shown: span.style.link == Link::Shown,
                        });
                        links.len() - 1
                    });
                if link.is_some() {
                    link_text = Some(String::new());
                }
                let (open, close) = span_tags(span, link, palette);
                out.push_str(&open);
                if is_path(span, link, palette) {
                    path = Some(String::new());
                }
                current = Some((span.start.saturating_add(span.len), close));
            }
        }
        if let Some(t) = link_text.as_mut() {
            t.push(c);
        }
        match path.as_mut() {
            Some(p) => p.push(c),
            None => push_escaped(&mut out, c, false),
        }
        pos = pos.saturating_add(c.len_utf16() as u32);
    }
    if let Some(p) = path.take() {
        path_markup(&p, palette, &mut out);
    }
    if let Some((_, close)) = current {
        out.push_str(&close);
        if let (Some(text), Some(link)) = (link_text.take(), links.last_mut()) {
            link.text = text;
        }
    }
    out
}

/// The tags a token class opens (its colour, then its style), and the ones that close them.
fn tok_tags(class: TokClass, palette: &CodePalette) -> (String, &'static str) {
    let mut open = String::new();
    let mut close = "";
    if let Some(c) = palette.of(class) {
        open.push_str(&format!("<font color=\"{c}\">"));
        close = "</font>";
    }
    if palette.italic.contains(&class) {
        open.push_str("<i>");
        close = if close.is_empty() { "</i>" } else { "</i></font>" };
    } else if palette.bold.contains(&class) {
        open.push_str("<b>");
        close = if close.is_empty() { "</b>" } else { "</b></font>" };
    }
    (open, close)
}

/// One line of code as markup: its tokens coloured by class.
fn code_line(line: &CodeLine, palette: &CodePalette, out: &mut String) {
    code_text(&line.text, &line.toks, palette, out);
}

/// `text` with its tokens (UTF-16 lengths, in order) coloured by class;
/// what they don't cover is plain.
fn code_text(text: &str, toks: &[Tok], palette: &CodePalette, out: &mut String) {
    let mut chars = text.chars();
    for tok in toks {
        let (open, close) = tok_tags(tok.class, palette);
        out.push_str(&open);
        let mut units = 0u32;
        while units < tok.len {
            let Some(c) = chars.next() else { break };
            units = units.saturating_add(c.len_utf16() as u32);
            push_escaped(out, c, true);
        }
        out.push_str(close);
    }
    for c in chars {
        push_escaped(out, c, true);
    }
}

/// Lines of a command or an output as markup, one `<br>` between them;
/// "" when none is coloured (the page shows the text as it is).
pub(crate) fn lines_markup(lines: &[CodeLine], palette: &CodePalette) -> String {
    if lines.iter().all(|l| l.toks.is_empty()) {
        return String::new();
    }
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push_str("<br>");
        }
        code_line(line, palette, &mut out);
    }
    out
}

/// A path as markup: the folder part in the `dir` colour, the name in `file`'s.
fn path_markup(text: &str, palette: &CodePalette, out: &mut String) {
    let cut = text.rfind('/').map_or(0, |at| at + 1);
    let (dir, file) = text.split_at(cut);
    for (part, class) in [(dir, TokClass::Dir), (file, TokClass::File)] {
        if part.is_empty() {
            continue;
        }
        let (open, close) = tok_tags(class, palette);
        out.push_str(&open);
        for c in part.chars() {
            push_escaped(out, c, false);
        }
        out.push_str(close);
    }
}

/// A code block as markup, one `<br>` between lines: coloured when it was
/// highlighted, else its source.
pub(crate) fn code_markup(block: &Block, palette: &CodePalette) -> String {
    let mut out = String::with_capacity(block.text.len() * 2);
    if block.lines.is_empty() {
        let text = block.text.strip_suffix('\n').unwrap_or(&block.text);
        for c in text.chars() {
            push_escaped(&mut out, c, true);
        }
        return out;
    }
    for (i, line) in block.lines.iter().enumerate() {
        if i > 0 {
            out.push_str("<br>");
        }
        code_line(line, palette, &mut out);
    }
    out
}

/// A diff as the page's diff view reads it: files with rows, each row's
/// text as markup with its changed words marked, at most `limit` rows. With
/// the palette's word grounds a mark is a ground (struck through when
/// removed) and the row says so (`r`: it needs rich text); else it's bold.
pub(crate) fn diff_json(doc: &DiffDoc, limit: usize, palette: &CodePalette) -> Value {
    let mut left = limit;
    let mut more = doc.more;
    let files: Vec<Value> = doc
        .files
        .iter()
        .map(|file| {
            let take = file.rows.len().min(left);
            left -= take;
            more |= take < file.rows.len();
            let rows: Vec<Value> = file.rows[..take]
                .iter()
                .map(|row| {
                    let kind = match row.kind {
                        DiffKind::Hunk => "hunk",
                        DiffKind::Context => "ctx",
                        DiffKind::Add => "add",
                        DiffKind::Del => "del",
                    };
                    let (s, rich) = marked(&row.text, &row.marks, row.kind, palette);
                    json!({ "k": kind, "o": row.old_no, "n": row.new_no, "s": s, "r": rich })
                })
                .collect();
            json!({
                "path": file.path,
                "adds": file.adds,
                "dels": file.dels,
                "numbered": file.rows.iter().any(|r| r.old_no > 0 || r.new_no > 0),
                // The longest line, in UTF-16 units: a scrolling view's width.
                "widest": file.rows[..take].iter().map(|r| r.text.encode_utf16().count()).max().unwrap_or(0),
                "rows": rows,
                "note": match file.note {
                    Some(DiffNote::Binary) => "binary",
                    Some(DiffNote::Truncated) => "truncated",
                    None => "",
                },
            })
        })
        .collect();
    json!({ "files": files, "adds": doc.adds, "dels": doc.dels, "more": more })
}

/// A diff line as markup, the `(start, len)` UTF-16 ranges marked: on the
/// word ground of an added or removed line (a removed word also struck
/// through), else bold. True when it wrote a ground (rich text).
fn marked(text: &str, marks: &[(u32, u32)], kind: DiffKind, palette: &CodePalette) -> (String, bool) {
    let ground = match kind {
        DiffKind::Add => palette.add_word.as_str(),
        DiffKind::Del => palette.del_word.as_str(),
        DiffKind::Hunk | DiffKind::Context => "",
    };
    let (open, close) = match (ground.is_empty(), kind) {
        (true, _) => ("<b>".to_owned(), "</b>"),
        (false, DiffKind::Del) => (format!("<span style=\"background-color:{ground}\"><s>"), "</s></span>"),
        (false, _) => (format!("<span style=\"background-color:{ground}\">"), "</span>"),
    };
    let mut rich = false;
    let mut out = String::with_capacity(text.len() + 16);
    let mut marks = marks.iter().peekable();
    let mut end: Option<u32> = None;
    let mut pos = 0u32;
    for c in text.chars() {
        if end.is_some_and(|e| pos >= e) {
            out.push_str(close);
            end = None;
        }
        if end.is_none() {
            while marks.peek().is_some_and(|(s, l)| *s < pos || *l == 0) {
                marks.next();
            }
            if let Some((start, len)) = marks.peek().copied()
                && *start <= pos
            {
                marks.next();
                out.push_str(&open);
                rich |= !ground.is_empty();
                end = Some(start.saturating_add(*len));
            }
        }
        push_escaped(&mut out, c, true);
        pos = pos.saturating_add(c.len_utf16() as u32);
    }
    if end.is_some() {
        out.push_str(close);
    }
    (out, rich)
}

// ---- Rows ----

/// What a row needs from outside the render crate's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Extra {
    /// The item ended (or was written) then, for "14:36" under an answer.
    pub when_ms: u64,
    /// The host cut the item's text.
    pub truncated: bool,
    /// A code block the user set to wrap.
    pub wrapped: bool,
    /// A block of a plan: drawn inside the plan's card.
    pub in_plan: bool,
    /// The images a user message carries.
    pub images: u8,
    /// A notice that carries Undo (the newest rewind's, while it can be undone).
    pub undo: bool,
}

fn act_word(act: Act) -> &'static str {
    match act {
        Act::Hidden => "",
        Act::On => "on",
        Act::LatestOnly => "latest_only",
        Act::NoRestorePoint => "no_restore_point",
        Act::NotGit => "not_git",
    }
}

/// A notice's wire word, as `Item.notice` carries it.
pub(crate) fn notice_word(notice: NoticeKind) -> &'static str {
    match notice {
        NoticeKind::Rewound => "rewound",
        NoticeKind::EditFromHere => "edit_from_here",
        NoticeKind::FilesRestored => "files_restored",
        NoticeKind::Forked => "forked",
    }
}

/// A notice of a rewind: the one Undo goes on.
pub(crate) fn is_undoable(notice: NoticeKind) -> bool {
    matches!(notice, NoticeKind::Rewound | NoticeKind::EditFromHere | NoticeKind::FilesRestored)
}

fn note_word(note: NoteKind) -> &'static str {
    match note {
        NoteKind::Recording => "recording",
        NoteKind::Missing => "missing",
        NoteKind::Expired => "expired",
        NoteKind::Unrecorded => "unrecorded",
    }
}

fn why_word(why: CheckpointWhy) -> &'static str {
    match why {
        CheckpointWhy::Busy => "busy",
        CheckpointWhy::Slow => "slow",
        CheckpointWhy::Large => "large",
        CheckpointWhy::Refused => "refused",
        CheckpointWhy::Off => "off",
        CheckpointWhy::Before => "before",
    }
}

fn set_acts(m: &mut ModelRow, acts: &Acts) {
    m.set_text(Role::ActFork, act_word(acts.fork));
    m.set_text(Role::ActRewind, act_word(acts.rewind));
    m.set_text(Role::ActEdit, act_word(acts.edit));
    m.set_text(Role::ActRestore, act_word(acts.restore));
}

/// The files a rewind left as they were, under its notice (`left:<item
/// id>`): each path with the reason in the app's words.
pub(crate) fn left_row(item_id: &str, turn: u32, depth: u8, left: &[(String, String)]) -> ModelRow {
    let mut m = ModelRow::new();
    m.set_text(Role::Key, format!("left:{item_id}"));
    m.set_text(Role::Kind, "left");
    m.set_text(Role::View, "left");
    m.set_int(Role::Turn, turn);
    m.set_int(Role::Depth, depth);
    m.set_text(Role::ItemId, item_id);
    let files: Vec<Value> = left.iter().map(|(path, why)| json!({ "path": path, "why": why })).collect();
    m.set_text(Role::Files, Value::Array(files).to_string());
    m.set_int(Role::Count, left.len());
    m
}

fn plan_state_word(state: PlanState) -> &'static str {
    match state {
        PlanState::Proposed => "proposed",
        PlanState::Implemented => "implemented",
        PlanState::Replaced => "replaced",
    }
}

fn question_state_word(state: QuestionState) -> &'static str {
    match state {
        QuestionState::Asked => "asked",
        QuestionState::Answered => "answered",
        QuestionState::Dismissed => "dismissed",
        QuestionState::Expired => "expired",
    }
}

/// The foot of a plan's card (`pf:<item id>`), after its last block shown:
/// "Show the whole plan", Change the plan and Implement. The page decides
/// which of them it offers.
pub(crate) fn plan_foot(
    item_id: &str,
    state: PlanState,
    more: u16,
    open: bool,
    turn: u32,
    depth: u8,
) -> ModelRow {
    let mut m = ModelRow::new();
    m.set_text(Role::Key, format!("pf:{item_id}"));
    m.set_text(Role::Kind, "plan_foot");
    m.set_text(Role::View, "plan_foot");
    m.set_int(Role::Turn, turn);
    m.set_int(Role::Depth, depth);
    m.set_text(Role::ItemId, item_id);
    m.set_text(Role::Status, plan_state_word(state));
    m.set_text(Role::PlanPart, "foot");
    m.set_int(Role::More, more);
    m.set_flag(Role::IsOpen, open);
    m
}

/// A row of the render crate as the model shows it. `source` is what Copy
/// takes from a tool line: its whole command. (A whole message is copied by
/// its item, not by a row.)
pub(crate) fn model_row(row: &Row, extra: Extra, source: &str, palette: &CodePalette) -> ModelRow {
    let mut m = ModelRow::new();
    let kind = row.kind.name();
    m.set_text(Role::Key, row.key.clone());
    m.set_text(Role::Kind, kind);
    m.set_text(Role::View, kind);
    m.set_int(Role::Turn, row.turn);
    m.set_int(Role::Depth, row.depth);
    m.set_text(Role::ItemId, row.item_id.clone());
    match &row.kind {
        RowKind::User { text, from, deny_of, via } => {
            m.set_text(Role::Text, text.clone());
            m.set_text(Role::From, from.clone());
            m.set_flag(Role::DenyOf, *deny_of);
            m.set_text(
                Role::Via,
                match via {
                    Via::Turn => "",
                    Via::Steer => "steer",
                    Via::Queue => "queue",
                },
            );
            m.set_int(Role::Images, extra.images);
            m.copy = text.clone();
        }
        RowKind::Block { block, index: _, last, phase } => {
            if extra.in_plan {
                m.set_text(Role::PlanPart, "body");
            }
            let (word, level) = block_word(block.kind);
            let (marker, number) = marker_word(block.marker);
            m.set_text(Role::BlockKind, word);
            // A heading's level; a list item's nesting.
            m.set_int(Role::Level, if block.kind == BlockKind::ListItem { block.depth } else { level });
            m.set_int(Role::Quote, block.quote);
            m.set_text(Role::Marker, marker);
            m.set_int(Role::MarkerNumber, number);
            m.set_flag(Role::Open, block.open);
            m.set_flag(Role::Streaming, block.open);
            m.set_flag(Role::Last, *last);
            m.set_text(
                Role::Phase,
                match phase {
                    Some(Phase::Interim) => "interim",
                    Some(Phase::Final) => "final",
                    None => "",
                },
            );
            if *last {
                m.set_real(Role::WhenMs, extra.when_ms);
                m.set_flag(Role::Truncated, extra.truncated);
            }
            match block.kind {
                BlockKind::Code => {
                    m.set_text(Role::View, "code");
                    m.set_text(Role::Text, block.text.strip_suffix('\n').unwrap_or(&block.text));
                    m.set_text(Role::Styled, code_markup(block, palette));
                    m.set_text(Role::Lang, block.lang.clone());
                    m.set_flag(Role::Wrapped, extra.wrapped);
                    let lines = if block.lines.is_empty() {
                        block.text.trim_end_matches('\n').split('\n').count()
                    } else {
                        block.lines.len()
                    };
                    m.set_int(Role::Lines, lines);
                    m.copy = m.text(Role::Text).to_owned();
                }
                BlockKind::Table => {
                    m.set_text(Role::View, "table");
                    if let Some(table) = &block.table {
                        let mut links = Vec::new();
                        let cells: Vec<Value> = table
                            .cells
                            .iter()
                            .map(|cell| {
                                json!({
                                    "styled": styled(&cell.text, &cell.spans, &mut links, palette),
                                    "header": cell.header,
                                })
                            })
                            .collect();
                        let align: Vec<&str> = table.align.iter().map(|a| align_word(*a)).collect();
                        let json = json!({
                            "columns": table.columns,
                            "align": align,
                            "more": table.more_rows,
                            "cells": cells,
                        });
                        m.set_text(Role::Table, json.to_string());
                        m.links = links;
                    }
                }
                _ => {
                    m.set_text(Role::Text, block.text.clone());
                    let mut links = Vec::new();
                    m.set_text(Role::Styled, styled(&block.text, &block.spans, &mut links, palette));
                    m.links = links;
                }
            }
            if m.copy.is_empty() {
                m.copy = block.text.clone();
            }
        }
        RowKind::Tool { label, status, has_detail, has_output, open, adds, dels, ms } => {
            let json =
                json!({ "class": class_word(label.class), "object": label.object, "more": label.more });
            m.set_text(Role::Label, json.to_string());
            m.set_text(Role::Text, label.object.clone());
            // The target as the crate tokenized it: a command as shell, a path as folder and name.
            let mut styled = String::new();
            code_text(&label.object, &label.toks, palette, &mut styled);
            m.set_text(Role::Styled, styled);
            m.set_text(Role::Status, status_word(*status));
            m.set_flag(Role::HasDetail, *has_detail);
            m.set_flag(Role::HasOutput, *has_output);
            m.set_flag(Role::IsOpen, *open);
            m.set_int(Role::Adds, *adds);
            m.set_int(Role::Dels, *dels);
            m.set_real(Role::Ms, *ms);
            m.copy = source.to_owned();
        }
        RowKind::Detail { of } => match of {
            DetailOf::Command { text, lines } => {
                m.set_text(Role::DetailKind, "command");
                m.set_text(Role::Text, text.clone());
                m.set_text(Role::Styled, lines_markup(lines, palette));
                m.copy = text.clone();
            }
            DetailOf::Diff { doc, edit, edits } => {
                m.set_text(Role::DetailKind, "diff");
                m.set_text(Role::Diff, diff_json(doc, INLINE_DIFF_ROWS, palette).to_string());
                m.set_int(Role::Edit, *edit);
                m.set_int(Role::Edits, *edits);
                m.set_int(Role::Adds, doc.adds);
                m.set_int(Role::Dels, doc.dels);
            }
            DetailOf::Output { text, cut, lines } => {
                m.set_text(Role::DetailKind, "output");
                m.set_text(Role::Text, text.clone());
                // Error, warning and pass lines marked; none marked: plain.
                m.set_text(Role::Styled, lines_markup(lines, palette));
                m.set_flag(Role::Cut, *cut);
                m.copy = text.clone();
            }
        },
        RowKind::Group { summary, count, failed, open } => {
            m.set_text(Role::Summary, summary_json(summary).to_string());
            m.set_int(Role::Count, *count);
            m.set_flag(Role::Failed, *failed);
            m.set_flag(Role::IsOpen, *open);
        }
        RowKind::Thought { preview, count, has_text, open } => {
            m.set_text(Role::Text, preview.clone());
            m.set_int(Role::Count, *count);
            m.set_flag(Role::HasText, *has_text);
            m.set_flag(Role::IsOpen, *open);
        }
        RowKind::Fold { worked_ms, hidden, open } => {
            m.set_real(Role::WorkedMs, *worked_ms);
            m.set_int(Role::Hidden, *hidden);
            m.set_flag(Role::IsOpen, *open);
        }
        RowKind::Changes { files, adds, dels, more, recorded } => {
            let files: Vec<Value> = files
                .iter()
                .map(|f| json!({ "path": f.path, "adds": f.adds, "dels": f.dels, "open": f.open }))
                .collect();
            m.set_text(Role::Files, Value::Array(files).to_string());
            m.set_int(Role::Adds, *adds);
            m.set_int(Role::Dels, *dels);
            m.set_int(Role::More, *more);
            m.set_flag(Role::Recorded, *recorded);
        }
        RowKind::ChangesNote { note, why, count } => {
            m.set_text(Role::Note, note_word(*note));
            m.set_text(Role::Why, why.map_or("", why_word));
            m.set_int(Role::Count, *count);
        }
        RowKind::Mark { kind, by } => {
            m.set_text(Role::MarkKind, mark_word(*kind));
            m.set_text(Role::By, by.clone());
        }
        RowKind::Notice { text, notice } => {
            m.set_text(Role::Text, text.clone());
            m.set_text(Role::Notice, notice.map_or("", notice_word));
            m.set_flag(Role::Undo, extra.undo && notice.is_some_and(is_undoable));
            m.copy = text.clone();
        }
        RowKind::Plan { state, shown, more, open } => {
            m.set_text(Role::Status, plan_state_word(*state));
            m.set_text(Role::PlanPart, "head");
            m.set_int(Role::Count, *shown);
            m.set_int(Role::More, *more);
            m.set_flag(Role::IsOpen, *open);
        }
        RowKind::Question { state, count, by, has_text, open } => {
            m.set_text(Role::Status, question_state_word(*state));
            m.set_int(Role::Count, *count);
            m.set_text(Role::By, by.clone());
            m.set_flag(Role::HasText, *has_text);
            m.set_flag(Role::IsOpen, *open);
        }
        RowKind::Live { label, thought, since_ms } => {
            m.set_text(
                Role::LiveLabel,
                match label {
                    LiveLabel::Starting => "starting",
                    LiveLabel::Thinking => "thinking",
                },
            );
            m.set_text(Role::Text, thought.clone());
            m.set_real(Role::SinceMs, *since_ms);
        }
    }
    set_acts(&mut m, &row.acts);
    m
}

// ---- Changes by key ----

/// One change to the model, applied in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RowOp {
    Remove { at: usize, count: usize },
    Insert { at: usize, rows: Vec<Arc<ModelRow>> },
    Change { at: usize, row: Arc<ModelRow>, roles: Vec<Role> },
}

/// The changes that turn `old` into `new`, matching rows by key: rows whose
/// key is gone are removed, new keys inserted where they go, and a row with
/// the same key changed in the roles that differ. A text that grows changes
/// only its last block and may add rows after it.
pub(crate) fn row_ops(old: &[Arc<ModelRow>], new: &[Arc<ModelRow>]) -> Vec<RowOp> {
    let new_keys: HashSet<&str> = new.iter().map(|r| r.key()).collect();
    let mut model: Vec<Arc<ModelRow>> = old.to_vec();
    let mut ops = Vec::new();
    // Rows that went, the last run first so earlier indexes hold.
    let mut i = model.len();
    while i > 0 {
        if new_keys.contains(model[i - 1].key()) {
            i -= 1;
            continue;
        }
        let end = i;
        while i > 0 && !new_keys.contains(model[i - 1].key()) {
            i -= 1;
        }
        model.drain(i..end);
        ops.push(RowOp::Remove { at: i, count: end - i });
    }
    let mut present: HashSet<String> = model.iter().map(|r| r.key().to_owned()).collect();
    let mut i = 0;
    while i < new.len() {
        let key = new[i].key();
        if model.get(i).is_some_and(|r| r.key() == key) {
            if !Arc::ptr_eq(&model[i], &new[i]) {
                let roles = new[i].changed_roles(&model[i]);
                if !roles.is_empty() {
                    ops.push(RowOp::Change { at: i, row: new[i].clone(), roles });
                }
                model[i] = new[i].clone();
            }
            i += 1;
            continue;
        }
        if present.contains(key)
            && let Some(pos) = model[i..].iter().position(|r| r.key() == key)
        {
            // What's before it here comes later in the new rows: it goes now
            // and is inserted again where it belongs.
            for r in &model[i..i + pos] {
                present.remove(r.key());
            }
            model.drain(i..i + pos);
            ops.push(RowOp::Remove { at: i, count: pos });
            continue;
        }
        let start = i;
        i += 1;
        while i < new.len() && !present.contains(new[i].key()) {
            i += 1;
        }
        let rows = new[start..i].to_vec();
        for r in &rows {
            present.insert(r.key().to_owned());
        }
        model.splice(start..start, rows.iter().cloned());
        ops.push(RowOp::Insert { at: start, rows });
    }
    if model.len() > new.len() {
        ops.push(RowOp::Remove { at: new.len(), count: model.len() - new.len() });
    }
    ops
}

/// `ops` applied to `rows`, as the model applies them.
#[cfg(test)]
pub(crate) fn apply_ops(rows: &mut Vec<Arc<ModelRow>>, ops: Vec<RowOp>) {
    for op in ops {
        match op {
            RowOp::Remove { at, count } => {
                let end = (at + count).min(rows.len());
                rows.drain(at.min(end)..end);
            }
            RowOp::Insert { at, rows: new } => {
                let at = at.min(rows.len());
                rows.splice(at..at, new);
            }
            RowOp::Change { at, row, .. } => {
                if let Some(r) = rows.get_mut(at) {
                    *r = row;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use g7_agents_render::{Style, markdown};

    use super::*;

    fn palette() -> CodePalette {
        CodePalette::from_json(
            r##"{ "keyword": "#ff00aa", "string": "#00aaff", "comment": "#888888", "inline": "#c0ffee",
                 "type": "not a colour\" onclick=\"x", "add": "#00ff00", "del": "#ff0000" }"##,
        )
    }

    /// The markup with every tag this file writes taken out: what's left
    /// must have no `<` or `>` and only the entities written here.
    fn without_our_tags(markup: &str) -> String {
        let mut s = markup.to_owned();
        for tag in ["<b>", "</b>", "<i>", "</i>", "<s>", "</s>", "</a>", "</font>", "</span>", "<br>"] {
            s = s.replace(tag, "");
        }
        let mut out = String::new();
        let mut rest = s.as_str();
        while let Some(at) = rest.find('<') {
            out.push_str(&rest[..at]);
            let tail = &rest[at..];
            let quoted = |prefix: &str, ok: fn(&str) -> bool| {
                let body = tail.strip_prefix(prefix)?;
                let end = body.find("\">")?;
                ok(&body[..end]).then_some(prefix.len() + end + 2)
            };
            let skip = quoted("<a href=\"", |n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                .or_else(|| quoted("<font color=\"", is_colour))
                .or_else(|| quoted("<span style=\"background-color:", is_colour));
            match skip {
                Some(len) => rest = &tail[len..],
                None => {
                    out.push_str(tail);
                    break;
                }
            }
        }
        out.push_str(rest);
        out
    }

    fn assert_only_our_tags(markup: &str) {
        let left = without_our_tags(markup);
        assert!(!left.contains('<') && !left.contains('>'), "a tag that isn't ours in {left:?}");
        let mut rest = left.as_str();
        while let Some(at) = rest.find('&') {
            let tail = &rest[at..];
            assert!(
                ["&amp;", "&lt;", "&gt;", "&quot;", "&nbsp;"].iter().any(|e| tail.starts_with(e)),
                "an entity that isn't ours: {:?}",
                &tail[..tail.len().min(12)]
            );
            rest = &tail[1..];
        }
    }

    const HOSTILE: &[&str] = &[
        "Open \u{202e}gnp.exe\u{202c} now",
        "<img src=x onerror=alert(1)> <script>alert(1)</script> <iframe src=\"https://evil.example\"></iframe>",
        "[x](javascript:alert(1)) [y](data:text/html,hi) [z](file:///etc/passwd)",
        "[https://good.example](https://evil.example) and <https://shown.example>",
        "**bold _both_** ~~gone~~ `<b>code</b>` &amp; &#60;b&#62; &lt;",
        "```html\n<a href=\"x\">\"quoted\" & 'single'</a>\n```",
        "| a | <b>b</b> |\n|---|---|\n| [l](https://x.example) | `c<` |",
        "> [!WARNING]\n> <font color=red>careful</font>",
        "nul\u{0}c1\u{85}bell\u{7} tab\there",
        "![alt <b>](https://img.example/x.png) ![plain](javascript:x)",
        "- [ ] todo <u>\n- [x] done\n  1. nested \"q\"",
        "line one  \nline two\\\nthree",
    ];

    fn rows_of(text: &str) -> Vec<ModelRow> {
        markdown(text)
            .blocks
            .into_iter()
            .enumerate()
            .map(|(index, block)| {
                let row = Row {
                    key: format!("b:m:{index}"),
                    kind: RowKind::Block { block, index: index as u16, last: false, phase: None },
                    turn: 1,
                    depth: 0,
                    item_id: "m".into(),
                    seq: 1,
                    acts: Default::default(),
                };
                model_row(&row, Extra::default(), "", &palette())
            })
            .collect()
    }

    #[test]
    fn hostile_text_is_markup_with_our_tags_only() {
        let mut checked = 0;
        for text in HOSTILE {
            for row in rows_of(text) {
                assert_only_our_tags(row.text(Role::Styled));
                if !row.text(Role::Table).is_empty() {
                    let table: Value = serde_json::from_str(row.text(Role::Table)).unwrap();
                    for cell in table["cells"].as_array().unwrap() {
                        assert_only_our_tags(cell["styled"].as_str().unwrap());
                    }
                }
                for link in &row.links {
                    let scheme = link.href.split(':').next().unwrap_or_default().to_ascii_lowercase();
                    assert!(["http", "https", "mailto"].contains(&scheme.as_str()), "{scheme}");
                }
                checked += 1;
            }
        }
        assert!(checked > HOSTILE.len());
        // Escaping alone, whatever the text.
        let all: String = (0u32..0x3000).filter_map(char::from_u32).collect();
        assert_only_our_tags(&escaped(&all, false));
        assert_only_our_tags(&escaped(&all, true));
        let marks = [(3, 40), (10, 5), (200, 9_000)];
        assert_only_our_tags(&marked(&all, &marks, DiffKind::Del, &palette()).0);
        let grounds = CodePalette::from_json(r##"{ "addWord": "#55693c", "delWord": "#663e40" }"##);
        assert_only_our_tags(&marked(&all, &marks, DiffKind::Del, &grounds).0);
        assert_only_our_tags(&marked(&all, &marks, DiffKind::Add, &grounds).0);
    }

    /// A fixture's input: a string, or `{ repeat, times, before, after }`
    /// (the render crate's form for very long inputs).
    fn fixture_input(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            Value::Object(o) => {
                let s = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
                let times = o.get("times").and_then(Value::as_u64).unwrap_or(1) as usize;
                format!("{}{}{}", s("before"), s("repeat").repeat(times), s("after"))
            }
            _ => String::new(),
        }
    }

    #[test]
    fn every_markdown_fixture_is_markup_with_our_tags_only() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../core/g7-agents-render/tests/fixtures/markdown");
        let mut files = 0;
        for entry in std::fs::read_dir(&dir).expect("the render crate's fixtures") {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let fixture: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let text = fixture_input(&fixture["in"]);
            for row in rows_of(&text) {
                let name = path.display();
                let left = without_our_tags(row.text(Role::Styled));
                assert!(!left.contains('<') && !left.contains('>'), "{name}: a tag that isn't ours");
                assert_only_our_tags(row.text(Role::Styled));
                if !row.text(Role::Table).is_empty() {
                    let table: Value = serde_json::from_str(row.text(Role::Table)).unwrap();
                    for cell in table["cells"].as_array().unwrap() {
                        assert_only_our_tags(cell["styled"].as_str().unwrap());
                    }
                }
                for link in &row.links {
                    let scheme = link.href.split(':').next().unwrap_or_default().to_ascii_lowercase();
                    assert!(["http", "https", "mailto"].contains(&scheme.as_str()), "{name}: {scheme}");
                }
            }
            files += 1;
        }
        assert!(files >= 20, "only {files} fixtures read");
    }

    #[test]
    fn spans_become_tags_and_links_numbers() {
        let text = "a bold link and <code>";
        let span = |start, len, style: Style, href: &str| Span { start, len, style, href: href.into() };
        let strong = Style { strong: true, ..Style::default() };
        let web = Style { link: Link::Web, em: true, ..Style::default() };
        let code = Style { code: true, ..Style::default() };
        let spans =
            [span(2, 4, strong, ""), span(7, 4, web, "https://e.example/?a=\"b\""), span(16, 6, code, "")];
        let mut links = Vec::new();
        let out = styled(text, &spans, &mut links, &palette());
        assert_eq!(
            out,
            "a <b>bold</b> <a href=\"0\"><i>link</i></a> and <font color=\"#c0ffee\">&lt;code&gt;</font>"
        );
        assert_eq!(links.len(), 1);
        assert_eq!(
            (links[0].href.as_str(), links[0].text.as_str(), links[0].shown),
            ("https://e.example/?a=\"b\"", "link", false)
        );
        // Out of range and out of order spans stay plain; nothing panics.
        let bad = [span(5, 100, strong, ""), span(1, 2, strong, ""), span(40, 2, strong, "")];
        let out = styled("tiny", &bad, &mut Vec::new(), &palette());
        assert_eq!(out, "tiny");
        // UTF-16: an astral character counts two units.
        let out = styled("😀ab", &[span(2, 1, strong, "")], &mut Vec::new(), &CodePalette::default());
        assert_eq!(out, "😀<b>a</b>b");
    }

    #[test]
    fn plans_questions_and_steered_messages_have_their_roles() {
        let row = |kind| Row {
            key: "k".into(),
            kind,
            turn: 2,
            depth: 0,
            item_id: "p1".into(),
            seq: 1,
            acts: Default::default(),
        };
        let plan = model_row(
            &row(RowKind::Plan { state: PlanState::Replaced, shown: 0, more: 4, open: false }),
            Extra::default(),
            "",
            &palette(),
        );
        assert_eq!(
            (plan.text(Role::View), plan.text(Role::Status), plan.text(Role::PlanPart)),
            ("plan", "replaced", "head")
        );
        assert_eq!(plan.get(Role::More), &RoleValue::Int(4));
        let question = model_row(
            &row(RowKind::Question {
                state: QuestionState::Answered,
                count: 3,
                by: "Pixel".into(),
                has_text: true,
                open: true,
            }),
            Extra::default(),
            "",
            &palette(),
        );
        assert_eq!((question.text(Role::Status), question.text(Role::By)), ("answered", "Pixel"));
        assert_eq!(question.get(Role::Count), &RoleValue::Int(3));
        let user = model_row(
            &row(RowKind::User {
                text: "Leave it".into(),
                from: String::new(),
                deny_of: false,
                via: Via::Steer,
            }),
            Extra { images: 2, ..Extra::default() },
            "",
            &palette(),
        );
        assert_eq!((user.text(Role::Via), user.get(Role::Images)), ("steer", &RoleValue::Int(2)));
        let foot = plan_foot("p1", PlanState::Proposed, 3, false, 2, 0);
        assert_eq!(
            (foot.key(), foot.text(Role::View), foot.text(Role::PlanPart)),
            ("pf:p1", "plan_foot", "foot")
        );
        // A block inside the card says so; the plan's text is its own markup.
        let mut block = rows_of("Add `DropReason` <b>now</b>").remove(0);
        assert_eq!(block.text(Role::PlanPart), "");
        let source = Row {
            key: "b:p1:0".into(),
            kind: RowKind::Block {
                block: markdown("Add it").blocks.remove(0),
                index: 0,
                last: true,
                phase: None,
            },
            turn: 1,
            depth: 0,
            item_id: "p1".into(),
            seq: 1,
            acts: Default::default(),
        };
        block = model_row(&source, Extra { in_plan: true, ..Extra::default() }, "", &palette());
        assert_eq!(block.text(Role::PlanPart), "body");
        let inline = inline_markup("Use `cargo test` <b>here</b> `", &palette());
        assert!(inline.contains("&lt;b&gt;here") && !inline.contains("<b>here"), "{inline}");
        assert_only_our_tags(&inline);
        assert_eq!(inline.matches("<font").count(), 1, "a lone backtick is text: {inline}");
        assert!(inline.ends_with('`'), "{inline}");
    }

    #[test]
    fn changes_notices_and_actions_have_their_roles() {
        let row =
            |kind, acts| Row { key: "k".into(), kind, turn: 3, depth: 0, item_id: "u1".into(), seq: 1, acts };
        let acts = Acts { fork: Act::LatestOnly, rewind: Act::On, edit: Act::On, restore: Act::NotGit };
        let user = model_row(
            &row(
                RowKind::User { text: "Fix it".into(), from: String::new(), deny_of: false, via: Via::Turn },
                acts,
            ),
            Extra::default(),
            "",
            &palette(),
        );
        assert_eq!(
            [Role::ActFork, Role::ActRewind, Role::ActEdit, Role::ActRestore]
                .map(|r| user.text(r).to_owned()),
            ["latest_only", "on", "on", "not_git"]
        );
        let hidden = model_row(
            &row(RowKind::Notice { text: "Stopped".into(), notice: None }, Acts::default()),
            Extra { undo: true, ..Extra::default() },
            "",
            &palette(),
        );
        assert_eq!(hidden.text(Role::ActFork), "", "a hidden action is empty");
        assert_eq!(hidden.get(Role::Undo), &RoleValue::Flag(false), "only a rewind's notice carries Undo");
        let rewound = model_row(
            &row(
                RowKind::Notice { text: "Rewound to here".into(), notice: Some(NoticeKind::EditFromHere) },
                Acts::default(),
            ),
            Extra { undo: true, ..Extra::default() },
            "",
            &palette(),
        );
        assert_eq!(
            (rewound.text(Role::Notice), rewound.get(Role::Undo)),
            ("edit_from_here", &RoleValue::Flag(true))
        );
        let forked = model_row(
            &row(
                RowKind::Notice { text: "Forked".into(), notice: Some(NoticeKind::Forked) },
                Acts::default(),
            ),
            Extra { undo: true, ..Extra::default() },
            "",
            &palette(),
        );
        assert_eq!(forked.get(Role::Undo), &RoleValue::Flag(false), "a fork has nothing to undo");
        let note = model_row(
            &row(
                RowKind::ChangesNote { note: NoteKind::Missing, why: Some(CheckpointWhy::Busy), count: 0 },
                Acts::default(),
            ),
            Extra::default(),
            "",
            &palette(),
        );
        assert_eq!(
            (note.text(Role::View), note.text(Role::Note), note.text(Role::Why)),
            ("changes_note", "missing", "busy")
        );
        let changes = model_row(
            &row(
                RowKind::Changes { files: Vec::new(), adds: 3, dels: 1, more: 0, recorded: true },
                Acts::default(),
            ),
            Extra::default(),
            "",
            &palette(),
        );
        assert_eq!(changes.get(Role::Recorded), &RoleValue::Flag(true));
        let left = left_row("n1", 3, 0, &[("a.rs".into(), "not a file".into())]);
        assert_eq!(
            (left.key(), left.text(Role::View), left.get(Role::Count)),
            ("left:n1", "left", &RoleValue::Int(1))
        );
        let files: Value = serde_json::from_str(left.text(Role::Files)).unwrap();
        assert_eq!(files, json!([{ "path": "a.rs", "why": "not a file" }]));
    }

    #[test]
    fn a_colour_from_the_page_must_be_a_colour() {
        let p = palette();
        assert_eq!(p.of(TokClass::Keyword), Some("#ff00aa"));
        assert_eq!(p.of(TokClass::Type), None, "a value that isn't a colour is left out");
        assert!(is_colour("#abc") && is_colour("#80aabbcc") && !is_colour("red") && !is_colour("#12345g"));
        assert_eq!(CodePalette::from_json("not json"), CodePalette::default());
    }

    #[test]
    fn code_keeps_its_spaces_and_colours_its_tokens() {
        let rows = rows_of("```rust\nfn main() {\n    let s = \"<x>\";\n}\n```");
        assert_eq!(rows.len(), 1);
        let code = &rows[0];
        assert_eq!(code.text(Role::View), "code");
        assert_eq!(code.text(Role::Lang), "rust");
        assert_eq!(code.text(Role::Text), "fn main() {\n    let s = \"<x>\";\n}");
        assert_eq!(code.copy_text(), code.text(Role::Text), "Copy takes the source");
        let styled = code.text(Role::Styled);
        assert_only_our_tags(styled);
        assert_eq!(styled.matches("<br>").count(), 2);
        assert!(styled.contains("&nbsp;&nbsp;&nbsp;&nbsp;"), "{styled}");
        assert!(styled.contains("<font color=\"#ff00aa\">"), "keywords are coloured: {styled}");
        assert_eq!(code.get(Role::Lines), &RoleValue::Int(3));
        // A language there are no colours for (two-face has Nix now): its source, escaped.
        let plain = &rows_of("```g7-unknown\n{ a = <b>; }\n```")[0];
        assert_eq!(plain.text(Role::Styled), "{&nbsp;a&nbsp;=&nbsp;&lt;b&gt;;&nbsp;}");
    }

    #[test]
    fn the_palette_colours_and_styles_code_tools_paths_and_diff_words() {
        let palette = CodePalette::from_json(
            r##"{ "keyword": "#ed93b1", "comment": "#a1a1a8", "function": "#7cb7ee", "dir": "#a1a1a8",
                 "file": "#f2f2f3", "error": "#f0a39c", "inline": "#f0997b", "addWord": "#55693c",
                 "delWord": "#663e40", "styles": { "comment": "italic", "keyword": "semibold", "tag": "bold?" } }"##,
        );
        // Classes keep their styles: a comment italic, a keyword semibold.
        let mut out = String::new();
        let toks = [
            Tok { len: 2, class: TokClass::Keyword },
            Tok { len: 1, class: TokClass::Plain },
            Tok { len: 4, class: TokClass::Comment },
        ];
        code_text("fn // x", &toks, &palette, &mut out);
        assert_eq!(
            out,
            "<font color=\"#ed93b1\"><b>fn</b></font>&nbsp;<font color=\"#a1a1a8\"><i>//&nbsp;x</i></font>"
        );
        assert_only_our_tags(&out);
        // A tool's target as its tokens say; an output with no line marked stays plain.
        let line =
            CodeLine { text: "cargo test".into(), toks: vec![Tok { len: 5, class: TokClass::Function }] };
        assert!(
            lines_markup(std::slice::from_ref(&line), &palette).starts_with("<font color=\"#7cb7ee\">cargo")
        );
        assert_eq!(lines_markup(&[CodeLine { text: "ok".into(), toks: Vec::new() }], &palette), "");
        // A path in inline code: the folder muted, the name in ink.
        let doc = g7_agents_render::markdown("See `src/sync/queue.rs`.");
        let mut spans = doc.blocks[0].spans.clone();
        spans.iter_mut().for_each(|s| s.style.path = true);
        let path = styled(&doc.blocks[0].text, &spans, &mut Vec::new(), &palette);
        assert!(
            path.contains("<font color=\"#a1a1a8\">src/sync/</font><font color=\"#f2f2f3\">queue.rs</font>"),
            "{path}"
        );
        assert_only_our_tags(&path);
        // A changed word is a ground; a removed one is struck through too.
        let diff = g7_agents_render::diff("-let a = 1;\n+let a = 2;\n");
        let json = diff_json(&diff, 10, &palette);
        let removed = json["files"][0]["rows"][0]["s"].as_str().unwrap();
        assert!(removed.contains("<span style=\"background-color:#663e40\"><s>1;</s></span>"), "{removed}");
        assert_eq!(json["files"][0]["rows"][0]["r"], true, "it needs rich text");
        assert_only_our_tags(removed);
        let plain = diff_json(&diff, 10, &CodePalette::default());
        assert_eq!(plain["files"][0]["rows"][1]["r"], false);
    }

    #[test]
    fn diffs_mark_changed_words_and_stop_at_the_limit() {
        let doc = g7_agents_render::diff("-let a = 1;\n+let a = 2;\n context <x>\n");
        let json = diff_json(&doc, 2, &CodePalette::default());
        assert_eq!(json["more"], true);
        let file = &json["files"][0];
        assert_eq!(file["numbered"], false, "no hunk headers: no line numbers");
        assert_eq!(file["rows"].as_array().unwrap().len(), 2);
        assert_eq!(file["rows"][0]["k"], "del");
        let added = file["rows"][1]["s"].as_str().unwrap();
        assert!(added.contains("<b>2"), "the changed word is bold: {added}");
        assert_only_our_tags(added);
        let numbered =
            diff_json(&g7_agents_render::diff("@@ -3,1 +3,1 @@\n-a\n+b\n"), 400, &CodePalette::default());
        assert_eq!(numbered["files"][0]["numbered"], true);
    }

    fn row(key: &str, text: &str) -> Arc<ModelRow> {
        let mut m = ModelRow::new();
        m.set_text(Role::Key, key);
        m.set_text(Role::Text, text);
        Arc::new(m)
    }

    fn keys(rows: &[Arc<ModelRow>]) -> Vec<String> {
        rows.iter().map(|r| format!("{}={}", r.key(), r.text(Role::Text))).collect()
    }

    /// Replays `steps`, checking the model stays equal to each list.
    fn replay(steps: &[Vec<Arc<ModelRow>>]) -> Vec<Vec<RowOp>> {
        let mut model: Vec<Arc<ModelRow>> = Vec::new();
        let mut all = Vec::new();
        for step in steps {
            let ops = row_ops(&model, step);
            apply_ops(&mut model, ops.clone());
            assert_eq!(keys(&model), keys(step));
            all.push(ops);
        }
        all
    }

    #[test]
    fn the_model_follows_every_list_by_key() {
        let steps = vec![
            vec![row("i:u", "hi"), row("live", "")],
            vec![row("i:u", "hi"), row("b:m:0", "Hel")],
            vec![row("i:u", "hi"), row("b:m:0", "Hello"), row("b:m:1", "wor")],
            // A fold closes over the work and the answer stays.
            vec![row("i:u", "hi"), row("f:1", ""), row("b:m:1", "world")],
            // It opens.
            vec![
                row("i:u", "hi"),
                row("f:1", ""),
                row("b:m:0", "Hello"),
                row("i:t", "ls"),
                row("b:m:1", "world"),
            ],
            // Order changes: a mark moves after its tool.
            vec![row("m:t", ""), row("i:u", "hi"), row("i:t", "ls")],
            vec![row("i:u", "hi"), row("i:t", "ls"), row("m:t", "")],
            vec![row("i:t", "ls"), row("i:u", "hi")],
            vec![],
            vec![row("a", "1"), row("b", "2"), row("c", "3")],
            vec![row("c", "3"), row("b", "2"), row("a", "1")],
            // A duplicated key never panics.
            vec![row("a", "1"), row("a", "2")],
            vec![row("a", "3")],
        ];
        replay(&steps);
    }

    #[test]
    fn a_streaming_text_changes_only_its_last_block() {
        let base = vec![row("i:u", "hi"), row("b:m:0", "First."), row("b:m:1", "Sec")];
        let grown = vec![row("i:u", "hi"), base[1].clone(), row("b:m:1", "Second."), row("b:m:2", "Th")];
        let ops = row_ops(&base, &grown);
        assert_eq!(ops.len(), 2, "{ops:?}");
        assert!(matches!(&ops[0], RowOp::Change { at: 2, roles, .. } if roles == &[Role::Text]));
        assert!(matches!(&ops[1], RowOp::Insert { at: 3, rows } if rows.len() == 1));
        // The same rows again: nothing.
        assert!(row_ops(&grown, &grown).is_empty());
        let copy: Vec<_> = grown.iter().map(|r| Arc::new((**r).clone())).collect();
        assert!(row_ops(&grown, &copy).is_empty(), "equal rows change nothing");
    }

    #[test]
    fn a_seeded_shuffle_of_lists_keeps_the_model_equal() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut steps = Vec::new();
        for _ in 0..300 {
            let len = (next() % 12) as usize;
            let mut keys: Vec<u64> = (0..16).collect();
            for i in (1..keys.len()).rev() {
                keys.swap(i, (next() % (i as u64 + 1)) as usize);
            }
            steps.push(
                keys[..len].iter().map(|k| row(&format!("k{k}"), &format!("{}", next() % 3))).collect(),
            );
        }
        replay(&steps);
    }
}
