// SPDX-License-Identifier: GPL-3.0-or-later
//! Agents on this PC: G7 Link connects to the agent host (`g7-agents`, a
//! private local socket), keeps what the Agents page shows, relays threads
//! and the sessions started on this PC outside G7 to phones with the
//! `agents` and `agent_sessions` toggles through the core
//! (docs/protocol/agents.md), and answers their requests.
//!
//! It connects to a host that is already running at start, without starting
//! one, so the sidebar's count is right; it starts the host when the Agents
//! page is open, the user asked, or a phone may use it.
//!
//! The PC's own notifications about agents come from the host, never from
//! here. A folder's path and a session's working directory never leave the
//! PC: phones get a folder's id and name.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use g7_agents::{
    api::{
        self, AgentId, AgentInfo, Approval, ApprovalAnswerParams, ApprovalOutcome, ClientKind, Decision,
        ErrorCode, Event, ExportFormat, Folder, HelloParams, Item, Mode, Origin, OriginKind, Session,
        SessionContinueParams, SessionReadParams, SessionSource, SessionState, State, SubscribeParams,
        ThreadExportParams, ThreadExportResult, ThreadInfo, ThreadReadParams, ThreadSelection,
        ThreadStartParams, UiViewingParams, cap,
    },
    client::{self, Client, ClientError, Events, Launch, When},
};
use g7_agents_render::{self as render, GuardEvent};
use g7link_core::{
    AgentAsk, AgentAsked, AgentCatalog, AgentCatalogAgent, AgentCatalogFolder, AgentEvent, AgentExport,
    AgentFailure, AgentItem, AgentItems, AgentPlanStep, AgentReply, AgentRequest, AgentSession,
    AgentSessionItems, AgentThread, AgentTool, DeviceId, Node, agent,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Notify, watch};

use crate::{core_host, i18n::tr, state::Changes};

mod rows;
pub(crate) use rows::{CodePalette, LinkRef, ModelRow, Role, RoleValue, RowOp};
#[cfg(test)]
pub(crate) use rows::{Extra, model_row};

/// Longest wait between attempts to start and reach the host while it's wanted.
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How often a host that isn't running is looked for, without starting it.
const LOOK_EVERY: Duration = Duration::from_secs(15);
/// How long a phone's request waits for the host to be reached.
const CONNECT_WAIT: Duration = Duration::from_secs(4);
/// How long a phone's request may take here; the core gives up at 15 s.
const REQUEST_LIMIT: Duration = Duration::from_secs(12);
/// Threads remembered for asks and finished turns (the host keeps at most 200).
const KNOWN_THREADS: usize = 200;
/// Threads the page lists.
const LISTED_THREADS: usize = 100;
/// Items read at a time for the page.
const READ_PAGE: u32 = 50;
/// Items the open view keeps; older ones are read again with "Load earlier".
pub(crate) const OPEN_ITEMS: usize = 400;
/// An item's text, detail and output are cut to this many bytes.
pub(crate) const ROW_TEXT: usize = 16_384;
/// Updates waiting for the timeline model; past this they make way for one `Reset`.
const UI_QUEUE: usize = 256;
/// Streaming text reaches the timeline at most this often (10 a second).
const DELTA_EVERY: Duration = Duration::from_millis(100);

/// The lists are shown again at most this often.
const LISTS_EVERY: Duration = Duration::from_millis(250);
/// The open view's tool steps shown in its timeline.
const STEPS_SHOWN: usize = 12;
/// A step's detail (its command) as the "Now" card shows it.
const STEP_DETAIL: usize = 400;
/// Host connections kept for phones following a session (one each).
const PHONE_WATCHES: usize = 2;

/// Whether the host can be reached, as the page says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Not looked for yet.
    #[default]
    Idle,
    Connecting,
    Connected,
    /// Installed but not running (and nothing asked to start it).
    NotRunning,
    NotInstalled,
    Failed,
}

// ---- The open view (pure) ----

/// One item of the open view, its text cut to [`ROW_TEXT`] bytes, with
/// whether its text is still arriving and what the timeline reads of a tool.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub item: Item,
    /// Text is still arriving.
    pub streaming: bool,
    /// What the tool step does: the host's class, or one read from its name
    /// for a step stored before classes.
    class: render::ToolClass,
    /// The file an edit or read is about: the host's path, or one read from
    /// the title of a step stored before paths.
    path: String,
}

/// Never prints the text.
impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("seq", &self.item.seq)
            .field("rev", &self.item.rev)
            .field("kind", &self.item.kind)
            .field("streaming", &self.streaming)
            .finish_non_exhaustive()
    }
}

/// `text` cut to [`ROW_TEXT`] bytes, and whether it was cut.
fn capped(text: &str) -> (String, bool) {
    let kept = api::cut(text, ROW_TEXT);
    (kept.to_owned(), kept.len() < text.len())
}

/// A step's class from its tool's name, for steps stored before the host
/// classed them (the same names the host classes).
fn class_of_name(name: &str) -> render::ToolClass {
    use render::ToolClass as C;
    match name {
        "Bash" | "shell" | "exec" | "local_shell" | "commandExecution" => C::Command,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" | "apply_patch" | "fileChange" => C::Edit,
        "Read" | "NotebookRead" => C::Read,
        "Grep" | "Glob" | "LS" => C::Search,
        "WebSearch" | "WebFetch" | "webSearch" => C::Web,
        "Task" | "Agent" => C::Agent,
        "TodoWrite" => C::Tasks,
        n if n.starts_with("mcp__") => C::Mcp,
        _ => C::Other,
    }
}

fn render_class(class: api::ToolClass) -> render::ToolClass {
    use render::ToolClass as C;
    match class {
        api::ToolClass::Command => C::Command,
        api::ToolClass::Edit => C::Edit,
        api::ToolClass::Read => C::Read,
        api::ToolClass::Search => C::Search,
        api::ToolClass::Web => C::Web,
        api::ToolClass::Mcp => C::Mcp,
        api::ToolClass::Agent => C::Agent,
        api::ToolClass::Tasks => C::Tasks,
        api::ToolClass::Other => C::Other,
    }
}

impl Entry {
    pub(crate) fn from_item(item: &Item) -> Self {
        let mut item = item.clone();
        if let Some(text) = item.text.as_mut() {
            let (kept, cut) = capped(text);
            *text = kept;
            item.truncated |= cut;
        }
        if let Some(tool) = item.tool.as_mut() {
            for field in [&mut tool.detail, &mut tool.output] {
                if let Some(text) = field.as_mut() {
                    *text = capped(text).0;
                }
            }
        }
        let (class, path) = match &item.tool {
            Some(tool) => {
                let class = tool.class.map_or_else(|| class_of_name(&tool.name), render_class);
                let path = match &tool.path {
                    Some(path) => path.clone(),
                    // "Edit src/main.rs": the title of a step from before paths.
                    None if tool.class.is_none()
                        && matches!(class, render::ToolClass::Edit | render::ToolClass::Read) =>
                    {
                        tool.title.split_once(' ').map(|(_, p)| p.trim().to_owned()).unwrap_or_default()
                    }
                    None => String::new(),
                };
                (class, path)
            }
            None => (render::ToolClass::Other, String::new()),
        };
        Self { item, streaming: false, class, path }
    }

    fn text(&self) -> &str {
        self.item.text.as_deref().unwrap_or_default()
    }

    /// What the timeline reads of it; `None` for a kind it doesn't know.
    fn item_in(&self) -> Option<render::ItemIn<'_>> {
        let item = &self.item;
        let kind = match item.kind {
            api::ItemKind::User => render::ItemKind::User,
            api::ItemKind::Assistant => render::ItemKind::Assistant,
            api::ItemKind::Tool => render::ItemKind::Tool,
            api::ItemKind::Notice => render::ItemKind::Notice,
            api::ItemKind::Reasoning => render::ItemKind::Reasoning,
            api::ItemKind::Unknown => return None,
        };
        let tool = item.tool.as_ref().map(|t| render::ToolIn {
            name: &t.name,
            title: &t.title,
            class: self.class,
            detail: t.detail.as_deref().unwrap_or_default(),
            path: &self.path,
            status: match t.status {
                api::ToolStatus::Running => render::ToolStatus::Running,
                api::ToolStatus::Ok => render::ToolStatus::Ok,
                api::ToolStatus::Error => render::ToolStatus::Error,
                api::ToolStatus::Denied => render::ToolStatus::Denied,
            },
            output: t.output.as_deref().unwrap_or_default(),
            by_rule: t.by_rule,
        });
        Some(render::ItemIn {
            seq: item.seq,
            id: &item.id,
            kind,
            ts_ms: item.ts_ms,
            end_ms: item.end_ms,
            rev: item.rev,
            text: self.text(),
            streaming: self.streaming,
            turn: item.turn.unwrap_or(0),
            group: item.group.as_deref(),
            phase: item.phase.map(|p| match p {
                api::Phase::Interim => render::Phase::Interim,
                api::Phase::Final => render::Phase::Final,
            }),
            from: item
                .origin
                .as_ref()
                .filter(|o| o.kind == OriginKind::Phone)
                .and_then(|o| o.device_name.as_deref())
                .unwrap_or_default(),
            deny_of: item.deny_of.as_deref(),
            tool,
            truncated: item.truncated,
        })
    }
}

/// Where an item goes among items ordered by `seq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    /// It replaces the item there.
    Replace(usize),
    /// It is new and goes there.
    Insert(usize),
    /// What's there is the same or newer.
    Stale,
}

pub(crate) fn place(entries: &[Arc<Entry>], entry: &Entry) -> Place {
    match entries.binary_search_by_key(&entry.item.seq, |e| e.item.seq) {
        Ok(i) if *entries[i] == *entry || (entries[i].item.rev > entry.item.rev && !entries[i].streaming) => {
            Place::Stale
        }
        Ok(i) => Place::Replace(i),
        Err(i) => Place::Insert(i),
    }
}

/// Items to drop from the front after inserting at `at` (`len` items
/// before): only an item added at the end pushes the oldest out, so reading
/// earlier items keeps them until the next new one.
pub(crate) fn overflow(len: usize, at: usize) -> usize {
    if at == len { (len + 1).saturating_sub(OPEN_ITEMS) } else { 0 }
}

/// Adds the part of `text`, which starts at byte `offset` of the item's
/// text, that the item doesn't have yet; true when it added some.
pub(crate) fn append(entry: &mut Entry, offset: usize, text: &str) -> bool {
    let current = entry.item.text.get_or_insert_default();
    let len = current.len();
    if offset > len || offset + text.len() <= len || !text.is_char_boundary(len - offset) {
        return false;
    }
    let new = &text[len - offset..];
    let kept = api::cut(new, ROW_TEXT.saturating_sub(len));
    entry.item.truncated |= kept.len() < new.len();
    entry.streaming = true;
    if kept.is_empty() {
        return false;
    }
    current.push_str(kept);
    true
}

/// What the timeline model applies, in order, on the GUI thread. The page's
/// other properties are read again on [`Changes::AGENTS`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum UiUpdate {
    /// The rows changed so.
    Ops(Vec<RowOp>),
    /// The rows are these now.
    Reset(Vec<Arc<ModelRow>>),
}

/// Lets streaming text reach the rows at most 10 times a second.
#[derive(Debug, Default)]
struct DeltaGate {
    last: Option<Instant>,
    scheduled: bool,
}

impl DeltaGate {
    /// Text arrived. Returns how long to wait before [`flush`](Self::flush)
    /// when a flush must be scheduled now (none is pending).
    fn push(&mut self, now: Instant) -> Option<Duration> {
        if std::mem::replace(&mut self.scheduled, true) {
            return None;
        }
        Some(self.last.map_or(Duration::ZERO, |last| (last + DELTA_EVERY).saturating_duration_since(now)))
    }

    fn flush(&mut self, now: Instant) {
        self.scheduled = false;
        self.last = Some(now);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenKind {
    Thread,
    Session,
}

/// What a delta did to the open view.
#[derive(Debug, PartialEq, Eq)]
enum DeltaOutcome {
    /// An item began with it.
    New,
    /// Text was added; the rows follow after the wait when one is given.
    Text(Option<Duration>),
    Ignored,
    /// Text before it is missing: read the thread again.
    Gap,
}

/// Requests answered while the view is open that the transcript keeps a
/// line for ("Allowed on Pixel 8 Pro", "Expired").
const RESOLVED_MARKS: usize = 64;
/// Rows the user opened or set to wrap, per view.
const OPEN_KEYS: usize = 512;

/// The thread or session the page shows: its items and what the page did
/// with them.
#[derive(Debug)]
struct OpenView {
    generation: u64,
    kind: OpenKind,
    id: String,
    entries: Vec<Arc<Entry>>,
    asks: Vec<Approval>,
    /// Marks for requests answered while it was open.
    resolved: Vec<render::Mark>,
    more: bool,
    loading: bool,
    /// A read to fill a gap in streaming text is running.
    mending: bool,
    gate: DeltaGate,
    /// Keys of the rows the user opened.
    open_keys: Vec<String>,
    /// Keys of the code blocks the user set to wrap.
    wraps: Vec<String>,
    /// Holds a request that arrives while the user types (§P8.2).
    guard: render::TypingGuard,
}

impl OpenView {
    fn new(generation: u64, kind: OpenKind, id: String) -> Self {
        Self {
            generation,
            kind,
            id,
            entries: Vec::new(),
            asks: Vec::new(),
            resolved: Vec::new(),
            more: false,
            loading: true,
            mending: false,
            gate: DeltaGate::default(),
            open_keys: Vec::new(),
            wraps: Vec::new(),
            guard: render::TypingGuard::new(),
        }
    }

    fn is(&self, kind: OpenKind, id: &str) -> bool {
        self.kind == kind && self.id == id
    }

    /// Applies an item; true when it changed something.
    fn apply(&mut self, entry: Entry) -> bool {
        match place(&self.entries, &entry) {
            Place::Stale => false,
            Place::Replace(i) => {
                self.entries[i] = Arc::new(entry);
                true
            }
            Place::Insert(i) => {
                let drop = overflow(self.entries.len(), i);
                self.entries.insert(i, Arc::new(entry));
                if drop > 0 {
                    self.entries.drain(..drop);
                    self.more = true;
                }
                true
            }
        }
    }

    fn delta(&mut self, seq: u64, item_id: &str, offset: usize, text: &str, now: Instant) -> DeltaOutcome {
        match self.entries.binary_search_by_key(&seq, |e| e.item.seq) {
            Ok(i) => {
                if offset > self.entries[i].text().len() {
                    return DeltaOutcome::Gap;
                }
                if append(Arc::make_mut(&mut self.entries[i]), offset, text) {
                    DeltaOutcome::Text(self.gate.push(now))
                } else {
                    DeltaOutcome::Ignored
                }
            }
            Err(_) if offset == 0 => {
                let mut entry =
                    Entry::from_item(&Item::new(seq, item_id, api::ItemKind::Assistant, now_ms()));
                append(&mut entry, 0, text);
                entry.streaming = true;
                self.apply(entry);
                DeltaOutcome::New
            }
            Err(_) => DeltaOutcome::Gap,
        }
    }

    /// Opens or closes a row.
    fn toggle(&mut self, key: &str) {
        toggle_key(&mut self.open_keys, key);
    }

    /// The marks the timeline puts under tools: requests waiting (held while
    /// the user types), and answered elsewhere or ended. A step a rule
    /// allowed gets the rule's line from the timeline, in place of who
    /// answered.
    fn marks(&self) -> Vec<render::Mark> {
        let by_rule = |id: &str| {
            self.entries.iter().any(|e| e.item.id == id && e.item.tool.as_ref().is_some_and(|t| t.by_rule))
        };
        let mut marks: Vec<render::Mark> = self
            .asks
            .iter()
            .filter_map(|a| {
                let kind = if self.guard.held(&a.token) {
                    render::MarkKind::Held
                } else {
                    render::MarkKind::Waiting
                };
                Some(render::Mark { item_id: a.item_id.clone()?, kind, by: String::new() })
            })
            .collect();
        for mark in &self.resolved {
            if !by_rule(&mark.item_id) && !marks.iter().any(|m| m.item_id == mark.item_id) {
                marks.push(mark.clone());
            }
        }
        marks
    }

    /// A request ended: the line its tool keeps, if it says something. Not
    /// for one this PC answered by hand (the tool's line already says it).
    fn resolve(&mut self, token: &str, outcome: ApprovalOutcome, by: Option<&Origin>) -> bool {
        let Some(at) = self.asks.iter().position(|a| a.token == token) else { return false };
        let ask = self.asks.remove(at);
        let phone = by.filter(|b| b.kind == OriginKind::Phone).and_then(|b| b.device_name.clone());
        let kind = match (outcome, &phone) {
            (ApprovalOutcome::Allowed, Some(_)) => Some(render::MarkKind::Allowed),
            (ApprovalOutcome::Denied, Some(_)) => Some(render::MarkKind::Denied),
            (ApprovalOutcome::Allowed | ApprovalOutcome::Denied, None) => None,
            (ApprovalOutcome::Expired, _) => Some(render::MarkKind::Expired),
            (ApprovalOutcome::Cancelled, _) => Some(render::MarkKind::Cancelled),
        };
        if let (Some(kind), Some(item_id)) = (kind, ask.item_id) {
            if self.resolved.len() >= RESOLVED_MARKS {
                self.resolved.remove(0);
            }
            self.resolved.push(render::Mark { item_id, kind, by: phone.unwrap_or_default() });
        }
        true
    }

    /// The guard heard of the requests now waiting.
    fn guard_requests(&mut self, now_ms: u64) {
        let ids: Vec<&str> = self.asks.iter().map(|a| a.token.as_str()).collect();
        self.guard.on(GuardEvent::Requests { ids: &ids, now_ms });
    }

    /// Tokens the guard holds, in order.
    fn held(&self) -> Vec<String> {
        self.asks.iter().filter(|a| self.guard.held(&a.token)).map(|a| a.token.clone()).collect()
    }
}

/// Adds `key` to `keys`, or removes it when it's there; at most
/// [`OPEN_KEYS`], the oldest first out.
fn toggle_key(keys: &mut Vec<String>, key: &str) {
    match keys.iter().position(|k| k == key) {
        Some(at) => {
            keys.remove(at);
        }
        None => {
            if keys.len() >= OPEN_KEYS {
                keys.remove(0);
            }
            keys.push(key.to_owned());
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

// ---- The timeline's rows ----

/// What the rows are computed from, taken under the bridge's lock.
#[derive(Debug, Clone)]
struct Snapshot {
    generation: u64,
    entries: Vec<Arc<Entry>>,
    state: render::ThreadState,
    since_ms: u64,
    marks: Vec<render::Mark>,
    open: Vec<String>,
    wraps: Vec<String>,
}

fn thread_state(state: State) -> render::ThreadState {
    match state {
        State::Working => render::ThreadState::Working,
        State::Waiting => render::ThreadState::Waiting,
        State::Done => render::ThreadState::Done,
        State::Failed => render::ThreadState::Failed,
        State::Interrupted => render::ThreadState::Interrupted,
    }
}

/// A session as a thread's state: what it does now, for the live line and
/// the folds.
fn session_state(state: SessionState) -> render::ThreadState {
    match state {
        SessionState::Working => render::ThreadState::Working,
        SessionState::NeedsYou | SessionState::NeedsYouGuess => render::ThreadState::Waiting,
        SessionState::Idle | SessionState::Ended => render::ThreadState::Done,
    }
}

fn snapshot(s: &BridgeState) -> Option<Snapshot> {
    let open = s.open.as_ref()?;
    let (state, since_ms) = match open.kind {
        OpenKind::Thread => s
            .threads
            .get(&open.id)
            .map_or((render::ThreadState::Done, 0), |t| (thread_state(t.state), t.state_since_ms)),
        OpenKind::Session => s
            .sessions
            .iter()
            .find(|x| x.id == open.id)
            .map_or((render::ThreadState::Done, 0), |x| (session_state(x.state), x.state_since_ms)),
    };
    Some(Snapshot {
        generation: open.generation,
        entries: open.entries.clone(),
        state,
        since_ms,
        marks: open.marks(),
        open: open.open_keys.clone(),
        wraps: open.wraps.clone(),
    })
}

/// The rows on screen for the open view, and the timeline that makes them.
#[derive(Default)]
struct Render {
    generation: u64,
    timeline: render::Timeline,
    shown: Vec<Arc<ModelRow>>,
    /// Each row on screen with what it was built from, to reuse it unchanged.
    built: HashMap<String, (render::Row, rows::Extra, Arc<ModelRow>)>,
    palette: CodePalette,
}

impl std::fmt::Debug for Render {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Render")
            .field("generation", &self.generation)
            .field("shown", &self.shown.len())
            .finish_non_exhaustive()
    }
}

impl Render {
    /// Starts over for the view `generation` (none shown yet).
    fn reset(&mut self, generation: u64) {
        self.generation = generation;
        self.timeline = render::Timeline::new();
        self.shown.clear();
        self.built.clear();
    }

    /// The rows for `snap`, and what changed from the ones shown; `None`
    /// when `snap` is of another view than this one.
    fn update(&mut self, snap: &Snapshot) -> Option<Vec<RowOp>> {
        if snap.generation != self.generation {
            return None;
        }
        let items: Vec<render::ItemIn<'_>> = snap.entries.iter().filter_map(|e| e.item_in()).collect();
        let by_id: HashMap<&str, &Entry> = snap.entries.iter().map(|e| (e.item.id.as_str(), &**e)).collect();
        let ctx = render::Ctx {
            state: snap.state,
            state_since_ms: snap.since_ms,
            now_ms: now_ms(),
            marks: &snap.marks,
            open: &snap.open,
        };
        let rows = self.timeline.rows(&items, &ctx);
        let mut built = HashMap::with_capacity(rows.len());
        let mut shown = Vec::with_capacity(rows.len());
        for row in rows {
            let entry = by_id.get(row.item_id.as_str());
            let extra = rows::Extra {
                when_ms: entry.map_or(0, |e| e.item.end_ms.unwrap_or(e.item.ts_ms)),
                truncated: entry.is_some_and(|e| e.item.truncated),
                wrapped: snap.wraps.contains(&row.key),
            };
            let model = match self.built.remove(&row.key) {
                Some((old, old_extra, model)) if old == row && old_extra == extra => model,
                _ => {
                    let source = entry
                        .and_then(|e| e.item.tool.as_ref())
                        .and_then(|t| t.detail.as_deref())
                        .unwrap_or_default();
                    Arc::new(rows::model_row(&row, extra, source, &self.palette))
                }
            };
            shown.push(model.clone());
            built.insert(row.key.clone(), (row, extra, model));
        }
        let ops = rows::row_ops(&self.shown, &shown);
        self.shown = shown;
        self.built = built;
        Some(ops)
    }

    fn row(&self, key: &str) -> Option<&Arc<ModelRow>> {
        self.shown.iter().find(|r| r.key() == key)
    }
}

#[derive(Debug, Default)]
struct RenderGate {
    running: bool,
    again: bool,
}

/// The rows of the open view are computed again, off the GUI thread: one
/// run at a time, and once more after it when something changed meanwhile.
fn rerender() {
    {
        let mut gate = bridge().render_gate();
        if gate.running {
            gate.again = true;
            return;
        }
        gate.running = true;
    }
    if core_host::try_host().is_none() {
        bridge().render_gate().running = false;
        return;
    }
    core_host::spawn(async {
        let _ = tokio::task::spawn_blocking(|| render_until_settled(bridge())).await;
    });
}

fn render_until_settled(bridge: &Bridge) {
    /// A run that ends by panicking lets the next one start.
    struct Running<'a>(&'a Bridge);
    impl Drop for Running<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                self.0.render_gate().running = false;
            }
        }
    }
    let _running = Running(bridge);
    loop {
        let snap = snapshot(&bridge.lock());
        if let Some(snap) = snap {
            render_once(bridge, &snap);
        }
        let mut gate = bridge.render_gate();
        if !gate.again {
            gate.running = false;
            return;
        }
        gate.again = false;
    }
}

fn render_once(bridge: &Bridge, snap: &Snapshot) {
    let mut render = bridge.render();
    let Some(ops) = render.update(snap) else { return };
    if !ops.is_empty() {
        bridge.send_ui(UiUpdate::Ops(ops), &render.shown);
    }
}

/// The view `generation` starts with no rows (opened, closed, or its host
/// went away); the model is told at once.
fn reset_rows(bridge: &Bridge, generation: u64) {
    let mut render = bridge.render();
    render.reset(generation);
    bridge.send_ui(UiUpdate::Reset(Vec::new()), &[]);
}

// ---- The timeline model's queue ----

type Waker = Arc<dyn Fn() -> bool + Send + Sync>;

/// Updates for `AgentTimeline`, applied on the GUI thread in order. When the
/// GUI falls behind, what's queued gives way to one `Reset`.
#[derive(Default)]
struct Feed {
    queue: VecDeque<UiUpdate>,
    scheduled: bool,
    waker: Option<Waker>,
}

impl std::fmt::Debug for Feed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Feed")
            .field("queued", &self.queue.len())
            .field("scheduled", &self.scheduled)
            .field("attached", &self.waker.is_some())
            .finish()
    }
}

impl Feed {
    /// Queues `update`, after which the rows are `after`; true when the GUI
    /// must be woken to take it.
    fn push(&mut self, update: UiUpdate, after: &[Arc<ModelRow>]) -> bool {
        if self.queue.len() >= UI_QUEUE {
            self.queue.clear();
            self.queue.push_back(UiUpdate::Reset(after.to_vec()));
        } else {
            self.queue.push_back(update);
        }
        !std::mem::replace(&mut self.scheduled, true)
    }

    fn take(&mut self) -> Vec<UiUpdate> {
        self.scheduled = false;
        self.queue.drain(..).collect()
    }
}

// ---- Phones following a session ----

/// One extra host connection that watches a session for phones.
#[derive(Debug)]
struct WatchConn {
    session: String,
    generation: u64,
    task: Option<tokio::task::AbortHandle>,
}

/// What a phone's watch needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchPlan {
    /// It already watches that session.
    Have,
    /// Another phone's connection watches it.
    Share,
    /// A new connection is needed and allowed.
    Open,
    /// Two connections are in use for other sessions.
    Busy,
}

/// The sessions phones follow: at most [`PHONE_WATCHES`] connections, shared
/// by phones following the same session.
#[derive(Debug, Default)]
struct PhoneWatches {
    conns: Vec<WatchConn>,
    by_phone: HashMap<DeviceId, String>,
}

impl PhoneWatches {
    fn plan(&self, phone: &DeviceId, session: &str) -> WatchPlan {
        if self.by_phone.get(phone).is_some_and(|s| s == session) {
            WatchPlan::Have
        } else if self.conns.iter().any(|c| c.session == session) {
            WatchPlan::Share
        } else if self.conns.len().saturating_sub(usize::from(self.only_watcher(phone))) < PHONE_WATCHES {
            WatchPlan::Open
        } else {
            WatchPlan::Busy
        }
    }

    /// `phone` alone holds its current connection (it would be freed).
    fn only_watcher(&self, phone: &DeviceId) -> bool {
        self.by_phone.get(phone).is_some_and(|s| self.by_phone.iter().filter(|(_, w)| *w == s).count() == 1)
    }

    /// `phone` follows `session` (with `conn` when it opened one). Returns
    /// the connections no phone uses any more, to stop.
    fn watch(&mut self, phone: DeviceId, session: String, conn: Option<WatchConn>) -> Vec<WatchConn> {
        let mut unused = self.unwatch(&phone);
        if let Some(conn) = conn {
            if self.conns.iter().any(|c| c.session == conn.session) {
                unused.push(conn);
            } else {
                self.conns.push(conn);
            }
        }
        self.by_phone.insert(phone, session);
        unused
    }

    /// `phone` follows nothing; returns the connections that became unused.
    fn unwatch(&mut self, phone: &DeviceId) -> Vec<WatchConn> {
        if self.by_phone.remove(phone).is_none() {
            return Vec::new();
        }
        let (kept, unused): (Vec<WatchConn>, Vec<WatchConn>) = std::mem::take(&mut self.conns)
            .into_iter()
            .partition(|c| self.by_phone.values().any(|s| *s == c.session));
        self.conns = kept;
        unused
    }

    /// A connection closed by itself (the host went away): phones on it
    /// follow nothing now.
    fn ended(&mut self, generation: u64) {
        let Some(at) = self.conns.iter().position(|c| c.generation == generation) else { return };
        let conn = self.conns.remove(at);
        self.by_phone.retain(|_, s| *s != conn.session);
    }
}

fn stop(conns: Vec<WatchConn>) {
    for conn in conns {
        if let Some(task) = conn.task {
            task.abort();
        }
    }
}

// ---- The bridge ----

#[derive(Debug, Default)]
struct BridgeState {
    client: Option<Client>,
    /// Counts connections, so what was told one isn't taken as told the next.
    conn: u64,
    status: Status,
    /// The connected host is older than this build (or an upgrade replaced its
    /// program) and threads ran on it: it's replaced once none runs, or when
    /// the user restarts it.
    outdated: bool,
    /// The host doesn't know `sessions.list` (it's older).
    sessions_unsupported: bool,
    /// The host takes a denial with the user's words (`deny_message`).
    says_why: bool,
    /// The page's side panels the user closed.
    panels: Panels,
    /// The user changed them before the saved ones were read.
    panels_set: bool,
    agents: Vec<AgentInfo>,
    folders: Vec<Folder>,
    threads: HashMap<String, ThreadInfo>,
    /// Sessions started outside G7, in the host's order.
    sessions: Vec<Session>,
    /// The host follows sessions started outside G7 (`None`: not known).
    following: Option<bool>,
    /// The user started the host, or added a folder.
    user_started: bool,
    /// The Agents page is on screen.
    page_open: bool,
    /// G7 Link's window is the active one.
    window_active: bool,
    open: Option<OpenView>,
    /// What `ui.viewing` last told the host on connection `conn`.
    viewing_sent: Option<(u64, UiViewingParams)>,
    /// The page's requests that haven't answered yet.
    busy: u32,
    watches: PhoneWatches,
    next_gen: u64,
    /// An export waiting for the page's save dialog.
    export: Option<PendingExport>,
}

/// Markdown fetched for "Export…", kept until it's saved or the dialog is closed.
#[derive(Default)]
struct PendingExport {
    path: std::path::PathBuf,
    text: String,
}

/// Never prints the transcript.
impl std::fmt::Debug for PendingExport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingExport").field("bytes", &self.text.len()).finish_non_exhaustive()
    }
}

impl BridgeState {
    fn next_gen(&mut self) -> u64 {
        self.next_gen += 1;
        self.next_gen
    }

    fn open_is(&self, kind: OpenKind, id: &str) -> bool {
        self.open.as_ref().is_some_and(|o| o.is(kind, id))
    }
}

#[derive(Debug, Default)]
struct ListsGate {
    last: Option<Instant>,
    scheduled: bool,
}

#[derive(Debug)]
struct Bridge {
    state: Mutex<BridgeState>,
    /// The open view's rows; never locked while `state` is held.
    render: Mutex<Render>,
    render_gate: Mutex<RenderGate>,
    feed: Mutex<Feed>,
    lists: Mutex<ListsGate>,
    wake: Notify,
    /// The user asked to restart an outdated host now.
    restart: Notify,
    /// One `ui.viewing` at a time, so the last one sent is the latest.
    viewing: tokio::sync::Mutex<()>,
    connected: watch::Sender<bool>,
}

impl Bridge {
    fn new() -> Self {
        Self {
            state: Mutex::default(),
            render: Mutex::default(),
            render_gate: Mutex::default(),
            feed: Mutex::default(),
            lists: Mutex::default(),
            wake: Notify::new(),
            restart: Notify::new(),
            viewing: tokio::sync::Mutex::new(()),
            connected: watch::Sender::new(false),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BridgeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn feed(&self) -> std::sync::MutexGuard<'_, Feed> {
        self.feed.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn render(&self) -> std::sync::MutexGuard<'_, Render> {
        self.render.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn render_gate(&self) -> std::sync::MutexGuard<'_, RenderGate> {
        self.render_gate.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_status(&self, status: Status) {
        let was = std::mem::replace(&mut self.lock().status, status);
        if was != status {
            changed();
        }
    }

    fn remember(&self, info: &ThreadInfo) {
        let mut s = self.lock();
        s.threads.insert(info.id.clone(), info.clone());
        if s.threads.len() > KNOWN_THREADS
            && let Some(oldest) = s.threads.values().min_by_key(|t| t.updated_ms).map(|t| t.id.clone())
        {
            s.threads.remove(&oldest);
        }
    }

    fn known(&self, thread_id: &str) -> Option<ThreadInfo> {
        self.lock().threads.get(thread_id).cloned()
    }

    /// Some thread the bridge knows is working or waiting.
    fn any_running(&self) -> bool {
        self.lock().threads.values().any(|t| t.state.is_running())
    }

    fn catalog(&self) -> AgentCatalog {
        let s = self.lock();
        catalog(&s.agents, &s.folders)
    }

    /// Sends an update to the timeline model, if one is on screen; the rows
    /// are `after` once it's applied.
    fn send_ui(&self, update: UiUpdate, after: &[Arc<ModelRow>]) {
        let waker = {
            let mut feed = self.feed();
            if feed.waker.is_none() {
                return;
            }
            if !feed.push(update, after) {
                return;
            }
            feed.waker.clone()
        };
        if let Some(waker) = waker
            && !waker()
        {
            // The GUI went away; it reads everything again when it's back.
            *self.feed() = Feed::default();
        }
    }
}

fn bridge() -> &'static Bridge {
    static BRIDGE: OnceLock<Bridge> = OnceLock::new();
    BRIDGE.get_or_init(Bridge::new)
}

fn lock() -> std::sync::MutexGuard<'static, BridgeState> {
    bridge().lock()
}

/// The page's properties changed: it reads them again ([`view`]).
fn changed() {
    if let Some(host) = core_host::try_host() {
        host.hub.update(|_| Changes::AGENTS);
    }
}

/// The lists changed: shown again at most every 250 ms.
fn lists_changed() {
    let gate = &bridge().lists;
    let wait = {
        let mut g = gate.lock().unwrap_or_else(|e| e.into_inner());
        if g.scheduled {
            return;
        }
        g.scheduled = true;
        g.last.map_or(Duration::ZERO, |last| (last + LISTS_EVERY).saturating_duration_since(Instant::now()))
    };
    if core_host::try_host().is_none() {
        gate.lock().unwrap_or_else(|e| e.into_inner()).scheduled = false;
        return;
    }
    core_host::spawn(async move {
        tokio::time::sleep(wait).await;
        {
            let mut g = gate.lock().unwrap_or_else(|e| e.into_inner());
            g.scheduled = false;
            g.last = Some(Instant::now());
        }
        changed();
    });
}

/// Whether the host binary is on this PC: G7 Link then offers `agent.host`
/// and `agent.sessions`.
pub fn host_installed() -> bool {
    client::find_host_binary().is_some()
}

/// Starts the bridge: it connects to a running host at once, and starts the
/// host when the page, the user or a phone needs it.
pub fn start() {
    core_host::spawn(run());
    core_host::spawn(async {
        let read = tokio::task::spawn_blocking(|| {
            core_host::try_data_dir()
                .map(|dir| crate::atomic_file::read_json::<Panels>(&dir.join(PANELS_FILE)))
        });
        if let Ok(Some(panels)) = read.await {
            let mut s = lock();
            if !s.panels_set {
                s.panels = panels;
            }
            drop(s);
            changed();
        }
    });
}

/// The page's side panels the user closed: the thread list and the
/// details beside the transcript. Remembered across restarts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Panels {
    #[serde(default)]
    list_hidden: bool,
    #[serde(default)]
    inspector_hidden: bool,
}

const PANELS_FILE: &str = "agents-panels.json";

/// The user shows or hides the thread list or the details (`None` leaves
/// one as it is). Saved at once, whole or not at all.
pub fn set_panels(list_hidden: Option<bool>, inspector_hidden: Option<bool>) {
    {
        let mut s = lock();
        let was = s.panels;
        s.panels.list_hidden = list_hidden.unwrap_or(was.list_hidden);
        s.panels.inspector_hidden = inspector_hidden.unwrap_or(was.inspector_hidden);
        s.panels_set = true;
        if s.panels == was {
            return;
        }
    }
    changed();
    let Some(dir) = core_host::try_data_dir() else { return };
    core_host::spawn(async move {
        // One save at a time, each of the newest state, so the last is the latest.
        static SAVING: Mutex<()> = Mutex::new(());
        let written = tokio::task::spawn_blocking(move || {
            let _one = SAVING.lock().unwrap_or_else(|e| e.into_inner());
            let panels = lock().panels;
            crate::atomic_file::write_json(&dir.join(PANELS_FILE), &panels)
        })
        .await;
        if let Ok(Err(error)) = written {
            tracing::warn!(%error, "the Agents page's panels weren't saved");
        }
    });
}

/// Something that decides whether the host is wanted changed (a toggle).
pub fn wake() {
    bridge().wake.notify_one();
}

/// A phone was unpaired: it may have been the one that wanted the host.
pub fn on_event(event: &g7link_core::NodeEvent) {
    if matches!(event, g7link_core::NodeEvent::DeviceRemoved(_)) {
        wake();
    }
}

// ---- Mapping (pure) ----

/// A wire word for one of the host's enums (they serialize as snake_case words).
fn word<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

/// What phones may start: every agent, the folders that exist (id and
/// label only), and supervised threads only.
pub(crate) fn catalog(agents: &[AgentInfo], folders: &[Folder]) -> AgentCatalog {
    AgentCatalog {
        agents: agents
            .iter()
            .map(|a| AgentCatalogAgent {
                id: a.id.as_str().to_owned(),
                name: a.name.clone(),
                installed: a.installed,
                signed_in: a.signed_in,
                login_hint: a.login_hint.clone(),
                auth: a.auth.as_ref().map(word),
            })
            .collect(),
        folders: folders
            .iter()
            .filter(|f| f.exists)
            .map(|f| AgentCatalogFolder { id: f.id.clone(), label: f.label.clone() })
            .collect(),
        modes: vec![agent::mode::SUPERVISED.to_owned()],
    }
}

/// The phone that started a thread, if one did.
fn started_by(info: &ThreadInfo) -> Option<DeviceId> {
    (info.origin.kind == OriginKind::Phone).then(|| info.origin.device_id.as_deref()?.parse().ok()).flatten()
}

/// The host's error code as a wire word: lowercase `[a-z0-9_]`, at most 40 bytes.
fn error_code(code: &str) -> String {
    code.chars()
        .take(agent::MAX_ERROR_CODE_BYTES)
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect()
}

/// A thread as `phone` sees it (`from_phone` says whether it started it).
pub(crate) fn thread(info: &ThreadInfo, phone: Option<&DeviceId>) -> AgentThread {
    AgentThread {
        id: info.id.clone(),
        agent: info.agent.as_str().to_owned(),
        folder: info.folder_label.clone(),
        title: info.title.clone(),
        state: word(&info.state),
        mode: word(&info.mode),
        from_phone: phone.is_some() && started_by(info).as_ref() == phone,
        updated: info.updated_ms,
        asks: info.pending,
        summary: info.summary.clone(),
        error: info.error.as_ref().map(|e| e.message.clone()),
        error_code: info.error.as_ref().map(|e| error_code(&e.code)).filter(|c| !c.is_empty()),
        step: info.step.clone(),
        since: (info.state_since_ms > 0).then_some(info.state_since_ms),
        plan: info.plan.as_ref().map(|plan| {
            plan.iter()
                .map(|step| AgentPlanStep { text: step.text.clone(), status: word(&step.status) })
                .collect()
        }),
    }
}

/// An item as phones get it, with every field. The core sends a phone
/// without `agent.items2` a reasoning item as a tool step, and never sends
/// a phone an absolute path (`docs/protocol/agents.md` §2.4).
pub(crate) fn item(item: &Item) -> AgentItem {
    AgentItem {
        seq: item.seq,
        id: item.id.clone(),
        kind: word(&item.kind),
        when: item.ts_ms,
        text: item.text.clone(),
        truncated: item.truncated,
        tool: item.tool.as_ref().map(|t| AgentTool {
            name: t.name.clone(),
            title: t.title.clone(),
            detail: t.detail.clone(),
            status: word(&t.status),
            output: t.output.clone(),
            class: t.class.as_ref().map(word),
            path: t.path.clone(),
            by_rule: t.by_rule,
        }),
        turn: item.turn,
        phase: item.phase.as_ref().map(word),
        end: item.end_ms,
        group: item.group.clone(),
        deny_of: item.deny_of.clone(),
    }
}

/// A guessed "needs you", in either of the host's two forms.
fn guessed_needs_you(s: &Session) -> bool {
    s.state == SessionState::NeedsYouGuess || (s.state == SessionState::NeedsYou && s.state_guess)
}

/// A session as phones see it: its folder's name, never a path; a guessed
/// "needs you" as `may_need_you`, which is never counted.
pub(crate) fn session(s: &Session) -> AgentSession {
    let state =
        if guessed_needs_you(s) { agent::session_state::MAY_NEED_YOU.to_owned() } else { word(&s.state) };
    AgentSession {
        id: s.id.clone(),
        agent: s.agent.as_str().to_owned(),
        folder: s.folder_label.clone(),
        title: s.title.clone(),
        state,
        guess: s.state_guess || s.state == SessionState::NeedsYouGuess,
        source: word(&s.source),
        updated: s.updated_ms,
        step: s.step.clone(),
        summary: s.summary.clone(),
        waiting_for: s.waiting_for.clone(),
        can_continue: s.can_continue,
        blocked: s.blocked.as_ref().map(word),
        since: (s.state_since_ms > 0).then_some(s.state_since_ms),
    }
}

/// An approval as phones see it: its thread's title and agent added.
pub(crate) fn ask(approval: &Approval, info: Option<&ThreadInfo>) -> AgentAsk {
    AgentAsk {
        token: approval.token.clone(),
        thread: approval.thread_id.clone(),
        thread_title: info.map(|t| t.title.clone()).unwrap_or_default(),
        agent: info.map_or(AgentId::Claude, |t| t.agent).as_str().to_owned(),
        kind: word(&approval.kind),
        title: approval.title.clone(),
        command: approval.command.clone(),
        diff: approval.diff.clone(),
        path: approval.path.clone(),
        reason: approval.reason.clone(),
        truncated: approval.truncated,
        expires: approval.expires_ms,
        item: approval.item_id.clone(),
    }
}

/// The protocol error for a failed host call (contract §6.1).
pub(crate) fn failure(e: &ClientError) -> AgentFailure {
    if e.is_unknown_method() {
        // An older host, until it's replaced.
        return AgentFailure::Unsupported;
    }
    match e.code() {
        Some(ErrorCode::BadRequest) => AgentFailure::BadMessage,
        Some(ErrorCode::NotFound | ErrorCode::AlreadyAnswered) => AgentFailure::NotFound,
        Some(ErrorCode::Denied) => AgentFailure::Denied,
        Some(ErrorCode::Busy | ErrorCode::Limit) => AgentFailure::Busy,
        Some(ErrorCode::Unavailable | ErrorCode::UnsupportedVersion) => AgentFailure::Unsupported,
        Some(ErrorCode::Internal) => AgentFailure::Internal,
        None if e.is_unreachable() => AgentFailure::Unsupported,
        None => AgentFailure::Internal,
    }
}

fn asked_outcome(outcome: ApprovalOutcome) -> String {
    word(&outcome)
}

/// Who answered, as phones show it: the phone's name, or this PC's for an
/// answer given here (the launcher, a notification, the CLI).
fn answered_by(by: &Origin, this_pc: impl Fn() -> String) -> Option<String> {
    match by.kind {
        OriginKind::Phone => by.device_name.clone().filter(|n| !n.trim().is_empty()),
        OriginKind::Launcher | OriginKind::Cli => Some(this_pc()),
    }
}

fn this_pc_name() -> String {
    core_host::try_host()
        .and_then(|host| {
            host.hub.read(|s| match &s.status {
                Some(crate::state::CoreStatus::Ready { name, .. }) => Some(name.clone()),
                _ => None,
            })
        })
        .or_else(|| hostname::get().ok().and_then(|h| h.into_string().ok()))
        .unwrap_or_else(|| "PC".into())
}

/// Threads in the page's order: waiting first, then the newest.
fn listed_threads(threads: &HashMap<String, ThreadInfo>) -> Vec<&ThreadInfo> {
    let mut list: Vec<&ThreadInfo> = threads.values().collect();
    list.sort_by(|a, b| {
        (b.state == State::Waiting).cmp(&(a.state == State::Waiting)).then(b.updated_ms.cmp(&a.updated_ms))
    });
    list.truncate(LISTED_THREADS);
    list
}

/// What certainly needs the user: threads waiting for an answer and
/// sessions the agent itself reports waiting. A guess never counts.
fn needs_you<'a>(threads: impl IntoIterator<Item = &'a ThreadInfo>, sessions: &[Session]) -> i32 {
    let threads = threads.into_iter().filter(|t| t.state == State::Waiting).count();
    let sessions = sessions.iter().filter(|s| s.state == SessionState::NeedsYou && !s.state_guess).count();
    i32::try_from(threads + sessions).unwrap_or(i32::MAX)
}

fn thread_row(t: &ThreadInfo) -> Value {
    json!({
        "id": t.id,
        "agent": t.agent.as_str(),
        "agent_name": t.agent.display_name(),
        "folder": t.folder_label,
        "title": t.title,
        "state": word(&t.state),
        "mode": word(&t.mode),
        "from": match t.origin.kind {
            OriginKind::Launcher => "pc",
            OriginKind::Phone => "phone",
            OriginKind::Cli => "cli",
        },
        "device": t.origin.device_name.clone().unwrap_or_default(),
        "updated_ms": t.updated_ms,
        "since_ms": t.state_since_ms,
        "pending": t.pending,
        "step": t.step.clone().unwrap_or_default(),
        "summary": t.summary.clone().unwrap_or_default(),
        "error_code": t.error.as_ref().map(|e| error_code(&e.code)).unwrap_or_default(),
        "error": t.error.as_ref().map(|e| e.message.clone()).unwrap_or_default(),
        "continued": t.continued,
        "plan": t.plan.as_deref().unwrap_or_default().iter()
            .map(|step| json!({ "text": step.text, "status": word(&step.status) }))
            .collect::<Vec<_>>(),
    })
}

fn session_row(s: &Session, home: Option<&std::path::Path>) -> Value {
    json!({
        "id": s.id,
        "agent": s.agent.as_str(),
        "agent_name": s.agent.display_name(),
        "folder": s.folder_label,
        "shown_cwd": shown_path(&s.cwd, home),
        "title": s.title,
        // The page shows a guess as "needs you" with `guess`: "May need you".
        "state": if guessed_needs_you(s) { "needs_you".to_owned() } else { word(&s.state) },
        "guess": s.state_guess || s.state == SessionState::NeedsYouGuess,
        "since_ms": s.state_since_ms,
        "waiting_for": s.waiting_for.clone().unwrap_or_default(),
        "source": word(&s.source),
        "open": s.open,
        "updated_ms": s.updated_ms,
        "step": s.step.clone().unwrap_or_default(),
        "summary": s.summary.clone().unwrap_or_default(),
        "can_continue": s.can_continue,
        "blocked": s.blocked.as_ref().map(word).unwrap_or_default(),
    })
}

/// The open view's newest tool steps, oldest first.
fn steps_json(entries: &[Arc<Entry>]) -> Value {
    let mut steps: Vec<Value> = entries
        .iter()
        .rev()
        .filter_map(|e| e.item.tool.as_ref().filter(|_| e.item.kind == api::ItemKind::Tool).map(|t| (e, t)))
        .take(STEPS_SHOWN)
        .map(|(e, t)| {
            json!({
                "seq": e.item.seq.to_string(),
                "title": t.title,
                "status": word(&t.status),
                "detail": api::cut(t.detail.as_deref().unwrap_or_default(), STEP_DETAIL),
                "when_ms": e.item.ts_ms,
            })
        })
        .collect();
    steps.reverse();
    Value::Array(steps)
}

/// The files the open view's finished edits changed, with lines added and
/// removed, by path.
fn changes_json(entries: &[Arc<Entry>]) -> Value {
    let items: Vec<render::ItemIn<'_>> = entries.iter().filter_map(|e| e.item_in()).collect();
    let (files, _, _, _) = render::changed_files(&items);
    Value::Array(
        files
            .into_iter()
            .filter(|f| !f.path.is_empty())
            .map(|f| json!({ "path": f.path, "added": f.adds, "removed": f.dels }))
            .collect(),
    )
}

/// Diff rows the request dock shows; it scrolls inside.
const ASK_DIFF_ROWS: usize = 2_000;

/// The request the dock shows (`{}` for none), and how many more wait.
/// Oldest first; the shown one is the first the typing guard doesn't hold,
/// or the first when it holds them all. Answering it shows the next.
fn ask_json(asks: &[Approval], held: impl Fn(&str) -> bool) -> (Value, i32) {
    let mut order: Vec<&Approval> = asks.iter().collect();
    order.sort_by_key(|a| a.created_ms);
    let Some(position) = order.iter().position(|a| !held(&a.token)).or((!order.is_empty()).then_some(0))
    else {
        return (json!({}), 0);
    };
    let a = order[position];
    let total = order.len();
    let more = i32::try_from(total - 1).unwrap_or(i32::MAX);
    let diff =
        a.diff.as_deref().filter(|d| !d.is_empty()).map(|d| rows::diff_json(&render::diff(d), ASK_DIFF_ROWS));
    let ask = json!({
        "token": a.token,
        "kind": word(&a.kind),
        "title": a.title,
        "command": a.command.clone().unwrap_or_default(),
        "commandStyled": rows::escaped(a.command.as_deref().unwrap_or_default(), true),
        "diff": a.diff.clone().unwrap_or_default(),
        "diffDoc": diff.map_or(String::new(), |d| d.to_string()),
        "path": a.path.clone().unwrap_or_default(),
        "reason": a.reason.clone().unwrap_or_default(),
        "truncated": a.truncated,
        "can_allow_thread": a.can_allow_thread,
        "created_ms": a.created_ms,
        "position": position + 1,
        "total": total,
        "held": held(&a.token),
        "itemId": a.item_id.clone().unwrap_or_default(),
    });
    (ask, more)
}

// ---- The connection ----

/// What the host is wanted for right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// Connect if it runs, but don't start it.
    Look,
    /// The page, the user or a phone needs it: start it if needed.
    Run,
}

/// Where the host's news goes, and what decides whether it's wanted: the
/// node, or a test.
trait Relay {
    fn want(&self) -> Want;
    async fn catalog(&self, catalog: Option<AgentCatalog>);
    async fn event(&self, event: AgentEvent, started_by: Option<DeviceId>);
    async fn ask(&self, ask: AgentAsk);
    async fn asked(&self, asked: AgentAsked);
    async fn session(&self, session: Option<AgentSession>, removed: Option<String>);
}

/// The node: phones get the host's news through the core.
struct NodeRelay(Node);

impl Relay for NodeRelay {
    fn want(&self) -> Want {
        let node = &self.0;
        let (user_started, page_open) = {
            let s = lock();
            (s.user_started, s.page_open)
        };
        let phones = || {
            node.paired_devices().unwrap_or_default().iter().any(|d| {
                node.device_toggles(d.id)
                    .unwrap_or_default()
                    .iter()
                    .any(|(name, on)| *on && *name == agent::TOGGLE)
            })
        };
        if user_started || page_open || phones() { Want::Run } else { Want::Look }
    }

    async fn catalog(&self, catalog: Option<AgentCatalog>) {
        let _ = self.0.set_agent_catalog(catalog).await;
    }

    async fn event(&self, event: AgentEvent, started_by: Option<DeviceId>) {
        if let Err(e) = self.0.agent_event(event, started_by).await {
            tracing::debug!(error = %e, "an agent event wasn't passed on");
        }
    }

    async fn ask(&self, ask: AgentAsk) {
        if let Err(e) = self.0.agent_ask(ask).await {
            tracing::debug!(error = %e, "an agent ask wasn't passed on");
        }
    }

    async fn asked(&self, asked: AgentAsked) {
        let _ = self.0.agent_asked(asked).await;
    }

    async fn session(&self, session: Option<AgentSession>, removed: Option<String>) {
        if let Err(e) = self.0.agent_session(session, removed).await {
            tracing::debug!(error = %e, "a session change wasn't passed on");
        }
    }
}

/// What G7 Link says to the host: it understands reasoning items and the
/// fields added with them (phones that don't get them as the core makes them).
fn hello() -> HelloParams {
    HelloParams::new(ClientKind::Link, &[cap::ITEMS2])
}

async fn run() {
    let Some(node) = core_host::wait_for_node().await else { return };
    let launch = Launch::current();
    follow(bridge(), &NodeRelay(node), &launch, |want| {
        let launch = launch.clone();
        async move { connect(&launch, want).await }
    })
    .await;
}

/// Connects to the host, starting it when it's wanted. An outdated host
/// (older than this build, or its program replaced by an upgrade) that runs
/// no thread is replaced by the current one either way.
async fn connect(launch: &Launch, want: Want) -> Result<(Client, Events), ClientError> {
    if want == Want::Run {
        return Client::connect_or_spawn_as(launch, hello()).await;
    }
    let (client, events) = Client::connect(&launch.socket, hello()).await?;
    match client.replace(launch, When::Idle).await {
        Ok(Some(replaced)) => Ok(replaced),
        Ok(None) => Ok((client, events)),
        Err(e) => {
            tracing::warn!(error = %e, "the outdated agent host wasn't replaced");
            Ok((client, events))
        }
    }
}

/// Keeps a connection to the host: a running one is always followed; one
/// that isn't running is looked for every 15 s, or started (retrying after
/// 1, 2, 4 … 30 s) while something needs it. Phones hear there's nothing
/// while it's away.
async fn follow<R, F, Fut>(bridge: &Bridge, relay: &R, launch: &Launch, connect: F)
where
    R: Relay,
    F: Fn(Want) -> Fut,
    Fut: Future<Output = Result<(Client, Events), ClientError>>,
{
    let mut backoff = Duration::from_secs(1);
    let mut told_none = false;
    loop {
        let wanted = relay.want();
        if wanted == Want::Run {
            bridge.set_status(Status::Connecting);
        }
        match connect(wanted).await {
            Ok((client, events)) => {
                backoff = Duration::from_secs(1);
                // An outdated host replaced while followed hands over its successor.
                let (mut next, mut fresh) = (Some((client, events)), false);
                while let Some((client, events)) = next.take() {
                    next = serve(bridge, relay, launch, client, events, fresh).await;
                    fresh = true;
                }
                told_none = true;
            }
            Err(e) => {
                let status = match &e {
                    ClientError::NotInstalled => Status::NotInstalled,
                    ClientError::Io(_) if wanted == Want::Look => Status::NotRunning,
                    _ => Status::Failed,
                };
                if wanted == Want::Run {
                    tracing::info!(error = %e, "can't reach the agent host");
                }
                bridge.set_status(status);
                if !std::mem::replace(&mut told_none, true) {
                    relay.catalog(None).await;
                }
                let wait = if wanted == Want::Look || status == Status::NotInstalled {
                    LOOK_EVERY
                } else {
                    let wait = backoff;
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                    wait
                };
                let _ = tokio::time::timeout(wait, bridge.wake.notified()).await;
            }
        }
    }
}

/// Follows the host until the connection closes, or until an outdated host
/// was replaced: then the connection to its successor, to follow next. A
/// `fresh` host (that successor) isn't replaced on its own again, so a binary
/// older than this build can't make it restart after every turn.
async fn serve<R: Relay>(
    bridge: &Bridge,
    relay: &R,
    launch: &Launch,
    client: Client,
    mut events: Events,
    fresh: bool,
) -> Option<(Client, Events)> {
    let seeded = async {
        let subscribe = SubscribeParams { threads: ThreadSelection::All, deltas: true, sessions: true };
        client.subscribe_with(subscribe).await?;
        let agents = client.agents_list(false).await?;
        let folders = client.folders_list().await?;
        let threads = client.threads_list(Some(LISTED_THREADS as u32), None).await.unwrap_or_default();
        let sessions = client.sessions_list(Some(api::MAX_SESSIONS as u32)).await;
        let following = following::read(&client).await;
        Ok::<_, ClientError>((agents, folders, threads, sessions, following))
    };
    let (agents, folders, threads, sessions, following) = match seeded.await {
        Ok(seeded) => seeded,
        Err(e) => {
            tracing::warn!(error = %e, "the agent host didn't answer");
            bridge.set_status(Status::Failed);
            return None;
        }
    };
    let outdated = client.outdated(&launch.version).is_some();
    let says_why = client.host().has(cap::DENY_MESSAGE);
    if outdated {
        tracing::info!(
            host = %client.host().host_version,
            "the agent host is out of date; it's replaced once no thread runs on it"
        );
    }
    let reopen = {
        let mut s = bridge.lock();
        s.client = Some(client.clone());
        s.conn += 1;
        // A new connection views nothing until told.
        s.viewing_sent = Some((s.conn, UiViewingParams::default()));
        s.status = Status::Connected;
        s.outdated = outdated;
        s.says_why = says_why;
        s.sessions_unsupported = matches!(&sessions, Err(e) if e.is_unknown_method());
        s.agents = agents;
        s.folders = folders;
        s.threads = threads.into_iter().map(|t| (t.id.clone(), t)).collect();
        s.sessions = sessions.unwrap_or_default();
        s.following = following;
        s.open.as_ref().map(|o| (o.generation, o.kind, o.id.clone()))
    };
    bridge.connected.send_replace(true);
    changed();
    core_host::refresh_capabilities();
    relay.catalog(Some(bridge.catalog())).await;
    if let Some((generation, kind, id)) = reopen
        && core_host::try_host().is_some()
    {
        core_host::spawn(load(client.clone(), generation, kind, id, false));
    }
    sync_viewing();
    // Replaced on its own once its threads finish; given up after a failed try
    // (the user can still restart it).
    let mut may_swap = outdated && !fresh;
    let successor = loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { break None };
                let settles =
                    matches!(event, Event::ThreadState(_) | Event::ThreadDone(_) | Event::ThreadRemoved(_));
                relay_event(bridge, relay, &client, event).await;
                if may_swap && settles && !bridge.any_running() {
                    match swap(bridge, &client, launch, When::Idle).await {
                        Ok(Some(successor)) => break Some(successor),
                        Ok(None) => {}
                        Err(_) => may_swap = false,
                    }
                }
            }
            () = bridge.restart.notified() => {
                if !bridge.lock().outdated {
                    continue;
                }
                match swap(bridge, &client, launch, When::Now).await {
                    Ok(Some(successor)) => break Some(successor),
                    Ok(None) => {}
                    Err(_) => crate::bridge::app::show_message(tr!("Agents", "Couldn't restart agents")),
                }
            }
        }
    };
    let generation = {
        let mut s = bridge.lock();
        s.client = None;
        s.outdated = false;
        s.says_why = false;
        s.sessions_unsupported = false;
        s.threads.clear();
        s.sessions.clear();
        s.following = None;
        s.open.as_mut().map(|open| {
            open.entries.clear();
            open.asks.clear();
            open.resolved.clear();
            open.loading = true;
            open.gate = DeltaGate::default();
            open.generation
        })
    };
    if let Some(generation) = generation {
        reset_rows(bridge, generation);
    }
    core_host::refresh_capabilities();
    bridge.connected.send_replace(false);
    if successor.is_none() {
        bridge.set_status(Status::NotRunning);
        changed();
        relay.catalog(None).await;
    }
    successor
}

/// Replaces the outdated host (see [`Client::replace`]); while the user's
/// restart runs the page says it's connecting.
async fn swap(
    bridge: &Bridge,
    client: &Client,
    launch: &Launch,
    when: When,
) -> Result<Option<(Client, Events)>, ClientError> {
    if when == When::Now {
        bridge.set_status(Status::Connecting);
    }
    let result = client.replace(launch, when).await;
    match &result {
        Ok(Some((new, _))) => {
            tracing::info!(version = %new.host().host_version, "the agent host was updated")
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "the agent host couldn't be updated"),
    }
    if when == When::Now && !matches!(result, Ok(Some(_))) {
        bridge.set_status(Status::Connected);
    }
    result
}

async fn relay_event<R: Relay>(bridge: &Bridge, relay: &R, client: &Client, event: Event) {
    match event {
        Event::ThreadState(e) => {
            bridge.remember(&e.thread);
            relay.event(AgentEvent::state(thread(&e.thread, None)), started_by(&e.thread)).await;
            if bridge.lock().open_is(OpenKind::Thread, &e.thread.id) {
                changed();
                rerender();
            }
            lists_changed();
        }
        Event::ThreadItem(e) => {
            open_item(bridge, OpenKind::Thread, &e.thread_id, &e.item);
            relay.event(AgentEvent::item(e.thread_id, item(&e.item)), None).await;
        }
        Event::ThreadDelta(e) => {
            open_delta(bridge, &e.thread_id, e.seq, &e.item_id, e.offset as usize, &e.text);
            relay.event(AgentEvent::delta(e.thread_id, e.seq, e.item_id, e.offset, e.text), None).await;
        }
        Event::ThreadDone(e) => {
            if let Some(info) = bridge.known(&e.thread_id) {
                let outcome = word(&e.outcome);
                relay
                    .event(AgentEvent::done(thread(&info, None), &outcome, e.summary), started_by(&info))
                    .await;
            }
            lists_changed();
        }
        Event::ThreadRemoved(e) => {
            let was_open = {
                let mut s = bridge.lock();
                s.threads.remove(&e.thread_id);
                if s.open_is(OpenKind::Thread, &e.thread_id) {
                    s.open.take().map(|o| o.generation)
                } else {
                    None
                }
            };
            if let Some(generation) = was_open {
                reset_rows(bridge, generation);
                changed();
                sync_viewing();
            }
            lists_changed();
            relay.event(AgentEvent::removed(e.thread_id), None).await;
        }
        Event::ApprovalRequested(e) => {
            let mut info = bridge.known(&e.approval.thread_id);
            if info.is_none() {
                let read = ThreadReadParams {
                    thread_id: e.approval.thread_id.clone(),
                    limit: Some(1),
                    ..Default::default()
                };
                if let Ok(page) = client.thread_read(read).await {
                    bridge.remember(&page.thread);
                    info = Some(page.thread);
                }
            }
            {
                let mut s = bridge.lock();
                if let Some(open) = s.open.as_mut().filter(|o| o.is(OpenKind::Thread, &e.approval.thread_id))
                    && !open.asks.iter().any(|a| a.token == e.approval.token)
                {
                    open.asks.push(e.approval.clone());
                    open.guard_requests(now_ms());
                    let guard = guard_followup(open);
                    drop(s);
                    guard.run();
                    changed();
                    rerender();
                }
            }
            relay.ask(ask(&e.approval, info.as_ref())).await;
        }
        Event::ApprovalResolved(e) => {
            {
                let mut s = bridge.lock();
                if let Some(open) = s.open.as_mut()
                    && open.resolve(&e.token, e.outcome, e.by.as_ref())
                {
                    open.guard_requests(now_ms());
                    drop(s);
                    changed();
                    rerender();
                }
            }
            // Whether a rule allowed it reaches phones with its step (`by_rule`).
            let asked = AgentAsked {
                token: e.token,
                thread: e.thread_id,
                outcome: asked_outcome(e.outcome),
                by_name: e.by.as_ref().and_then(|by| answered_by(by, this_pc_name)),
                message: e.message.filter(|_| e.outcome == ApprovalOutcome::Denied),
            };
            relay.asked(asked).await;
        }
        Event::AgentsChanged(e) => {
            bridge.lock().agents = e.agents;
            changed();
            relay.catalog(Some(bridge.catalog())).await;
        }
        Event::FoldersChanged(e) => {
            bridge.lock().folders = e.folders;
            changed();
            relay.catalog(Some(bridge.catalog())).await;
        }
        Event::SettingsChanged(e) => {
            bridge.lock().following = Some(e.follow_sessions);
            changed();
        }
        // The host asks the PC to show a thread (a notification's Open).
        Event::UiOpen(e) => {
            if core_host::try_host().is_some() {
                crate::bridge::app::show_page("agents");
                open_thread(e.thread_id);
            }
        }
        Event::SessionState(e) => {
            let open = {
                let mut s = bridge.lock();
                upsert_session(&mut s.sessions, e.session.clone());
                s.open_is(OpenKind::Session, &e.session.id)
            };
            if open {
                changed();
            }
            lists_changed();
            relay.session(Some(session(&e.session)), None).await;
        }
        Event::SessionRemoved(e) => {
            bridge.lock().sessions.retain(|s| s.id != e.session_id);
            lists_changed();
            relay.session(None, Some(e.session_id)).await;
        }
        // Only the session the page shows: phones watch on their own connections.
        Event::SessionItem(e) => open_item(bridge, OpenKind::Session, &e.session_id, &e.item),
    }
}

/// Puts a changed session in its place: exact "needs you" first, then
/// working, guessed "needs you", idle and ended; the newest first in each.
fn upsert_session(sessions: &mut Vec<Session>, session: Session) {
    match sessions.iter_mut().find(|s| s.id == session.id) {
        Some(s) => *s = session,
        None => sessions.push(session),
    }
    sessions.sort_by(|a, b| a.rank().cmp(&b.rank()).then(b.updated_ms.cmp(&a.updated_ms)));
    sessions.truncate(api::MAX_SESSIONS);
}

/// An item of a thread or session: shown when it's the open one.
fn open_item(bridge: &Bridge, kind: OpenKind, id: &str, item: &Item) {
    let mut s = bridge.lock();
    let Some(open) = s.open.as_mut().filter(|o| o.is(kind, id)) else { return };
    let tool_changed = item.tool.is_some();
    if open.apply(Entry::from_item(item)) {
        drop(s);
        rerender();
        if tool_changed {
            // The inspector's steps and changes.
            changed();
        }
    }
}

/// Streaming text of a thread: the rows follow at most 10 times a second,
/// or the thread is read again when some went missing.
fn open_delta(bridge: &Bridge, thread_id: &str, seq: u64, item_id: &str, offset: usize, text: &str) {
    let now = Instant::now();
    let mut s = bridge.lock();
    let client = s.client.clone();
    let Some(open) = s.open.as_mut().filter(|o| o.is(OpenKind::Thread, thread_id)) else { return };
    match open.delta(seq, item_id, offset, text, now) {
        DeltaOutcome::New => {
            drop(s);
            rerender();
        }
        DeltaOutcome::Text(Some(wait)) => {
            let generation = open.generation;
            drop(s);
            if core_host::try_host().is_some() {
                core_host::spawn(async move {
                    tokio::time::sleep(wait).await;
                    flush_deltas(generation);
                });
            }
        }
        DeltaOutcome::Text(None) | DeltaOutcome::Ignored => {}
        DeltaOutcome::Gap => {
            if std::mem::replace(&mut open.mending, true) {
                return;
            }
            let (generation, id) = (open.generation, open.id.clone());
            drop(s);
            if let Some(client) = client
                && core_host::try_host().is_some()
            {
                core_host::spawn(async move {
                    let params = ThreadReadParams {
                        thread_id: id,
                        after_seq: Some(seq.saturating_sub(1)),
                        limit: Some(READ_PAGE),
                        ..Default::default()
                    };
                    let items = client.thread_read(params).await.map(|page| page.items).unwrap_or_default();
                    {
                        let mut s = lock();
                        let Some(open) = s.open.as_mut().filter(|o| o.generation == generation) else {
                            return;
                        };
                        open.mending = false;
                        for item in &items {
                            open.apply(Entry::from_item(item));
                        }
                    }
                    rerender();
                });
            }
        }
    }
}

/// The streaming text joined since the last time reaches the rows.
fn flush_deltas(generation: u64) {
    {
        let mut s = lock();
        let Some(open) = s.open.as_mut().filter(|o| o.generation == generation) else { return };
        open.gate.flush(Instant::now());
    }
    rerender();
}

/// What follows a change to the typing guard once the lock is let go: a
/// timer that lets held requests go when the user stops typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GuardFollowup {
    generation: u64,
    idle_at: Option<u64>,
}

fn guard_followup(open: &OpenView) -> GuardFollowup {
    GuardFollowup { generation: open.generation, idle_at: open.guard.idle_at() }
}

impl GuardFollowup {
    fn run(self) {
        let Some(at) = self.idle_at else { return };
        if core_host::try_host().is_none() {
            return;
        }
        let wait = Duration::from_millis(at.saturating_sub(now_ms()));
        core_host::spawn(async move {
            tokio::time::sleep(wait).await;
            guard_idle(self.generation, at);
        });
    }
}

/// The user stopped typing at `at`: held requests take the dock. A later
/// keystroke moved the time on, and its own timer comes later.
fn guard_idle(generation: u64, at: u64) {
    {
        let mut s = lock();
        let Some(open) = s.open.as_mut().filter(|o| o.generation == generation) else { return };
        if open.guard.idle_at() != Some(at) {
            return;
        }
        open.guard.on(GuardEvent::Idle);
    }
    changed();
    rerender();
}

/// The connected client, waiting a little for one when a request needs it.
async fn connected_client() -> Option<Client> {
    if let Some(client) = lock().client.clone() {
        return Some(client);
    }
    wake();
    let mut connected = bridge().connected.subscribe();
    let _ = tokio::time::timeout(CONNECT_WAIT, connected.wait_for(|on| *on)).await;
    lock().client.clone()
}

fn device_name(device: &DeviceId) -> String {
    core_host::node()
        .and_then(|node| node.paired_devices().ok())
        .and_then(|devices| devices.into_iter().find(|d| d.id == *device))
        .map(|d| d.info.name)
        .unwrap_or_else(|| device.short())
}

// ---- Phones' requests ----

/// Answers a phone's request ([`g7link_core::Platform::agent_request`]):
/// called on a blocking thread, after the core checked the toggles, the
/// limits and the mode.
pub fn handle_request(from: &DeviceId, request: AgentRequest) -> Result<AgentReply, AgentFailure> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let from = *from;
    core_host::spawn(async move {
        let _ = tx.send(run_request(from, request).await);
    });
    rx.recv_timeout(REQUEST_LIMIT).unwrap_or(Err(AgentFailure::Internal))
}

async fn run_request(from: DeviceId, request: AgentRequest) -> Result<AgentReply, AgentFailure> {
    if let AgentRequest::SessionWatch { session: None } = request {
        stop(lock().watches.unwatch(&from));
        return Ok(AgentReply::Ok);
    }
    let client = connected_client().await.ok_or(AgentFailure::Unsupported)?;
    let origin = || Origin::phone(from.to_string(), device_name(&from));
    let fail = |e: ClientError| failure(&e);
    Ok(match request {
        AgentRequest::Threads => {
            let threads = client.threads_list(Some(agent::MAX_THREADS as u32), None).await.map_err(fail)?;
            AgentReply::Threads(threads.iter().map(|t| thread(t, Some(&from))).collect())
        }
        AgentRequest::Start { agent: agent_id, folder, prompt, .. } => {
            let agent_id = AgentId::parse(&agent_id).ok_or(AgentFailure::NotFound)?;
            let params = ThreadStartParams {
                agent: agent_id,
                folder_id: folder,
                prompt,
                mode: Mode::Supervised,
                origin: origin(),
            };
            let info = client.thread_start(params).await.map_err(fail)?;
            bridge().remember(&info);
            lists_changed();
            AgentReply::Thread(thread(&info, Some(&from)))
        }
        AgentRequest::Send { thread: thread_id, text } => {
            client.thread_send(thread_id, text, origin()).await.map_err(fail)?;
            AgentReply::Ok
        }
        AgentRequest::Cancel { thread: thread_id } => {
            client.thread_interrupt(thread_id).await.map_err(fail)?;
            AgentReply::Ok
        }
        AgentRequest::Read { thread: thread_id, before, limit } => {
            let params =
                ThreadReadParams { thread_id, before_seq: before, after_seq: None, limit: Some(limit) };
            let page = client.thread_read(params).await.map_err(fail)?;
            AgentReply::Items(AgentItems {
                thread: thread(&page.thread, Some(&from)),
                items: page.items.iter().map(item).collect(),
                more_before: page.more_before,
                asks: page.pending.iter().map(|a| ask(a, Some(&page.thread))).collect(),
            })
        }
        AgentRequest::Answer { token, allow, message } => {
            let decision = if allow { Decision::Allow } else { Decision::Deny };
            // The core passes words only with a denial, only while this PC
            // offers them; a host without them would drop them unsaid.
            if message.is_some() && (allow || !says_why()) {
                return Err(AgentFailure::Unsupported);
            }
            let params = ApprovalAnswerParams { token, decision, by: origin(), message };
            client.approval_answer_with(params).await.map_err(fail)?;
            AgentReply::Ok
        }
        AgentRequest::Sessions => {
            let sessions = client.sessions_list(Some(agent::MAX_SESSIONS as u32)).await.map_err(fail)?;
            AgentReply::Sessions(sessions.iter().map(session).collect())
        }
        AgentRequest::SessionRead { session: session_id, before, limit } => {
            let params = SessionReadParams { session_id, before_seq: before, limit: Some(limit) };
            let page = client.session_read(params).await.map_err(fail)?;
            AgentReply::SessionItems(AgentSessionItems {
                session: session(&page.session),
                items: page.items.iter().map(item).collect(),
                more_before: page.more_before,
            })
        }
        AgentRequest::SessionWatch { session: Some(session_id) } => {
            phone_watch(from, session_id).await?;
            AgentReply::Ok
        }
        AgentRequest::SessionWatch { session: None } => AgentReply::Ok,
        AgentRequest::SessionContinue { session: session_id } => {
            let params = SessionContinueParams { session_id, mode: Mode::Supervised, origin: origin() };
            let info = client.session_continue(params).await.map_err(fail)?;
            bridge().remember(&info);
            lists_changed();
            AgentReply::Thread(thread(&info, Some(&from)))
        }
        AgentRequest::Export { thread: thread_id, session: session_id } => {
            let params = ThreadExportParams { thread_id, session_id, format: ExportFormat::Markdown };
            AgentReply::Export(export_reply(client.thread_export(params).await.map_err(fail)?))
        }
    })
}

/// The host's export as a phone gets it (the core keeps it within the wire's limits).
fn export_reply(result: ThreadExportResult) -> AgentExport {
    AgentExport { text: result.text, truncated: result.truncated, file_name: result.file_name }
}

/// A phone follows `session_id`: one host connection watches a session (the
/// page's own uses the main one), so phones get their own, shared by phones
/// following the same session, at most two.
async fn phone_watch(from: DeviceId, session_id: String) -> Result<(), AgentFailure> {
    let plan = lock().watches.plan(&from, &session_id);
    match plan {
        WatchPlan::Have => return Ok(()),
        WatchPlan::Busy => return Err(AgentFailure::Busy),
        WatchPlan::Share => {
            stop(lock().watches.watch(from, session_id, None));
            return Ok(());
        }
        WatchPlan::Open => {}
    }
    let fail = |e: ClientError| failure(&e);
    let (client, mut events) = Client::connect(&client::socket_path(), hello()).await.map_err(fail)?;
    client.session_watch(Some(session_id.clone())).await.map_err(fail)?;
    let Some(node) = core_host::node() else { return Err(AgentFailure::Unsupported) };
    let generation = lock().next_gen();
    let task = tokio::spawn(async move {
        // Holding the client keeps the connection, and with it the watch.
        let _client = client;
        while let Some(event) = events.recv().await {
            if let Event::SessionItem(e) = event
                && let Err(err) = node.agent_session_item(e.session_id, item(&e.item)).await
            {
                tracing::debug!(error = %err, "a session item wasn't passed on");
            }
        }
        lock().watches.ended(generation);
    });
    let conn = WatchConn { session: session_id.clone(), generation, task: Some(task.abort_handle()) };
    let mut s = lock();
    // Another phone may have taken the last connection meanwhile.
    if s.watches.plan(&from, &session_id) == WatchPlan::Busy {
        drop(s);
        task.abort();
        return Err(AgentFailure::Busy);
    }
    let unused = s.watches.watch(from, session_id, Some(conn));
    drop(s);
    stop(unused);
    Ok(())
}

// ---- The Agents page ----

/// Everything the page's `Agents` object shows, read on the GUI thread.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct View {
    pub status: String,
    pub setup: String,
    pub threads: String,
    pub sessions: String,
    pub needs_you: i32,
    pub open_kind: String,
    pub open_id: String,
    pub open_info: String,
    /// The oldest request waiting in the open thread (`{}` for none).
    pub ask: String,
    /// Requests waiting behind it.
    pub asks_waiting: i32,
    pub steps: String,
    pub changes: String,
    pub more: bool,
    pub loading: bool,
    pub busy: bool,
    /// Where the save dialog suggests saving the waiting export ("" for none).
    pub export_path: String,
    /// The host is out of date and threads run on it: restarting finishes the update.
    pub outdated: bool,
    /// The host can't list sessions started on this PC until it's updated.
    pub sessions_unsupported: bool,
    /// A request can be denied with words for the agent.
    pub can_say_why: bool,
    /// The user hid the thread list.
    pub list_hidden: bool,
    /// The user hid the details beside the transcript.
    pub inspector_hidden: bool,
}

pub(crate) fn view() -> View {
    let home = dirs::home_dir();
    let s = lock();
    let threads = listed_threads(&s.threads);
    let open = s.open.as_ref();
    let open_info = match open {
        Some(o) if o.kind == OpenKind::Thread => s.threads.get(&o.id).map(thread_row),
        Some(o) => s.sessions.iter().find(|x| x.id == o.id).map(|x| {
            let mut row = session_row(x, home.as_deref());
            // The page may add the folder the session runs in; this stays on the PC.
            row["cwd"] = json!(x.cwd);
            row
        }),
        None => None,
    };
    let (ask, asks_waiting) = match open {
        Some(o) => ask_json(&o.asks, |token| o.guard.held(token)),
        None => ask_json(&[], |_| false),
    };
    View {
        status: word(&s.status),
        setup: setup_json(&s, home.as_deref()),
        threads: Value::Array(threads.iter().map(|t| thread_row(t)).collect()).to_string(),
        sessions: Value::Array(s.sessions.iter().map(|x| session_row(x, home.as_deref())).collect())
            .to_string(),
        needs_you: needs_you(s.threads.values(), &s.sessions),
        open_kind: match open.map(|o| o.kind) {
            Some(OpenKind::Thread) => "thread".into(),
            Some(OpenKind::Session) => "session".into(),
            None => String::new(),
        },
        open_id: open.map(|o| o.id.clone()).unwrap_or_default(),
        open_info: open_info.unwrap_or_else(|| json!({})).to_string(),
        ask: ask.to_string(),
        asks_waiting,
        steps: steps_json(open.map_or(&[][..], |o| &o.entries)).to_string(),
        changes: changes_json(open.map_or(&[][..], |o| &o.entries)).to_string(),
        more: open.is_some_and(|o| o.more),
        loading: open.is_some_and(|o| o.loading),
        busy: s.busy > 0,
        export_path: s.export.as_ref().map(|e| e.path.to_string_lossy().into_owned()).unwrap_or_default(),
        outdated: s.outdated,
        sessions_unsupported: s.sessions_unsupported,
        can_say_why: s.says_why && open.is_some_and(|o| o.kind == OpenKind::Thread),
        list_hidden: s.panels.list_hidden,
        inspector_hidden: s.panels.inspector_hidden,
    }
}

/// Whether the host takes a denial with the user's words: phones are then
/// offered `agent.say_why`.
pub fn says_why() -> bool {
    lock().says_why
}

/// The timeline model is on screen: `waker` queues [`take_ui`] on the GUI
/// thread (false when it's gone). It takes every row first.
pub(crate) fn attach_ui(waker: impl Fn() -> bool + Send + Sync + 'static) {
    let bridge = bridge();
    let render = bridge.render();
    *bridge.feed() = Feed { waker: Some(Arc::new(waker)), ..Feed::default() };
    bridge.send_ui(UiUpdate::Reset(render.shown.clone()), &render.shown);
}

/// What the timeline model applies now, in order.
pub(crate) fn take_ui() -> Vec<UiUpdate> {
    bridge().feed().take()
}

/// What the setup area shows, as JSON: `{ status, following, agents:
/// [...], folders: [...] }`. Only this PC sees the plan, the version and
/// folder paths.
fn setup_json(s: &BridgeState, home: Option<&std::path::Path>) -> String {
    let agents: Vec<_> = s
        .agents
        .iter()
        .map(|a| {
            json!({
                "id": a.id.as_str(),
                "name": a.name,
                "installed": a.installed,
                "signedIn": a.signed_in,
                "version": a.version.clone().unwrap_or_default(),
                "auth": a.auth.as_ref().map(word).unwrap_or_default(),
                "plan": a.plan.clone().unwrap_or_default(),
                "loginHint": a.login_hint,
                "problem": a.problem.clone().unwrap_or_default(),
            })
        })
        .collect();
    let folders: Vec<_> = s
        .folders
        .iter()
        .map(|f| {
            json!({
                "id": f.id,
                "label": f.label,
                "path": f.path,
                "shownPath": shown_path(&f.path, home),
                "exists": f.exists,
            })
        })
        .collect();
    json!({ "status": word(&s.status), "following": s.following, "agents": agents, "folders": folders })
        .to_string()
}

/// `path` with the home folder written `~`.
fn shown_path(path: &str, home: Option<&std::path::Path>) -> String {
    let Some(home) = home.and_then(|h| h.to_str()).filter(|h| h.len() > 1) else { return path.to_owned() };
    match path.strip_prefix(home) {
        Some("") => "~".to_owned(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

/// The page is on screen (or left): it starts the host if needed, and asks
/// the agents again. Leaving closes what it showed.
pub fn page_visible(visible: bool) {
    let refresh = {
        let mut s = lock();
        s.page_open = visible;
        visible.then(|| s.client.clone()).flatten()
    };
    wake();
    if !visible {
        close();
    }
    sync_viewing();
    if let Some(client) = refresh {
        core_host::spawn(async move {
            if let Ok(agents) = client.agents_list(true).await {
                lock().agents = agents;
                changed();
            }
        });
    }
    changed();
}

/// G7 Link's window became the active one, or stopped being it (another
/// window took focus, or it was hidden).
pub fn window_active(active: bool) {
    let changed = std::mem::replace(&mut lock().window_active, active) != active;
    if changed {
        sync_viewing();
    }
}

/// What the host should hear the page shows: the open thread or session
/// while the page is on screen in the active window, else nothing. The host
/// skips "needs you" and "finished" notifications for a thread on screen.
fn wanted_viewing(s: &BridgeState) -> UiViewingParams {
    match &s.open {
        Some(open) if s.page_open && s.window_active => match open.kind {
            OpenKind::Thread => UiViewingParams { thread_id: Some(open.id.clone()), session_id: None },
            OpenKind::Session => UiViewingParams { thread_id: None, session_id: Some(open.id.clone()) },
        },
        _ => UiViewingParams::default(),
    }
}

/// Tells the host what the page shows when that changed: one call at a time,
/// each reading what's wanted when it runs, so the last one sent is the latest.
fn sync_viewing() {
    if core_host::try_host().is_none() {
        return;
    }
    core_host::spawn(async {
        let bridge = bridge();
        let _one = bridge.viewing.lock().await;
        let (client, conn, wanted) = {
            let s = bridge.lock();
            let Some(client) = s.client.clone() else { return };
            let wanted = wanted_viewing(&s);
            if s.viewing_sent.as_ref().is_some_and(|(conn, sent)| *conn == s.conn && *sent == wanted) {
                return;
            }
            (client, s.conn, wanted)
        };
        if client.ui_viewing_with(wanted.clone()).await.is_ok() {
            let mut s = bridge.lock();
            if s.conn == conn {
                s.viewing_sent = Some((conn, wanted));
            }
        }
    });
}

/// The user asked to start the host.
pub fn start_host() {
    lock().user_started = true;
    wake();
}

/// The user agreed to restart an outdated host now: threads running on it stop.
pub fn restart_host() {
    if lock().outdated {
        bridge().restart.notify_one();
    }
}

/// Reads the agents, folders, threads and sessions again.
pub fn refresh() {
    let Some(client) = lock().client.clone() else {
        wake();
        return;
    };
    core_host::spawn(async move {
        let agents = client.agents_list(true).await;
        let folders = client.folders_list().await;
        let threads = client.threads_list(Some(LISTED_THREADS as u32), None).await;
        let sessions = client.sessions_list(Some(api::MAX_SESSIONS as u32)).await;
        let following = following::read(&client).await;
        {
            let mut s = lock();
            if let Ok(agents) = agents {
                s.agents = agents;
            }
            if let Ok(folders) = folders {
                s.folders = folders;
            }
            if let Ok(threads) = threads {
                s.threads = threads.into_iter().map(|t| (t.id.clone(), t)).collect();
            }
            s.sessions_unsupported = matches!(&sessions, Err(e) if e.is_unknown_method());
            if let Ok(sessions) = sessions {
                s.sessions = sessions;
            }
            if following.is_some() {
                s.following = following;
            }
        }
        changed();
    });
}

fn busy(by: i32) {
    {
        let mut s = lock();
        s.busy = s.busy.saturating_add_signed(by);
    }
    changed();
}

fn folder_error(e: &ClientError) -> String {
    match e.code() {
        Some(ErrorCode::BadRequest) => {
            tr!("Agents", "That folder can't be used. Pick a folder you can read, other than the whole disk.")
        }
        Some(ErrorCode::Limit) => tr!("Agents", "You can add up to 64 folders."),
        Some(ErrorCode::NotFound) => tr!("Agents", "That folder is no longer in the list."),
        _ if e.is_unreachable() => tr!("Agents", "Couldn't reach the agent host"),
        _ => tr!("Agents", "Couldn't change the folders"),
    }
}

/// Adds a folder agents may work in (after the user confirmed it).
pub fn add_folder(path: String) {
    lock().user_started = true;
    core_host::spawn(async move {
        let _ = added_folder(path).await;
    });
}

/// Adds a folder; false (after saying why) when it couldn't.
async fn added_folder(path: String) -> bool {
    let Some(client) = connected_client().await else {
        crate::bridge::app::show_message(tr!("Agents", "Couldn't reach the agent host"));
        return false;
    };
    match client.folders_add(path, None).await {
        Ok(folder) => {
            let mut s = lock();
            if !s.folders.iter().any(|f| f.id == folder.id) {
                s.folders.push(folder);
            }
            drop(s);
            changed();
            true
        }
        Err(e) => {
            crate::bridge::app::show_message(folder_error(&e));
            false
        }
    }
}

/// Removes a folder from the list; threads in it keep running.
pub fn remove_folder(id: String) {
    core_host::spawn(async move {
        let Some(client) = connected_client().await else {
            crate::bridge::app::show_message(tr!("Agents", "Couldn't reach the agent host"));
            return;
        };
        match client.folders_remove(id.clone()).await {
            Ok(()) => {
                lock().folders.retain(|f| f.id != id);
                changed();
            }
            Err(e) => crate::bridge::app::show_message(folder_error(&e)),
        }
    });
}

/// Turns following sessions started outside G7 on or off.
pub fn set_following(on: bool) {
    core_host::spawn(async move {
        let Some(client) = connected_client().await else {
            crate::bridge::app::show_message(tr!("Agents", "Couldn't reach the agent host"));
            return;
        };
        match following::set(&client, on).await {
            Ok(on) => {
                lock().following = Some(on);
                changed();
                if let Ok(sessions) = client.sessions_list(Some(api::MAX_SESSIONS as u32)).await {
                    lock().sessions = sessions;
                    lists_changed();
                }
            }
            Err(e) if e.is_unknown_method() => crate::bridge::app::show_message(outdated_host()),
            Err(e) => {
                tracing::debug!(error = %e, "following sessions wasn't changed");
                crate::bridge::app::show_message(tr!("Agents", "Couldn't change that setting"));
            }
        }
    });
}

/// The host's "Follow sessions started outside G7" switch (reconcile item 5).
mod following {
    use super::*;

    /// `None` from a host that doesn't have the setting.
    pub(super) async fn read(client: &Client) -> Option<bool> {
        client.settings_get().await.ok().map(|s| s.follow_sessions)
    }

    pub(super) async fn set(client: &Client, on: bool) -> Result<bool, ClientError> {
        client.sessions_set_following(on).await.map(|s| s.follow_sessions)
    }
}

pub fn open_thread(id: String) {
    open(OpenKind::Thread, id);
}

pub fn open_session(id: String) {
    open(OpenKind::Session, id);
}

fn open(kind: OpenKind, id: String) {
    let (generation, was_session, client) = {
        let mut s = lock();
        if s.open_is(kind, &id) {
            return;
        }
        let generation = s.next_gen();
        let was = s.open.replace(OpenView::new(generation, kind, id.clone()));
        (generation, was.is_some_and(|o| o.kind == OpenKind::Session), s.client.clone())
    };
    reset_rows(bridge(), generation);
    changed();
    sync_viewing();
    // Without a host it's read once one is reached.
    if let Some(client) = client {
        core_host::spawn(load(client, generation, kind, id, was_session));
    }
}

/// Reads the open view's newest items (and watches a session's).
async fn load(client: Client, generation: u64, kind: OpenKind, id: String, unwatch: bool) {
    let read = match kind {
        OpenKind::Thread => {
            if unwatch {
                let _ = client.session_watch(None).await;
            }
            let params = ThreadReadParams { thread_id: id, limit: Some(READ_PAGE), ..Default::default() };
            let read = client.thread_read(params).await;
            read.map(|page| {
                bridge().remember(&page.thread);
                (page.items, page.more_before, page.pending)
            })
        }
        OpenKind::Session => {
            if let Err(e) = client.session_watch(Some(id.clone())).await
                && e.code() == Some(ErrorCode::Limit)
            {
                crate::bridge::app::show_message(tr!(
                    "Agents",
                    "Too many sessions are being followed. New lines show when you open it again."
                ));
            }
            let params = SessionReadParams { session_id: id, before_seq: None, limit: Some(READ_PAGE) };
            let read = client.session_read(params).await;
            read.map(|page| {
                upsert_session(&mut lock().sessions, page.session);
                (page.items, page.more_before, Vec::new())
            })
        }
    };
    {
        let mut s = lock();
        let Some(open) = s.open.as_mut().filter(|o| o.generation == generation) else { return };
        open.loading = false;
        match read {
            Ok((items, more, asks)) => {
                for item in &items {
                    open.apply(Entry::from_item(item));
                }
                open.more |= more;
                open.asks = asks;
                open.guard_requests(now_ms());
            }
            Err(e) => {
                tracing::debug!(error = %e, "the open thread wasn't read");
                drop(s);
                crate::bridge::app::show_message(request_error(&e, Request::Read));
            }
        }
    }
    rerender();
    changed();
}

/// Closes what the page shows: the host stops counting it as on screen.
pub fn close() {
    let (was, client) = {
        let mut s = lock();
        (s.open.take(), s.client.clone())
    };
    let Some(was) = was else { return };
    reset_rows(bridge(), 0);
    changed();
    sync_viewing();
    let Some(client) = client else { return };
    if was.kind != OpenKind::Session || core_host::try_host().is_none() {
        return;
    }
    core_host::spawn(async move {
        // A session opened meanwhile watches itself.
        if lock().open.is_some() {
            return;
        }
        let _ = client.session_watch(None).await;
    });
}

/// Reads the 50 items before the oldest one shown.
pub fn load_earlier() {
    let (generation, kind, id, before, client) = {
        let mut s = lock();
        let client = s.client.clone();
        let Some(open) = s.open.as_mut().filter(|o| o.more && !o.loading) else { return };
        let Some(before) = open.entries.first().map(|e| e.item.seq) else { return };
        open.loading = true;
        (open.generation, open.kind, open.id.clone(), before, client)
    };
    changed();
    let Some(client) = client else { return };
    core_host::spawn(async move {
        let read = match kind {
            OpenKind::Thread => {
                let params = ThreadReadParams {
                    thread_id: id,
                    before_seq: Some(before),
                    limit: Some(READ_PAGE),
                    ..Default::default()
                };
                client.thread_read(params).await.map(|page| (page.items, page.more_before))
            }
            OpenKind::Session => {
                let params =
                    SessionReadParams { session_id: id, before_seq: Some(before), limit: Some(READ_PAGE) };
                client.session_read(params).await.map(|page| (page.items, page.more_before))
            }
        };
        {
            let mut s = lock();
            let Some(open) = s.open.as_mut().filter(|o| o.generation == generation) else { return };
            open.loading = false;
            if let Ok((items, more)) = read {
                open.more = more;
                for item in &items {
                    open.apply(Entry::from_item(item));
                }
            }
        }
        rerender();
        changed();
    });
}

/// What the page asked for, to say what went wrong in its words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
    Start,
    Send,
    Stop,
    Answer,
    Delete,
    Read,
    Export,
    Continue(SessionSource, AgentId),
}

/// What an older host that hasn't been replaced yet can't do.
fn outdated_host() -> String {
    tr!("Agents", "The agent host is out of date. Restart agents to finish the update.")
}

fn request_error(e: &ClientError, request: Request) -> String {
    if e.is_unreachable() {
        return tr!("Agents", "Couldn't reach the agent host");
    }
    if e.is_unknown_method() {
        return outdated_host();
    }
    match (e.code(), request) {
        (Some(ErrorCode::Busy | ErrorCode::Limit), Request::Start) => {
            tr!("Agents", "Too many threads are running. Wait for one to finish.")
        }
        (Some(ErrorCode::Busy), Request::Send) => {
            tr!("Agents", "Still open in a terminal. Close it there to reply here.")
        }
        (Some(ErrorCode::Busy), Request::Continue(source, agent)) => match (source, agent) {
            (SessionSource::Terminal, _) => {
                tr!("Agents", "Still open in a terminal. Close it there to continue here.")
            }
            (SessionSource::DesktopApp, AgentId::Claude) => {
                tr!("Agents", "Still open in the Claude app. Close it there to continue here.")
            }
            (SessionSource::DesktopApp, AgentId::Codex) => {
                tr!("Agents", "Still open in the Codex app. Close it there to continue here.")
            }
            (SessionSource::Other, _) => {
                tr!("Agents", "Still open in another app. Close it there to continue here.")
            }
        },
        (Some(ErrorCode::Denied), Request::Continue(..)) => {
            tr!("Agents", "Add its folder to your agent folders first.")
        }
        (Some(ErrorCode::Denied), Request::Start) => {
            tr!("Agents", "That folder isn't one of your agent folders.")
        }
        (Some(ErrorCode::Unavailable), Request::Start | Request::Send | Request::Continue(..)) => {
            tr!("Agents", "That agent isn't ready. Check it in Setup.")
        }
        (Some(ErrorCode::NotFound), Request::Start) => tr!("Agents", "That folder is no longer in the list."),
        (Some(ErrorCode::NotFound | ErrorCode::AlreadyAnswered), Request::Answer) => {
            tr!("Agents", "That request was already answered.")
        }
        (Some(ErrorCode::NotFound), _) => tr!("Agents", "It's no longer there."),
        (_, Request::Stop) => tr!("Agents", "Couldn't stop it"),
        (_, Request::Delete) => tr!("Agents", "Couldn't delete it"),
        (_, Request::Export) => tr!("Agents", "Couldn't export it"),
        _ => tr!("Agents", "Something went wrong. Try again."),
    }
}

/// Runs a page request on the host, with `busy` set while it runs.
fn page_request<T, Fut>(
    request: Request,
    call: impl FnOnce(Client) -> Fut + Send + 'static,
    done: impl FnOnce(T) + Send + 'static,
) where
    Fut: Future<Output = Result<T, ClientError>> + Send,
    T: Send + 'static,
{
    busy(1);
    core_host::spawn(async move {
        let result = match connected_client().await {
            Some(client) => call(client).await.map_err(Some),
            None => Err(None),
        };
        busy(-1);
        match result {
            Ok(value) => done(value),
            Err(Some(e)) => crate::bridge::app::show_message(request_error(&e, request)),
            Err(None) => crate::bridge::app::show_message(tr!("Agents", "Couldn't reach the agent host")),
        }
    });
}

/// Starts a thread from the page and opens it.
pub fn start_thread(agent: String, folder_id: String, prompt: String, mode: String) {
    let Some(agent) = AgentId::parse(&agent) else { return };
    let mode = if mode == "auto_edit" { Mode::AutoEdit } else { Mode::Supervised };
    page_request(
        Request::Start,
        move |client| async move {
            let params = ThreadStartParams { agent, folder_id, prompt, mode, origin: Origin::launcher() };
            client.thread_start(params).await
        },
        |info: ThreadInfo| {
            bridge().remember(&info);
            lists_changed();
            open_thread(info.id);
        },
    );
}

fn open_id(kind: OpenKind) -> Option<String> {
    lock().open.as_ref().filter(|o| o.kind == kind).map(|o| o.id.clone())
}

/// Sends a message to the open thread.
pub fn send(text: String) {
    let Some(id) = open_id(OpenKind::Thread) else { return };
    if text.trim().is_empty() {
        return;
    }
    guard_event(GuardEvent::Sent);
    page_request(
        Request::Send,
        move |client| async move { client.thread_send(id, text, Origin::launcher()).await },
        |()| {},
    );
}

/// Stops the open thread's turn.
pub fn stop_thread() {
    let Some(id) = open_id(OpenKind::Thread) else { return };
    page_request(Request::Stop, move |client| async move { client.thread_interrupt(id).await }, |()| {});
}

/// Answers one of the open thread's approvals: `allow`, `deny` or `allow_thread`.
pub fn answer(token: String, decision: &str) {
    let decision = match decision {
        "allow" => Decision::Allow,
        "allow_thread" => Decision::AllowThread,
        _ => Decision::Deny,
    };
    let answered = token.clone();
    page_request(
        Request::Answer,
        move |client| async move { client.approval_answer(token, decision, Origin::launcher()).await },
        move |()| answered_here(&answered),
    );
}

/// This PC answered a request: it leaves the dock (with no line under its
/// tool: the tool's own line says it).
fn answered_here(token: &str) {
    {
        let mut s = lock();
        if let Some(open) = s.open.as_mut() {
            open.asks.retain(|a| a.token != token);
            open.guard_requests(now_ms());
        }
    }
    changed();
    rerender();
}

/// Denies the shown request with the user's words for the agent (§P8.3):
/// only while the host takes them, never with empty words.
pub fn deny_with_message(token: String, text: String) {
    let text = text.trim().to_owned();
    if text.is_empty() || token.is_empty() || !lock().says_why {
        return;
    }
    let message = api::cut(&text, api::MAX_DENY_MESSAGE).to_owned();
    guard_event(GuardEvent::Sent);
    let answered = token.clone();
    page_request(
        Request::Answer,
        move |client| async move {
            let params = ApprovalAnswerParams {
                token,
                decision: Decision::Deny,
                by: Origin::launcher(),
                message: Some(message),
            };
            client.approval_answer_with(params).await
        },
        move |()| answered_here(&answered),
    );
}

/// The open view's typing guard heard `event`.
fn guard_event(event: GuardEvent<'_>) {
    let (before, after, followup) = {
        let mut s = lock();
        let Some(open) = s.open.as_mut() else { return };
        let before = open.held();
        open.guard.on(event);
        (before, open.held(), guard_followup(open))
    };
    followup.run();
    if before != after {
        changed();
        rerender();
    }
}

/// The user typed in the composer (not text set by the page).
pub fn typed() {
    guard_event(GuardEvent::Typed { now_ms: now_ms() });
}

/// The composer gained or lost the keyboard.
pub fn composer_focus(focused: bool) {
    guard_event(if focused { GuardEvent::Focus } else { GuardEvent::Blur });
}

/// Opens or closes a row of the open view.
pub fn toggle(key: String) {
    {
        let mut s = lock();
        let Some(open) = s.open.as_mut() else { return };
        open.toggle(&key);
    }
    rerender();
}

/// Wraps a code block's long lines, or stops.
pub fn toggle_wrap(key: String) {
    {
        let mut s = lock();
        let Some(open) = s.open.as_mut() else { return };
        toggle_key(&mut open.wraps, &key);
    }
    rerender();
}

/// What Copy takes from a row: a code block's source, a command, an output,
/// a message. `i:<item id>` is that item's whole text (an answer's "Copy").
pub fn copy_text(key: &str) -> Option<String> {
    let text = bridge().render().row(key).map(|r| r.copy_text().to_owned()).filter(|t| !t.is_empty());
    text.or_else(|| {
        let id = key.strip_prefix("i:")?;
        let s = lock();
        let entry = s.open.as_ref()?.entries.iter().find(|e| e.item.id == id)?.clone();
        Some(entry.text().to_owned()).filter(|t| !t.is_empty())
    })
}

/// The link numbered `index` in a row's markup.
pub fn link(key: &str, index: usize) -> Option<LinkRef> {
    bridge().render().row(key)?.link(index).cloned()
}

/// Opens a link the render crate validated, after the page showed where it
/// goes (or at once when its text is its target).
pub fn open_link(url: String) {
    let scheme = url.split(':').next().unwrap_or_default().to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "mailto") {
        return;
    }
    std::thread::spawn(move || {
        if let Err(e) = crate::sys::shell::open_url(&url) {
            tracing::debug!(error = %e, "a link didn't open");
            crate::bridge::app::show_message(tr!("Agents", "Couldn't open the link"));
        }
    });
}

/// The page's colours for code changed (a theme): rows are coloured again.
pub fn set_code_colors(json: &str) {
    let palette = CodePalette::from_json(json);
    {
        let bridge = bridge();
        let mut render = bridge.render();
        if render.palette == palette {
            return;
        }
        render.palette = palette;
        render.built.clear();
    }
    rerender();
}

/// Deletes a thread (and closes it when it's open).
pub fn delete_thread(id: String) {
    let deleted = id.clone();
    page_request(
        Request::Delete,
        move |client| async move { client.thread_delete(id).await },
        move |()| {
            let open = lock().open_is(OpenKind::Thread, &deleted);
            lock().threads.remove(&deleted);
            if open {
                close();
            }
            lists_changed();
        },
    );
}

/// Makes a session started outside G7 a thread here, and opens it.
pub fn continue_session(id: String) {
    let (source, agent) = lock()
        .sessions
        .iter()
        .find(|s| s.id == id)
        .map_or((SessionSource::Other, AgentId::Claude), |s| (s.source, s.agent));
    page_request(
        Request::Continue(source, agent),
        move |client| async move {
            let params =
                SessionContinueParams { session_id: id, mode: Mode::Supervised, origin: Origin::launcher() };
            client.session_continue(params).await
        },
        |info: ThreadInfo| {
            bridge().remember(&info);
            lists_changed();
            open_thread(info.id);
        },
    );
}

/// Adds the folder a session runs in (after the user confirmed it), then
/// continues the session.
pub fn add_folder_and_continue(path: String, session_id: String) {
    lock().user_started = true;
    core_host::spawn(async move {
        if added_folder(path).await {
            continue_session(session_id);
        }
    });
}

/// Opens the open thread in a terminal with the agent's own resume command.
pub fn open_in_terminal() {
    let Some(id) = open_id(OpenKind::Thread) else { return };
    page_request(
        Request::Read,
        move |client| async move { client.thread_terminal_command(id).await },
        |command: api::TerminalCommandResult| terminal::open(command.argv, command.cwd),
    );
}

/// Puts text on this PC's clipboard.
pub fn copy(text: String) {
    crate::copy::copy_text(text);
}

/// The open thread or session, as `thread.export` names it.
fn export_params() -> Option<ThreadExportParams> {
    let s = lock();
    let open = s.open.as_ref()?;
    let (thread_id, session_id) = match open.kind {
        OpenKind::Thread => (Some(open.id.clone()), None),
        OpenKind::Session => (None, Some(open.id.clone())),
    };
    Some(ThreadExportParams { thread_id, session_id, format: ExportFormat::Markdown })
}

/// Copies the open thread or session as Markdown.
pub fn copy_markdown() {
    let Some(params) = export_params() else { return };
    page_request(
        Request::Export,
        move |client| async move { client.thread_export(params).await },
        |result: ThreadExportResult| {
            crate::copy::copy_text(result.text);
            crate::bridge::app::show_message(tr!("Agents", "Copied as Markdown"));
        },
    );
}

/// Gets the open thread or session as Markdown for the page's save dialog,
/// which opens on [`View::export_path`].
pub fn export_markdown() {
    let Some(params) = export_params() else { return };
    page_request(
        Request::Export,
        move |client| async move { client.thread_export(params).await },
        |result: ThreadExportResult| {
            let folder = dirs::document_dir().or_else(dirs::home_dir).unwrap_or_default();
            let path = export_path(&folder, &result.file_name);
            lock().export = Some(PendingExport { path, text: result.text });
            changed();
        },
    );
}

/// `name` in `folder`, as a plain file name whatever the host sent.
fn export_path(folder: &std::path::Path, name: &str) -> std::path::PathBuf {
    let name = std::path::Path::new(name)
        .file_name()
        .filter(|n| !n.to_string_lossy().starts_with('.'))
        .map_or_else(|| "transcript.md".into(), |n| n.to_os_string());
    folder.join(name)
}

/// Saves the waiting export where the user chose, written whole or not at all.
pub fn save_export(path: std::path::PathBuf) {
    let Some(export) = lock().export.take() else { return };
    changed();
    core_host::spawn(async move {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let target = path.clone();
        let written =
            tokio::task::spawn_blocking(move || crate::atomic_file::write(&target, export.text.as_bytes()))
                .await
                .unwrap_or_else(|e| Err(std::io::Error::other(e)));
        match written {
            Ok(()) => crate::bridge::app::show_message(tr!("Agents", "Saved %1", name)),
            Err(error) => {
                tracing::warn!(%error, "can't save an agent export");
                crate::bridge::app::show_message(tr!("Agents", "Couldn't save it"));
            }
        }
    });
}

/// The save dialog was closed without saving.
pub fn drop_export() {
    if lock().export.take().is_some() {
        changed();
    }
}

/// Runs a command in the user's terminal.
mod terminal {
    use super::*;

    /// `xdg-terminal-exec` (the desktop's preferred terminal) runs it in
    /// `cwd`; without one the command is copied to paste in a terminal.
    pub(super) fn open(argv: Vec<String>, cwd: String) {
        if argv.is_empty() {
            return;
        }
        let Some(launcher) = find("xdg-terminal-exec") else {
            let line = format!(
                "cd {} && {}",
                quote(&cwd),
                argv.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
            );
            crate::copy::copy_text(line);
            crate::bridge::app::show_message(tr!("Agents", "The command is copied. Paste it in a terminal."));
            return;
        };
        let child = std::process::Command::new(launcher)
            .args(&argv)
            .current_dir(&cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match child {
            // Reaped when the launcher exits (it hands the terminal off).
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => {
                tracing::debug!(error = %e, "the terminal didn't start");
                crate::bridge::app::show_message(tr!("Agents", "Couldn't open a terminal"));
            }
        }
    }

    fn find(name: &str) -> Option<std::path::PathBuf> {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(name))
            .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
    }

    /// `s` as one word for a POSIX shell.
    pub(super) fn quote(s: &str) -> String {
        if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./=:@%+,".contains(&b)) {
            s.to_owned()
        } else {
            format!("'{}'", s.replace('\'', r"'\''"))
        }
    }
}

#[cfg(test)]
mod tests {
    use g7_agents::api::{ApprovalKind, ItemKind, ThreadError, Tool, ToolStatus};

    use super::*;

    fn folder(id: &str, exists: bool) -> Folder {
        Folder {
            id: id.into(),
            label: "g7linux".into(),
            path: "/home/me/Code/g7linux".into(),
            exists,
            added_ms: 1,
        }
    }

    fn info(origin: Origin) -> ThreadInfo {
        ThreadInfo {
            id: "0123456789abcdef0123456789abcdef".into(),
            agent: AgentId::Codex,
            folder_id: "f0123456789ab".into(),
            folder_label: "g7linux".into(),
            title: "Fix the build".into(),
            state: State::Waiting,
            mode: Mode::Supervised,
            origin,
            created_ms: 1,
            updated_ms: 2,
            last_seq: 3,
            pending: 1,
            summary: None,
            error: Some(ThreadError { code: "usage_limit".into(), message: "Usage limit reached".into() }),
            native_id: Some("native".into()),
            step: Some("Run command".into()),
            plan: Some(vec![api::PlanStep {
                text: "Reproduce it".into(),
                status: api::PlanStatus::InProgress,
            }]),
            state_since_ms: 5,
            subdir: None,
            continued: false,
        }
    }

    fn a_session(state: SessionState, guess: bool) -> Session {
        Session {
            id: format!("sc{}", "ab".repeat(16)),
            agent: AgentId::Claude,
            title: "Fix the login test".into(),
            cwd: "/home/me/code/app".into(),
            folder_id: Some("f0123456789ab".into()),
            folder_label: "app".into(),
            state,
            state_guess: guess,
            state_since_ms: 7,
            waiting_for: Some("permission prompt".into()),
            source: SessionSource::Terminal,
            open: true,
            updated_ms: 9,
            last_seq: 64,
            step: Some("Run command".into()),
            summary: None,
            can_continue: false,
            blocked: Some(api::ContinueBlock::Open),
        }
    }

    fn text_item(seq: u64, rev: u32, text: &str) -> Item {
        Item { rev, text: Some(text.into()), ..Item::new(seq, format!("i{seq}"), ItemKind::Assistant, 1) }
    }

    fn a_tool(name: &str, title: &str, detail: &str, status: ToolStatus) -> Tool {
        Tool {
            name: name.into(),
            title: title.into(),
            detail: Some(detail.into()),
            status,
            output: None,
            class: None,
            path: None,
            by_rule: false,
        }
    }

    fn tool_item(seq: u64, name: &str, title: &str, detail: &str) -> Item {
        Item {
            rev: 1,
            tool: Some(a_tool(name, title, detail, ToolStatus::Ok)),
            ..Item::new(seq, format!("t{seq}"), ItemKind::Tool, seq)
        }
    }

    fn entry(item: &Item) -> Entry {
        Entry::from_item(item)
    }

    #[test]
    fn the_catalog_never_carries_a_path_or_a_plan() {
        let mut claude = AgentInfo::missing(AgentId::Claude);
        claude.installed = true;
        claude.signed_in = true;
        claude.plan = Some("max".into());
        claude.version = Some("2.1.310".into());
        let wire = catalog(
            &[claude, AgentInfo::missing(AgentId::Codex)],
            &[folder("f1", true), folder("f2", false)],
        );
        assert_eq!(wire.folders.len(), 1, "a folder that's gone is left out");
        assert_eq!(wire.folders[0].label, "g7linux");
        assert_eq!(wire.modes, vec!["supervised".to_owned()]);
        assert_eq!(wire.agents[0].id, "claude");
        assert!(wire.agents[0].signed_in && !wire.agents[1].installed);
        assert_eq!(wire.agents[1].login_hint, "codex login");
        assert_eq!(wire.agents[1].auth, None);
        let shown = serde_json::to_string(&wire).unwrap();
        assert!(!shown.contains("/home") && !shown.contains("max") && !shown.contains("2.1.310"), "{shown}");
    }

    #[test]
    fn from_phone_is_true_only_for_the_phone_that_started_it() {
        let phone = DeviceId([7; 32]);
        let other = DeviceId([8; 32]);
        let mine = info(Origin::phone(phone.to_string(), "Pixel"));
        assert!(thread(&mine, Some(&phone)).from_phone);
        assert!(!thread(&mine, Some(&other)).from_phone);
        assert!(!thread(&mine, None).from_phone);
        assert_eq!(started_by(&mine), Some(phone));
        let launcher = info(Origin::launcher());
        assert!(!thread(&launcher, Some(&phone)).from_phone);
        assert_eq!(started_by(&launcher), None);

        let wire = thread(&mine, Some(&phone));
        assert_eq!(
            (wire.state.as_str(), wire.mode.as_str(), wire.agent.as_str()),
            ("waiting", "supervised", "codex")
        );
        assert_eq!((wire.folder.as_str(), wire.asks, wire.updated), ("g7linux", 1, 2));
        assert_eq!(wire.error_code.as_deref(), Some("usage_limit"));
        assert_eq!((wire.step.as_deref(), wire.since), (Some("Run command"), Some(5)));
        let plan = wire.plan.as_ref().unwrap();
        assert_eq!((plan[0].text.as_str(), plan[0].status.as_str()), ("Reproduce it", "in_progress"));
        assert_eq!(error_code("usageLimitExceeded"), "usagelimitexceeded");
        assert_eq!(error_code("AGENT-EXITED"), "agent_exited");
        assert!(wire.is_valid());
        let mut old = info(Origin::launcher());
        old.state_since_ms = 0;
        assert_eq!(thread(&old, None).since, None, "a thread from 0.1.13 has no start time");
    }

    #[test]
    fn a_session_reaches_phones_without_its_path_and_a_guess_is_never_needs_you() {
        let exact = session(&a_session(SessionState::NeedsYou, false));
        assert_eq!((exact.state.as_str(), exact.guess), ("needs_you", false));
        assert_eq!((exact.folder.as_str(), exact.source.as_str()), ("app", "terminal"));
        assert_eq!(exact.blocked.as_deref(), Some("open"));
        assert!(exact.is_valid(), "{exact:?}");
        let shown = serde_json::to_string(&exact).unwrap();
        assert!(!shown.contains("/home") && !shown.contains("code/app"), "{shown}");

        let guessed = session(&a_session(SessionState::NeedsYou, true));
        assert_eq!((guessed.state.as_str(), guessed.guess), ("may_need_you", true));
        assert!(guessed.is_valid());
        let guessed = session(&a_session(SessionState::NeedsYouGuess, false));
        assert_eq!((guessed.state.as_str(), guessed.guess), ("may_need_you", true));
        assert!(guessed.is_valid());
        assert_eq!(exact.since, Some(7));
        assert_eq!(session(&a_session(SessionState::Ended, false)).state, "ended");
    }

    #[test]
    fn only_exact_needs_you_is_counted() {
        let mut threads = HashMap::new();
        let waiting = info(Origin::launcher());
        let mut done = info(Origin::launcher());
        done.id = "1".repeat(32);
        done.state = State::Done;
        threads.insert(waiting.id.clone(), waiting);
        threads.insert(done.id.clone(), done);
        let sessions = [
            a_session(SessionState::NeedsYou, false),
            a_session(SessionState::NeedsYou, true),
            a_session(SessionState::NeedsYouGuess, false),
            a_session(SessionState::Working, true),
        ];
        assert_eq!(needs_you(threads.values(), &sessions), 2);
        assert_eq!(needs_you(threads.values(), &[]), 1);
        assert_eq!(listed_threads(&threads)[0].state, State::Waiting, "waiting threads come first");
    }

    #[test]
    fn sessions_keep_the_hosts_order() {
        let mut list = vec![a_session(SessionState::Idle, true)];
        let mut ended = a_session(SessionState::Ended, false);
        ended.id = format!("sx{}", "cd".repeat(16));
        upsert_session(&mut list, ended);
        let mut exact = a_session(SessionState::NeedsYou, false);
        exact.id = format!("sx{}", "ef".repeat(16));
        upsert_session(&mut list, exact);
        let states: Vec<_> = list.iter().map(|s| s.state).collect();
        assert_eq!(states, [SessionState::NeedsYou, SessionState::Idle, SessionState::Ended]);
        // A changed session replaces itself.
        upsert_session(&mut list, a_session(SessionState::Working, false));
        assert_eq!(list.len(), 3);
        assert_eq!(list[1].state, SessionState::Working);
    }

    #[test]
    fn items_and_asks_map_to_wire_words() {
        let mut bash = a_tool("Bash", "Run command", "ls", ToolStatus::Denied);
        bash.class = Some(api::ToolClass::Command);
        let tool = Item {
            rev: 1,
            tool: Some(bash),
            turn: Some(2),
            end_ms: Some(12),
            group: Some("toolu_00".into()),
            ..Item::new(4, "toolu_01", ItemKind::Tool, 9)
        };
        let wire = item(&tool);
        assert_eq!((wire.kind.as_str(), wire.tool.as_ref().unwrap().status.as_str()), ("tool", "denied"));
        assert_eq!(wire.tool.as_ref().unwrap().class.as_deref(), Some("command"));
        assert_eq!((wire.turn, wire.end, wire.group.as_deref()), (Some(2), Some(12), Some("toolu_00")));
        assert!(wire.is_valid());
        let answer = Item {
            text: Some("Done.".into()),
            phase: Some(api::Phase::Final),
            ..Item::new(5, "msg_1:0", ItemKind::Assistant, 10)
        };
        assert_eq!(item(&answer).phase.as_deref(), Some("final"));
        let denial = Item {
            text: Some("Run only the unit tests".into()),
            deny_of: Some("toolu_01".into()),
            ..Item::new(6, "u6", ItemKind::User, 11)
        };
        assert_eq!(item(&denial).deny_of.as_deref(), Some("toolu_01"));
        let thinking = Item { end_ms: Some(3), ..Item::new(7, "rs_1", ItemKind::Reasoning, 2) };
        let wire = item(&thinking);
        assert_eq!(wire.kind, "reasoning", "the core makes it a step for a phone without agent.items2");
        assert_eq!(wire.clone().for_legacy().kind, "tool");

        let approval = Approval {
            token: "aaaabbbbccccddddeeeeffff00001111".into(),
            thread_id: "0123456789abcdef0123456789abcdef".into(),
            kind: ApprovalKind::Edit,
            title: "Edit src/main.rs".into(),
            command: None,
            diff: Some("-a\n+b".into()),
            path: Some("src/main.rs".into()),
            reason: None,
            truncated: false,
            can_allow_thread: true,
            created_ms: 1,
            expires_ms: 7_200_001,
            item_id: Some("toolu_02".into()),
        };
        let thread_info = info(Origin::launcher());
        let wire = ask(&approval, Some(&thread_info));
        assert_eq!(
            (wire.kind.as_str(), wire.agent.as_str(), wire.thread_title.as_str()),
            ("edit", "codex", "Fix the build")
        );
        assert_eq!(wire.expires, 7_200_001);
        assert_eq!(wire.item.as_deref(), Some("toolu_02"));
        assert!(wire.is_valid());
        assert_eq!(asked_outcome(ApprovalOutcome::Cancelled), "cancelled");
        let pc = || "guchu".to_owned();
        assert_eq!(answered_by(&Origin::phone("abc", "Pixel"), pc).as_deref(), Some("Pixel"));
        assert_eq!(answered_by(&Origin::launcher(), pc).as_deref(), Some("guchu"));
        let (shown, more) = ask_json(&[approval], |_| false);
        assert_eq!(shown["can_allow_thread"], true);
        assert_eq!(shown["path"], "src/main.rs");
        assert_eq!(
            (shown["itemId"].as_str(), shown["position"].as_u64(), shown["total"].as_u64()),
            (Some("toolu_02"), Some(1), Some(1))
        );
        let doc: Value = serde_json::from_str(shown["diffDoc"].as_str().unwrap()).unwrap();
        assert_eq!((doc["adds"].as_u64(), doc["dels"].as_u64()), (Some(1), Some(1)));
        assert_eq!(more, 0);
    }

    fn command_ask(token: &str, command: &str, created_ms: u64) -> Approval {
        Approval {
            token: token.into(),
            thread_id: "0123456789abcdef0123456789abcdef".into(),
            kind: ApprovalKind::Command,
            title: "Run command".into(),
            command: Some(command.into()),
            diff: None,
            path: None,
            reason: Some("To check the types.".into()),
            truncated: false,
            can_allow_thread: true,
            created_ms,
            expires_ms: created_ms + 7_200_000,
            item_id: Some(format!("toolu_{token}")),
        }
    }

    #[test]
    fn the_page_shows_one_request_at_a_time_the_oldest_first() {
        let (none, more) = ask_json(&[], |_| false);
        assert_eq!((none, more), (json!({}), 0));
        let asks = [
            command_ask("b", "rg --files", 20),
            command_ask("a", "pnpm exec tsc --noEmit", 10),
            command_ask("c", "cargo test", 30),
        ];
        let (shown, more) = ask_json(&asks, |_| false);
        assert_eq!(shown["token"], "a");
        assert_eq!(shown["command"], "pnpm exec tsc --noEmit", "the command reaches the page as it is");
        assert_eq!(shown["commandStyled"], "pnpm&nbsp;exec&nbsp;tsc&nbsp;--noEmit");
        assert_eq!((shown["position"].as_u64(), shown["total"].as_u64()), (Some(1), Some(3)), "\"1 of 3\"");
        assert_eq!(more, 2);
        // Answering the first shows the next.
        let (next, more) = ask_json(&asks[..1], |_| false);
        assert_eq!((next["token"].as_str(), more), (Some("b"), 0));
        // One the guard holds gives way to the next, which is "2 of 3".
        let (shown, _) = ask_json(&asks, |t| t == "a");
        assert_eq!((shown["token"].as_str(), shown["position"].as_u64()), (Some("b"), Some(2)));
        assert_eq!(shown["held"], false);
        // All held: the first, held.
        let (shown, _) = ask_json(&asks, |_| true);
        assert_eq!((shown["token"].as_str(), shown["held"].as_bool()), (Some("a"), Some(true)));
    }

    #[test]
    fn a_request_that_arrives_while_typing_is_held_until_a_pause() {
        let mut view = OpenView::new(1, OpenKind::Thread, "t".into());
        view.asks.push(command_ask("a", "ls", 10));
        view.guard_requests(1_000);
        assert!(view.held().is_empty(), "nobody was typing");
        assert_eq!(view.marks()[0].kind, render::MarkKind::Waiting);
        view.guard.on(GuardEvent::Focus);
        view.guard.on(GuardEvent::Typed { now_ms: 2_000 });
        view.asks.push(command_ask("b", "rm -rf build", 20));
        view.guard_requests(2_400);
        assert_eq!(view.held(), ["b"]);
        assert_eq!(guard_followup(&view).idle_at, Some(3_500), "it goes 1.5 s after the last keystroke");
        let marks = view.marks();
        let held = marks.iter().find(|m| m.item_id == "toolu_b").unwrap();
        assert_eq!(held.kind, render::MarkKind::Held);
        // The dock shows the one not held.
        let (shown, _) = ask_json(&view.asks, |t| view.guard.held(t));
        assert_eq!(shown["token"], "a");
        // Typing goes on: the pause moves.
        view.guard.on(GuardEvent::Typed { now_ms: 3_000 });
        assert_eq!(guard_followup(&view).idle_at, Some(4_500));
        view.guard.on(GuardEvent::Idle);
        assert!(view.held().is_empty() && guard_followup(&view).idle_at.is_none());
        // Sending lets go too, and the words were for the shown request.
        view.guard.on(GuardEvent::Typed { now_ms: 5_000 });
        view.asks.push(command_ask("c", "cargo test", 30));
        view.guard_requests(5_100);
        assert_eq!(view.held(), ["c"]);
        view.guard.on(GuardEvent::Sent);
        assert!(view.held().is_empty());
        // Leaving the composer lets go as well.
        view.guard.on(GuardEvent::Focus);
        view.guard.on(GuardEvent::Typed { now_ms: 6_000 });
        view.asks.push(command_ask("d", "make", 40));
        view.guard_requests(6_100);
        assert_eq!(view.held(), ["d"]);
        view.guard.on(GuardEvent::Blur);
        assert!(view.held().is_empty());
    }

    #[test]
    fn a_request_answered_elsewhere_leaves_a_line_only_when_it_says_something() {
        let mut view = OpenView::new(1, OpenKind::Thread, "t".into());
        for (token, at) in [("a", 1), ("b", 2), ("c", 3), ("d", 4)] {
            view.asks.push(command_ask(token, "ls", at));
        }
        let phone = Origin::phone("abc", "Pixel 8 Pro");
        assert!(view.resolve("a", ApprovalOutcome::Allowed, Some(&phone)));
        assert!(view.resolve("b", ApprovalOutcome::Denied, Some(&Origin::launcher())));
        assert!(view.resolve("c", ApprovalOutcome::Expired, None));
        assert!(!view.resolve("zz", ApprovalOutcome::Allowed, None), "not one of this view's");
        let marks = view.marks();
        let find = |id: &str| marks.iter().find(|m| m.item_id == id).map(|m| (m.kind, m.by.clone()));
        assert_eq!(find("toolu_a"), Some((render::MarkKind::Allowed, "Pixel 8 Pro".into())));
        assert_eq!(find("toolu_b"), None, "answered here by hand: the tool's line says it");
        assert_eq!(find("toolu_c"), Some((render::MarkKind::Expired, String::new())));
        assert_eq!(find("toolu_d"), Some((render::MarkKind::Waiting, String::new())));
        // A step a rule allowed: the timeline gives it the rule's line, in
        // place of who answered.
        let mut ruled = a_tool("Bash", "Run command", "ls", ToolStatus::Ok);
        ruled.by_rule = true;
        view.apply(entry(&Item { tool: Some(ruled), ..Item::new(9, "toolu_a", ItemKind::Tool, 5) }));
        assert!(view.marks().iter().all(|m| m.item_id != "toolu_a"));
        assert!(view.entries[0].item_in().unwrap().tool.unwrap().by_rule);
    }

    #[test]
    fn the_host_hears_what_the_page_shows_only_while_its_window_is_active() {
        let mut s = BridgeState { page_open: true, window_active: true, ..BridgeState::default() };
        assert_eq!(wanted_viewing(&s), UiViewingParams::default(), "nothing open");
        s.open = Some(OpenView::new(1, OpenKind::Thread, "t1".into()));
        assert_eq!(wanted_viewing(&s), UiViewingParams { thread_id: Some("t1".into()), session_id: None });
        s.window_active = false;
        assert_eq!(
            wanted_viewing(&s),
            UiViewingParams::default(),
            "another window has focus, or it's hidden"
        );
        s.window_active = true;
        s.page_open = false;
        assert_eq!(wanted_viewing(&s), UiViewingParams::default(), "another page is shown");
        s.page_open = true;
        s.open = Some(OpenView::new(2, OpenKind::Session, "s1".into()));
        assert_eq!(wanted_viewing(&s), UiViewingParams { thread_id: None, session_id: Some("s1".into()) });
    }

    #[test]
    fn an_older_host_says_so_instead_of_coming_back_empty() {
        use g7_agents::api::{RpcError, UNKNOWN_METHOD};
        let old = ClientError::Remote(RpcError::new(ErrorCode::BadRequest, UNKNOWN_METHOD));
        assert_eq!(request_error(&old, Request::Export), outdated_host());
        assert_eq!(
            request_error(&old, Request::Continue(SessionSource::Terminal, AgentId::Codex)),
            outdated_host()
        );
        assert_eq!(failure(&old), AgentFailure::Unsupported, "phones hear it can't be done yet");
    }

    #[test]
    fn host_errors_become_protocol_errors() {
        use g7_agents::api::RpcError;
        let remote = |code| failure(&ClientError::Remote(RpcError::new(code, "x")));
        assert_eq!(remote(ErrorCode::BadRequest), AgentFailure::BadMessage);
        assert_eq!(remote(ErrorCode::NotFound), AgentFailure::NotFound);
        assert_eq!(remote(ErrorCode::AlreadyAnswered), AgentFailure::NotFound);
        assert_eq!(remote(ErrorCode::Denied), AgentFailure::Denied);
        assert_eq!(remote(ErrorCode::Busy), AgentFailure::Busy);
        assert_eq!(remote(ErrorCode::Limit), AgentFailure::Busy);
        assert_eq!(remote(ErrorCode::Unavailable), AgentFailure::Unsupported);
        assert_eq!(remote(ErrorCode::Internal), AgentFailure::Internal);
        assert_eq!(failure(&ClientError::Closed), AgentFailure::Unsupported);
        assert_eq!(failure(&ClientError::NotInstalled), AgentFailure::Unsupported);
        assert_eq!(failure(&ClientError::Protocol("x".into())), AgentFailure::Internal);
    }

    #[test]
    fn items_stay_in_order_and_newer_revisions_win() {
        let mut view = OpenView::new(1, OpenKind::Thread, "t".into());
        assert!(view.apply(entry(&text_item(5, 0, "five"))));
        assert!(view.apply(entry(&text_item(3, 0, "three"))));
        assert!(view.apply(entry(&text_item(5, 1, "five, done"))));
        assert!(!view.apply(entry(&text_item(5, 0, "five"))), "an older revision is stale");
        assert!(!view.apply(entry(&text_item(5, 1, "five, done"))), "the same item changes nothing");
        let seqs: Vec<_> = view.entries.iter().map(|e| e.item.seq).collect();
        assert_eq!(seqs, [3, 5]);
        assert_eq!(view.entries[1].text(), "five, done");
    }

    #[test]
    fn a_long_transcript_keeps_the_newest_items() {
        let mut view = OpenView::new(1, OpenKind::Thread, "t".into());
        for seq in 1..=(OPEN_ITEMS as u64 + 10) {
            view.apply(entry(&text_item(seq, 0, "x")));
        }
        assert_eq!(view.entries.len(), OPEN_ITEMS);
        assert_eq!(view.entries[0].item.seq, 11);
        assert!(view.more);
        // Earlier items read again stay until the next new one.
        view.apply(entry(&text_item(5, 0, "x")));
        assert_eq!((view.entries.len(), view.entries[0].item.seq), (OPEN_ITEMS + 1, 5));
        view.apply(entry(&text_item(OPEN_ITEMS as u64 + 11, 0, "x")));
        assert_eq!(view.entries.len(), OPEN_ITEMS);
        assert_eq!(overflow(3, 1), 0);
        assert_eq!(overflow(OPEN_ITEMS, OPEN_ITEMS), 1);
    }

    #[test]
    fn an_items_text_is_capped() {
        let long = "é".repeat(ROW_TEXT);
        let capped = entry(&text_item(1, 0, &long));
        assert!(capped.text().len() <= ROW_TEXT && capped.item.truncated);
        let mut streaming = entry(&text_item(1, 0, &"a".repeat(ROW_TEXT - 1)));
        assert!(append(&mut streaming, ROW_TEXT - 1, "bcd"));
        assert!(streaming.item.truncated && streaming.text().len() == ROW_TEXT);
    }

    #[test]
    fn deltas_reach_the_rows_at_most_ten_times_a_second() {
        let start = Instant::now();
        let mut view = OpenView::new(1, OpenKind::Thread, "t".into());
        assert_eq!(view.delta(7, "m1", 0, "Hel", start), DeltaOutcome::New);
        assert!(view.entries[0].streaming);
        assert_eq!(view.delta(7, "m1", 3, "lo", start), DeltaOutcome::Text(Some(Duration::ZERO)));
        assert_eq!(view.delta(7, "m1", 5, " there", start), DeltaOutcome::Text(None), "with the first");
        assert_eq!(view.delta(7, "m1", 2, "l", start), DeltaOutcome::Ignored, "text it already has");
        assert_eq!(view.delta(7, "m1", 40, "x", start), DeltaOutcome::Gap);
        assert_eq!(view.delta(9, "m2", 4, "x", start), DeltaOutcome::Gap, "an item whose start is missing");
        assert_eq!(view.entries[0].text(), "Hello there");
        view.gate.flush(start);
        // Within 100 ms of a flush the next waits for the rest of it.
        let soon = start + Duration::from_millis(30);
        assert_eq!(view.delta(7, "m1", 11, "!", soon), DeltaOutcome::Text(Some(Duration::from_millis(70))));
        assert_eq!(view.delta(7, "m1", 12, "!", soon), DeltaOutcome::Text(None));
        // The finished item replaces the text.
        let mut finished = text_item(7, 1, "Hello there!!");
        finished.id = "m1".into();
        assert!(view.apply(entry(&finished)));
        assert!(!view.entries[0].streaming);
    }

    #[test]
    fn a_full_queue_gives_way_to_one_reset() {
        let mut feed = Feed::default();
        assert!(feed.push(UiUpdate::Reset(Vec::new()), &[]), "the first update wakes the GUI");
        assert!(!feed.push(UiUpdate::Reset(Vec::new()), &[]), "later ones wait for it");
        feed.take();
        for _ in 0..(UI_QUEUE + 40) {
            feed.push(UiUpdate::Ops(Vec::new()), &[]);
        }
        let updates = feed.take();
        assert_eq!(updates.iter().filter(|u| matches!(u, UiUpdate::Reset(_))).count(), 1);
        assert!(matches!(updates[0], UiUpdate::Reset(_)));
        assert_eq!(updates.len(), 40);
        assert!(feed.push(UiUpdate::Reset(Vec::new()), &[]), "after taking, the next update wakes it again");
    }

    #[test]
    fn steps_and_changes_come_from_the_finished_edits() {
        let entries: Vec<Arc<Entry>> = [
            tool_item(1, "Bash", "Run command", "cargo test"),
            tool_item(2, "Edit", "Edit src/hold.rs", "-a\n+b\n+c"),
            tool_item(
                3,
                "apply_patch",
                "Edit files",
                "a.rs (update)\n@@ -1,1 +1,1 @@\n-x\n+y\nb.rs (add)\nnew\n",
            ),
            tool_item(4, "Write", "Write src/hold.rs", "+d"),
        ]
        .iter()
        .map(|i| Arc::new(entry(i)))
        .collect();
        let changes = changes_json(&entries);
        let find = |path: &str| changes.as_array().unwrap().iter().find(|c| c["path"] == path).cloned();
        assert_eq!(find("src/hold.rs"), Some(json!({ "path": "src/hold.rs", "added": 3, "removed": 1 })));
        assert_eq!(find("a.rs"), Some(json!({ "path": "a.rs", "added": 1, "removed": 1 })));
        assert_eq!(find("b.rs"), Some(json!({ "path": "b.rs", "added": 1, "removed": 0 })));
        let steps = steps_json(&entries);
        assert_eq!(steps.as_array().unwrap().len(), 4);
        assert_eq!(steps[0]["seq"], "1");
        assert_eq!(steps[3]["title"], "Write src/hold.rs");
    }

    fn snap(view: &OpenView, state: render::ThreadState) -> Snapshot {
        Snapshot {
            generation: view.generation,
            entries: view.entries.clone(),
            state,
            since_ms: 1,
            marks: view.marks(),
            open: view.open_keys.clone(),
            wraps: view.wraps.clone(),
        }
    }

    #[test]
    fn the_timeline_rows_follow_the_items_with_exact_changes() {
        let mut view = OpenView::new(4, OpenKind::Thread, "t".into());
        let mut render = Render::default();
        render.reset(4);
        let mut model: Vec<Arc<ModelRow>> = Vec::new();
        let mut step = |render: &mut Render, view: &OpenView, state| {
            let ops = render.update(&snap(view, state)).expect("its own view");
            let count = ops.len();
            rows::apply_ops(&mut model, ops);
            let keys = |rows: &[Arc<ModelRow>]| rows.iter().map(|r| r.key().to_owned()).collect::<Vec<_>>();
            assert_eq!(keys(&model), keys(&render.shown));
            assert!(model.iter().zip(&render.shown).all(|(a, b)| **a == **b));
            (count, keys(&model))
        };
        let user = Item {
            text: Some("Fix it".into()),
            turn: Some(1),
            origin: Some(Origin::launcher()),
            ..Item::new(1, "u1", ItemKind::User, 100)
        };
        view.apply(entry(&user));
        let (_, keys) = step(&mut render, &view, render::ThreadState::Working);
        assert_eq!(keys, ["i:u1", "live"], "\"Starting…\"");
        // Text streams: the live line goes, the blocks come.
        view.delta(2, "m1", 0, "First paragraph.\n\nSec", Instant::now());
        let (_, keys) = step(&mut render, &view, render::ThreadState::Working);
        assert_eq!(keys, ["i:u1", "b:m1:0", "b:m1:1"]);
        let first = render.shown[1].clone();
        view.delta(2, "m1", 21, "ond paragraph.", Instant::now());
        let (count, _) = step(&mut render, &view, render::ThreadState::Working);
        assert_eq!(count, 1, "only the last block changes");
        assert!(Arc::ptr_eq(&first, &render.shown[1]), "the finished block is reused as it is");
        // A command runs and ends; the turn ends with an answer.
        let mut bash = a_tool("Bash", "Run command", "cargo test", ToolStatus::Ok);
        bash.class = Some(api::ToolClass::Command);
        view.apply(entry(&Item {
            tool: Some(bash),
            turn: Some(1),
            end_ms: Some(4_000),
            ..Item::new(3, "toolu_1", ItemKind::Tool, 1_000)
        }));
        view.apply(entry(&Item {
            text: Some("Done: it **works**.".into()),
            turn: Some(1),
            phase: Some(api::Phase::Final),
            end_ms: Some(5_000),
            ..Item::new(4, "m2", ItemKind::Assistant, 4_500)
        }));
        let (_, keys) = step(&mut render, &view, render::ThreadState::Done);
        assert_eq!(keys, ["i:u1", "f:1", "b:m2:0"], "the work folds behind \"Worked for\"");
        let answer = &render.shown[2];
        assert_eq!(answer.text(Role::Styled), "Done: it <b>works</b>.");
        assert_eq!(answer.get(Role::WhenMs), &RoleValue::Real(5_000.0));
        // Opening the fold puts the work back, at depth 1.
        view.toggle("f:1");
        let (_, keys) = step(&mut render, &view, render::ThreadState::Done);
        assert_eq!(keys, ["i:u1", "f:1", "b:m1:0", "b:m1:1", "i:toolu_1", "b:m2:0"]);
        assert_eq!(render.shown[4].get(Role::Depth), &RoleValue::Int(1));
        assert_eq!(render.shown[4].copy_text(), "cargo test", "Copy on a tool line takes its command");
        view.toggle("f:1");
        let (_, keys) = step(&mut render, &view, render::ThreadState::Done);
        assert_eq!(keys, ["i:u1", "f:1", "b:m2:0"]);
        // Another view's snapshot is not applied.
        let other = OpenView::new(5, OpenKind::Thread, "u".into());
        assert!(render.update(&snap(&other, render::ThreadState::Done)).is_none());
    }

    #[test]
    fn an_answer_is_copied_whole_and_its_link_by_number() {
        let mut render = Render::default();
        let mut view = OpenView::new(2, OpenKind::Thread, "t".into());
        render.reset(2);
        view.apply(entry(&Item {
            text: Some("See [the checklist](https://example.com/c?x=1).\n\n```sh\nmake test\n```".into()),
            phase: Some(api::Phase::Final),
            turn: Some(1),
            ..Item::new(1, "m1", ItemKind::Assistant, 1)
        }));
        render.update(&snap(&view, render::ThreadState::Done)).unwrap();
        let text = render.row("b:m1:0").unwrap();
        assert_eq!(text.text(Role::Styled), "See <a href=\"0\">the checklist</a>.");
        let link = text.link(0).unwrap();
        assert_eq!(
            (link.href.as_str(), link.text.as_str(), link.shown),
            ("https://example.com/c?x=1", "the checklist", false)
        );
        assert!(text.link(1).is_none());
        assert_eq!(render.row("b:m1:1").unwrap().copy_text(), "make test");
    }

    #[test]
    fn phones_share_at_most_two_watch_connections() {
        let (a, b, c) = (DeviceId([1; 32]), DeviceId([2; 32]), DeviceId([3; 32]));
        let conn = |session: &str, generation| WatchConn { session: session.into(), generation, task: None };
        let mut w = PhoneWatches::default();
        assert_eq!(w.plan(&a, "s1"), WatchPlan::Open);
        assert!(w.watch(a, "s1".into(), Some(conn("s1", 1))).is_empty());
        assert_eq!(w.plan(&a, "s1"), WatchPlan::Have);
        assert_eq!(w.plan(&b, "s1"), WatchPlan::Share);
        w.watch(b, "s1".into(), None);
        assert_eq!(w.plan(&c, "s2"), WatchPlan::Open);
        w.watch(c, "s2".into(), Some(conn("s2", 2)));
        assert_eq!(w.plan(&a, "s3"), WatchPlan::Busy, "two connections, both in use");
        // c alone holds s2: moving c to s3 frees it.
        assert_eq!(w.plan(&c, "s3"), WatchPlan::Open);
        let unused = w.watch(c, "s3".into(), Some(conn("s3", 3)));
        assert_eq!(unused.iter().map(|u| u.session.as_str()).collect::<Vec<_>>(), ["s2"]);
        // a leaves s1 but b still follows it.
        assert!(w.unwatch(&a).is_empty());
        assert_eq!(w.unwatch(&b).len(), 1);
        // The host went away under s3.
        w.ended(3);
        assert!(w.conns.is_empty() && w.by_phone.is_empty());
    }

    #[test]
    fn an_export_is_saved_under_a_plain_file_name() {
        let folder = std::path::Path::new("/home/me/Documents");
        assert_eq!(
            export_path(folder, "claude-fix-it-2026-11-19.md"),
            folder.join("claude-fix-it-2026-11-19.md")
        );
        assert_eq!(export_path(folder, "../../etc/passwd"), folder.join("passwd"));
        assert_eq!(export_path(folder, ".bashrc"), folder.join("transcript.md"));
        assert_eq!(export_path(folder, ""), folder.join("transcript.md"));

        let reply = export_reply(ThreadExportResult {
            text: "# Hi\n".into(),
            truncated: true,
            file_name: "a.md".into(),
        });
        assert_eq!(reply, AgentExport { text: "# Hi\n".into(), truncated: true, file_name: "a.md".into() });
    }

    #[test]
    fn commands_are_quoted_for_a_shell() {
        assert_eq!(terminal::quote("claude"), "claude");
        assert_eq!(terminal::quote("/home/me/My Code"), "'/home/me/My Code'");
        assert_eq!(terminal::quote("it's"), r"'it'\''s'");
        assert_eq!(terminal::quote(""), "''");
    }

    /// A host on a socket in a test folder, speaking §2 enough for the bridge:
    /// it answers, sends one event after `threads.list`, and drops the
    /// connection when told to. Its log has each method it was asked.
    async fn fake_host(path: std::path::PathBuf, close: Arc<Notify>, log: Arc<Mutex<Vec<String>>>) {
        fake_host_of("test", path, close, log).await;
    }

    /// [`fake_host`] reporting `version`; "0.1.13" also runs a thread and
    /// doesn't know the methods 0.1.14 added.
    async fn fake_host_of(
        version: &'static str,
        path: std::path::PathBuf,
        close: Arc<Notify>,
        log: Arc<Mutex<Vec<String>>>,
    ) {
        let old = version == "0.1.13";
        let mut running = info(Origin::launcher());
        running.id = "f".repeat(32);
        running.state = State::Working;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            loop {
                tokio::select! {
                    line = lines.next_line() => {
                        let Ok(Some(line)) = line else { break };
                        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                        let method = request["method"].as_str().unwrap_or_default().to_owned();
                        let entry = if method == "subscribe" { format!("subscribe {}", request["params"]) } else { method.clone() };
                        log.lock().unwrap().push(entry);
                        let result = match method.as_str() {
                            "hello" => serde_json::json!({ "version": 1, "host_version": version, "pid": 1 }),
                            "agents.list" => serde_json::json!({ "agents": [AgentInfo::missing(AgentId::Claude)] }),
                            "folders.list" => serde_json::json!({ "folders": [folder("f0123456789ab", true)] }),
                            "threads.list" if old => serde_json::json!({ "threads": [running] }),
                            "threads.list" => serde_json::json!({ "threads": [] }),
                            "sessions.list" | "settings.get" if old => serde_json::Value::Null,
                            "sessions.list" => serde_json::json!({ "sessions": [] }),
                            "settings.get" => serde_json::json!({ "follow_sessions": true }),
                            _ => serde_json::json!({}),
                        };
                        let reply = if result.is_null() {
                            let error = serde_json::json!({ "code": "BAD_REQUEST", "message": "unknown method" });
                            serde_json::json!({ "id": request["id"], "error": error })
                        } else {
                            serde_json::json!({ "id": request["id"], "result": result })
                        };
                        write.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
                        if method == "threads.list" {
                            let event = serde_json::json!({
                                "event": "thread.removed",
                                "params": { "thread_id": "0123456789abcdef0123456789abcdef" },
                            });
                            write.write_all(format!("{event}\n").as_bytes()).await.unwrap();
                        }
                    }
                    () = close.notified() => break,
                }
            }
        }
    }

    #[derive(Debug)]
    struct TestRelay {
        want: Want,
        log: Mutex<Vec<String>>,
    }

    impl TestRelay {
        fn new(want: Want) -> Self {
            Self { want, log: Mutex::default() }
        }

        fn count(&self, what: &str) -> usize {
            self.log.lock().unwrap().iter().filter(|l| *l == what).count()
        }
    }

    impl Relay for TestRelay {
        fn want(&self) -> Want {
            self.want
        }
        async fn catalog(&self, catalog: Option<AgentCatalog>) {
            let line = catalog.map_or("catalog none".to_owned(), |c| format!("catalog {}", c.folders.len()));
            self.log.lock().unwrap().push(line);
        }
        async fn event(&self, event: AgentEvent, _started_by: Option<DeviceId>) {
            self.log.lock().unwrap().push(format!("event {}", event.kind));
        }
        async fn ask(&self, _ask: AgentAsk) {}
        async fn asked(&self, asked: AgentAsked) {
            self.log.lock().unwrap().push(format!("asked {}", asked.outcome));
        }
        async fn session(&self, _session: Option<AgentSession>, _removed: Option<String>) {}
    }

    /// A launch that never starts a host (the fake hosts never look outdated).
    fn test_launch() -> &'static Launch {
        Box::leak(Box::new(Launch::new("/nonexistent/host.sock".into(), client::VERSION, || {
            Err(std::io::Error::other("tests don't start hosts"))
        })))
    }

    async fn until(what: &str, done: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A private folder with a fake host's socket in it.
    fn host_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        // The client only trusts a socket in a folder no one else can enter.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("host.sock");
        (dir, path)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_bridge_follows_the_host_and_reconnects() {
        let (_dir, path) = host_dir();
        let close = Arc::new(Notify::new());
        let log = Arc::new(Mutex::new(Vec::new()));
        let server = tokio::spawn(fake_host(path.clone(), close.clone(), log.clone()));
        let bridge: &'static Bridge = Box::leak(Box::new(Bridge::new()));
        let relay: &'static TestRelay = Box::leak(Box::new(TestRelay::new(Want::Run)));
        let socket = path.clone();
        let follower = tokio::spawn(follow(bridge, relay, test_launch(), move |_| {
            let socket = socket.clone();
            async move { Client::connect(&socket, hello()).await }
        }));

        until("connected", || relay.count("catalog 1") == 1 && relay.count("event removed") == 1).await;
        assert_eq!(bridge.lock().status, Status::Connected);
        assert_eq!(bridge.lock().folders[0].path, "/home/me/Code/g7linux", "the page keeps the path");
        let subscribed = log.lock().unwrap().iter().find(|l| l.starts_with("subscribe")).cloned().unwrap();
        assert!(subscribed.contains("\"sessions\":true"), "{subscribed}");
        assert!(log.lock().unwrap().iter().any(|l| l == "sessions.list"));
        assert_eq!(bridge.lock().following, Some(true));

        // The host goes away: phones hear there's nothing, and the bridge comes back.
        close.notify_one();
        until("told phones", || relay.count("catalog none") == 1).await;
        until("reconnected", || relay.count("catalog 1") == 2).await;
        assert_eq!(bridge.lock().status, Status::Connected);
        follower.abort();
        server.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_running_host_is_followed_without_starting_one() {
        let (_dir, path) = host_dir();
        let log = Arc::new(Mutex::new(Vec::new()));
        let server = tokio::spawn(fake_host(path.clone(), Arc::new(Notify::new()), log));
        // Looking doesn't retry, so the host must be listening before the bridge looks.
        until("listening", || path.exists()).await;
        let bridge: &'static Bridge = Box::leak(Box::new(Bridge::new()));
        let relay: &'static TestRelay = Box::leak(Box::new(TestRelay::new(Want::Look)));
        let spawned = Arc::new(Mutex::new(0));
        let (socket, count) = (path.clone(), spawned.clone());
        let follower = tokio::spawn(follow(bridge, relay, test_launch(), move |want| {
            let socket = socket.clone();
            if want == Want::Run {
                *count.lock().unwrap() += 1;
            }
            async move { Client::connect(&socket, hello()).await }
        }));
        until("connected", || relay.count("catalog 1") == 1).await;
        assert_eq!(bridge.lock().status, Status::Connected);
        assert_eq!(*spawned.lock().unwrap(), 0, "nothing asked to start the host");
        follower.abort();
        server.abort();

        // No host: it's looked for, not started, and phones hear there's nothing once.
        let (_dir, missing) = host_dir();
        let bridge: &'static Bridge = Box::leak(Box::new(Bridge::new()));
        let relay: &'static TestRelay = Box::leak(Box::new(TestRelay::new(Want::Look)));
        let follower = tokio::spawn(follow(bridge, relay, test_launch(), move |_| {
            let socket = missing.clone();
            async move { Client::connect(&socket, hello()).await }
        }));
        until("looked", || relay.count("catalog none") == 1).await;
        assert_eq!(bridge.lock().status, Status::NotRunning);
        follower.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_old_host_running_threads_is_kept_and_the_page_says_to_restart() {
        let (_dir, path) = host_dir();
        let log = Arc::new(Mutex::new(Vec::new()));
        let server = tokio::spawn(fake_host_of("0.1.13", path.clone(), Arc::new(Notify::new()), log));
        let bridge: &'static Bridge = Box::leak(Box::new(Bridge::new()));
        let relay: &'static TestRelay = Box::leak(Box::new(TestRelay::new(Want::Run)));
        let socket = path.clone();
        let follower = tokio::spawn(follow(bridge, relay, test_launch(), move |_| {
            let socket = socket.clone();
            async move { Client::connect(&socket, hello()).await }
        }));
        until("connected", || relay.count("event removed") == 1).await;
        {
            let s = bridge.lock();
            assert_eq!(s.status, Status::Connected);
            assert!(s.outdated, "0.1.13 is older than this build");
            assert!(s.sessions_unsupported, "its unknown method isn't taken for no sessions");
            assert_eq!(s.threads.len(), 1);
        }
        assert!(bridge.any_running(), "so it's kept until its thread finishes");
        follower.abort();
        server.abort();
    }

    #[test]
    fn the_panels_are_remembered_as_the_user_left_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PANELS_FILE);
        assert_eq!(crate::atomic_file::read_json::<Panels>(&path), Panels::default(), "both shown at first");
        let panels = Panels { list_hidden: true, inspector_hidden: false };
        crate::atomic_file::write_json(&path, &panels).unwrap();
        assert_eq!(crate::atomic_file::read_json::<Panels>(&path), panels);
        std::fs::write(&path, br#"{"inspector_hidden":true}"#).unwrap();
        let read = crate::atomic_file::read_json::<Panels>(&path);
        assert_eq!(read, Panels { list_hidden: false, inspector_hidden: true }, "a missing field is shown");
    }

    #[test]
    fn paths_under_home_show_with_a_tilde() {
        let home = std::path::Path::new("/home/me");
        assert_eq!(shown_path("/home/me/Code/g7linux", Some(home)), "~/Code/g7linux");
        assert_eq!(shown_path("/home/me", Some(home)), "~");
        assert_eq!(shown_path("/home/meg/x", Some(home)), "/home/meg/x");
        assert_eq!(shown_path("/srv/x", None), "/srv/x");
    }

    #[test]
    fn the_page_shows_paths_and_plans_but_phones_never_do() {
        let mut s = BridgeState::default();
        let mut claude = AgentInfo::missing(AgentId::Claude);
        claude.plan = Some("max".into());
        s.agents = vec![claude];
        s.folders = vec![folder("f1", true)];
        s.following = Some(true);
        let json: Value = serde_json::from_str(&setup_json(&s, None)).unwrap();
        assert_eq!(json["agents"][0]["plan"], "max");
        assert_eq!(json["folders"][0]["path"], "/home/me/Code/g7linux");
        assert_eq!(json["status"], "idle");
        assert_eq!(json["following"], true);
        let row = session_row(&a_session(SessionState::Idle, false), Some(std::path::Path::new("/home/me")));
        assert_eq!(row["shown_cwd"], "~/code/app");
        let guess = session_row(&a_session(SessionState::NeedsYouGuess, false), None);
        assert_eq!((guess["state"].as_str(), guess["guess"].as_bool()), (Some("needs_you"), Some(true)));
        assert!(row.get("cwd").is_none(), "only the open session carries its folder's path");
    }
}
