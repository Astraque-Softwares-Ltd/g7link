// SPDX-License-Identifier: MPL-2.0
// Parts ported from t3code (MIT); see THIRD-PARTY-T3CODE.md.
//! A thread's items into the rows a client draws: turns that fold behind
//! "Worked for …" once answered, runs of finished tools gathered into one
//! line, thoughts, requests waiting at their tool, plans, questions the
//! agent asked, and the live line.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::bounds::{
    CACHE_BYTES, CACHE_ENTRIES, GROUP_MIN, MAX_CHANGED_FILES, MAX_DEPTH, MAX_ITEMS_IN, MAX_ROWS_OUT,
    MAX_SRC_BYTES, PLAN_FOLD_LINES, clean, cut_bytes, is_stripped_char, sat16,
};
use crate::diff::{DiffDoc, diff};
use crate::label::{Summary, ToolLabel, live_thought_line, summary, tool_label};
use crate::markdown::{Block, BlockKind, is_false, is_zero, parse};
use crate::stream::{StreamCache, blocks as stream_blocks};

// ── Input ──────────────────────────────────────────────────────────────────

/// What the timeline needs of an item; each client builds it from its own.
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ItemIn<'a> {
    pub seq: u64,
    pub id: &'a str,
    pub kind: ItemKind,
    pub ts_ms: u64,
    pub end_ms: Option<u64>,
    pub rev: u32,
    pub text: &'a str,
    /// Its text is still arriving.
    pub streaming: bool,
    /// 1-based; 0 for items stored before turns were recorded.
    pub turn: u32,
    /// The tool item this was produced inside (a sub-agent's work).
    pub group: Option<&'a str>,
    pub phase: Option<Phase>,
    /// A user item: the phone's name; empty for this PC.
    pub from: &'a str,
    /// A user item: the tool item this message denied.
    pub deny_of: Option<&'a str>,
    pub tool: Option<ToolIn<'a>>,
    /// The host cut the item's text or output.
    pub truncated: bool,
    /// A user item: how it reached the agent.
    pub via: Via,
    /// A plan item: where it stands; `None` reads as proposed.
    pub plan_state: Option<PlanState>,
    /// A question item: what the client knows of it; `None` reads as asked,
    /// with no count. A mark on the item overrides its state (see
    /// [`QuestionIn`]).
    pub question: Option<QuestionIn<'a>>,
}

impl std::fmt::Debug for ItemIn<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemIn")
            .field("seq", &self.seq)
            .field("kind", &self.kind)
            .field("rev", &self.rev)
            .field("turn", &self.turn)
            .field("streaming", &self.streaming)
            .field("tool", &self.tool)
            .field("via", &self.via)
            .field("plan_state", &self.plan_state)
            .field("question", &self.question)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    User,
    Assistant,
    Tool,
    Notice,
    Reasoning,
    /// A plan the agent proposed in plan mode; its text is the plan.
    Plan,
    /// Questions the agent asked; its text, when any, is the answers.
    Question,
}

/// How a user item reached the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    /// It started a turn.
    #[default]
    Turn,
    /// Sent while the agent worked, and taken into the running turn.
    Steer,
    /// Sent while the agent worked, and sent when the turn ended.
    Queue,
}

impl Via {
    pub fn is_turn(&self) -> bool {
        matches!(self, Via::Turn)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanState {
    #[default]
    Proposed,
    Implemented,
    /// A newer plan of the thread took its place.
    Replaced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuestionState {
    /// Waiting for the user.
    #[default]
    Asked,
    Answered,
    /// The user chose not to answer.
    Dismissed,
    /// It ended unanswered: it lapsed, or the turn ended or was stopped.
    Expired,
}

/// A question item as the client knows it from the item and the question's
/// events. A mark on the item, when there is one, decides the state
/// instead: `Waiting` and `Held` → asked, `Allowed` → answered by the mark's
/// `by`, `Denied` → dismissed, `Expired` and `Cancelled` → expired. A
/// question still asked when its turn has ended reads as expired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct QuestionIn<'a> {
    pub state: QuestionState,
    /// How many questions it asked, 1 to 4; 0 when not known (a secret
    /// question on a phone).
    pub count: u8,
    /// Who answered: a phone's name; empty for this device.
    pub by: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Interim,
    Final,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ToolIn<'a> {
    pub name: &'a str,
    pub title: &'a str,
    pub class: ToolClass,
    /// The command, or a diff.
    pub detail: &'a str,
    /// The file an edit or read is about; may be empty.
    pub path: &'a str,
    pub status: ToolStatus,
    /// The tail of the output.
    pub output: &'a str,
    /// The host allowed it from the thread's "Always allow" rule: a `Rule`
    /// mark unless `Ctx.marks` names the tool.
    pub by_rule: bool,
}

impl std::fmt::Debug for ToolIn<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolIn")
            .field("class", &self.class)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolClass {
    Command,
    Edit,
    Read,
    Search,
    Web,
    Mcp,
    Agent,
    Tasks,
    #[default]
    Other,
    /// Writing the plan file in plan mode: no row (the plan item shows it).
    Plan,
}

/// The tool search an agent runs to find its tools: no row, not counted.
const TOOL_SEARCH: &str = "ToolSearch";

/// Whether a tool item makes a line of its own: not a task list, not the
/// plan file, not a tool search.
pub(crate) fn is_shown_tool(tool: &ToolIn<'_>) -> bool {
    match tool.class {
        ToolClass::Tasks | ToolClass::Plan => false,
        ToolClass::Other => tool.name != TOOL_SEARCH,
        _ => true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Running,
    Ok,
    Error,
    Denied,
}

/// A request, at its tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    pub item_id: String,
    pub kind: MarkKind,
    /// Who answered: a phone's name; empty for this device.
    #[serde(default)]
    pub by: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkKind {
    Waiting,
    /// Waiting, and the typing guard holds it.
    Held,
    Allowed,
    Denied,
    Expired,
    Cancelled,
    /// Allowed by an "Always allow" rule of the thread.
    Rule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadState {
    Working,
    Waiting,
    Done,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ctx<'a> {
    pub state: ThreadState,
    pub state_since_ms: u64,
    pub now_ms: u64,
    pub marks: &'a [Mark],
    /// Keys of the rows the user opened.
    pub open: &'a [String],
}

// ── Output ─────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// Stable while the row exists; clients diff by it.
    pub key: String,
    #[serde(flatten)]
    pub kind: RowKind,
    pub turn: u32,
    /// 1 inside an open group or fold, more when nested.
    pub depth: u8,
    pub item_id: String,
    pub seq: u64,
}

impl std::fmt::Debug for Row {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Row")
            .field("key", &self.key)
            .field("kind", &self.kind)
            .field("turn", &self.turn)
            .field("depth", &self.depth)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RowKind {
    User {
        text: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        from: String,
        #[serde(default, skip_serializing_if = "is_false")]
        deny_of: bool,
        /// Sent while the agent worked: taken in (`steer`) or sent after
        /// (`queue`).
        #[serde(default, skip_serializing_if = "Via::is_turn")]
        via: Via,
    },
    /// One Markdown block of an answer, a message or an opened thought.
    Block {
        block: Block,
        index: u16,
        #[serde(default, skip_serializing_if = "is_false")]
        last: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<Phase>,
    },
    Tool {
        label: ToolLabel,
        status: RowStatus,
        #[serde(default, skip_serializing_if = "is_false")]
        has_detail: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        has_output: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        open: bool,
        /// Edits: lines added and removed.
        #[serde(default, skip_serializing_if = "is_zero")]
        adds: u32,
        #[serde(default, skip_serializing_if = "is_zero")]
        dels: u32,
        /// How long it ran, once it ended; 0 before.
        #[serde(default, skip_serializing_if = "is_zero")]
        ms: u64,
    },
    /// Under an open tool, or a file of an open `Changes` row.
    Detail {
        of: DetailOf,
    },
    Group {
        summary: Summary,
        count: u16,
        #[serde(default, skip_serializing_if = "is_false")]
        failed: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        open: bool,
    },
    Thought {
        preview: String,
        count: u16,
        has_text: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        open: bool,
    },
    Fold {
        worked_ms: u64,
        hidden: u16,
        #[serde(default, skip_serializing_if = "is_false")]
        open: bool,
    },
    Changes {
        files: Vec<FileStat>,
        adds: u32,
        dels: u32,
        #[serde(default, skip_serializing_if = "is_zero")]
        more: u16,
    },
    Mark {
        #[serde(rename = "mark_kind")]
        kind: MarkKind,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        by: String,
    },
    Notice {
        text: String,
    },
    /// The head of a plan's card; `shown` of its blocks follow as `Block`
    /// rows (`index` 0 up), and `more` are left out until it is opened
    /// (key `p:<item id>`): a long plan shows its first
    /// `PLAN_FOLD_LINES` lines, a replaced one only this row.
    Plan {
        state: PlanState,
        #[serde(default, skip_serializing_if = "is_zero")]
        shown: u16,
        #[serde(default, skip_serializing_if = "is_zero")]
        more: u16,
        #[serde(default, skip_serializing_if = "is_false")]
        open: bool,
    },
    /// The agent's questions, one line; opened (key `q:<item id>`), the
    /// answers follow as `Block` rows when the item has them.
    Question {
        state: QuestionState,
        #[serde(default, skip_serializing_if = "is_zero")]
        count: u8,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        by: String,
        #[serde(default, skip_serializing_if = "is_false")]
        has_text: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        open: bool,
    },
    Live {
        label: LiveLabel,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        thought: String,
        since_ms: u64,
    },
}

impl RowKind {
    /// The kind's name, as in the JSON and the desktop model's `kind` role.
    pub fn name(&self) -> &'static str {
        match self {
            RowKind::User { .. } => "user",
            RowKind::Block { .. } => "block",
            RowKind::Tool { .. } => "tool",
            RowKind::Detail { .. } => "detail",
            RowKind::Group { .. } => "group",
            RowKind::Thought { .. } => "thought",
            RowKind::Fold { .. } => "fold",
            RowKind::Changes { .. } => "changes",
            RowKind::Mark { .. } => "mark",
            RowKind::Notice { .. } => "notice",
            RowKind::Plan { .. } => "plan",
            RowKind::Question { .. } => "question",
            RowKind::Live { .. } => "live",
        }
    }
}

impl std::fmt::Debug for RowKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RowStatus {
    Running,
    /// Waiting for the user's answer (a `Waiting` or `Held` mark).
    Asking,
    Ok,
    Failed,
    Denied,
    /// Still running when its turn ended.
    Stopped,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DetailOf {
    /// The whole command.
    Command { text: String },
    /// An edit's diff. Under a `Changes` file: `edit` of `edits` (1-based);
    /// both 0 under a tool.
    Diff {
        doc: DiffDoc,
        #[serde(default, skip_serializing_if = "is_zero")]
        edit: u16,
        #[serde(default, skip_serializing_if = "is_zero")]
        edits: u16,
    },
    /// The output's tail; `cut` when the host dropped its start.
    Output {
        text: String,
        #[serde(default, skip_serializing_if = "is_false")]
        cut: bool,
    },
}

impl std::fmt::Debug for DetailOf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DetailOf::Command { .. } => f.write_str("Command"),
            DetailOf::Diff { doc, edit, edits } => {
                f.debug_struct("Diff").field("doc", doc).field("edit", edit).field("edits", edits).finish()
            }
            DetailOf::Output { cut, .. } => {
                f.debug_struct("Output").field("cut", cut).finish_non_exhaustive()
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FileStat {
    pub path: String,
    pub adds: u32,
    pub dels: u32,
    /// Its key `c:<turn>:<index>` is open: its edits follow the row.
    #[serde(default, skip_serializing_if = "is_false")]
    pub open: bool,
}

impl std::fmt::Debug for FileStat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileStat")
            .field("adds", &self.adds)
            .field("dels", &self.dels)
            .field("open", &self.open)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LiveLabel {
    Starting,
    Thinking,
}

// ── Changed files ──────────────────────────────────────────────────────────

/// Lines added and removed by one edit tool, per file.
#[derive(Debug, Clone, Default)]
struct EditStat {
    files: Vec<(String, u32, u32)>,
}

/// An edit's lines per file: by the diff's own file names when it has
/// several (`path` is then only the first), else under `path`.
fn edit_stat(tool: &ToolIn<'_>) -> EditStat {
    let doc = diff(tool.detail);
    let path = clean(tool.path);
    if doc.files.len() > 1 || (path.is_empty() && !doc.files.is_empty()) {
        return EditStat { files: doc.files.iter().map(|f| (f.path.clone(), f.adds, f.dels)).collect() };
    }
    EditStat { files: vec![(path.into_owned(), doc.adds, doc.dels)] }
}

fn is_ok_edit(item: &ItemIn<'_>) -> bool {
    item.kind == ItemKind::Tool
        && item.tool.is_some_and(|t| t.class == ToolClass::Edit && t.status == ToolStatus::Ok)
}

/// The files the `Ok` edit tools among `items` changed: per distinct path,
/// lines added and removed, sorted by path and cut to `MAX_CHANGED_FILES`;
/// with the totals over every file and how many files were left out.
pub fn changed_files(items: &[ItemIn<'_>]) -> (Vec<FileStat>, u32, u32, u16) {
    let stats: Vec<EditStat> =
        items.iter().filter(|i| is_ok_edit(i)).filter_map(|i| i.tool.map(|t| edit_stat(&t))).collect();
    let refs: Vec<&EditStat> = stats.iter().collect();
    merge_files(&refs)
}

fn merge_files(stats: &[&EditStat]) -> (Vec<FileStat>, u32, u32, u16) {
    let mut by_path: HashMap<&str, (u32, u32)> = HashMap::new();
    for stat in stats {
        for (path, adds, dels) in &stat.files {
            let e = by_path.entry(path.as_str()).or_default();
            e.0 = e.0.saturating_add(*adds);
            e.1 = e.1.saturating_add(*dels);
        }
    }
    let mut files: Vec<FileStat> = by_path
        .into_iter()
        .map(|(path, (adds, dels))| FileStat { path: path.to_owned(), adds, dels, open: false })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let adds = files.iter().fold(0u32, |n, f| n.saturating_add(f.adds));
    let dels = files.iter().fold(0u32, |n, f| n.saturating_add(f.dels));
    let more = sat16(files.len().saturating_sub(MAX_CHANGED_FILES));
    files.truncate(MAX_CHANGED_FILES);
    (files, adds, dels, more)
}

// ── The timeline ───────────────────────────────────────────────────────────

struct DocEntry {
    rev: u32,
    len: usize,
    streaming: bool,
    seq: u64,
    bytes: usize,
    blocks: Vec<Block>,
}

/// Keeps parsed Markdown and diff counts between calls for one thread.
#[derive(Default)]
pub struct Timeline {
    docs: HashMap<String, DocEntry>,
    doc_bytes: usize,
    streams: HashMap<String, StreamCache>,
    /// Block counts of texts behind a closed fold: (rev, len, count).
    counts: HashMap<String, (u32, usize, usize)>,
    /// Edit tools' line counts: (rev, detail len, stat).
    edits: HashMap<String, (u32, usize, EditStat)>,
    /// Diffs of opened edit tools: (rev, detail len, doc).
    diffs: HashMap<String, (u32, usize, DiffDoc)>,
}

impl std::fmt::Debug for Timeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Timeline")
            .field("docs", &self.docs.len())
            .field("doc_bytes", &self.doc_bytes)
            .field("streams", &self.streams.len())
            .finish_non_exhaustive()
    }
}

fn doc_bytes(text_len: usize, blocks: &[Block]) -> usize {
    let mut n = text_len + 64;
    for b in blocks {
        n += 96 + b.text.len() + b.spans.len() * 48 + b.spans.iter().map(|s| s.href.len()).sum::<usize>();
        n += b.lines.iter().map(|l| 32 + l.text.len() + l.toks.len() * 8).sum::<usize>();
        if let Some(t) = &b.table {
            n += t.cells.iter().map(|c| 32 + c.text.len() + c.spans.len() * 48).sum::<usize>();
        }
    }
    n
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// The rows for `items` (ascending `seq`) in the state `ctx` describes.
    pub fn rows(&mut self, items: &[ItemIn<'_>], ctx: &Ctx<'_>) -> Vec<Row> {
        let window = &items[items.len().saturating_sub(MAX_ITEMS_IN)..];
        let view = View::new(window, ctx);
        let mut used = HashSet::new();
        let mut out = Emit { tl: self, rows: Vec::new(), count: 0, count_only: false, used: &mut used };
        view.emit_all(&mut out);
        let mut rows = std::mem::take(&mut out.rows);
        if rows.len() > MAX_ROWS_OUT {
            rows.drain(..rows.len() - MAX_ROWS_OUT);
        }
        let present: HashSet<&str> = view.its.iter().map(|i| i.id).collect();
        self.docs.retain(|id, _| present.contains(id.as_str()));
        self.doc_bytes = self.docs.values().map(|e| e.bytes).sum();
        self.streams.retain(|id, _| used.contains(id.as_str()));
        self.counts.retain(|id, _| present.contains(id.as_str()));
        self.edits.retain(|id, _| present.contains(id.as_str()));
        self.diffs.retain(|id, _| used.contains(id.as_str()));
        rows
    }

    /// Entries and estimated bytes in the parsed-Markdown cache (for tests).
    #[doc(hidden)]
    pub fn cached(&self) -> (usize, usize) {
        (self.docs.len(), self.doc_bytes)
    }

    fn blocks(&mut self, it: &ItemIn<'_>) -> Vec<Block> {
        if let Some(e) = self.docs.get_mut(it.id)
            && e.rev == it.rev
            && e.len == it.text.len()
            && e.streaming == it.streaming
        {
            e.seq = it.seq;
            return e.blocks.clone();
        }
        let mut cache = self.streams.remove(it.id).unwrap_or_default();
        let (blocks, _) = stream_blocks(it.text, it.streaming, &mut cache);
        if it.streaming {
            self.streams.insert(it.id.to_owned(), cache);
        }
        let bytes = doc_bytes(it.text.len(), &blocks);
        if let Some(old) = self.docs.insert(
            it.id.to_owned(),
            DocEntry {
                rev: it.rev,
                len: it.text.len(),
                streaming: it.streaming,
                seq: it.seq,
                bytes,
                blocks: blocks.clone(),
            },
        ) {
            self.doc_bytes = self.doc_bytes.saturating_sub(old.bytes);
        }
        self.doc_bytes += bytes;
        while self.docs.len() > CACHE_ENTRIES || self.doc_bytes > CACHE_BYTES {
            let Some(oldest) = self.docs.iter().min_by_key(|(_, e)| e.seq).map(|(k, _)| k.clone()) else {
                break;
            };
            if let Some(e) = self.docs.remove(&oldest) {
                self.doc_bytes = self.doc_bytes.saturating_sub(e.bytes);
            }
        }
        blocks
    }

    fn block_count(&mut self, it: &ItemIn<'_>) -> usize {
        if let Some(e) = self.docs.get(it.id)
            && e.rev == it.rev
            && e.len == it.text.len()
            && e.streaming == it.streaming
        {
            return e.blocks.len();
        }
        if let Some(&(rev, len, n)) = self.counts.get(it.id)
            && rev == it.rev
            && len == it.text.len()
        {
            return n;
        }
        let text = clean(cut_bytes(it.text, MAX_SRC_BYTES));
        let parsed = parse(&text, false);
        let n = parsed.blocks.len();
        self.counts.insert(it.id.to_owned(), (it.rev, it.text.len(), n));
        n
    }

    fn edit_stat(&mut self, it: &ItemIn<'_>, tool: &ToolIn<'_>) -> EditStat {
        if let Some((rev, len, stat)) = self.edits.get(it.id)
            && *rev == it.rev
            && *len == tool.detail.len()
        {
            return stat.clone();
        }
        let stat = edit_stat(tool);
        self.edits.insert(it.id.to_owned(), (it.rev, tool.detail.len(), stat.clone()));
        stat
    }

    fn diff_doc(&mut self, it: &ItemIn<'_>, tool: &ToolIn<'_>) -> DiffDoc {
        if let Some((rev, len, doc)) = self.diffs.get(it.id)
            && *rev == it.rev
            && *len == tool.detail.len()
        {
            return doc.clone();
        }
        let doc = diff(tool.detail);
        self.diffs.insert(it.id.to_owned(), (it.rev, tool.detail.len(), doc.clone()));
        doc
    }
}

/// Where rows go: into the list, or only counted (behind a closed fold).
struct Emit<'t, 'u> {
    tl: &'t mut Timeline,
    rows: Vec<Row>,
    count: usize,
    count_only: bool,
    used: &'u mut HashSet<String>,
}

impl Emit<'_, '_> {
    fn push(&mut self, make: impl FnOnce() -> Row) {
        if self.count_only {
            self.count += 1;
        } else {
            self.rows.push(make());
        }
    }
}

/// A row-producing piece of a turn, before runs are grouped.
#[derive(Debug, Clone)]
enum Unit {
    User(usize),
    Assistant(usize),
    Notice(usize),
    Thought(Vec<usize>),
    Tool(usize),
    Plan(usize),
    Question(usize),
}

impl Unit {
    fn first(&self) -> usize {
        match self {
            Unit::User(i)
            | Unit::Assistant(i)
            | Unit::Notice(i)
            | Unit::Tool(i)
            | Unit::Plan(i)
            | Unit::Question(i) => *i,
            Unit::Thought(ids) => ids.first().copied().unwrap_or_default(),
        }
    }
}

/// The lines a block takes in a plan's card: its text's lines (a final
/// line break starts none), a table's rows with its head; at least 1.
fn block_lines(b: &Block) -> usize {
    match b.kind {
        BlockKind::Table => b.table.as_ref().map_or(1, |t| t.cells.len() / usize::from(t.columns.max(1))),
        BlockKind::Rule => 1,
        _ => b.text.lines().count(),
    }
    .max(1)
}

/// How many of a plan's blocks its closed card shows: those that end within
/// its first `PLAN_FOLD_LINES` lines, and at least the first.
fn plan_fold(blocks: &[Block]) -> usize {
    let mut lines = 0usize;
    for (k, b) in blocks.iter().enumerate() {
        lines = lines.saturating_add(block_lines(b));
        if lines > PLAN_FOLD_LINES {
            return k.max(1);
        }
    }
    blocks.len()
}

struct View<'c, 'a> {
    its: Vec<&'c ItemIn<'a>>,
    ctx: &'c Ctx<'c>,
    open: HashSet<&'c str>,
    marks: HashMap<&'c str, &'c Mark>,
    /// Tools the host allowed by a rule, with no mark of their own.
    by_rule: HashSet<&'c str>,
    /// Marks whose item is not a tool line here, in their order.
    orphans: Vec<&'c Mark>,
    children: Vec<Vec<usize>>,
    /// Top-level items split into turns: (turn, indices).
    turns: Vec<(u32, Vec<usize>)>,
}

/// Whether a mark makes a row of its own (reconcile §7): an answer given here
/// by hand already shows in the tool line.
fn mark_shown(mark: &Mark) -> bool {
    match mark.kind {
        MarkKind::Allowed | MarkKind::Denied => !mark.by.is_empty(),
        _ => true,
    }
}

/// Whether Markdown text makes no block at all: only spaces, tabs and line
/// breaks (and characters the cleaning removes). Wider Unicode spaces are
/// content to Markdown.
fn md_blank(s: &str) -> bool {
    s.chars().all(|c| matches!(c, ' ' | '\t' | '\n') || is_stripped_char(c))
}

fn plain_text(s: &str) -> String {
    clean(s).into_owned()
}

impl<'c, 'a> View<'c, 'a> {
    fn new(window: &'c [ItemIn<'a>], ctx: &'c Ctx<'c>) -> Self {
        let mut seen = HashSet::new();
        let its: Vec<&ItemIn<'a>> = window.iter().filter(|i| seen.insert(i.id)).collect();
        let index: HashMap<&str, usize> = its.iter().enumerate().map(|(n, i)| (i.id, n)).collect();
        let is_tool_line =
            |i: &ItemIn<'_>| i.kind == ItemKind::Tool && i.tool.is_some_and(|t| is_shown_tool(&t));
        // A question's mark is its state, not a row of its own.
        let takes_mark = |i: &ItemIn<'_>| is_tool_line(i) || i.kind == ItemKind::Question;
        let mut marks = HashMap::new();
        let mut orphans = Vec::new();
        for mark in ctx.marks {
            if marks.contains_key(mark.item_id.as_str())
                || orphans.iter().any(|m: &&Mark| m.item_id == mark.item_id)
            {
                continue;
            }
            match index.get(mark.item_id.as_str()) {
                Some(&n) if takes_mark(its[n]) => {
                    marks.insert(mark.item_id.as_str(), mark);
                }
                _ => orphans.push(mark),
            }
        }
        let by_rule = its
            .iter()
            .filter(|i| is_tool_line(i) && i.tool.is_some_and(|t| t.by_rule) && !marks.contains_key(i.id))
            .map(|i| i.id)
            .collect();
        // Sub-agent work goes under the tool it ran in, when that tool came
        // first; the nesting is bounded.
        let mut parent: Vec<Option<usize>> = vec![None; its.len()];
        let mut level: Vec<u8> = vec![0; its.len()];
        for (n, it) in its.iter().enumerate() {
            let Some(group) = it.group else { continue };
            if let Some(&p) = index.get(group)
                && p < n
                && is_tool_line(its[p])
                && level[p] < MAX_DEPTH
            {
                parent[n] = Some(p);
                level[n] = level[p] + 1;
            }
        }
        let mut children = vec![Vec::new(); its.len()];
        let mut turns: Vec<(u32, Vec<usize>)> = Vec::new();
        let mut prev = 0u32;
        for (n, it) in its.iter().enumerate() {
            if let Some(p) = parent[n] {
                children[p].push(n);
                continue;
            }
            let t = if it.turn > 0 {
                prev.max(it.turn)
            } else if it.kind == ItemKind::User && it.deny_of.is_none() {
                prev.saturating_add(1)
            } else {
                prev.max(1)
            };
            prev = t;
            match turns.last_mut() {
                Some((last, members)) if *last == t => members.push(n),
                _ => turns.push((t, vec![n])),
            }
        }
        View {
            its,
            ctx,
            open: ctx.open.iter().map(String::as_str).collect(),
            marks,
            by_rule,
            orphans,
            children,
            turns,
        }
    }

    fn is_open(&self, key: &str) -> bool {
        self.open.contains(key)
    }

    fn waiting(&self, n: usize) -> bool {
        self.marks.get(self.its[n].id).is_some_and(|m| matches!(m.kind, MarkKind::Waiting | MarkKind::Held))
    }

    /// A question's state, count and who answered: its mark decides when it
    /// has one, else what the client knows; still asked once its turn has
    /// ended, it reads as expired.
    fn question(&self, n: usize, settled: bool) -> (QuestionState, u8, &str) {
        let it = self.its[n];
        let known = it.question.unwrap_or_default();
        let from_mark = self.marks.get(it.id).and_then(|m| match m.kind {
            MarkKind::Waiting | MarkKind::Held => Some((QuestionState::Asked, "")),
            MarkKind::Allowed => Some((QuestionState::Answered, m.by.as_str())),
            MarkKind::Denied => Some((QuestionState::Dismissed, "")),
            MarkKind::Expired | MarkKind::Cancelled => Some((QuestionState::Expired, "")),
            MarkKind::Rule => None,
        });
        let (state, by) = match from_mark {
            Some(marked) => marked,
            None if known.state == QuestionState::Asked && settled => (QuestionState::Expired, ""),
            None => (known.state, if known.state == QuestionState::Answered { known.by } else { "" }),
        };
        (state, known.count, by)
    }

    fn status(&self, n: usize, settled: bool) -> RowStatus {
        if self.waiting(n) {
            return RowStatus::Asking;
        }
        match self.its[n].tool.map(|t| t.status) {
            Some(ToolStatus::Running) | None => {
                if settled {
                    RowStatus::Stopped
                } else {
                    RowStatus::Running
                }
            }
            Some(ToolStatus::Ok) => RowStatus::Ok,
            Some(ToolStatus::Error) => RowStatus::Failed,
            Some(ToolStatus::Denied) => RowStatus::Denied,
        }
    }

    /// Items into units: empty texts and task lists make none, consecutive
    /// thoughts make one.
    fn units(&self, members: &[usize]) -> Vec<Unit> {
        let mut units: Vec<Unit> = Vec::new();
        for &n in members {
            let it = self.its[n];
            match it.kind {
                ItemKind::User => units.push(Unit::User(n)),
                ItemKind::Notice => units.push(Unit::Notice(n)),
                ItemKind::Assistant => {
                    if !md_blank(it.text) {
                        units.push(Unit::Assistant(n));
                    }
                }
                ItemKind::Reasoning => {
                    if md_blank(it.text) {
                        continue;
                    }
                    match units.last_mut() {
                        Some(Unit::Thought(ids)) => ids.push(n),
                        _ => units.push(Unit::Thought(vec![n])),
                    }
                }
                ItemKind::Tool => {
                    if it.tool.is_some_and(|t| is_shown_tool(&t)) {
                        units.push(Unit::Tool(n));
                    }
                }
                ItemKind::Plan => units.push(Unit::Plan(n)),
                ItemKind::Question => units.push(Unit::Question(n)),
            }
        }
        units
    }

    fn row(&self, key: String, kind: RowKind, turn: u32, depth: u8, n: Option<usize>) -> Row {
        let (item_id, seq) = n.map_or((String::new(), 0), |n| (self.its[n].id.to_owned(), self.its[n].seq));
        Row { key, kind, turn, depth, item_id, seq }
    }

    fn emit_all(&self, out: &mut Emit<'_, '_>) {
        let last_turn = self.turns.len().saturating_sub(1);
        let finished =
            matches!(self.ctx.state, ThreadState::Done | ThreadState::Failed | ThreadState::Interrupted);
        for (k, (turn, members)) in self.turns.iter().enumerate() {
            let settled = k != last_turn || finished;
            let folds_here = k != last_turn || self.ctx.state == ThreadState::Done;
            self.emit_turn(out, *turn, members, settled, folds_here);
        }
        let last = self.turns.last();
        let last_turn_no = last.map_or(0, |(t, _)| *t);
        for mark in &self.orphans {
            if mark_shown(mark) {
                let key = format!("m:{}", mark.item_id);
                out.push(|| Row {
                    key,
                    kind: RowKind::Mark { kind: mark.kind, by: plain_text(&mark.by) },
                    turn: last_turn_no,
                    depth: 0,
                    item_id: mark.item_id.clone(),
                    seq: 0,
                });
            }
        }
        if self.ctx.state == ThreadState::Working {
            self.emit_live(out, last);
        }
    }

    fn emit_live(&self, out: &mut Emit<'_, '_>, last: Option<&(u32, Vec<usize>)>) {
        let (turn, members) = match last {
            Some((t, m)) => (*t, m.as_slice()),
            None => (0, &[][..]),
        };
        let mut all: Vec<usize> = Vec::new();
        let mut stack: Vec<usize> = members.to_vec();
        while let Some(n) = stack.pop() {
            all.push(n);
            stack.extend(self.children[n].iter().copied());
        }
        let busy = all.iter().any(|&n| {
            let it = self.its[n];
            match it.kind {
                ItemKind::Assistant | ItemKind::Plan => it.streaming,
                ItemKind::Tool => {
                    it.tool.is_some_and(|t| is_shown_tool(&t))
                        && matches!(self.status(n, false), RowStatus::Running | RowStatus::Asking)
                }
                ItemKind::Question => self.question(n, false).0 == QuestionState::Asked,
                ItemKind::User | ItemKind::Notice | ItemKind::Reasoning => false,
            }
        });
        if busy {
            return;
        }
        let anchor = members.iter().position(|&n| self.its[n].kind == ItemKind::User);
        let label = match anchor {
            Some(a) if a + 1 >= members.len() => LiveLabel::Starting,
            None if members.is_empty() => LiveLabel::Starting,
            _ => LiveLabel::Thinking,
        };
        let thought = members
            .iter()
            .rev()
            .map(|&n| self.its[n])
            .find(|it| it.kind == ItemKind::Reasoning)
            .filter(|it| it.end_ms.is_none() && !md_blank(it.text))
            .map(|it| live_thought_line(it.text))
            .unwrap_or_default();
        let since_ms = self.ctx.state_since_ms;
        out.push(|| Row {
            key: "live".to_owned(),
            kind: RowKind::Live { label, thought, since_ms },
            turn,
            depth: 0,
            item_id: String::new(),
            seq: 0,
        });
    }

    fn answer(&self, members: &[usize], settled: bool) -> Option<usize> {
        let assistants: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&n| self.its[n].kind == ItemKind::Assistant && !md_blank(self.its[n].text))
            .collect();
        if let Some(&n) = assistants.iter().rev().find(|&&n| self.its[n].phase == Some(Phase::Final)) {
            return Some(n);
        }
        let no_phase = members.iter().all(|&n| self.its[n].phase.is_none());
        if settled && no_phase { assistants.last().copied() } else { None }
    }

    /// Every item of a turn, sub-agent work included.
    fn with_children(&self, members: &[usize]) -> Vec<usize> {
        let mut all = Vec::new();
        let mut stack: Vec<usize> = members.iter().rev().copied().collect();
        while let Some(n) = stack.pop() {
            all.push(n);
            stack.extend(self.children[n].iter().rev().copied());
        }
        all
    }

    fn emit_turn(
        &self,
        out: &mut Emit<'_, '_>,
        turn: u32,
        members: &[usize],
        settled: bool,
        folds_here: bool,
    ) {
        let all = self.with_children(members);
        let pending = all.iter().any(|&n| self.waiting(n));
        let units = self.units(members);
        let folds = settled && folds_here && !pending;
        let answer = if folds { self.answer(members, settled) } else { None };
        // A turn that ended with a plan and no answer folds behind its plan.
        let plan = units.iter().rev().find_map(|u| if let Unit::Plan(n) = u { Some(*n) } else { None });
        let Some(end) = answer.or(plan.filter(|_| folds)) else {
            self.emit_units(out, &units, 0, turn, settled);
            return;
        };
        let anchor = units.iter().position(|u| matches!(u, Unit::User(_)));
        // After the anchor and without the answer, in order; plans and
        // messages sent while it worked stay out of the fold.
        let rest: Vec<(bool, &Unit)> = units
            .iter()
            .enumerate()
            .filter(|(k, u)| Some(*k) != anchor && !matches!(u, Unit::Assistant(n) if Some(*n) == answer))
            .map(|(_, u)| (self.stays_out(u), u))
            .collect();
        if let Some(a) = anchor {
            self.emit_unit(out, &units[a], 0, turn, settled);
        }
        // Count what the fold hides, then show it if it is open.
        let was = (out.count_only, out.count);
        out.count_only = true;
        out.count = 0;
        self.emit_rest(out, &rest, turn, settled, true);
        let hidden_rows = out.count;
        (out.count_only, out.count) = was;
        let mut open = false;
        if hidden_rows > 0 {
            let key = format!("f:{turn}");
            open = self.is_open(&key);
            let start = anchor.map_or(members.first().copied(), |a| Some(units[a].first()));
            let since = start.map_or(0, |n| self.its[n].ts_ms);
            let end =
                all.iter().map(|&n| self.its[n].end_ms.unwrap_or(self.its[n].ts_ms)).max().unwrap_or(since);
            let worked_ms = end.saturating_sub(since);
            let count = sat16(hidden_rows);
            out.push(|| self.row(key, RowKind::Fold { worked_ms, hidden: count, open }, turn, 0, start));
        }
        self.emit_rest(out, &rest, turn, settled, open);
        if let Some(answer) = answer {
            self.emit_blocks(out, answer, 0, turn);
        }
        self.emit_changes(out, turn, &all, end);
    }

    /// A unit a fold never hides: a plan, a message sent while it worked.
    fn stays_out(&self, unit: &Unit) -> bool {
        match unit {
            Unit::Plan(_) => true,
            Unit::User(n) => !self.its[*n].via.is_turn(),
            _ => false,
        }
    }

    /// A folded turn's units after its anchor: those that stay out at depth
    /// 0, and, when `hidden` (the fold is open, or rows are being counted),
    /// the runs between them at depth 1.
    fn emit_rest(
        &self,
        out: &mut Emit<'_, '_>,
        rest: &[(bool, &Unit)],
        turn: u32,
        settled: bool,
        hidden: bool,
    ) {
        let mut run: Vec<Unit> = Vec::new();
        for (out_of_fold, unit) in rest {
            if !*out_of_fold {
                run.push((*unit).clone());
                continue;
            }
            if hidden {
                self.emit_units(out, &run, 1, turn, settled);
            }
            run.clear();
            if !out.count_only {
                self.emit_unit(out, unit, 0, turn, settled);
            }
        }
        if hidden {
            self.emit_units(out, &run, 1, turn, settled);
        }
    }

    fn emit_changes(&self, out: &mut Emit<'_, '_>, turn: u32, all: &[usize], answer: usize) {
        let edits: Vec<usize> = all.iter().copied().filter(|&n| is_ok_edit(self.its[n])).collect();
        if edits.is_empty() {
            return;
        }
        let mut stats = Vec::with_capacity(edits.len());
        for &n in &edits {
            let it = self.its[n];
            if let Some(tool) = it.tool {
                stats.push((n, out.tl.edit_stat(it, &tool)));
            }
        }
        let refs: Vec<&EditStat> = stats.iter().map(|(_, s)| s).collect();
        let (mut files, adds, dels, more) = merge_files(&refs);
        for (index, file) in files.iter_mut().enumerate() {
            file.open = self.is_open(&format!("c:{turn}:{index}"));
        }
        let shown: Vec<(usize, FileStat)> =
            files.iter().cloned().enumerate().filter(|(_, f)| f.open).collect();
        out.push(|| {
            self.row(format!("c:{turn}"), RowKind::Changes { files, adds, dels, more }, turn, 0, Some(answer))
        });
        for (index, file) in shown {
            let of_file: Vec<usize> = stats
                .iter()
                .filter(|(_, s)| s.files.iter().any(|(p, _, _)| *p == file.path))
                .map(|(n, _)| *n)
                .collect();
            let edits = sat16(of_file.len());
            for (k, &n) in of_file.iter().enumerate() {
                let it = self.its[n];
                let Some(tool) = it.tool else { continue };
                if out.count_only {
                    out.count += 1;
                    continue;
                }
                out.used.insert(it.id.to_owned());
                let mut doc = out.tl.diff_doc(it, &tool);
                if clean(tool.path).as_ref() != file.path && doc.files.len() > 1 {
                    doc.files.retain(|f| f.path == file.path);
                    doc.adds = doc.files.iter().fold(0u32, |n, f| n.saturating_add(f.adds));
                    doc.dels = doc.files.iter().fold(0u32, |n, f| n.saturating_add(f.dels));
                }
                let key = format!("d:{}:e{index}", it.id);
                let edit = sat16(k + 1);
                out.push(|| {
                    self.row(
                        key,
                        RowKind::Detail { of: DetailOf::Diff { doc, edit, edits } },
                        turn,
                        1,
                        Some(n),
                    )
                });
            }
        }
    }

    /// Units at one level, with runs of finished tools and thoughts grouped.
    fn emit_units(&self, out: &mut Emit<'_, '_>, units: &[Unit], depth: u8, turn: u32, settled: bool) {
        let mut i = 0;
        while i < units.len() {
            if !self.groupable(&units[i], settled) {
                self.emit_unit(out, &units[i], depth, turn, settled);
                i += 1;
                continue;
            }
            let mut j = i;
            while j < units.len() && self.groupable(&units[j], settled) {
                let ends = self.ends_run(&units[j]);
                j += 1;
                if ends {
                    break;
                }
            }
            let run = &units[i..j];
            let tools = run.iter().filter(|u| matches!(u, Unit::Tool(_))).count();
            if run.len() >= GROUP_MIN && tools > 0 {
                self.emit_group(out, run, depth, turn, settled);
            } else {
                for u in run {
                    self.emit_unit(out, u, depth, turn, settled);
                }
            }
            i = j;
        }
    }

    fn groupable(&self, unit: &Unit, settled: bool) -> bool {
        match unit {
            Unit::Thought(_) => true,
            Unit::Tool(n) => !matches!(self.status(*n, settled), RowStatus::Running | RowStatus::Asking),
            _ => false,
        }
    }

    /// The mark row a tool has, if any: (kind, by).
    fn shown_mark(&self, n: usize) -> Option<(MarkKind, &'c str)> {
        let id = self.its[n].id;
        match self.marks.get(id) {
            Some(m) => mark_shown(m).then_some((m.kind, m.by.as_str())),
            None => self.by_rule.contains(id).then_some((MarkKind::Rule, "")),
        }
    }

    /// A tool whose mark makes a row ends the run after it.
    fn ends_run(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Tool(n) if self.shown_mark(*n).is_some())
    }

    fn emit_group(&self, out: &mut Emit<'_, '_>, run: &[Unit], depth: u8, turn: u32, settled: bool) {
        let first = run[0].first();
        let key = format!("g:{}", self.its[first].id);
        let open = self.is_open(&key);
        let tools: Vec<ToolIn<'_>> =
            run.iter().filter_map(|u| if let Unit::Tool(n) = u { self.its[*n].tool } else { None }).collect();
        let failed = run.iter().any(|u| {
            matches!(u, Unit::Tool(n) if matches!(self.status(*n, settled), RowStatus::Failed | RowStatus::Denied))
        });
        let count = sat16(run.len());
        let summary = summary(&tools);
        out.push(|| self.row(key, RowKind::Group { summary, count, failed, open }, turn, depth, Some(first)));
        if open {
            for u in run {
                self.emit_unit(out, u, depth.saturating_add(1), turn, settled);
            }
        } else if let Some(Unit::Tool(n)) = run.last() {
            self.emit_mark(out, *n, depth, turn);
        }
    }

    fn emit_mark(&self, out: &mut Emit<'_, '_>, n: usize, depth: u8, turn: u32) {
        let id = self.its[n].id;
        if let Some((kind, by)) = self.shown_mark(n) {
            out.push(|| {
                self.row(format!("m:{id}"), RowKind::Mark { kind, by: plain_text(by) }, turn, depth, Some(n))
            });
        }
    }

    fn emit_unit(&self, out: &mut Emit<'_, '_>, unit: &Unit, depth: u8, turn: u32, settled: bool) {
        match unit {
            Unit::User(n) => {
                let it = self.its[*n];
                out.push(|| {
                    self.row(
                        format!("i:{}", it.id),
                        RowKind::User {
                            text: plain_text(it.text),
                            from: plain_text(it.from),
                            deny_of: it.deny_of.is_some(),
                            via: it.via,
                        },
                        turn,
                        depth,
                        Some(*n),
                    )
                });
            }
            Unit::Notice(n) => {
                let it = self.its[*n];
                out.push(|| {
                    self.row(
                        format!("i:{}", it.id),
                        RowKind::Notice { text: plain_text(it.text) },
                        turn,
                        depth,
                        Some(*n),
                    )
                });
            }
            Unit::Assistant(n) => self.emit_blocks(out, *n, depth, turn),
            Unit::Thought(ids) => {
                let first = ids[0];
                let key = format!("t:{}", self.its[first].id);
                let open = self.is_open(&key);
                let count = sat16(ids.len());
                out.push(|| {
                    let preview = live_thought_line(self.its[first].text);
                    self.row(
                        key,
                        RowKind::Thought { preview, count, has_text: true, open },
                        turn,
                        depth,
                        Some(first),
                    )
                });
                if open {
                    for &n in ids {
                        self.emit_blocks(out, n, depth.saturating_add(1), turn);
                    }
                }
            }
            Unit::Tool(n) => self.emit_tool(out, *n, depth, turn, settled),
            Unit::Plan(n) => self.emit_plan(out, *n, depth, turn),
            Unit::Question(n) => self.emit_question(out, *n, depth, turn, settled),
        }
    }

    fn emit_plan(&self, out: &mut Emit<'_, '_>, n: usize, depth: u8, turn: u32) {
        let it = self.its[n];
        if it.streaming {
            out.used.insert(it.id.to_owned());
        }
        let key = format!("p:{}", it.id);
        let open = self.is_open(&key);
        let state = it.plan_state.unwrap_or_default();
        let blocks = out.tl.blocks(it);
        let total = blocks.len();
        let shown = if open {
            total
        } else if state == PlanState::Replaced {
            0
        } else {
            plan_fold(&blocks)
        };
        let more = sat16(total - shown);
        out.push(|| {
            self.row(key, RowKind::Plan { state, shown: sat16(shown), more, open }, turn, depth, Some(n))
        });
        for (index, block) in blocks.into_iter().take(shown).enumerate() {
            let key = format!("b:{}:{index}", it.id);
            let kind = RowKind::Block { block, index: sat16(index), last: index + 1 == total, phase: None };
            out.push(|| self.row(key, kind, turn, depth, Some(n)));
        }
    }

    fn emit_question(&self, out: &mut Emit<'_, '_>, n: usize, depth: u8, turn: u32, settled: bool) {
        let it = self.its[n];
        let key = format!("q:{}", it.id);
        let open = self.is_open(&key);
        let (state, count, by) = self.question(n, settled);
        let has_text = !md_blank(it.text);
        out.push(|| {
            self.row(
                key,
                RowKind::Question { state, count, by: plain_text(by), has_text, open },
                turn,
                depth,
                Some(n),
            )
        });
        if open && has_text {
            self.emit_blocks(out, n, depth.saturating_add(1), turn);
        }
    }

    fn emit_blocks(&self, out: &mut Emit<'_, '_>, n: usize, depth: u8, turn: u32) {
        let it = self.its[n];
        if it.streaming {
            out.used.insert(it.id.to_owned());
        }
        if out.count_only {
            out.count += out.tl.block_count(it);
            return;
        }
        let blocks = out.tl.blocks(it);
        let last = blocks.len().saturating_sub(1);
        let phase = if it.kind == ItemKind::Assistant { it.phase } else { None };
        for (index, block) in blocks.into_iter().enumerate() {
            let key = format!("b:{}:{index}", it.id);
            let kind = RowKind::Block { block, index: sat16(index), last: index == last, phase };
            out.push(|| self.row(key, kind, turn, depth, Some(n)));
        }
    }

    fn emit_tool(&self, out: &mut Emit<'_, '_>, n: usize, depth: u8, turn: u32, settled: bool) {
        let it = self.its[n];
        let Some(tool) = it.tool else { return };
        let key = format!("i:{}", it.id);
        let open = self.is_open(&key);
        let status = self.status(n, settled);
        let command = tool.class == ToolClass::Command && !tool.detail.trim().is_empty();
        let edit = tool.class == ToolClass::Edit && !tool.detail.trim().is_empty();
        let has_output = !tool.output.trim().is_empty();
        let (adds, dels) = if tool.class == ToolClass::Edit && !out.count_only {
            let stat = out.tl.edit_stat(it, &tool);
            stat.files
                .iter()
                .fold((0u32, 0u32), |(a, d), (_, fa, fd)| (a.saturating_add(*fa), d.saturating_add(*fd)))
        } else {
            (0, 0)
        };
        let ms = it.end_ms.map_or(0, |end| end.saturating_sub(it.ts_ms));
        out.push(|| {
            let label = tool_label(&tool);
            self.row(
                key,
                RowKind::Tool {
                    label,
                    status,
                    has_detail: command || edit,
                    has_output,
                    open,
                    adds,
                    dels,
                    ms,
                },
                turn,
                depth,
                Some(n),
            )
        });
        let inner = depth.saturating_add(1);
        if open {
            if command {
                out.push(|| {
                    self.row(
                        format!("d:{}:c", it.id),
                        RowKind::Detail { of: DetailOf::Command { text: plain_text(tool.detail) } },
                        turn,
                        inner,
                        Some(n),
                    )
                });
            }
            if edit {
                if out.count_only {
                    out.count += 1;
                } else {
                    out.used.insert(it.id.to_owned());
                    let doc = out.tl.diff_doc(it, &tool);
                    out.push(|| {
                        self.row(
                            format!("d:{}:f", it.id),
                            RowKind::Detail { of: DetailOf::Diff { doc, edit: 0, edits: 0 } },
                            turn,
                            inner,
                            Some(n),
                        )
                    });
                }
            }
            if has_output {
                out.push(|| {
                    self.row(
                        format!("d:{}:o", it.id),
                        RowKind::Detail {
                            of: DetailOf::Output { text: plain_text(tool.output), cut: it.truncated },
                        },
                        turn,
                        inner,
                        Some(n),
                    )
                });
            }
        }
        self.emit_mark(out, n, depth, turn);
        if open && !self.children[n].is_empty() {
            let units = self.units(&self.children[n]);
            self.emit_units(out, &units, inner, turn, settled);
        }
    }
}
