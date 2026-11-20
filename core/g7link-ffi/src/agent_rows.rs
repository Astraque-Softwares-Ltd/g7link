// SPDX-License-Identifier: MPL-2.0
//! An agent thread as rows for the app to draw (`g7-agents-render`), with
//! the typing guard and the diff reader the approvals dock uses.
//!
//! The rows carry no sentences: labels, statuses and times are data the app
//! turns into its own translated words. No `Debug` here prints agent text
//! (protocol v0 §11).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

use g7_agents_render as render;

use crate::{
    AgentItem, AgentItemKind, AgentItemPhase, AgentPlanState, AgentQuestionOutcome, AgentThreadState,
    AgentToolClass, AgentToolStatus, AgentVia,
};

/// `Debug` for a record that prints only the fields named, never its text.
macro_rules! quiet_debug {
    ($ty:ident { $($field:ident),* }) => {
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($ty))$(.field(stringify!($field), &self.$field))*.finish_non_exhaustive()
            }
        }
    };
}

fn unix_ms(ms: i64) -> u64 {
    u64::try_from(ms).unwrap_or(0)
}

fn signed_ms(ms: u64) -> i64 {
    i64::try_from(ms).unwrap_or(i64::MAX)
}

// ---- Markdown ----

/// One Markdown block of an answer, a thought or a table cell's row: a
/// paragraph, heading, list item, code block, table, rule or alert.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentBlock {
    pub kind: AgentBlockKind,
    /// List nesting, 0 to 16.
    pub depth: u8,
    /// Block-quote nesting.
    pub quote: u8,
    pub marker: AgentMarker,
    /// The visible text; for a code block, its source (what Copy copies).
    pub text: String,
    /// Styled runs over `text`, in UTF-16 units: sorted, never overlapping;
    /// gaps are plain text.
    pub spans: Vec<AgentSpan>,
    /// A code block's language word, lowercased; empty otherwise.
    pub lang: String,
    /// A code block's coloured lines; empty when it isn't coloured (show
    /// `text` plain).
    pub lines: Vec<AgentCodeLine>,
    pub table: Option<AgentTable>,
    /// The last block of a text that is still arriving.
    pub open: bool,
}

quiet_debug!(AgentBlock { kind, depth, quote, marker, open });

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentBlockKind {
    Paragraph,
    Heading { level: u8 },
    ListItem,
    Code,
    Table,
    Rule,
    Alert { alert: AgentAlertKind },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentAlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

/// A list item's marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentMarker {
    None,
    Bullet,
    Number { number: u32 },
    Todo,
    Done,
}

/// A styled run of a block's text, in UTF-16 units.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentSpan {
    pub start: u32,
    pub len: u32,
    pub style: AgentStyle,
    /// A link's target (`http`, `https` or `mailto` only); empty otherwise.
    pub href: String,
}

quiet_debug!(AgentSpan { start, len, style });

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AgentStyle {
    pub strong: bool,
    pub em: bool,
    pub strike: bool,
    pub code: bool,
    pub link: AgentLink,
    /// The alt text of an image, kept as a link to it: put the image glyph
    /// before it. The image is never loaded.
    pub image: bool,
}

/// A link: `Web` and `Mail` show their target before opening; `Shown`'s
/// text is its target, so it opens at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentLink {
    None,
    Web,
    Mail,
    Shown,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentTable {
    pub columns: u8,
    pub align: Vec<AgentAlign>,
    /// Row by row; the first row's cells are headers.
    pub cells: Vec<AgentCell>,
    /// Rows left out ("%n more rows").
    pub more_rows: u32,
}

quiet_debug!(AgentTable { columns, more_rows });

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentCell {
    pub text: String,
    pub spans: Vec<AgentSpan>,
    pub header: bool,
}

quiet_debug!(AgentCell { header });

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentAlign {
    None,
    Left,
    Center,
    Right,
}

// ---- Code ----

/// A line of code, its tokens covering it in order.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentCodeLine {
    pub text: String,
    pub toks: Vec<AgentTok>,
}

quiet_debug!(AgentCodeLine { toks });

/// A run of a code line, `len` UTF-16 units long, coloured by its class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AgentTok {
    pub len: u32,
    pub tok_class: AgentTokClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentTokClass {
    Plain,
    Comment,
    Keyword,
    String,
    Number,
    Constant,
    Type,
    Function,
    Variable,
    Operator,
    Punctuation,
    Tag,
    Attribute,
    Inserted,
    Deleted,
}

// ---- Diff ----

/// An edit read as a diff: its files, their rows, and the totals.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentDiffDoc {
    pub files: Vec<AgentDiffFile>,
    pub adds: u32,
    pub dels: u32,
    /// Rows were left out.
    pub more: bool,
}

quiet_debug!(AgentDiffDoc { files, adds, dels, more });

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentDiffFile {
    /// Empty when the diff named no file.
    pub path: String,
    pub adds: u32,
    pub dels: u32,
    pub rows: Vec<AgentDiffRow>,
}

quiet_debug!(AgentDiffFile { adds, dels, rows });

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentDiffRow {
    pub kind: AgentDiffKind,
    /// Line numbers; 0 for none.
    pub old_no: u32,
    pub new_no: u32,
    pub text: String,
    /// The words that changed within the line.
    pub marks: Vec<AgentDiffMark>,
}

quiet_debug!(AgentDiffRow { kind, old_no, new_no, marks });

/// A changed range within a diff row's text, in UTF-16 units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AgentDiffMark {
    pub start: u32,
    pub len: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentDiffKind {
    Hunk,
    Context,
    Add,
    Del,
}

// ---- Timeline ----

/// A request's state, shown under the step it's about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentMarkKind {
    /// "Waiting for you".
    Waiting,
    /// Waiting, held by the typing guard: "Waiting for you".
    Held,
    /// "Allowed on %1" (`by`), or "Allowed" when `by` is this device.
    Allowed,
    Denied,
    Expired,
    Cancelled,
    /// Allowed by the PC user's rule for the thread ("Allowed by your rule
    /// for this thread"): a step with `AgentTool.by_rule`, which `rows`
    /// marks by itself.
    Rule,
}

/// A request at its tool item (`item_id`): `by` names the device that
/// answered it. Pass `Allowed` and `Denied` only for an answer given on
/// another device (the step's own line says the rest).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentMark {
    pub item_id: String,
    pub kind: AgentMarkKind,
    pub by: String,
}

quiet_debug!(AgentMark { kind });

/// Text still arriving for item `item_id` (`seq`): the text so far, from
/// `Event.AgentDelta`s. Its item may not have come yet.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentStreaming {
    pub item_id: String,
    pub seq: u64,
    pub kind: AgentItemKind,
    pub text: String,
}

quiet_debug!(AgentStreaming { seq, kind });

/// One row of a thread, top to bottom. `key` is stable while the row
/// exists: diff by it and use it as the list key.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentRow {
    pub key: String,
    pub kind: AgentRowKind,
    pub turn: u32,
    /// 1 inside an open fold, group or step.
    pub depth: u8,
    pub item_id: String,
    pub seq: u64,
}

quiet_debug!(AgentRow { key, kind, turn, depth, seq });

/// What a row shows. No sentences: the app words the labels.
#[derive(Clone, PartialEq, Eq, uniffi::Enum)]
pub enum AgentRowKind {
    /// Your message, or one from `from` (a phone's name; empty for the PC).
    /// `deny_of`: you denied a request with it ("You denied it and said").
    /// `via`: sent while the agent worked ("Sent while it worked").
    User {
        text: String,
        from: String,
        deny_of: bool,
        via: Option<AgentVia>,
    },
    /// One Markdown block of an answer; `index` from 0, `last` for the
    /// message's last block (its "Copy message").
    Block {
        block: AgentBlock,
        index: u16,
        last: bool,
        phase: Option<AgentItemPhase>,
    },
    /// A step. `adds` and `dels`: an edit's lines added and removed ("+a −d");
    /// `ms`: how long it ran once it ended, else 0 (shown from 1 s up).
    Tool {
        label: AgentToolLabel,
        status: AgentRowStatus,
        has_detail: bool,
        has_output: bool,
        open: bool,
        adds: u32,
        dels: u32,
        ms: i64,
    },
    /// Under an open tool row: its command, its edit or its output.
    Detail {
        of: AgentDetailOf,
    },
    /// A run of steps in one line; `failed` when one of them failed or was denied.
    Group {
        summary: AgentSummary,
        count: u16,
        failed: bool,
        open: bool,
    },
    /// What the agent thought: "Thought", "Thought %n times", or the preview.
    Thought {
        preview: String,
        count: u16,
        has_text: bool,
        open: bool,
    },
    /// A finished turn's fold: "Worked for %1"; `hidden` rows behind it.
    Fold {
        worked_ms: i64,
        hidden: u16,
        open: bool,
    },
    /// The files the turn changed: "%n files changed", +adds −dels.
    Changes {
        files: Vec<AgentFileStat>,
        adds: u32,
        dels: u32,
        more: u16,
    },
    /// A request's state under its step.
    Mark {
        kind: AgentMarkKind,
        by: String,
    },
    Notice {
        text: String,
    },
    /// The head of a plan's card ("Plan"); `shown` of its blocks follow as
    /// `Block` rows, and `more` are left out until it's opened (its key).
    /// A `Replaced` one shows this row only until opened.
    Plan {
        state: AgentPlanState,
        shown: u16,
        more: u16,
        open: bool,
    },
    /// The agent's questions, one line: "Asked you a question" / "Asked you
    /// %n questions" (`count`, 0 when not known), "You answered", "Answered
    /// on %1" (`by`), "Not answered". Opened, the answers follow as `Block`
    /// rows when the item has them (`has_text`).
    Question {
        state: AgentQuestionState,
        count: u8,
        by: String,
        has_text: bool,
        open: bool,
    },
    /// The working line, last: "Starting…" or "Thinking", with the thought
    /// so far when there is one, since `since_ms`.
    Live {
        label: AgentLiveLabel,
        thought: String,
        since_ms: i64,
    },
}

/// Names the row kind only.
impl std::fmt::Debug for AgentRowKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::User { .. } => "User",
            Self::Block { .. } => "Block",
            Self::Tool { .. } => "Tool",
            Self::Detail { .. } => "Detail",
            Self::Group { .. } => "Group",
            Self::Thought { .. } => "Thought",
            Self::Fold { .. } => "Fold",
            Self::Changes { .. } => "Changes",
            Self::Mark { .. } => "Mark",
            Self::Notice { .. } => "Notice",
            Self::Plan { .. } => "Plan",
            Self::Question { .. } => "Question",
            Self::Live { .. } => "Live",
        })
    }
}

/// Where a question stands, for its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentQuestionState {
    /// Waiting for the user.
    Asked,
    Answered,
    /// The user chose not to answer.
    Dismissed,
    /// It ended unanswered: it lapsed, or its turn ended.
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentRowStatus {
    Running,
    Ok,
    Failed,
    Denied,
    /// Was running when its turn ended.
    Stopped,
    /// Waits for an answer to its request: "Wants to run %1".
    Asking,
}

/// What an open tool row, or an open file of a `Changes` row, shows under it.
#[derive(Clone, PartialEq, Eq, uniffi::Enum)]
pub enum AgentDetailOf {
    Command {
        text: String,
    },
    /// An edit. Under a `Changes` file: edit `edit` of `edits`, from 1 ("Edit
    /// %1 of %2" when `edits` > 1); both 0 under a tool row.
    Diff {
        doc: AgentDiffDoc,
        edit: u16,
        edits: u16,
    },
    /// The tail of what it printed; `cut`: the start was left out.
    Output {
        text: String,
        cut: bool,
    },
}

impl std::fmt::Debug for AgentDetailOf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command { .. } => f.write_str("Command"),
            Self::Diff { doc, edit, edits } => {
                f.debug_struct("Diff").field("doc", doc).field("edit", edit).field("edits", edits).finish()
            }
            Self::Output { cut, .. } => f.debug_struct("Output").field("cut", cut).finish_non_exhaustive(),
        }
    }
}

/// A step's label: its class and what it acted on ("Ran %1", "Edited %1"),
/// one line; `more` further files ("%1 and %n more").
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentToolLabel {
    pub tool_class: AgentToolClass,
    pub target: String,
    pub more: u16,
}

quiet_debug!(AgentToolLabel { tool_class, more });

/// A group's line: its first one or two clauses and how many other actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AgentSummary {
    pub first: AgentClause,
    pub second: Option<AgentClause>,
    pub others: u16,
}

/// "Ran %n commands", "Changed %n files", "Read %n files", "Searched %n
/// times", "Searched the web %n times", "Used %n tools" (`Other`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AgentClause {
    pub tool_class: AgentToolClass,
    pub count: u16,
}

/// A file the turn changed. Its key is `"c:<turn>:<index>"` (index from 0
/// in `files`); while that key is open, the row is followed by the turn's
/// edits to the file as `Detail` rows ("Edit %1 of %2").
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentFileStat {
    pub path: String,
    pub adds: u32,
    pub dels: u32,
    pub open: bool,
}

quiet_debug!(AgentFileStat { adds, dels, open });

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentLiveLabel {
    /// Nothing has come since your message: "Starting…".
    Starting,
    Thinking,
}

// ---- From the render crate ----

impl From<render::Block> for AgentBlock {
    fn from(b: render::Block) -> Self {
        AgentBlock {
            kind: b.kind.into(),
            depth: b.depth,
            quote: b.quote,
            marker: b.marker.into(),
            text: b.text,
            spans: b.spans.into_iter().map(Into::into).collect(),
            lang: b.lang,
            lines: b.lines.into_iter().map(Into::into).collect(),
            table: b.table.map(Into::into),
            open: b.open,
        }
    }
}

impl From<render::BlockKind> for AgentBlockKind {
    fn from(kind: render::BlockKind) -> Self {
        match kind {
            render::BlockKind::Paragraph => Self::Paragraph,
            render::BlockKind::Heading(level) => Self::Heading { level },
            render::BlockKind::ListItem => Self::ListItem,
            render::BlockKind::Code => Self::Code,
            render::BlockKind::Table => Self::Table,
            render::BlockKind::Rule => Self::Rule,
            render::BlockKind::Alert(alert) => Self::Alert { alert: alert.into() },
        }
    }
}

impl From<render::AlertKind> for AgentAlertKind {
    fn from(kind: render::AlertKind) -> Self {
        match kind {
            render::AlertKind::Note => Self::Note,
            render::AlertKind::Tip => Self::Tip,
            render::AlertKind::Important => Self::Important,
            render::AlertKind::Warning => Self::Warning,
            render::AlertKind::Caution => Self::Caution,
        }
    }
}

impl From<render::Marker> for AgentMarker {
    fn from(marker: render::Marker) -> Self {
        match marker {
            render::Marker::None => Self::None,
            render::Marker::Bullet => Self::Bullet,
            render::Marker::Number(number) => Self::Number { number },
            render::Marker::Todo => Self::Todo,
            render::Marker::Done => Self::Done,
        }
    }
}

impl From<render::Span> for AgentSpan {
    fn from(span: render::Span) -> Self {
        AgentSpan { start: span.start, len: span.len, style: span.style.into(), href: span.href }
    }
}

impl From<render::Style> for AgentStyle {
    fn from(style: render::Style) -> Self {
        AgentStyle {
            strong: style.strong,
            em: style.em,
            strike: style.strike,
            code: style.code,
            link: match style.link {
                render::Link::None => AgentLink::None,
                render::Link::Web => AgentLink::Web,
                render::Link::Mail => AgentLink::Mail,
                render::Link::Shown => AgentLink::Shown,
            },
            image: style.image,
        }
    }
}

impl From<render::Table> for AgentTable {
    fn from(table: render::Table) -> Self {
        AgentTable {
            columns: table.columns,
            align: table
                .align
                .into_iter()
                .map(|align| match align {
                    render::Align::None => AgentAlign::None,
                    render::Align::Left => AgentAlign::Left,
                    render::Align::Center => AgentAlign::Center,
                    render::Align::Right => AgentAlign::Right,
                })
                .collect(),
            cells: table
                .cells
                .into_iter()
                .map(|cell| AgentCell {
                    text: cell.text,
                    spans: cell.spans.into_iter().map(Into::into).collect(),
                    header: cell.header,
                })
                .collect(),
            more_rows: table.more_rows,
        }
    }
}

impl From<render::CodeLine> for AgentCodeLine {
    fn from(line: render::CodeLine) -> Self {
        AgentCodeLine {
            text: line.text,
            toks: line
                .toks
                .into_iter()
                .map(|tok| AgentTok { len: tok.len, tok_class: tok.class.into() })
                .collect(),
        }
    }
}

impl From<render::TokClass> for AgentTokClass {
    fn from(class: render::TokClass) -> Self {
        use render::TokClass as T;
        match class {
            T::Plain => Self::Plain,
            T::Comment => Self::Comment,
            T::Keyword => Self::Keyword,
            T::String => Self::String,
            T::Number => Self::Number,
            T::Constant => Self::Constant,
            T::Type => Self::Type,
            T::Function => Self::Function,
            T::Variable => Self::Variable,
            T::Operator => Self::Operator,
            T::Punctuation => Self::Punctuation,
            T::Tag => Self::Tag,
            T::Attribute => Self::Attribute,
            T::Inserted => Self::Inserted,
            T::Deleted => Self::Deleted,
        }
    }
}

impl From<render::DiffDoc> for AgentDiffDoc {
    fn from(doc: render::DiffDoc) -> Self {
        AgentDiffDoc {
            files: doc
                .files
                .into_iter()
                .map(|file| AgentDiffFile {
                    path: file.path,
                    adds: file.adds,
                    dels: file.dels,
                    rows: file.rows.into_iter().map(Into::into).collect(),
                })
                .collect(),
            adds: doc.adds,
            dels: doc.dels,
            more: doc.more,
        }
    }
}

impl From<render::DiffRow> for AgentDiffRow {
    fn from(row: render::DiffRow) -> Self {
        AgentDiffRow {
            kind: match row.kind {
                render::DiffKind::Hunk => AgentDiffKind::Hunk,
                render::DiffKind::Context => AgentDiffKind::Context,
                render::DiffKind::Add => AgentDiffKind::Add,
                render::DiffKind::Del => AgentDiffKind::Del,
            },
            old_no: row.old_no,
            new_no: row.new_no,
            text: row.text,
            marks: row.marks.into_iter().map(|(start, len)| AgentDiffMark { start, len }).collect(),
        }
    }
}

/// An edit's text read as a diff, for the request sheet and the full-screen
/// diff: a git diff, a hunk list, or lines that start with `+`, `-` or a
/// space. Line numbers are 0 when the text has no hunk headers.
#[uniffi::export]
pub fn agent_diff(text: String) -> AgentDiffDoc {
    render::diff(&text).into()
}

// ---- The typing guard ----

/// Holds the requests that arrive while you type, so a keystroke never
/// answers one (`TYPING_IDLE_MS`, 1.5 s). One per thread screen. A held
/// request shows "Arrived while you were typing" with its buttons off, and
/// its mark is `Held`.
#[derive(uniffi::Object)]
pub struct AgentTypingGuard {
    guard: Mutex<render::TypingGuard>,
}

impl std::fmt::Debug for AgentTypingGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AgentTypingGuard").field(&*self.lock()).finish()
    }
}

impl AgentTypingGuard {
    fn lock(&self) -> MutexGuard<'_, render::TypingGuard> {
        self.guard.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn on(&self, event: render::GuardEvent<'_>) {
        self.lock().on(event);
    }
}

#[uniffi::export]
impl AgentTypingGuard {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self { guard: Mutex::new(render::TypingGuard::new()) })
    }

    /// The tokens of the thread's waiting requests, whenever that set changes.
    pub fn requests(&self, ids: Vec<String>, now_ms: i64) {
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        self.on(render::GuardEvent::Requests { ids: &ids, now_ms: unix_ms(now_ms) });
    }

    /// The composer's text changed by the user's typing (not when it's set).
    pub fn typed(&self, now_ms: i64) {
        self.on(render::GuardEvent::Typed { now_ms: unix_ms(now_ms) });
    }

    pub fn focus(&self) {
        self.on(render::GuardEvent::Focus);
    }

    pub fn blur(&self) {
        self.on(render::GuardEvent::Blur);
    }

    /// The user sent a message, or denied with one.
    pub fn sent(&self) {
        self.on(render::GuardEvent::Sent);
    }

    /// Call at `idle_at_ms()`: the typing paused, so held requests go free.
    pub fn idle(&self) {
        self.on(render::GuardEvent::Idle);
    }

    /// Whether the request with this token is held.
    pub fn held(&self, id: String) -> bool {
        self.lock().held(&id)
    }

    /// When to call `idle()` while anything is held; null when nothing is.
    /// Ask again after every `typed`.
    pub fn idle_at_ms(&self) -> Option<i64> {
        self.lock().idle_at().map(signed_ms)
    }
}

// ---- The timeline ----

impl From<render::Row> for AgentRow {
    fn from(row: render::Row) -> Self {
        AgentRow {
            key: row.key,
            kind: row.kind.into(),
            turn: row.turn,
            depth: row.depth,
            item_id: row.item_id,
            seq: row.seq,
        }
    }
}

impl From<render::RowKind> for AgentRowKind {
    fn from(kind: render::RowKind) -> Self {
        use render::RowKind as K;
        match kind {
            K::User { text, from, deny_of, via } => Self::User {
                text,
                from,
                deny_of,
                via: match via {
                    render::Via::Turn => None,
                    render::Via::Steer => Some(AgentVia::Steer),
                    render::Via::Queue => Some(AgentVia::Queue),
                },
            },
            K::Block { block, index, last, phase } => {
                Self::Block { block: block.into(), index, last, phase: phase.map(Into::into) }
            }
            K::Tool { label, status, has_detail, has_output, open, adds, dels, ms } => Self::Tool {
                label: label.into(),
                status: status.into(),
                has_detail,
                has_output,
                open,
                adds,
                dels,
                ms: signed_ms(ms),
            },
            K::Detail { of } => Self::Detail { of: of.into() },
            K::Group { summary, count, failed, open } => {
                Self::Group { summary: summary.into(), count, failed, open }
            }
            K::Thought { preview, count, has_text, open } => Self::Thought { preview, count, has_text, open },
            K::Fold { worked_ms, hidden, open } => {
                Self::Fold { worked_ms: signed_ms(worked_ms), hidden, open }
            }
            K::Changes { files, adds, dels, more } => Self::Changes {
                files: files
                    .into_iter()
                    .map(|f| AgentFileStat { path: f.path, adds: f.adds, dels: f.dels, open: f.open })
                    .collect(),
                adds,
                dels,
                more,
            },
            K::Mark { kind, by } => Self::Mark { kind: kind.into(), by },
            K::Notice { text } => Self::Notice { text },
            K::Plan { state, shown, more, open } => Self::Plan { state: state.into(), shown, more, open },
            K::Question { state, count, by, has_text, open } => {
                Self::Question { state: state.into(), count, by, has_text, open }
            }
            K::Live { label, thought, since_ms } => Self::Live {
                label: match label {
                    render::LiveLabel::Starting => AgentLiveLabel::Starting,
                    render::LiveLabel::Thinking => AgentLiveLabel::Thinking,
                },
                thought,
                since_ms: signed_ms(since_ms),
            },
        }
    }
}

impl From<render::DetailOf> for AgentDetailOf {
    fn from(of: render::DetailOf) -> Self {
        match of {
            render::DetailOf::Command { text } => Self::Command { text },
            render::DetailOf::Diff { doc, edit, edits } => Self::Diff { doc: doc.into(), edit, edits },
            render::DetailOf::Output { text, cut } => Self::Output { text, cut },
        }
    }
}

impl From<render::RowStatus> for AgentRowStatus {
    fn from(status: render::RowStatus) -> Self {
        match status {
            render::RowStatus::Running => Self::Running,
            render::RowStatus::Asking => Self::Asking,
            render::RowStatus::Ok => Self::Ok,
            render::RowStatus::Failed => Self::Failed,
            render::RowStatus::Denied => Self::Denied,
            render::RowStatus::Stopped => Self::Stopped,
        }
    }
}

impl From<render::ToolLabel> for AgentToolLabel {
    fn from(label: render::ToolLabel) -> Self {
        AgentToolLabel { tool_class: label.class.into(), target: label.object, more: label.more }
    }
}

impl From<render::Summary> for AgentSummary {
    fn from(summary: render::Summary) -> Self {
        AgentSummary {
            first: summary.first.into(),
            second: summary.second.map(Into::into),
            others: summary.others,
        }
    }
}

impl From<render::Clause> for AgentClause {
    fn from(clause: render::Clause) -> Self {
        AgentClause { tool_class: clause.class.into(), count: clause.count }
    }
}

impl From<render::MarkKind> for AgentMarkKind {
    fn from(kind: render::MarkKind) -> Self {
        use render::MarkKind as M;
        match kind {
            M::Waiting => Self::Waiting,
            M::Held => Self::Held,
            M::Allowed => Self::Allowed,
            M::Denied => Self::Denied,
            M::Expired => Self::Expired,
            M::Cancelled => Self::Cancelled,
            M::Rule => Self::Rule,
        }
    }
}

impl From<AgentMarkKind> for render::MarkKind {
    fn from(kind: AgentMarkKind) -> Self {
        use render::MarkKind as M;
        match kind {
            AgentMarkKind::Waiting => M::Waiting,
            AgentMarkKind::Held => M::Held,
            AgentMarkKind::Allowed => M::Allowed,
            AgentMarkKind::Denied => M::Denied,
            AgentMarkKind::Expired => M::Expired,
            AgentMarkKind::Cancelled => M::Cancelled,
            AgentMarkKind::Rule => M::Rule,
        }
    }
}

impl From<render::Phase> for AgentItemPhase {
    fn from(phase: render::Phase) -> Self {
        match phase {
            render::Phase::Interim => Self::Interim,
            render::Phase::Final => Self::Final,
        }
    }
}

impl From<AgentItemPhase> for render::Phase {
    fn from(phase: AgentItemPhase) -> Self {
        match phase {
            AgentItemPhase::Interim => Self::Interim,
            AgentItemPhase::Final => Self::Final,
        }
    }
}

impl From<render::ToolClass> for AgentToolClass {
    fn from(class: render::ToolClass) -> Self {
        use render::ToolClass as C;
        match class {
            C::Command => Self::Command,
            C::Edit => Self::Edit,
            C::Read => Self::Read,
            C::Search => Self::Search,
            C::Web => Self::Web,
            C::Mcp => Self::Mcp,
            C::Agent => Self::Agent,
            C::Tasks => Self::Tasks,
            C::Other => Self::Other,
            C::Plan => Self::Plan,
        }
    }
}

impl From<AgentToolClass> for render::ToolClass {
    fn from(class: AgentToolClass) -> Self {
        use render::ToolClass as C;
        match class {
            AgentToolClass::Command => C::Command,
            AgentToolClass::Edit => C::Edit,
            AgentToolClass::Read => C::Read,
            AgentToolClass::Search => C::Search,
            AgentToolClass::Web => C::Web,
            AgentToolClass::Mcp => C::Mcp,
            AgentToolClass::Agent => C::Agent,
            AgentToolClass::Tasks => C::Tasks,
            AgentToolClass::Other => C::Other,
            AgentToolClass::Plan => C::Plan,
        }
    }
}

impl From<render::PlanState> for AgentPlanState {
    fn from(state: render::PlanState) -> Self {
        match state {
            render::PlanState::Proposed => Self::Proposed,
            render::PlanState::Implemented => Self::Implemented,
            render::PlanState::Replaced => Self::Replaced,
        }
    }
}

impl From<AgentPlanState> for render::PlanState {
    fn from(state: AgentPlanState) -> Self {
        match state {
            AgentPlanState::Proposed => Self::Proposed,
            AgentPlanState::Implemented => Self::Implemented,
            AgentPlanState::Replaced => Self::Replaced,
        }
    }
}

impl From<render::QuestionState> for AgentQuestionState {
    fn from(state: render::QuestionState) -> Self {
        match state {
            render::QuestionState::Asked => Self::Asked,
            render::QuestionState::Answered => Self::Answered,
            render::QuestionState::Dismissed => Self::Dismissed,
            render::QuestionState::Expired => Self::Expired,
        }
    }
}

fn via(via: Option<AgentVia>) -> render::Via {
    match via {
        None => render::Via::Turn,
        Some(AgentVia::Steer) => render::Via::Steer,
        Some(AgentVia::Queue) => render::Via::Queue,
    }
}

fn item_kind(kind: AgentItemKind) -> render::ItemKind {
    match kind {
        AgentItemKind::User => render::ItemKind::User,
        AgentItemKind::Assistant => render::ItemKind::Assistant,
        AgentItemKind::Tool => render::ItemKind::Tool,
        AgentItemKind::Notice => render::ItemKind::Notice,
        AgentItemKind::Reasoning => render::ItemKind::Reasoning,
        AgentItemKind::Plan => render::ItemKind::Plan,
        AgentItemKind::Question => render::ItemKind::Question,
    }
}

fn tool_status(status: AgentToolStatus) -> render::ToolStatus {
    match status {
        AgentToolStatus::Running => render::ToolStatus::Running,
        AgentToolStatus::Ok => render::ToolStatus::Ok,
        AgentToolStatus::Error => render::ToolStatus::Error,
        AgentToolStatus::Denied => render::ToolStatus::Denied,
    }
}

fn thread_state(state: AgentThreadState) -> render::ThreadState {
    match state {
        AgentThreadState::Working => render::ThreadState::Working,
        AgentThreadState::Waiting => render::ThreadState::Waiting,
        AgentThreadState::Done => render::ThreadState::Done,
        AgentThreadState::Failed => render::ThreadState::Failed,
        AgentThreadState::Interrupted => render::ThreadState::Interrupted,
    }
}

/// A stand-in for the revision the wire doesn't carry: it changes when an
/// item's text or detail is replaced by another of the same length, so the
/// timeline's cache (keyed by revision and length) never shows the old one.
/// Reads at most 256 bytes of the two, whatever their size.
fn revision(text: &str, detail: &str) -> u32 {
    const EDGE: usize = 64;
    let mut hash: u32 = 0x811c_9dc5;
    for part in [text, detail] {
        let bytes = part.as_bytes();
        let head = &bytes[..bytes.len().min(EDGE)];
        let tail = &bytes[bytes.len().saturating_sub(EDGE)..];
        for &b in head.iter().chain(tail) {
            hash = (hash ^ u32::from(b)).wrapping_mul(0x0100_0193);
        }
        hash = (hash ^ 0xff).wrapping_mul(0x0100_0193);
    }
    hash
}

// ---- The composer's `@` and `/` ----

/// What the text at the cursor asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentTriggerKind {
    /// An `@` at the start of the text or after a space: files of the
    /// folder (`agent_files` with `query`).
    Mention,
    /// A `/` as the text's first character: a command (`agent_commands`,
    /// filtered by `query`).
    Slash,
}

/// A trigger at the cursor: `start` is where its `@` or `/` is, in UTF-16
/// units; `query` what follows it up to the cursor.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentTrigger {
    pub kind: AgentTriggerKind,
    pub start: u32,
    pub query: String,
}

quiet_debug!(AgentTrigger { kind, start });

/// The text after a pick, and where the cursor goes (UTF-16 units).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentTriggerApplied {
    pub text: String,
    pub cursor: u32,
}

quiet_debug!(AgentTriggerApplied { cursor });

/// An `@path` in the text, for a chip: from `start`, `length` UTF-16 units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AgentMention {
    pub start: u32,
    pub length: u32,
}

impl From<render::Trigger> for AgentTrigger {
    fn from(t: render::Trigger) -> Self {
        let kind = match t.kind {
            render::TriggerKind::Mention => AgentTriggerKind::Mention,
            render::TriggerKind::Slash => AgentTriggerKind::Slash,
        };
        AgentTrigger { kind, start: t.start, query: t.query }
    }
}

/// The trigger at `cursor` (UTF-16 units) in the composer's `text`, if any:
/// call it on each change, and open or close the `@` or `/` list.
#[uniffi::export]
pub fn agent_trigger(text: String, cursor: u32) -> Option<AgentTrigger> {
    render::trigger(&text, cursor).map(Into::into)
}

/// `text` with `trigger`'s run up to `cursor` replaced by the pick
/// (`insert`: a path relative to the folder, or a command's name, bare), and
/// where the cursor goes. A path with spaces is written `@"the path"`; a
/// directory (ending in `/`) leaves the cursor after it so the list opens again.
#[uniffi::export]
pub fn agent_trigger_apply(
    text: String,
    trigger: AgentTrigger,
    cursor: u32,
    insert: String,
) -> AgentTriggerApplied {
    let kind = match trigger.kind {
        AgentTriggerKind::Mention => render::TriggerKind::Mention,
        AgentTriggerKind::Slash => render::TriggerKind::Slash,
    };
    let trigger = render::Trigger { kind, start: trigger.start, query: trigger.query };
    let (text, cursor) = render::apply(&text, &trigger, cursor, &insert);
    AgentTriggerApplied { text, cursor }
}

/// The `@path` mentions of `text`, in order, for chips.
#[uniffi::export]
pub fn agent_mentions(text: String) -> Vec<AgentMention> {
    render::mentions(&text).into_iter().map(|(start, length)| AgentMention { start, length }).collect()
}

/// One thread's rows, with the parsed answers kept between calls so a
/// finished answer isn't read again. One per open thread; drop it when the
/// thread closes.
#[derive(uniffi::Object)]
pub struct AgentTimeline {
    timeline: Mutex<render::Timeline>,
}

impl std::fmt::Debug for AgentTimeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTimeline").finish_non_exhaustive()
    }
}

#[uniffi::export]
impl AgentTimeline {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self { timeline: Mutex::new(render::Timeline::new()) })
    }

    /// The thread's rows, top to bottom. Never call it on the main thread:
    /// a long thread takes a while.
    ///
    /// `items`: the thread's items as read and updated. `streaming`: item
    /// ids whose text is still arriving, with the text so far (it wins over
    /// the item's own text; an entry whose item hasn't come yet is shown
    /// after the item before it). `state` and `state_since_ms`: the thread's
    /// state and since when (`AgentThread.since_ms`). `marks`: the requests
    /// at their steps; a step with `by_rule` gets the `Rule` mark by itself,
    /// in place of an `Allowed` passed for it. A `Question` item's state is
    /// its `question`'s; a mark on it wins (`Waiting` asked, `Allowed`
    /// answered by `by`, `Denied` dismissed, `Expired` not answered). `open`:
    /// the keys of the rows the user opened.
    #[allow(clippy::too_many_arguments)]
    pub fn rows(
        &self,
        items: Vec<AgentItem>,
        streaming: Vec<AgentStreaming>,
        state: AgentThreadState,
        state_since_ms: i64,
        now_ms: i64,
        marks: Vec<AgentMark>,
        open: Vec<String>,
    ) -> Vec<AgentRow> {
        let inputs = inputs(&items, &streaming);
        let marks = marks_for(&items, marks);
        let ctx = render::Ctx {
            state: thread_state(state),
            state_since_ms: unix_ms(state_since_ms),
            now_ms: unix_ms(now_ms),
            marks: &marks,
            open: &open,
        };
        let rows = self.timeline.lock().unwrap_or_else(|e| e.into_inner()).rows(&inputs, &ctx);
        rows.into_iter().map(Into::into).collect()
    }
}

/// `marks` for the timeline. A step the PC's rule allowed shows the `Rule`
/// mark, which says more than "Allowed on <device>": an `Allowed` mark for
/// it is left out so the timeline gives it that one.
fn marks_for(items: &[AgentItem], marks: Vec<AgentMark>) -> Vec<render::Mark> {
    let by_rule =
        |id: &str| items.iter().any(|item| item.id == id && item.tool.as_ref().is_some_and(|t| t.by_rule));
    marks
        .into_iter()
        .filter(|m| !(m.kind == AgentMarkKind::Allowed && by_rule(&m.item_id)))
        .map(|m| render::Mark { item_id: m.item_id, kind: m.kind.into(), by: m.by })
        .collect()
}

/// What a `Question` item says of itself, so a thread opened again reads as
/// it ended: its `question` (the host's count and outcome), or, from a PC
/// that doesn't send it, answered when it has the answers as text. A mark
/// on the item wins over either.
fn question_in<'a>(item: &'a AgentItem, text: &str) -> Option<render::QuestionIn<'a>> {
    if item.kind != AgentItemKind::Question {
        return None;
    }
    let Some(q) = &item.question else {
        return (!text.trim().is_empty()).then_some(render::QuestionIn {
            state: render::QuestionState::Answered,
            count: 0,
            by: "",
        });
    };
    let state = match q.outcome {
        None => render::QuestionState::Asked,
        Some(AgentQuestionOutcome::Answered) => render::QuestionState::Answered,
        Some(AgentQuestionOutcome::Dismissed) => render::QuestionState::Dismissed,
        Some(AgentQuestionOutcome::Expired | AgentQuestionOutcome::Cancelled) => {
            render::QuestionState::Expired
        }
    };
    Some(render::QuestionIn { state, count: q.count, by: q.by_name.as_deref().unwrap_or("") })
}

/// The timeline's view of `items` and the text still arriving, in `seq` order.
fn inputs<'a>(items: &'a [AgentItem], streaming: &'a [AgentStreaming]) -> Vec<render::ItemIn<'a>> {
    let live: HashMap<&str, &AgentStreaming> = streaming.iter().map(|s| (s.item_id.as_str(), s)).collect();
    let mut inputs: Vec<render::ItemIn<'a>> = items
        .iter()
        .map(|item| {
            let arriving = live.get(item.id.as_str()).copied();
            let text = arriving.map_or(item.text.as_deref().unwrap_or(""), |s| s.text.as_str());
            let tool = item.tool.as_ref().map(|t| render::ToolIn {
                name: &t.name,
                title: &t.title,
                class: t.tool_class.map_or(render::ToolClass::Other, Into::into),
                detail: t.detail.as_deref().unwrap_or(""),
                path: t.path.as_deref().unwrap_or(""),
                status: tool_status(t.status),
                output: t.output.as_deref().unwrap_or(""),
                by_rule: t.by_rule,
            });
            render::ItemIn {
                seq: item.seq,
                id: &item.id,
                kind: item_kind(item.kind),
                ts_ms: unix_ms(item.when_ms),
                end_ms: item.end_ms.map(unix_ms),
                rev: revision(text, tool.as_ref().map_or("", |t| t.detail)),
                text,
                streaming: arriving.is_some(),
                turn: item.turn,
                group: item.group.as_deref(),
                phase: item.phase.map(Into::into),
                from: "",
                deny_of: item.deny_of.as_deref(),
                tool,
                truncated: item.truncated,
                via: via(item.via),
                plan_state: item.plan_state.map(Into::into),
                question: question_in(item, text),
            }
        })
        .collect();
    inputs.sort_by_key(|input| input.seq);
    for s in streaming {
        if items.iter().any(|item| item.id == s.item_id) {
            continue;
        }
        let (ts_ms, turn, group) = inputs
            .iter()
            .rev()
            .find(|input| input.seq < s.seq)
            .map_or((0, 0, None), |before| (before.ts_ms, before.turn, before.group));
        inputs.push(render::ItemIn {
            seq: s.seq,
            id: &s.item_id,
            kind: item_kind(s.kind),
            ts_ms,
            end_ms: None,
            rev: revision(&s.text, ""),
            text: &s.text,
            streaming: true,
            turn,
            group,
            phase: None,
            from: "",
            deny_of: None,
            tool: None,
            truncated: false,
            via: render::Via::Turn,
            plan_state: None,
            question: None,
        });
    }
    inputs.sort_by_key(|input| input.seq);
    inputs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentTool;

    fn item(seq: u64, kind: AgentItemKind, text: &str) -> AgentItem {
        AgentItem {
            seq,
            id: format!("i{seq}"),
            kind,
            when_ms: 1_000 * seq as i64,
            text: (!text.is_empty()).then(|| text.to_owned()),
            truncated: false,
            tool: None,
            turn: 1,
            phase: None,
            end_ms: None,
            group: None,
            deny_of: None,
            via: None,
            images: Vec::new(),
            plan_state: None,
            question: None,
        }
    }

    /// A finished turn: the prompt, a command that ran, a denied edit and
    /// the answer in two paragraphs.
    fn finished_turn() -> Vec<AgentItem> {
        let tool = |seq, class, detail: &str, status| AgentItem {
            tool: Some(AgentTool {
                name: "Bash".into(),
                title: "Run command".into(),
                detail: Some(detail.into()),
                status,
                output: None,
                tool_class: Some(class),
                path: (class == AgentToolClass::Edit).then(|| "src/lib.rs".into()),
                by_rule: false,
            }),
            end_ms: Some(1_000 * seq as i64 + 2_500),
            ..item(seq, AgentItemKind::Tool, "")
        };
        vec![
            item(1, AgentItemKind::User, "Fix the test."),
            tool(2, AgentToolClass::Command, "cargo test", AgentToolStatus::Ok),
            tool(3, AgentToolClass::Edit, "-a\n+b\n+c", AgentToolStatus::Ok),
            AgentItem {
                phase: Some(AgentItemPhase::Final),
                end_ms: Some(9_000),
                ..item(4, AgentItemKind::Assistant, "Fixed.\n\nAll **green**.")
            },
        ]
    }

    fn keys(rows: &[AgentRow]) -> Vec<&str> {
        rows.iter().map(|row| row.key.as_str()).collect()
    }

    #[test]
    fn a_finished_turn_folds_behind_its_answer_as_the_render_crate_says() {
        let timeline = AgentTimeline::new();
        let rows = |open: Vec<String>| {
            timeline.rows(finished_turn(), vec![], AgentThreadState::Done, 9_000, 10_000, vec![], open)
        };
        let closed = rows(vec![]);
        assert_eq!(keys(&closed), ["i:i1", "f:1", "b:i4:0", "b:i4:1", "c:1"]);
        match &closed[1].kind {
            AgentRowKind::Fold { worked_ms, hidden, open } => {
                assert_eq!((*worked_ms, *open), (8_000, false));
                assert!(*hidden >= 1);
            }
            other => panic!("{other:?}"),
        }
        match &closed[3].kind {
            AgentRowKind::Block { block, index, last, phase } => {
                assert_eq!((*index, *last, *phase), (1, true, Some(AgentItemPhase::Final)));
                assert_eq!(block.text, "All green.");
                assert_eq!(block.spans.len(), 1);
                assert!(block.spans[0].style.strong);
            }
            other => panic!("{other:?}"),
        }
        match &closed[4].kind {
            AgentRowKind::Changes { files, adds, dels, .. } => {
                assert_eq!((files.len(), *adds, *dels), (1, 2, 1));
                assert_eq!(files[0].path, "src/lib.rs");
            }
            other => panic!("{other:?}"),
        }

        // Opened, the two steps show inside it as a group, and opened, the
        // group shows them one deeper.
        let open = rows(vec!["f:1".into()]);
        assert_eq!(keys(&open), ["i:i1", "f:1", "g:i2", "b:i4:0", "b:i4:1", "c:1"]);
        assert!(matches!(open[2].kind, AgentRowKind::Group { count: 2, failed: false, .. }));
        let open = rows(vec!["f:1".into(), "g:i2".into()]);
        let tool = open.iter().find(|row| row.key == "i:i3").expect("the edit inside the group");
        assert_eq!(tool.depth, 2);
        match &tool.kind {
            AgentRowKind::Tool { label, status, adds, dels, ms, .. } => {
                assert_eq!((label.tool_class, label.target.as_str()), (AgentToolClass::Edit, "src/lib.rs"));
                assert_eq!((*status, *adds, *dels, *ms), (AgentRowStatus::Ok, 2, 1, 2_500));
            }
            other => panic!("{other:?}"),
        }

        // The rows are the render crate's, field for field.
        let items = finished_turn();
        let direct = render::Timeline::new().rows(
            &inputs(&items, &[]),
            &render::Ctx {
                state: render::ThreadState::Done,
                state_since_ms: 9_000,
                now_ms: 10_000,
                marks: &[],
                open: &["f:1".to_owned(), "g:i2".to_owned()],
            },
        );
        assert_eq!(open, direct.into_iter().map(AgentRow::from).collect::<Vec<_>>());
    }

    #[test]
    fn text_still_arriving_shows_as_an_open_block_and_the_live_line_waits_for_it() {
        let timeline = AgentTimeline::new();
        let prompt = vec![item(1, AgentItemKind::User, "Fix the test.")];
        let starting =
            timeline.rows(prompt.clone(), vec![], AgentThreadState::Working, 1_000, 3_000, vec![], vec![]);
        assert_eq!(keys(&starting), ["i:i1", "live"]);
        assert!(matches!(
            starting[1].kind,
            AgentRowKind::Live { label: AgentLiveLabel::Starting, since_ms: 1_000, .. }
        ));

        let arriving = |text: &str| AgentStreaming {
            item_id: "m2".into(),
            seq: 2,
            kind: AgentItemKind::Assistant,
            text: text.into(),
        };
        let rows = timeline.rows(
            prompt.clone(),
            vec![arriving("First.\n\nSecond, so f")],
            AgentThreadState::Working,
            1_000,
            3_000,
            vec![],
            vec![],
        );
        assert_eq!(keys(&rows), ["i:i1", "b:m2:0", "b:m2:1"], "no live line while text arrives");
        match &rows[2].kind {
            AgentRowKind::Block { block, .. } => assert!(block.open),
            other => panic!("{other:?}"),
        }
        assert_eq!(rows[2].turn, 1, "it takes the turn of the item before it");

        // A request waiting on a step: the step asks, and no live line.
        let mut items = prompt;
        items.push(AgentItem {
            tool: Some(AgentTool {
                name: "Bash".into(),
                title: "Run command".into(),
                detail: Some("rm -rf target".into()),
                status: AgentToolStatus::Running,
                output: None,
                tool_class: Some(AgentToolClass::Command),
                path: None,
                by_rule: false,
            }),
            ..item(2, AgentItemKind::Tool, "")
        });
        let marks = vec![AgentMark { item_id: "i2".into(), kind: AgentMarkKind::Waiting, by: String::new() }];
        let rows = timeline.rows(items, vec![], AgentThreadState::Waiting, 1_000, 3_000, marks, vec![]);
        assert_eq!(keys(&rows), ["i:i1", "i:i2", "m:i2"]);
        assert!(matches!(rows[1].kind, AgentRowKind::Tool { status: AgentRowStatus::Asking, .. }));
        assert!(matches!(rows[2].kind, AgentRowKind::Mark { kind: AgentMarkKind::Waiting, .. }));
    }

    #[test]
    fn a_step_the_pcs_rule_allowed_says_so_rather_than_where() {
        let allowed_on_pc = vec![
            AgentMark { item_id: "i2".into(), kind: AgentMarkKind::Allowed, by: "Desktop".into() },
            AgentMark { item_id: "i3".into(), kind: AgentMarkKind::Allowed, by: "Desktop".into() },
        ];
        let open = vec!["f:1".into(), "g:i2".into()];
        let mut items = finished_turn();
        if let Some(tool) = items[1].tool.as_mut() {
            tool.by_rule = true;
        }
        let rows = AgentTimeline::new().rows(
            items,
            vec![],
            AgentThreadState::Done,
            9_000,
            10_000,
            allowed_on_pc,
            open,
        );
        let marks: Vec<(&str, AgentMarkKind)> = rows
            .iter()
            .filter_map(|row| match &row.kind {
                AgentRowKind::Mark { kind, .. } => Some((row.key.as_str(), *kind)),
                _ => None,
            })
            .collect();
        assert_eq!(marks, [("m:i2", AgentMarkKind::Rule), ("m:i3", AgentMarkKind::Allowed)]);
    }

    #[test]
    fn plans_questions_and_steered_messages_have_rows_of_their_own() {
        let mut steered = item(1, AgentItemKind::User, "And the logout.");
        steered.via = Some(AgentVia::Steer);
        let plan_file = AgentItem {
            tool: Some(AgentTool {
                name: "plan".into(),
                title: "Plan".into(),
                detail: None,
                status: AgentToolStatus::Ok,
                output: None,
                tool_class: Some(AgentToolClass::Plan),
                path: None,
                by_rule: false,
            }),
            ..item(2, AgentItemKind::Tool, "")
        };
        let mut plan = item(3, AgentItemKind::Plan, "1. Read the code\n2. Fix it");
        plan.plan_state = Some(AgentPlanState::Proposed);
        let question = |outcome, by_name: Option<&str>| crate::AgentItemQuestion {
            count: 2,
            secret: false,
            outcome,
            by_name: by_name.map(Into::into),
        };
        let mut asked = item(4, AgentItemKind::Question, "");
        asked.question = Some(question(None, None));
        let mut answered = item(5, AgentItemKind::Question, "Auth: OAuth");
        answered.question = Some(question(Some(crate::AgentQuestionOutcome::Answered), Some("Pixel")));
        let rows = AgentTimeline::new().rows(
            vec![steered, plan_file, plan, asked, answered],
            vec![],
            AgentThreadState::Waiting,
            1_000,
            3_000,
            vec![],
            vec![],
        );
        assert!(rows.iter().all(|r| r.item_id != "i2"), "the plan file makes no row: {rows:?}");
        assert!(rows.iter().any(|r| matches!(r.kind, AgentRowKind::User { via: Some(AgentVia::Steer), .. })));
        assert!(rows.iter().any(|r| r.item_id == "i3"
            && matches!(r.kind, AgentRowKind::Plan { state: AgentPlanState::Proposed, .. })));
        assert!(rows.iter().any(|r| r.item_id == "i4"
            && matches!(r.kind, AgentRowKind::Question { state: AgentQuestionState::Asked, count: 2, .. })));
        assert!(rows.iter().any(|r| r.item_id == "i5"
            && matches!(&r.kind, AgentRowKind::Question { state: AgentQuestionState::Answered, by, .. } if by == "Pixel")));

        // Opened again in a finished turn, from a PC that sends no state: its
        // answers say it was answered.
        let mut answered = item(2, AgentItemKind::Question, "Auth: OAuth");
        answered.end_ms = Some(2_500);
        let rows = AgentTimeline::new().rows(
            vec![
                item(1, AgentItemKind::User, "Fix it."),
                answered,
                item(3, AgentItemKind::Assistant, "Done."),
            ],
            vec![],
            AgentThreadState::Done,
            1_000,
            9_000,
            vec![],
            vec![],
        );
        assert!(
            rows.iter().all(|r| !matches!(
                r.kind,
                AgentRowKind::Question { state: AgentQuestionState::Expired, .. }
            )),
            "{rows:?}"
        );
    }

    #[test]
    fn the_composer_finds_its_trigger_and_writes_the_pick() {
        let trigger = agent_trigger("see @src/ma".into(), 11).unwrap();
        assert_eq!(
            (trigger.kind, trigger.start, trigger.query.as_str()),
            (AgentTriggerKind::Mention, 4, "src/ma")
        );
        let applied = agent_trigger_apply("see @src/ma".into(), trigger, 11, "src/main.rs".into());
        assert!(applied.text.starts_with("see @src/main.rs"), "{}", applied.text);
        assert!(applied.cursor as usize >= "see @src/main.rs".len());
        assert_eq!(agent_mentions(applied.text).first().map(|m| m.start), Some(4));
        assert_eq!(agent_trigger("/rev".into(), 4).map(|t| t.kind), Some(AgentTriggerKind::Slash));
        assert!(agent_trigger("mail me@example.com".into(), 19).is_none(), "an address isn't a mention");
    }

    #[test]
    fn a_reasoning_item_without_text_makes_no_row_of_its_own() {
        let items = vec![
            item(1, AgentItemKind::User, "Fix the test."),
            AgentItem { end_ms: None, ..item(2, AgentItemKind::Reasoning, "") },
        ];
        let rows =
            AgentTimeline::new().rows(items, vec![], AgentThreadState::Working, 1_000, 3_000, vec![], vec![]);
        assert_eq!(keys(&rows), ["i:i1", "live"]);
        assert!(matches!(rows[1].kind, AgentRowKind::Live { label: AgentLiveLabel::Thinking, .. }));
    }

    #[test]
    fn the_typing_guard_holds_what_arrives_while_you_type() {
        let guard = AgentTypingGuard::new();
        guard.focus();
        guard.typed(10_000);
        guard.requests(vec!["t1".into()], 10_500);
        assert!(guard.held("t1".into()));
        assert_eq!(guard.idle_at_ms(), Some(11_500));
        guard.idle();
        assert!(!guard.held("t1".into()));
        assert_eq!(guard.idle_at_ms(), None);
        guard.typed(20_000);
        guard.requests(vec!["t1".into(), "t2".into()], 20_100);
        assert!(!guard.held("t1".into()), "seen before");
        assert!(guard.held("t2".into()));
        guard.blur();
        assert!(!guard.held("t2".into()));
        guard.typed(-5);
        guard.requests(vec!["t3".into()], -1);
        assert!(!guard.held("t3".into()), "not focused");
    }

    #[test]
    fn an_edit_reads_as_a_diff() {
        let doc = agent_diff("-old line\n+new line\n context".into());
        assert_eq!((doc.adds, doc.dels, doc.more, doc.files.len()), (1, 1, false, 1));
        let rows = &doc.files[0].rows;
        assert_eq!(
            rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
            [AgentDiffKind::Del, AgentDiffKind::Add, AgentDiffKind::Context]
        );
        assert!(rows.iter().all(|r| r.old_no == 0 && r.new_no == 0), "no hunk header, no numbers");
        assert!(!rows[1].marks.is_empty(), "the changed word is marked");
    }

    #[test]
    fn rows_cross_the_boundary_intact_and_never_print_text() {
        fn round_trip<T>(value: T) -> T
        where
            T: uniffi::Lower<crate::UniFfiTag, FfiType = uniffi::RustBuffer>
                + uniffi::Lift<crate::UniFfiTag, FfiType = uniffi::RustBuffer>,
        {
            <T as uniffi::Lift<crate::UniFfiTag>>::try_lift(<T as uniffi::Lower<crate::UniFfiTag>>::lower(
                value,
            ))
            .unwrap()
        }
        let mut items = finished_turn();
        items[3].text = Some(
            "# secret\n\n| a | b |\n|---|--:|\n| [x](https://e.example) | `y` |\n\n```rust\nfn f() {}\n```"
                .into(),
        );
        let rows = AgentTimeline::new().rows(
            items,
            vec![],
            AgentThreadState::Done,
            9_000,
            10_000,
            vec![],
            vec!["f:1".into(), "g:i2".into(), "i:i2".into(), "i:i3".into(), "c:1:0".into()],
        );
        assert!(rows.len() > 10, "{rows:?}");
        assert!(
            rows.iter().any(|r| matches!(
                &r.kind,
                AgentRowKind::Detail { of: AgentDetailOf::Diff { edits: 1, .. } }
            ))
        );
        for row in &rows {
            assert_eq!(&round_trip(row.clone()), row);
            assert!(!format!("{row:?}").contains("secret"));
        }
        let doc = agent_diff("+secret".into());
        assert_eq!(round_trip(doc.clone()), doc);
        assert!(!format!("{doc:?}").contains("secret"));
    }
}
