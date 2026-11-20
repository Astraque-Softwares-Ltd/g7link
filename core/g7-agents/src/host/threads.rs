// SPDX-License-Identifier: MPL-2.0
//! The thread registry: threads and their state machine, items and deltas,
//! approvals (single use, first answer wins), folders, and the agent
//! sessions behind each thread.
//!
//! Everything here runs under one lock with no awaits held, and every event
//! a thread produces is broadcast while that lock is held, so each thread's
//! events reach clients in order.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};

use crate::api::*;
use crate::host::agents::{
    self, AgentEvent, AgentProcess, Answer, ApprovalSpec, DenyReason, Probe, SessionCmd, Step, ToolSpec,
    TurnEnd, code,
};
use crate::host::notify::Note;
use crate::host::store::{ApprovalRecord, Store, now_ms};
use crate::host::{Answered, Host, claude, codex, env};

/// Whether Codex may run `auto_edit`. With `approvalPolicy: "on-request"`
/// codex-cli 0.170.0 ran a command inside the workspace sandbox without
/// asking (smoke, 2026-11-19), so it can't promise "asks before commands":
/// `auto_edit` is refused for Codex.
pub const CODEX_AUTO_EDIT: bool = false;

const RESTART_NOTICE: &str = "Stopped: the agent host restarted.";
/// A tool step copied from a session that never finished.
pub const UNFINISHED_OUTPUT: &str = "Stopped before it finished.";
/// `thread.state` for a change of `step` alone is sent at most this often.
const STEP_EVERY: Duration = Duration::from_secs(1);

fn random_hex(bytes: usize) -> String {
    let mut out = String::with_capacity(bytes * 2);
    for _ in 0..bytes {
        out.push_str(&format!("{:02x}", rand::random::<u8>()));
    }
    out
}

struct Session {
    generation: u64,
    cmds: mpsc::UnboundedSender<SessionCmd>,
    bridge_token: Option<String>,
}

/// Assistant text being streamed.
struct Stream {
    item: Item,
    /// Bytes already sent as deltas.
    sent: usize,
    /// Text not yet sent.
    pending: String,
}

#[derive(Default)]
struct Turn {
    interrupt_requested: bool,
    /// The agent's ids for items (stream keys, tool use ids) → our seq.
    keys: HashMap<String, u64>,
    streams: BTreeMap<u64, Stream>,
    /// Tool steps not yet ended.
    tools: HashMap<u64, Item>,
    /// Tool steps whose approval was denied, expired or cancelled.
    refused_tools: HashSet<u64>,
    /// How each running tool step is shown as the thread's `step`.
    tool_steps: HashMap<u64, String>,
    flush_scheduled: bool,
    last_text: Option<String>,
    final_text: Option<String>,
}

pub struct Entry {
    pub info: ThreadInfo,
    next_seq: u64,
    /// The folder's path when the thread was loaded or started.
    cwd: Option<PathBuf>,
    session: Option<Session>,
    turn: Turn,
    turn_no: u64,
    allow_edits: bool,
    allow_commands: HashSet<String>,
    /// When this thread's last agent process exited (Unix ms).
    process_ended_ms: u64,
    /// The `step` clients last heard, and when.
    step_announced: Option<String>,
    step_sent: Option<Instant>,
    step_timer: bool,
}

impl Entry {
    fn new(info: ThreadInfo, next_seq: u64, cwd: Option<PathBuf>) -> Self {
        Self {
            step_announced: info.step.clone(),
            info,
            next_seq: next_seq.max(1),
            cwd,
            session: None,
            turn: Turn::default(),
            turn_no: 0,
            allow_edits: false,
            allow_commands: HashSet::new(),
            process_ended_ms: 0,
            step_sent: None,
            step_timer: false,
        }
    }

    /// Changes `state`, and `state_since_ms` with it.
    fn set_state(&mut self, state: State) {
        if self.info.state != state {
            self.info.state = state;
            self.info.state_since_ms = now_ms();
        }
    }

    /// What the running turn is doing: the newest tool step still running.
    fn current_step(&self) -> Option<String> {
        if !self.info.state.is_running() {
            return None;
        }
        let (seq, item) = self.turn.tools.iter().max_by_key(|(seq, _)| **seq)?;
        self.turn
            .tool_steps
            .get(seq)
            .cloned()
            .or_else(|| item.tool.as_ref().map(|t| clean_cut(&t.title, MAX_STEP).0))
    }

    fn alloc(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.info.last_seq = seq;
        self.info.updated_ms = now_ms();
        seq
    }
}

/// What `session.continue` hands the registry.
#[derive(Debug)]
pub(crate) struct Continued {
    pub agent: AgentId,
    pub folder_id: String,
    pub subdir: Option<String>,
    pub title: String,
    pub mode: Mode,
    pub origin: Origin,
    /// The session's UUID: Claude's session id, Codex's thread id.
    pub native_id: String,
    /// Oldest first.
    pub items: Vec<Item>,
    pub notice: String,
}

pub struct PendingApproval {
    approval: Approval,
    reply: Option<oneshot::Sender<Answer>>,
    raw_command: Option<String>,
    tool_seq: Option<u64>,
    bridge_tool_use_id: Option<String>,
    generation: u64,
    /// Asking order, for showing the oldest first.
    order: u64,
}

pub struct Registry {
    pub folders: Vec<Folder>,
    folder_exists: HashMap<String, bool>,
    pub threads: HashMap<String, Entry>,
    approvals: HashMap<String, PendingApproval>,
    answered: Answered,
    bridges: HashMap<String, (String, u64)>,
    next_generation: u64,
    next_order: u64,
    pub login_blocked: HashMap<AgentId, String>,
    approval_lifetime_ms: u64,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("id", &self.info.id)
            .field("state", &self.info.state)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for PendingApproval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingApproval").field("thread_id", &self.approval.thread_id).finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("threads", &self.threads.len())
            .field("approvals", &self.approvals.len())
            .finish_non_exhaustive()
    }
}

fn notice_item(seq: u64, text: &str) -> Item {
    Item {
        seq,
        id: format!("notice-{seq}"),
        kind: ItemKind::Notice,
        ts_ms: now_ms(),
        rev: 0,
        text: Some(text.to_owned()),
        truncated: false,
        origin: None,
        tool: None,
    }
}

/// A running tool step for `spec`.
pub(crate) fn tool_item(seq: u64, key: &str, spec: &ToolSpec) -> Item {
    let (detail, was_cut) = match &spec.detail {
        Some(d) => {
            let (d, was_cut) = clean_cut(d, MAX_TOOL_DETAIL);
            (Some(d), was_cut)
        }
        None => (None, false),
    };
    Item {
        seq,
        id: cut(key, MAX_ITEM_ID).to_owned(),
        kind: ItemKind::Tool,
        ts_ms: now_ms(),
        rev: 0,
        text: None,
        truncated: was_cut,
        origin: None,
        tool: Some(Tool {
            name: clean_cut(&spec.name, MAX_TOOL_NAME).0,
            title: clean_cut(&spec.title, MAX_TOOL_TITLE).0,
            detail,
            status: ToolStatus::Running,
            output: None,
        }),
    }
}

fn title_of(prompt: &str) -> String {
    let line = prompt.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    clean_cut(line, MAX_TITLE).0
}

fn summary_of(text: &str) -> String {
    let collapsed = collapse_whitespace(&clean(text));
    cut(&collapsed, MAX_SUMMARY).to_owned()
}

fn clean_error(error: &ThreadError) -> ThreadError {
    ThreadError {
        code: clean_cut(&error.code, MAX_ERROR_CODE).0,
        message: clean_cut(&error.message, MAX_ERROR_MESSAGE).0,
    }
}

fn emit_item(host: &Host, thread_id: &str, item: &Item) {
    host.store.append_item(thread_id, item);
    host.hub.broadcast(&Event::ThreadItem(ThreadItemEvent {
        thread_id: thread_id.to_owned(),
        item: item.clone(),
    }));
}

fn emit_state(host: &Host, entry: &mut Entry) {
    entry.info.step = entry.current_step();
    entry.step_announced = entry.info.step.clone();
    entry.step_sent = Some(Instant::now());
    host.store.save_meta(&entry.info, entry.next_seq);
    host.hub.broadcast(&Event::ThreadState(ThreadResult { thread: entry.info.clone() }));
}

/// The working directory of a thread in `folder`: the folder itself, or
/// `subdir` inside it, which must still be inside it once links are resolved.
pub(crate) fn thread_dir(folder: &Path, subdir: Option<&str>) -> Option<PathBuf> {
    let Some(subdir) = subdir.filter(|s| !s.is_empty()) else { return Some(folder.to_owned()) };
    let rel = Path::new(subdir);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    let base = std::fs::canonicalize(folder).ok()?;
    let dir = std::fs::canonicalize(base.join(rel)).ok()?;
    (dir.starts_with(&base) && dir.is_dir()).then_some(dir)
}

/// `subdir` for a working directory inside a folder: `None` when they are
/// the same, an error for anything but plain components.
pub(crate) fn subdir_of(dir: &Path, folder: &Path) -> Result<Option<String>, ()> {
    let rel = dir.strip_prefix(folder).map_err(|_| ())?;
    if rel.as_os_str().is_empty() {
        return Ok(None);
    }
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(());
    }
    rel.to_str().map(|s| Some(s.to_owned())).ok_or(())
}

fn notify(host: &Host, note: Note) {
    if let Some(notifier) = &host.notifier {
        notifier.send(note);
    }
}

/// Posts a "needs you" or "finished" notification, unless a launcher or G7
/// Link shows that thread to the user right now: then the thread's
/// notification, if one is up, is taken down instead. Clients still get
/// every event, so lists, badges and phones change as usual.
fn notify_unless_viewed(host: &Host, thread_id: &str, note: Note) {
    if host.hub.viewed(thread_id) {
        notify(host, Note::Removed { thread_id: thread_id.to_owned() });
    } else {
        notify(host, note);
    }
}

fn err(code: ErrorCode, message: impl AsRef<str>) -> RpcError {
    RpcError::new(code, message)
}

impl Registry {
    /// Reads the store. Threads that were running become `interrupted`, and
    /// the oldest threads past [`MAX_THREADS_KEPT`] are pruned.
    pub fn load(store: &mut Store, approval_lifetime_ms: u64) -> Registry {
        let folders = store.load_folders();
        let mut threads = HashMap::new();
        for stored in store.load_threads() {
            let mut info = stored.info;
            let mut next_seq = stored.next_seq;
            info.pending = 0;
            info.step = None;
            if info.state_since_ms == 0 {
                info.state_since_ms = info.updated_ms;
            }
            if info.state.is_running() {
                info.state = State::Interrupted;
                info.state_since_ms = now_ms();
                let item = notice_item(next_seq, RESTART_NOTICE);
                next_seq += 1;
                info.last_seq = item.seq;
                if let Err(e) = store.append_item(&info.id, &item) {
                    tracing::warn!("couldn't save a thread item: {e}");
                }
                if let Err(e) = store.save_meta(&info, next_seq) {
                    tracing::warn!("couldn't save a thread: {e}");
                }
            }
            let cwd = folders
                .iter()
                .find(|f| f.id == info.folder_id)
                .and_then(|f| thread_dir(Path::new(&f.path), info.subdir.as_deref()));
            threads.insert(info.id.clone(), Entry::new(info, next_seq, cwd));
        }
        let mut registry = Registry {
            folder_exists: folders.iter().map(|f| (f.id.clone(), Path::new(&f.path).is_dir())).collect(),
            folders,
            threads,
            approvals: HashMap::new(),
            answered: Answered::default(),
            bridges: HashMap::new(),
            next_generation: 1,
            next_order: 0,
            login_blocked: HashMap::new(),
            approval_lifetime_ms,
        };
        for id in registry.prunable() {
            registry.threads.remove(&id);
            if let Err(e) = store.delete_thread(&id) {
                tracing::warn!("couldn't delete a thread: {e}");
            }
        }
        registry
    }

    /// Threads past the limit, oldest not running first.
    fn prunable(&self) -> Vec<String> {
        let excess = self.threads.len().saturating_sub(MAX_THREADS_KEPT);
        if excess == 0 {
            return Vec::new();
        }
        let mut idle: Vec<(&String, u64)> = self
            .threads
            .iter()
            .filter(|(_, e)| !e.info.state.is_running())
            .map(|(id, e)| (id, e.info.updated_ms))
            .collect();
        idle.sort_by_key(|(_, t)| *t);
        idle.into_iter().take(excess).map(|(id, _)| id.clone()).collect()
    }

    fn running(&self) -> (usize, HashMap<AgentId, usize>) {
        let mut total = 0;
        let mut per = HashMap::new();
        for entry in self.threads.values().filter(|e| e.info.state.is_running()) {
            total += 1;
            *per.entry(entry.info.agent).or_insert(0) += 1;
        }
        (total, per)
    }

    fn pending_for(&self, thread_id: &str) -> usize {
        self.approvals.values().filter(|p| p.approval.thread_id == thread_id).count()
    }

    /// The thread's waiting approvals, oldest first.
    fn pending_list(&self, thread_id: &str) -> Vec<Approval> {
        let mut list: Vec<&PendingApproval> =
            self.approvals.values().filter(|p| p.approval.thread_id == thread_id).collect();
        list.sort_by_key(|p| p.order);
        list.into_iter().map(|p| p.approval.clone()).collect()
    }
}

impl Host {
    pub fn any_running(&self) -> bool {
        self.lock_reg().threads.values().any(|e| e.info.state.is_running())
    }

    fn busy_message(&self, agent: AgentId, total: usize, per: &HashMap<AgentId, usize>) -> Option<String> {
        if total >= MAX_RUNNING {
            return Some(format!(
                "{} is already running {MAX_RUNNING} threads. Stop one to start another.",
                self.hostname
            ));
        }
        if per.get(&agent).copied().unwrap_or(0) >= MAX_RUNNING_PER_AGENT {
            return Some(format!(
                "{} is already running {MAX_RUNNING_PER_AGENT} {} threads. Stop one to start another.",
                self.hostname,
                agent.display_name()
            ));
        }
        None
    }

    // ---- Agents ----

    /// Both probes, from the cache unless stale or `refresh`.
    pub async fn probe(&self, refresh: bool) -> [Probe; 2] {
        let mut cache = self.probes.lock().await;
        let stale = cache.at.is_none_or(|at| at.elapsed() >= Duration::from_millis(PROBE_CACHE_MS));
        if refresh || stale || cache.claude.is_none() || cache.codex.is_none() {
            let (claude, codex) = tokio::join!(
                agents::probe_claude(self.config.claude_bin.as_deref(), &self.config.env),
                agents::probe_codex(self.config.codex_bin.as_deref(), &self.config.env),
            );
            if refresh {
                let mut reg = self.lock_reg();
                for probe in [&claude, &codex] {
                    if probe.info.ready() {
                        reg.login_blocked.remove(&probe.info.id);
                    }
                }
            }
            cache.claude = Some(claude);
            cache.codex = Some(codex);
            cache.at = Some(std::time::Instant::now());
        }
        let mut probes = [
            cache.claude.clone().unwrap_or(Probe { info: AgentInfo::missing(AgentId::Claude), bin: None }),
            cache.codex.clone().unwrap_or(Probe { info: AgentInfo::missing(AgentId::Codex), bin: None }),
        ];
        {
            let reg = self.lock_reg();
            for probe in &mut probes {
                if let Some(message) = reg.login_blocked.get(&probe.info.id) {
                    probe.info.signed_in = false;
                    probe.info.problem = Some(clean_cut(message, MAX_PROBLEM).0);
                }
            }
        }
        let shown: Vec<AgentInfo> = probes.iter().map(|p| p.info.clone()).collect();
        if cache.shown.as_ref() != Some(&shown) {
            if cache.shown.is_some() {
                self.hub.broadcast(&Event::AgentsChanged(AgentsListResult { agents: shown.clone() }));
            }
            cache.shown = Some(shown);
        }
        probes
    }

    pub async fn agents(&self, refresh: bool) -> Vec<AgentInfo> {
        self.probe(refresh).await.into_iter().map(|p| p.info).collect()
    }

    // ---- Folders ----

    pub fn folders_list(&self) -> Vec<Folder> {
        let reg = self.lock_reg();
        reg.folders.iter().map(|f| Folder { exists: Path::new(&f.path).is_dir(), ..f.clone() }).collect()
    }

    /// Sends `folders.changed` when a folder appeared or disappeared.
    pub fn check_folders(&self) {
        let changed = {
            let mut reg = self.lock_reg();
            let now: HashMap<String, bool> =
                reg.folders.iter().map(|f| (f.id.clone(), Path::new(&f.path).is_dir())).collect();
            let changed = now != reg.folder_exists;
            reg.folder_exists = now;
            changed
        };
        if changed {
            self.hub.broadcast(&Event::FoldersChanged(FoldersListResult { folders: self.folders_list() }));
        }
    }

    pub async fn folders_add(&self, path: String, label: Option<String>) -> Result<Folder, RpcError> {
        let _serial = self.folders_lock.lock().await;
        if path.len() > MAX_PATH || label.as_ref().is_some_and(|l| l.len() > MAX_LABEL) {
            return Err(err(ErrorCode::BadRequest, "Too long."));
        }
        if !Path::new(&path).is_absolute() {
            return Err(err(ErrorCode::BadRequest, "The path must be absolute."));
        }
        let checked = tokio::task::spawn_blocking(move || -> Result<PathBuf, &'static str> {
            let canonical = std::fs::canonicalize(&path).map_err(|_| "That folder doesn't exist.")?;
            if !canonical.is_dir() {
                return Err("That isn't a folder.");
            }
            if canonical == Path::new("/") {
                return Err("Pick a folder other than the root.");
            }
            if canonical.components().any(|c| matches!(c, Component::ParentDir)) {
                return Err("That path isn't allowed.");
            }
            std::fs::read_dir(&canonical).map_err(|_| "That folder can't be read.")?;
            Ok(canonical)
        })
        .await
        .map_err(|_| err(ErrorCode::Internal, "The check failed."))?;
        let canonical = checked.map_err(|m| err(ErrorCode::BadRequest, m))?;
        let Some(canonical_str) = canonical.to_str().map(str::to_owned) else {
            return Err(err(ErrorCode::BadRequest, "That path isn't valid text."));
        };
        if canonical_str.len() > MAX_PATH {
            return Err(err(ErrorCode::BadRequest, "Too long."));
        }
        let label = label
            .map(|l| clean_cut(l.trim(), MAX_LABEL).0)
            .filter(|l| !l.is_empty())
            .or_else(|| canonical.file_name().map(|n| clean_cut(&n.to_string_lossy(), MAX_LABEL).0))
            .unwrap_or_else(|| canonical_str.clone());
        let (folder, list) = {
            let reg = self.lock_reg();
            if let Some(existing) = reg.folders.iter().find(|f| f.path == canonical_str) {
                return Ok(Folder { exists: true, ..existing.clone() });
            }
            if reg.folders.len() >= MAX_FOLDERS {
                return Err(err(ErrorCode::Limit, "You can list up to 64 folders."));
            }
            let mut id = format!("f{}", random_hex(6));
            while reg.folders.iter().any(|f| f.id == id) {
                id = format!("f{}", random_hex(6));
            }
            let folder = Folder { id, label, path: canonical_str, exists: true, added_ms: now_ms() };
            let mut list = reg.folders.clone();
            list.push(folder.clone());
            (folder, list)
        };
        self.store
            .save_folders(list.clone())
            .await
            .map_err(|_| err(ErrorCode::Internal, "Couldn't save the folder list."))?;
        {
            let mut reg = self.lock_reg();
            reg.folder_exists.insert(folder.id.clone(), true);
            reg.folders = list;
        }
        self.hub.broadcast(&Event::FoldersChanged(FoldersListResult { folders: self.folders_list() }));
        Ok(folder)
    }

    pub async fn folders_remove(&self, id: &str) -> Result<(), RpcError> {
        let _serial = self.folders_lock.lock().await;
        let list = {
            let reg = self.lock_reg();
            if !is_valid_folder_id(id) || !reg.folders.iter().any(|f| f.id == id) {
                return Err(err(ErrorCode::NotFound, "No such folder."));
            }
            reg.folders.iter().filter(|f| f.id != id).cloned().collect::<Vec<_>>()
        };
        self.store
            .save_folders(list.clone())
            .await
            .map_err(|_| err(ErrorCode::Internal, "Couldn't save the folder list."))?;
        {
            let mut reg = self.lock_reg();
            reg.folder_exists.remove(id);
            reg.folders = list;
        }
        self.hub.broadcast(&Event::FoldersChanged(FoldersListResult { folders: self.folders_list() }));
        Ok(())
    }

    // ---- Threads ----

    pub fn threads_list(&self, params: ThreadsListParams) -> Vec<ThreadInfo> {
        let limit = params.limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT) as usize;
        let reg = self.lock_reg();
        let mut threads: Vec<ThreadInfo> = reg
            .threads
            .values()
            .filter(|e| params.before_ms.is_none_or(|before| e.info.updated_ms < before))
            .map(|e| e.info.clone())
            .collect();
        threads.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms).then_with(|| b.id.cmp(&a.id)));
        threads.truncate(limit);
        threads
    }

    pub(crate) fn unavailable(probe: &Probe) -> RpcError {
        let info = &probe.info;
        let message = if !info.installed {
            format!("{} isn't installed.", info.name)
        } else if let Some(problem) = &info.problem {
            problem.clone()
        } else {
            format!("{} isn't signed in. Run \"{}\" in a terminal.", info.name, info.login_hint)
        };
        err(ErrorCode::Unavailable, message)
    }

    pub async fn thread_start(self: &Arc<Self>, p: ThreadStartParams) -> Result<ThreadInfo, RpcError> {
        if p.prompt.is_empty() || p.prompt.len() > MAX_PROMPT {
            return Err(err(ErrorCode::BadRequest, "The prompt must be 1 to 16384 bytes."));
        }
        if !p.origin.is_valid() {
            return Err(err(ErrorCode::BadRequest, "Bad origin."));
        }
        if !is_valid_folder_id(&p.folder_id) {
            return Err(err(ErrorCode::NotFound, "No such folder."));
        }
        if p.origin.kind == OriginKind::Phone && p.mode != Mode::Supervised {
            return Err(err(
                ErrorCode::Denied,
                "A phone can only start threads that ask before each change.",
            ));
        }
        if p.agent == AgentId::Codex && p.mode == Mode::AutoEdit && !CODEX_AUTO_EDIT {
            return Err(err(ErrorCode::Unavailable, "Codex can't edit without asking yet."));
        }
        let probe = self.probe(false).await[p.agent as usize].clone();
        let Some(bin) = probe.bin.clone().filter(|_| probe.info.ready()) else {
            return Err(Self::unavailable(&probe));
        };
        let mut reg = self.lock_reg();
        if let Some(message) = reg.login_blocked.get(&p.agent) {
            return Err(err(ErrorCode::Unavailable, message));
        }
        let Some(folder) = reg.folders.iter().find(|f| f.id == p.folder_id).cloned() else {
            return Err(err(ErrorCode::NotFound, "No such folder."));
        };
        let cwd = PathBuf::from(&folder.path);
        if !cwd.is_dir() {
            return Err(err(ErrorCode::Denied, "The folder is gone."));
        }
        let (total, per) = reg.running();
        if let Some(message) = self.busy_message(p.agent, total, &per) {
            return Err(err(ErrorCode::Busy, message));
        }
        let mut id = random_hex(16);
        while reg.threads.contains_key(&id) {
            id = random_hex(16);
        }
        let now = now_ms();
        let info = ThreadInfo {
            id: id.clone(),
            agent: p.agent,
            folder_id: folder.id.clone(),
            folder_label: folder.label.clone(),
            title: title_of(&p.prompt),
            state: State::Working,
            mode: p.mode,
            origin: p.origin.clone(),
            created_ms: now,
            updated_ms: now,
            last_seq: 0,
            pending: 0,
            summary: None,
            error: None,
            native_id: None,
            step: None,
            plan: None,
            state_since_ms: now,
            subdir: None,
            continued: false,
        };
        let mut entry = Entry::new(info, 1, Some(cwd));
        entry.turn_no = 1;
        let user = self.user_item(&mut entry, &p.prompt, &p.origin);
        emit_state(self, &mut entry);
        emit_item(self, &id, &user);
        reg.threads.insert(id.clone(), entry);
        self.start_session(&mut reg, &id, p.prompt, &bin);
        if p.origin.kind == OriginKind::Phone {
            notify(self, Note::PhoneStarted { thread: reg.threads[&id].info.clone(), continued: false });
        }
        for old in reg.prunable() {
            self.remove_thread(&mut reg, &old);
        }
        reg.threads.get(&id).map(|e| e.info.clone()).ok_or_else(|| err(ErrorCode::Internal, "lost"))
    }

    fn user_item(&self, entry: &mut Entry, text: &str, origin: &Origin) -> Item {
        let seq = entry.alloc();
        Item {
            seq,
            id: format!("user-{seq}"),
            kind: ItemKind::User,
            ts_ms: now_ms(),
            rev: 0,
            text: Some(text.to_owned()),
            truncated: false,
            origin: Some(origin.clone()),
            tool: None,
        }
    }

    /// Starts a process for the thread with `text` as the first message;
    /// resumes when the thread has a native id.
    fn start_session(self: &Arc<Self>, reg: &mut Registry, thread_id: &str, text: String, bin: &Path) {
        let generation = reg.next_generation;
        reg.next_generation += 1;
        let Some(entry) = reg.threads.get_mut(thread_id) else { return };
        let Some(cwd) = entry.cwd.clone() else { return };
        let (tx, rx) = mpsc::unbounded_channel();
        let events = self.events.clone();
        let agent = entry.info.agent;
        let child_env = env::child_env(agent, self.config.env.iter().cloned());
        let mut bridge_token = None;
        let spawned: std::io::Result<()> = match agent {
            AgentId::Claude => {
                let (session_id, resume) = match &entry.info.native_id {
                    Some(id) if claude::is_session_id(id) => (id.clone(), true),
                    _ => (claude::new_session_id(), false),
                };
                let token = random_hex(16);
                let launch = claude::Launch {
                    bin: bin.to_owned(),
                    cwd: cwd.clone(),
                    mode: entry.info.mode,
                    session_id: session_id.clone(),
                    resume,
                    mcp_config: self.config.runtime_dir.join(format!("mcp-{thread_id}-{generation}.json")),
                    bridge_exe: self.config.bridge_exe.clone(),
                    socket: self.config.socket.clone(),
                    bridge_token: token.clone(),
                    env: child_env,
                    first_text: text.clone(),
                };
                claude::spawn(&launch).map(|(process, lines)| {
                    if !resume {
                        entry.info.native_id = Some(session_id.clone());
                        self.store.save_meta(&entry.info, entry.next_seq);
                    }
                    bridge_token = Some(token);
                    let driver = claude::ClaudeDriver::new(cwd, session_id, text);
                    let thread = thread_id.to_owned();
                    let mcp = launch.mcp_config.clone();
                    tokio::spawn(async move {
                        agents::run_session(process, lines, driver, thread, generation, rx, events).await;
                        let _ = std::fs::remove_file(mcp);
                    });
                })
            }
            AgentId::Codex => {
                let resume = entry.info.native_id.clone();
                AgentProcess::spawn(bin, &["app-server".to_owned()], &cwd, &child_env).map(
                    |(process, lines)| {
                        let driver = codex::CodexDriver::new(cwd, entry.info.mode, resume, text);
                        let thread = thread_id.to_owned();
                        tokio::spawn(agents::run_session(
                            process, lines, driver, thread, generation, rx, events,
                        ));
                    },
                )
            }
        };
        match spawned {
            Ok(()) => {
                if let Some(token) = &bridge_token {
                    reg.bridges.insert(token.clone(), (thread_id.to_owned(), generation));
                }
                if let Some(entry) = reg.threads.get_mut(thread_id) {
                    entry.session = Some(Session { generation, cmds: tx, bridge_token });
                }
            }
            Err(e) => {
                tracing::warn!("couldn't start an agent: {e}");
                let message = format!("Couldn't start {}: {}", agent.display_name(), e.kind());
                self.finish_turn(reg, thread_id, TurnEnd::failed(code::START_FAILED, message));
            }
        }
    }

    pub async fn thread_send(self: &Arc<Self>, p: ThreadSendParams) -> Result<(), RpcError> {
        if p.text.is_empty() || p.text.len() > MAX_PROMPT {
            return Err(err(ErrorCode::BadRequest, "The text must be 1 to 16384 bytes."));
        }
        if !p.origin.is_valid() {
            return Err(err(ErrorCode::BadRequest, "Bad origin."));
        }
        let (agent, resume) = {
            let reg = self.lock_reg();
            let entry =
                reg.threads.get(&p.thread_id).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
            let alive = entry.session.as_ref().is_some_and(|s| !s.cmds.is_closed());
            let ours_until = entry.info.updated_ms.max(entry.process_ended_ms);
            let resume = entry.info.native_id.clone().filter(|_| !alive && !entry.info.state.is_running());
            (entry.info.agent, resume.map(|native| (native, ours_until)))
        };
        // A new process would resume the agent's session: refuse while
        // something else (a terminal, the agent's app) has it open.
        if let Some((native, ours_until)) = resume
            && let Some(source) = self.open_elsewhere(agent, &native, ours_until).await
        {
            return Err(err(ErrorCode::Busy, crate::host::sessions::still_open_message(agent, source)));
        }
        let probe = self.probe(false).await[agent as usize].clone();
        let mut guard = self.lock_reg();
        let reg = &mut *guard;
        let (total, per) = reg.running();
        let blocked = reg.login_blocked.get(&agent).cloned();
        let folder_ok = {
            let Some(entry) = reg.threads.get(&p.thread_id) else {
                return Err(err(ErrorCode::NotFound, "No such thread."));
            };
            if entry.info.state.is_running() {
                return Err(err(
                    ErrorCode::Busy,
                    format!("You can reply when {} stops.", agent.display_name()),
                ));
            }
            reg.folders
                .iter()
                .find(|f| f.id == entry.info.folder_id)
                .and_then(|f| thread_dir(Path::new(&f.path), entry.info.subdir.as_deref()))
                .filter(|path| path.is_dir())
        };
        if let Some(message) = self.busy_message(agent, total, &per) {
            return Err(err(ErrorCode::Busy, message));
        }
        let Some(cwd) = folder_ok else {
            return Err(err(ErrorCode::Denied, "The folder was removed or is gone."));
        };
        let Some(entry) = reg.threads.get_mut(&p.thread_id) else {
            return Err(err(ErrorCode::NotFound, "No such thread."));
        };
        let alive = entry.session.as_ref().is_some_and(|s| !s.cmds.is_closed());
        if !alive {
            if let Some(message) = blocked {
                return Err(err(ErrorCode::Unavailable, message));
            }
            if probe.bin.is_none() || !probe.info.ready() {
                return Err(Self::unavailable(&probe));
            }
        }
        entry.cwd = Some(cwd);
        entry.turn = Turn::default();
        entry.turn_no += 1;
        entry.set_state(State::Working);
        entry.info.error = None;
        let user = self.user_item(entry, &p.text, &p.origin);
        emit_state(self, entry);
        emit_item(self, &p.thread_id, &user);
        let delivered = alive
            && entry.session.as_ref().is_some_and(|s| s.cmds.send(SessionCmd::Send(p.text.clone())).is_ok());
        if !delivered {
            let old_token = entry.session.take().and_then(|s| s.bridge_token);
            if let Some(old) = old_token {
                reg.bridges.remove(&old);
            }
            let Some(bin) = probe.bin else { return Err(Self::unavailable(&probe)) };
            self.start_session(reg, &p.thread_id, p.text, &bin);
        }
        Ok(())
    }

    pub fn thread_interrupt(self: &Arc<Self>, thread_id: &str) -> Result<(), RpcError> {
        let mut reg = self.lock_reg();
        let Some(entry) = reg.threads.get_mut(thread_id) else {
            return Err(err(ErrorCode::NotFound, "No such thread."));
        };
        if !entry.info.state.is_running() || entry.turn.interrupt_requested {
            return Ok(());
        }
        entry.turn.interrupt_requested = true;
        let turn_no = entry.turn_no;
        let sent = entry.session.as_ref().is_some_and(|s| s.cmds.send(SessionCmd::Interrupt).is_ok());
        if !sent {
            self.finish_turn(&mut reg, thread_id, TurnEnd::interrupted());
            return Ok(());
        }
        let host = Arc::downgrade(self);
        let thread = thread_id.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(agents::INTERRUPT_GRACE).await;
            let Some(host) = host.upgrade() else { return };
            let reg = host.lock_reg();
            if let Some(entry) = reg.threads.get(&thread)
                && entry.turn_no == turn_no
                && entry.info.state.is_running()
                && let Some(session) = &entry.session
            {
                // The agent didn't stop on its own: stop the process. The
                // exit ends the turn as interrupted.
                let _ = session.cmds.send(SessionCmd::Kill);
            }
        });
        Ok(())
    }

    pub async fn thread_read(&self, p: ThreadReadParams) -> Result<ThreadReadResult, RpcError> {
        if !is_valid_id(&p.thread_id) {
            return Err(err(ErrorCode::NotFound, "No such thread."));
        }
        let limit = p.limit.unwrap_or(DEFAULT_READ_LIMIT).clamp(1, MAX_READ_LIMIT) as usize;
        let (info, live, pending) = {
            let reg = self.lock_reg();
            let Some(entry) = reg.threads.get(&p.thread_id) else {
                return Err(err(ErrorCode::NotFound, "No such thread."));
            };
            let live: Vec<Item> = entry.turn.streams.values().map(|s| s.item.clone()).collect();
            (entry.info.clone(), live, reg.pending_list(&p.thread_id))
        };
        let mut items = self
            .store
            .read_items(&p.thread_id)
            .await
            .map_err(|_| err(ErrorCode::Internal, "Couldn't read the thread."))?;
        for item in live {
            items.insert(item.seq, item);
        }
        let selected: Vec<Item> = match (p.after_seq, p.before_seq) {
            (Some(after), _) => {
                items.range(after.saturating_add(1)..).take(limit).map(|(_, i)| i.clone()).collect()
            }
            (None, Some(before)) => {
                let mut v: Vec<Item> =
                    items.range(..before).rev().take(limit).map(|(_, i)| i.clone()).collect();
                v.reverse();
                v
            }
            (None, None) => {
                let mut v: Vec<Item> = items.values().rev().take(limit).cloned().collect();
                v.reverse();
                v
            }
        };
        let base = serde_json::to_vec(&ThreadReadResult {
            thread: info.clone(),
            items: Vec::new(),
            more_before: false,
            pending: pending.clone(),
        })
        .map(|v| v.len())
        .unwrap_or(0);
        let mut budget = MAX_READ_RESULT.saturating_sub(base + 16);
        let mut kept: Vec<Item> = Vec::new();
        for item in selected.into_iter().rev() {
            let size = serde_json::to_vec(&item).map(|v| v.len() + 1).unwrap_or(0);
            if size > budget {
                break;
            }
            budget -= size;
            kept.push(item);
        }
        kept.reverse();
        let first = kept.first().map(|i| i.seq);
        let more_before = match first {
            Some(first) => items.range(..first).next().is_some(),
            None => match p.before_seq {
                Some(before) => items.range(..before).next().is_some(),
                None => !items.is_empty() && p.after_seq.is_none(),
            },
        };
        Ok(ThreadReadResult { thread: info, items: kept, more_before, pending })
    }

    fn remove_thread(&self, reg: &mut Registry, thread_id: &str) {
        if let Some(entry) = reg.threads.remove(thread_id) {
            if let Some(session) = entry.session {
                let _ = session.cmds.send(SessionCmd::Close);
                if let Some(token) = session.bridge_token {
                    reg.bridges.remove(&token);
                }
            }
            self.store.delete(thread_id);
            self.hub.broadcast(&Event::ThreadRemoved(ThreadRemovedEvent { thread_id: thread_id.to_owned() }));
            notify(self, Note::Removed { thread_id: thread_id.to_owned() });
        }
    }

    pub fn thread_delete(&self, thread_id: &str) -> Result<(), RpcError> {
        let mut reg = self.lock_reg();
        let Some(entry) = reg.threads.get(thread_id) else {
            return Err(err(ErrorCode::NotFound, "No such thread."));
        };
        if entry.info.state.is_running() {
            return Err(err(ErrorCode::Busy, "Stop the thread first."));
        }
        self.remove_thread(&mut reg, thread_id);
        Ok(())
    }

    pub fn thread_terminal_command(&self, thread_id: &str) -> Result<TerminalCommandResult, RpcError> {
        let reg = self.lock_reg();
        let Some(entry) = reg.threads.get(thread_id) else {
            return Err(err(ErrorCode::NotFound, "No such thread."));
        };
        let Some(native) = entry.info.native_id.clone() else {
            return Err(err(ErrorCode::Unavailable, "The agent hasn't started this thread yet."));
        };
        let cwd = reg
            .folders
            .iter()
            .find(|f| f.id == entry.info.folder_id)
            .and_then(|f| thread_dir(Path::new(&f.path), entry.info.subdir.as_deref()))
            .or_else(|| entry.cwd.clone())
            .map(|c| c.to_string_lossy().into_owned())
            .ok_or_else(|| err(ErrorCode::Unavailable, "The folder was removed."))?;
        let argv = match entry.info.agent {
            AgentId::Claude => vec!["claude".to_owned(), "--resume".to_owned(), native],
            AgentId::Codex => vec!["codex".to_owned(), "resume".to_owned(), native],
        };
        Ok(TerminalCommandResult { argv, cwd })
    }

    // ---- Continued sessions ----

    /// Makes a thread from a session started elsewhere (`session.continue`
    /// step 6): `done`, with the session's newest items renumbered from 1, a
    /// notice, and no process. The next `thread.send` resumes the session.
    pub(crate) fn insert_continued(self: &Arc<Self>, c: Continued) -> Result<ThreadInfo, RpcError> {
        let mut reg = self.lock_reg();
        let Some(folder) = reg.folders.iter().find(|f| f.id == c.folder_id).cloned() else {
            return Err(err(ErrorCode::Denied, "The folder was removed."));
        };
        let Some(cwd) = thread_dir(Path::new(&folder.path), c.subdir.as_deref()) else {
            return Err(err(ErrorCode::Denied, "The folder is gone."));
        };
        if reg
            .threads
            .values()
            .any(|e| e.info.agent == c.agent && e.info.native_id.as_deref() == Some(c.native_id.as_str()))
        {
            return Err(err(ErrorCode::NotFound, "That session is already a thread here."));
        }
        let mut id = random_hex(16);
        while reg.threads.contains_key(&id) {
            id = random_hex(16);
        }
        let now = now_ms();
        let mut items: Vec<Item> = Vec::with_capacity(c.items.len() + 1);
        for (n, mut item) in c.items.into_iter().enumerate() {
            item.seq = n as u64 + 1;
            item.rev = 0;
            if let Some(tool) = item.tool.as_mut()
                && tool.status == ToolStatus::Running
            {
                tool.status = ToolStatus::Error;
                tool.output = Some(UNFINISHED_OUTPUT.to_owned());
            }
            items.push(item);
        }
        let notice_seq = items.len() as u64 + 1;
        items.push(notice_item(notice_seq, &c.notice));
        let info = ThreadInfo {
            id: id.clone(),
            agent: c.agent,
            folder_id: folder.id.clone(),
            folder_label: folder.label.clone(),
            title: c.title,
            state: State::Done,
            mode: c.mode,
            origin: c.origin.clone(),
            created_ms: now,
            updated_ms: now,
            last_seq: notice_seq,
            pending: 0,
            summary: None,
            error: None,
            native_id: Some(c.native_id),
            step: None,
            plan: None,
            state_since_ms: now,
            subdir: c.subdir,
            continued: true,
        };
        for item in &items {
            self.store.append_item(&id, item);
        }
        let mut entry = Entry::new(info, notice_seq + 1, Some(cwd));
        emit_state(self, &mut entry);
        let info = entry.info.clone();
        reg.threads.insert(id.clone(), entry);
        if c.origin.kind == OriginKind::Phone {
            notify(self, Note::PhoneStarted { thread: info.clone(), continued: true });
        }
        for old in reg.prunable() {
            self.remove_thread(&mut reg, &old);
        }
        Ok(info)
    }

    /// The agent's own ids of every thread: their sessions aren't listed as
    /// started elsewhere.
    pub(crate) fn native_ids(&self) -> HashSet<String> {
        self.lock_reg().threads.values().filter_map(|e| e.info.native_id.clone()).collect()
    }

    // ---- Agent events ----

    pub fn on_agent_event(self: &Arc<Self>, thread_id: &str, generation: u64, event: AgentEvent) {
        let mut reg = self.lock_reg();
        let current = reg
            .threads
            .get(thread_id)
            .is_some_and(|e| e.session.as_ref().is_some_and(|s| s.generation == generation));
        match event {
            AgentEvent::Exited { code } => {
                if !current {
                    return;
                }
                let Some(entry) = reg.threads.get_mut(thread_id) else { return };
                entry.process_ended_ms = now_ms();
                let token = entry.session.take().and_then(|s| s.bridge_token);
                let interrupted = entry.turn.interrupt_requested;
                let running = entry.info.state.is_running();
                if let Some(token) = token {
                    reg.bridges.remove(&token);
                }
                if running {
                    let end = if interrupted {
                        TurnEnd::interrupted()
                    } else {
                        let text = code.map_or_else(|| "exit signal".to_owned(), |c| format!("exit {c}"));
                        TurnEnd::failed(code::AGENT_EXITED, text)
                    };
                    self.finish_turn(&mut reg, thread_id, end);
                }
            }
            AgentEvent::Approval { reply, .. } if !current => {
                let _ = reply.send(Answer::Deny(DenyReason::Cancelled));
            }
            _ if !current => {}
            AgentEvent::Approval { spec, reply } => {
                self.request_approval(&mut reg, thread_id, generation, spec, reply, None);
            }
            AgentEvent::Step(step) => self.apply_step(&mut reg, thread_id, step),
        }
    }

    fn apply_step(self: &Arc<Self>, reg: &mut Registry, thread_id: &str, step: Step) {
        if let Step::TurnEnded(end) = step {
            self.finish_turn(reg, thread_id, end);
            return;
        }
        let Some(entry) = reg.threads.get_mut(thread_id) else { return };
        if let Step::Plan(plan) = step {
            let plan = (!plan.is_empty()).then_some(plan);
            if entry.info.plan != plan {
                entry.info.plan = plan;
                emit_state(self, entry);
            }
            return;
        }
        self.apply_item_step(entry, thread_id, step);
        self.refresh_step(entry, thread_id);
    }

    /// Sends `thread.state` when the running step changed: at once when the
    /// last one went out over a second ago, else once that second is up.
    fn refresh_step(self: &Arc<Self>, entry: &mut Entry, thread_id: &str) {
        let step = entry.current_step();
        entry.info.step = step;
        if entry.info.step == entry.step_announced {
            return;
        }
        let wait = entry.step_sent.map_or(Duration::ZERO, |at| STEP_EVERY.saturating_sub(at.elapsed()));
        if wait.is_zero() {
            entry.step_announced = entry.info.step.clone();
            entry.step_sent = Some(Instant::now());
            self.hub.broadcast(&Event::ThreadState(ThreadResult { thread: entry.info.clone() }));
            return;
        }
        if entry.step_timer {
            return;
        }
        entry.step_timer = true;
        let host = Arc::downgrade(self);
        let thread = thread_id.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            let Some(host) = host.upgrade() else { return };
            let mut reg = host.lock_reg();
            if let Some(entry) = reg.threads.get_mut(&thread) {
                entry.step_timer = false;
                host.refresh_step(entry, &thread);
            }
        });
    }

    fn apply_item_step(self: &Arc<Self>, entry: &mut Entry, thread_id: &str, step: Step) {
        match step {
            Step::NativeId(id) => {
                entry.info.native_id = Some(cut(&id, MAX_NATIVE_ID).to_owned());
                self.store.save_meta(&entry.info, entry.next_seq);
            }
            Step::AssistantOpen { key } => {
                if !entry.info.state.is_running() || entry.turn.keys.contains_key(&key) {
                    return;
                }
                let seq = entry.alloc();
                entry.turn.keys.insert(key.clone(), seq);
                let item = Item {
                    seq,
                    id: cut(&key, MAX_ITEM_ID).to_owned(),
                    kind: ItemKind::Assistant,
                    ts_ms: now_ms(),
                    rev: 0,
                    text: Some(String::new()),
                    truncated: false,
                    origin: None,
                    tool: None,
                };
                entry.turn.streams.insert(seq, Stream { item, sent: 0, pending: String::new() });
            }
            Step::AssistantDelta { key, text } => {
                let Some(&seq) = entry.turn.keys.get(&key) else { return };
                let Some(stream) = entry.turn.streams.get_mut(&seq) else { return };
                let cleaned = clean(&text);
                let current = stream.item.text.get_or_insert_with(String::new);
                let room = MAX_ITEM_TEXT.saturating_sub(current.len());
                let part = cut(&cleaned, room);
                if part.len() < cleaned.len() {
                    stream.item.truncated = true;
                }
                current.push_str(part);
                stream.pending.push_str(part);
                if stream.pending.len() >= DELTA_FLUSH_BYTES {
                    flush_stream(self, thread_id, stream);
                } else if !entry.turn.flush_scheduled {
                    entry.turn.flush_scheduled = true;
                    let host = Arc::downgrade(self);
                    let thread = thread_id.to_owned();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(DELTA_FLUSH_MS)).await;
                        if let Some(host) = host.upgrade() {
                            host.flush_deltas(&thread);
                        }
                    });
                }
            }
            Step::AssistantDone { key, text, final_answer } => {
                let seq = match entry.turn.keys.get(&key) {
                    Some(&seq) => seq,
                    None => {
                        let seq = entry.alloc();
                        entry.turn.keys.insert(key.clone(), seq);
                        seq
                    }
                };
                let mut item = match entry.turn.streams.remove(&seq) {
                    Some(mut stream) => {
                        flush_stream(self, thread_id, &mut stream);
                        stream.item
                    }
                    None => Item {
                        seq,
                        id: cut(&key, MAX_ITEM_ID).to_owned(),
                        kind: ItemKind::Assistant,
                        ts_ms: now_ms(),
                        rev: 0,
                        text: None,
                        truncated: false,
                        origin: None,
                        tool: None,
                    },
                };
                let (text, truncated) = clean_cut(&text, MAX_ITEM_TEXT);
                if !text.trim().is_empty() {
                    entry.turn.last_text = Some(text.clone());
                    if final_answer {
                        entry.turn.final_text = Some(text.clone());
                    }
                }
                item.text = Some(text);
                item.truncated = truncated;
                item.ts_ms = now_ms();
                entry.info.updated_ms = now_ms();
                emit_item(self, thread_id, &item);
            }
            Step::ToolStart { key, tool } => {
                if entry.turn.keys.contains_key(&key) {
                    return;
                }
                let seq = entry.alloc();
                entry.turn.keys.insert(key.clone(), seq);
                let item = tool_item(seq, &key, &tool);
                entry.turn.tools.insert(seq, item.clone());
                entry.turn.tool_steps.insert(seq, tool.step());
                emit_item(self, thread_id, &item);
            }
            Step::ToolEnd { key, status, output } => {
                let Some(&seq) = entry.turn.keys.get(&key) else { return };
                let Some(mut item) = entry.turn.tools.remove(&seq) else { return };
                entry.turn.tool_steps.remove(&seq);
                item.rev += 1;
                if let Some(tool) = item.tool.as_mut() {
                    tool.status = status;
                    if let Some(output) = output {
                        let (output, cut) = clean_tail(&output, MAX_TOOL_OUTPUT);
                        tool.output = Some(output);
                        item.truncated |= cut;
                    }
                }
                entry.info.updated_ms = now_ms();
                emit_item(self, thread_id, &item);
            }
            Step::Notice(text) => {
                let seq = entry.alloc();
                let item = notice_item(seq, &clean_cut(&text, MAX_ITEM_TEXT).0);
                emit_item(self, thread_id, &item);
            }
            Step::Plan(_) | Step::TurnEnded(_) => {}
        }
    }

    pub fn flush_deltas(&self, thread_id: &str) {
        let mut reg = self.lock_reg();
        let Some(entry) = reg.threads.get_mut(thread_id) else { return };
        entry.turn.flush_scheduled = false;
        for stream in entry.turn.streams.values_mut() {
            flush_stream(self, thread_id, stream);
        }
    }

    /// Ends the running turn: open text is finished, steps still running
    /// end, pending approvals are cancelled, then `thread.state` and
    /// `thread.done`.
    fn finish_turn(self: &Arc<Self>, reg: &mut Registry, thread_id: &str, end: TurnEnd) {
        let tokens: Vec<String> = reg
            .approvals
            .iter()
            .filter(|(_, p)| p.approval.thread_id == thread_id)
            .map(|(t, _)| t.clone())
            .collect();
        for token in tokens {
            self.resolve(
                reg,
                &token,
                ApprovalOutcome::Cancelled,
                None,
                Answer::Deny(DenyReason::Cancelled),
                false,
            );
        }
        let Some(entry) = reg.threads.get_mut(thread_id) else { return };
        if !entry.info.state.is_running() {
            return;
        }
        let turn = std::mem::take(&mut entry.turn);
        for (_, mut stream) in turn.streams {
            flush_stream(self, thread_id, &mut stream);
            emit_item(self, thread_id, &stream.item);
        }
        let mut tools: Vec<(u64, Item)> = turn.tools.into_iter().collect();
        tools.sort_by_key(|(seq, _)| *seq);
        for (seq, mut item) in tools {
            item.rev += 1;
            if let Some(tool) = item.tool.as_mut() {
                tool.status =
                    if turn.refused_tools.contains(&seq) { ToolStatus::Denied } else { ToolStatus::Error };
            }
            emit_item(self, thread_id, &item);
        }
        let summary = end
            .summary
            .as_deref()
            .or(turn.final_text.as_deref())
            .or(turn.last_text.as_deref())
            .map(summary_of)
            .unwrap_or_default();
        entry.set_state(end.outcome.state());
        entry.info.pending = 0;
        entry.info.updated_ms = now_ms();
        entry.info.error = end.error.as_ref().filter(|_| end.outcome == Outcome::Failed).map(clean_error);
        if end.outcome == Outcome::Done && !summary.is_empty() {
            entry.info.summary = Some(summary.clone());
        }
        emit_state(self, entry);
        self.hub.broadcast(&Event::ThreadDone(ThreadDoneEvent {
            thread_id: thread_id.to_owned(),
            outcome: end.outcome,
            summary: summary.clone(),
            error: entry.info.error.clone(),
        }));
        match end.outcome {
            Outcome::Done => {
                notify_unless_viewed(self, thread_id, Note::Done { thread: entry.info.clone(), summary });
            }
            Outcome::Failed => {
                let message = entry.info.error.as_ref().map(|e| e.message.clone()).unwrap_or_default();
                notify(self, Note::Failed { thread: entry.info.clone(), message });
            }
            Outcome::Interrupted => notify(self, Note::Removed { thread_id: thread_id.to_owned() }),
        }
        let agent = entry.info.agent;
        let login = entry.info.error.as_ref().filter(|e| e.code == code::LOGIN).map(|e| e.message.clone());
        self.schedule_idle_close(thread_id, entry.turn_no);
        if let Some(message) = login {
            let message =
                if message.is_empty() { format!("{} is signed out.", agent.display_name()) } else { message };
            reg.login_blocked.insert(agent, message);
            let host = self.clone();
            tokio::spawn(async move {
                host.agents(false).await;
            });
        }
    }

    fn schedule_idle_close(self: &Arc<Self>, thread_id: &str, turn_no: u64) {
        let host = Arc::downgrade(self);
        let thread = thread_id.to_owned();
        let idle = self.config.agent_idle;
        tokio::spawn(async move {
            tokio::time::sleep(idle).await;
            let Some(host) = host.upgrade() else { return };
            let mut reg = host.lock_reg();
            let Some(entry) = reg.threads.get_mut(&thread) else { return };
            if entry.turn_no != turn_no || entry.info.state.is_running() {
                return;
            }
            if let Some(session) = entry.session.take() {
                let _ = session.cmds.send(SessionCmd::Close);
                if let Some(token) = session.bridge_token {
                    reg.bridges.remove(&token);
                }
            }
        });
    }

    // ---- Approvals ----

    fn request_approval(
        self: &Arc<Self>,
        reg: &mut Registry,
        thread_id: &str,
        generation: u64,
        spec: ApprovalSpec,
        reply: oneshot::Sender<Answer>,
        bridge_tool_use_id: Option<String>,
    ) {
        let lifetime = reg.approval_lifetime_ms;
        let pending_count = reg.pending_for(thread_id);
        let Some(entry) = reg.threads.get_mut(thread_id) else {
            let _ = reply.send(Answer::Deny(DenyReason::Cancelled));
            return;
        };
        let live = entry.info.state.is_running()
            && entry.session.as_ref().is_some_and(|s| s.generation == generation);
        if !live {
            let _ = reply.send(Answer::Deny(DenyReason::Cancelled));
            return;
        }
        // The step comes before its approval.
        if let (Some(key), Some(tool)) = (&spec.tool_key, &spec.tool)
            && !entry.turn.keys.contains_key(key)
        {
            let seq = entry.alloc();
            entry.turn.keys.insert(key.clone(), seq);
            let item = tool_item(seq, key, tool);
            entry.turn.tools.insert(seq, item.clone());
            entry.turn.tool_steps.insert(seq, tool.step());
            emit_item(self, thread_id, &item);
        }
        let tool_seq = spec.tool_key.as_ref().and_then(|k| entry.turn.keys.get(k).copied());
        let allowed = match spec.kind {
            ApprovalKind::Edit => entry.allow_edits,
            ApprovalKind::Command => spec.command.as_ref().is_some_and(|c| entry.allow_commands.contains(c)),
            ApprovalKind::Other => false,
        };
        if allowed {
            let _ = reply.send(Answer::Allow);
            return;
        }
        if pending_count >= MAX_PENDING_PER_THREAD {
            let _ = reply.send(Answer::Deny(DenyReason::TooMany));
            return;
        }
        let mut token = random_hex(16);
        while reg.approvals.contains_key(&token) || reg.answered.contains(&token) {
            token = random_hex(16);
        }
        let Some(entry) = reg.threads.get_mut(thread_id) else { return };
        let mut truncated = false;
        let mut field = |value: &Option<String>, max: usize| {
            value.as_ref().map(|v| {
                let (v, cut) = clean_cut(v, max);
                truncated |= cut;
                v
            })
        };
        let command = field(&spec.command, MAX_APPROVAL_COMMAND);
        let diff = field(&spec.diff, MAX_APPROVAL_DIFF);
        let path = field(&spec.path, MAX_APPROVAL_PATH);
        let reason = field(&spec.reason, MAX_APPROVAL_REASON);
        let now = now_ms();
        let approval = Approval {
            token: token.clone(),
            thread_id: thread_id.to_owned(),
            kind: spec.kind,
            title: clean_cut(&spec.title, MAX_APPROVAL_TITLE).0,
            command,
            diff,
            path,
            reason,
            truncated,
            can_allow_thread: spec.kind != ApprovalKind::Other,
            created_ms: now,
            expires_ms: now + lifetime,
        };
        entry.info.pending = u32::try_from(pending_count + 1).unwrap_or(u32::MAX);
        entry.set_state(State::Waiting);
        entry.info.updated_ms = now;
        emit_state(self, entry);
        self.hub.broadcast(&Event::ApprovalRequested(ApprovalRequestedEvent { approval: approval.clone() }));
        if pending_count == 0 {
            notify_unless_viewed(
                self,
                thread_id,
                Note::Approval { thread: entry.info.clone(), approval: approval.clone() },
            );
        }
        let order = reg.next_order;
        reg.next_order += 1;
        reg.approvals.insert(
            token.clone(),
            PendingApproval {
                approval,
                reply: Some(reply),
                raw_command: spec.command,
                tool_seq,
                bridge_tool_use_id,
                generation,
                order,
            },
        );
        let host = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(lifetime)).await;
            if let Some(host) = host.upgrade() {
                let mut reg = host.lock_reg();
                if reg.approvals.contains_key(&token) {
                    host.resolve(
                        &mut reg,
                        &token,
                        ApprovalOutcome::Expired,
                        None,
                        Answer::Deny(DenyReason::Expired),
                        true,
                    );
                }
            }
        });
    }

    /// Answers one approval: the agent gets `answer`, every client gets
    /// `approval.resolved`. The token is gone before the agent hears back.
    fn resolve(
        &self,
        reg: &mut Registry,
        token: &str,
        outcome: ApprovalOutcome,
        by: Option<Origin>,
        answer: Answer,
        announce_state: bool,
    ) {
        let Some(mut pending) = reg.approvals.remove(token) else { return };
        reg.answered.insert(token.to_owned());
        let thread_id = pending.approval.thread_id.clone();
        if outcome != ApprovalOutcome::Cancelled {
            self.store.append_approval(
                &thread_id,
                ApprovalRecord { tool_seq: pending.tool_seq, outcome, by: by.clone(), ts_ms: now_ms() },
            );
        }
        if let Some(reply) = pending.reply.take() {
            let _ = reply.send(answer);
        }
        let remaining = reg.pending_for(&thread_id);
        let next = reg.pending_list(&thread_id).into_iter().next();
        self.hub.broadcast(&Event::ApprovalResolved(ApprovalResolvedEvent {
            token: token.to_owned(),
            thread_id: thread_id.clone(),
            outcome,
            by,
        }));
        let Some(entry) = reg.threads.get_mut(&thread_id) else { return };
        if outcome != ApprovalOutcome::Allowed
            && let Some(seq) = pending.tool_seq
        {
            entry.turn.refused_tools.insert(seq);
        }
        entry.info.pending = u32::try_from(remaining).unwrap_or(u32::MAX);
        if remaining == 0 && entry.info.state == State::Waiting {
            entry.set_state(State::Working);
        }
        if announce_state {
            entry.info.updated_ms = now_ms();
            emit_state(self, entry);
        }
        match next {
            Some(approval) => {
                notify_unless_viewed(
                    self,
                    &thread_id,
                    Note::Approval { thread: entry.info.clone(), approval },
                );
            }
            None => notify(self, Note::ApprovalGone { thread_id, token: token.to_owned() }),
        }
    }

    pub fn approval_answer(&self, token: &str, decision: Decision, by: Origin) -> Result<(), RpcError> {
        if !by.is_valid() {
            return Err(err(ErrorCode::BadRequest, "Bad origin."));
        }
        let mut reg = self.lock_reg();
        let Some(pending) = reg.approvals.get(token) else {
            return Err(if reg.answered.contains(token) {
                err(ErrorCode::AlreadyAnswered, "Already answered.")
            } else {
                err(ErrorCode::NotFound, "No such request.")
            });
        };
        let thread_id = pending.approval.thread_id.clone();
        let kind = pending.approval.kind;
        let raw_command = pending.raw_command.clone();
        if decision == Decision::AllowThread
            && (!pending.approval.can_allow_thread || by.kind == OriginKind::Phone)
        {
            return Err(err(ErrorCode::Denied, "Only the PC can allow a whole thread."));
        }
        match decision {
            Decision::Allow => {
                self.resolve(&mut reg, token, ApprovalOutcome::Allowed, Some(by), Answer::Allow, true)
            }
            Decision::Deny => self.resolve(
                &mut reg,
                token,
                ApprovalOutcome::Denied,
                Some(by),
                Answer::Deny(DenyReason::User),
                true,
            ),
            Decision::AllowThread => {
                if let Some(entry) = reg.threads.get_mut(&thread_id) {
                    match kind {
                        ApprovalKind::Edit => entry.allow_edits = true,
                        ApprovalKind::Command => {
                            if let Some(command) = &raw_command {
                                entry.allow_commands.insert(command.clone());
                            }
                        }
                        ApprovalKind::Other => {}
                    }
                }
                self.resolve(
                    &mut reg,
                    token,
                    ApprovalOutcome::Allowed,
                    Some(by.clone()),
                    Answer::Allow,
                    true,
                );
                // Requests already waiting that the new rule covers.
                let covered: Vec<String> = reg
                    .approvals
                    .iter()
                    .filter(|(_, p)| {
                        p.approval.thread_id == thread_id
                            && p.approval.kind == kind
                            && (kind == ApprovalKind::Edit || p.raw_command == raw_command)
                    })
                    .map(|(t, _)| t.clone())
                    .collect();
                for t in covered {
                    self.resolve(
                        &mut reg,
                        &t,
                        ApprovalOutcome::Allowed,
                        Some(by.clone()),
                        Answer::Allow,
                        true,
                    );
                }
            }
        }
        Ok(())
    }

    // ---- The bridge ----

    pub fn bridge_lookup(&self, token: &str) -> Option<(String, u64)> {
        self.lock_reg().bridges.get(token).cloned()
    }

    pub async fn bridge_ask(
        self: &Arc<Self>,
        thread_id: &str,
        generation: u64,
        p: BridgeAskParams,
    ) -> BridgeAskResult {
        let (tx, rx) = oneshot::channel();
        {
            let mut reg = self.lock_reg();
            let cwd = reg.threads.get(thread_id).and_then(|e| e.cwd.clone()).unwrap_or_default();
            let tool = claude::tool_spec(&p.tool_name, &p.input, &cwd);
            let key = (!p.tool_use_id.is_empty()).then(|| p.tool_use_id.clone());
            let spec = ApprovalSpec::from_tool(&tool, key.clone());
            self.request_approval(&mut reg, thread_id, generation, spec, tx, key);
        }
        match rx.await {
            Ok(Answer::Allow) => BridgeAskResult { behavior: BridgeBehavior::Allow, message: None },
            Ok(Answer::Deny(reason)) => {
                BridgeAskResult { behavior: BridgeBehavior::Deny, message: Some(reason.message().to_owned()) }
            }
            Err(_) => BridgeAskResult {
                behavior: BridgeBehavior::Deny,
                message: Some(DenyReason::Cancelled.message().to_owned()),
            },
        }
    }

    pub fn bridge_cancel(&self, thread_id: &str, generation: u64, tool_use_id: &str) {
        let mut reg = self.lock_reg();
        let tokens: Vec<String> = reg
            .approvals
            .iter()
            .filter(|(_, p)| {
                p.approval.thread_id == thread_id
                    && p.generation == generation
                    && p.bridge_tool_use_id.as_deref() == Some(tool_use_id)
            })
            .map(|(t, _)| t.clone())
            .collect();
        for token in tokens {
            self.resolve(
                &mut reg,
                &token,
                ApprovalOutcome::Cancelled,
                None,
                Answer::Deny(DenyReason::Cancelled),
                true,
            );
        }
    }

    /// The bridge went away: what it was asking can't be answered any more.
    pub fn bridge_closed(&self, thread_id: &str, generation: u64) {
        let mut reg = self.lock_reg();
        let tokens: Vec<String> = reg
            .approvals
            .iter()
            .filter(|(_, p)| {
                p.approval.thread_id == thread_id
                    && p.generation == generation
                    && p.bridge_tool_use_id.is_some()
            })
            .map(|(t, _)| t.clone())
            .collect();
        for token in tokens {
            self.resolve(
                &mut reg,
                &token,
                ApprovalOutcome::Cancelled,
                None,
                Answer::Deny(DenyReason::Cancelled),
                true,
            );
        }
    }

    // ---- Shutdown ----

    /// Interrupts running threads, then closes every agent's stdin, then
    /// stops what is left (§4.6).
    pub async fn shutdown_threads(self: &Arc<Self>) {
        let running: Vec<String> = {
            let reg = self.lock_reg();
            reg.threads.iter().filter(|(_, e)| e.info.state.is_running()).map(|(id, _)| id.clone()).collect()
        };
        for id in &running {
            let _ = self.thread_interrupt(id);
        }
        let deadline = tokio::time::Instant::now()
            + agents::INTERRUPT_GRACE
            + agents::TERM_GRACE
            + Duration::from_secs(1);
        while self.any_running() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        {
            let mut reg = self.lock_reg();
            let ids: Vec<String> = reg.threads.keys().cloned().collect();
            for id in ids {
                if reg.threads.get(&id).is_some_and(|e| e.info.state.is_running()) {
                    self.finish_turn(&mut reg, &id, TurnEnd::interrupted());
                }
                if let Some(entry) = reg.threads.get(&id)
                    && let Some(session) = &entry.session
                {
                    let _ = session.cmds.send(SessionCmd::Close);
                }
            }
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(9);
        loop {
            let alive = self.lock_reg().threads.values().any(|e| e.session.is_some());
            if !alive || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn flush_stream(host: &Host, thread_id: &str, stream: &mut Stream) {
    while !stream.pending.is_empty() {
        let chunk = cut(&stream.pending, MAX_DELTA_TEXT).to_owned();
        if chunk.is_empty() {
            stream.pending.clear();
            break;
        }
        host.hub.broadcast(&Event::ThreadDelta(ThreadDeltaEvent {
            thread_id: thread_id.to_owned(),
            seq: stream.item.seq,
            item_id: stream.item.id.clone(),
            offset: u32::try_from(stream.sent).unwrap_or(u32::MAX),
            text: chunk.clone(),
        }));
        stream.sent += chunk.len();
        stream.pending.drain(..chunk.len());
    }
}

#[cfg(test)]
impl Host {
    /// A running thread with a session whose commands land in the receiver.
    pub(crate) fn test_thread(&self, agent: AgentId) -> (String, u64, mpsc::UnboundedReceiver<SessionCmd>) {
        let mut reg = self.lock_reg();
        let id = random_hex(16);
        let generation = reg.next_generation;
        reg.next_generation += 1;
        let now = now_ms();
        let info = ThreadInfo {
            id: id.clone(),
            agent,
            folder_id: "f000000000001".into(),
            folder_label: "work".into(),
            title: "t".into(),
            state: State::Working,
            mode: Mode::Supervised,
            origin: Origin::launcher(),
            created_ms: now,
            updated_ms: now,
            last_seq: 0,
            pending: 0,
            summary: None,
            error: None,
            native_id: None,
            step: None,
            plan: None,
            state_since_ms: 0,
            subdir: None,
            continued: false,
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let mut entry = Entry::new(info, 1, Some(PathBuf::from("/tmp")));
        entry.session = Some(Session { generation, cmds: tx, bridge_token: None });
        reg.threads.insert(id.clone(), entry);
        (id, generation, rx)
    }

    pub(crate) fn test_ask(
        self: &Arc<Self>,
        thread_id: &str,
        generation: u64,
        spec: ApprovalSpec,
    ) -> oneshot::Receiver<Answer> {
        let (tx, rx) = oneshot::channel();
        let mut reg = self.lock_reg();
        self.request_approval(&mut reg, thread_id, generation, spec, tx, None);
        rx
    }

    pub(crate) fn test_pending(&self, thread_id: &str) -> Vec<Approval> {
        self.lock_reg().pending_list(thread_id)
    }

    pub(crate) fn test_finish(self: &Arc<Self>, thread_id: &str, end: TurnEnd) {
        let mut reg = self.lock_reg();
        self.finish_turn(&mut reg, thread_id, end);
    }

    pub(crate) fn test_state(&self, thread_id: &str) -> ThreadInfo {
        self.lock_reg().threads[thread_id].info.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::test_host;

    fn command(text: &str) -> ApprovalSpec {
        ApprovalSpec {
            kind: ApprovalKind::Command,
            title: "Run command".into(),
            command: Some(text.into()),
            diff: None,
            path: None,
            reason: Some("Codex says why\u{1b}[0m".into()),
            tool_key: Some(format!("tool-{text}")),
            tool: Some(ToolSpec {
                name: "shell".into(),
                title: "Run command".into(),
                detail: Some(text.into()),
                kind: ApprovalKind::Command,
                command: Some(text.into()),
                diff: None,
                path: None,
                description: None,
            }),
        }
    }

    fn edit(path: &str) -> ApprovalSpec {
        ApprovalSpec {
            kind: ApprovalKind::Edit,
            title: format!("Edit {path}"),
            command: None,
            diff: Some("+a".into()),
            path: Some(path.into()),
            reason: None,
            tool_key: None,
            tool: None,
        }
    }

    #[tokio::test]
    async fn an_approval_is_single_use() {
        let dir = tempfile::tempdir().unwrap();
        let host = test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let (id, generation, _cmds) = host.test_thread(AgentId::Codex);
        let rx = host.test_ask(&id, generation, command("touch a"));
        let pending = host.test_pending(&id);
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].reason.as_deref(),
            Some("Codex says why[0m"),
            "control characters are stripped"
        );
        assert_eq!(host.test_state(&id).state, State::Waiting);
        assert_eq!(host.test_state(&id).pending, 1);
        let token = pending[0].token.clone();
        assert!(is_valid_thread_id(&token));
        host.approval_answer(&token, Decision::Allow, Origin::launcher()).unwrap();
        assert_eq!(rx.await.unwrap(), Answer::Allow);
        assert_eq!(host.test_state(&id).state, State::Working);
        let again = host.approval_answer(&token, Decision::Deny, Origin::launcher()).unwrap_err();
        assert_eq!(again.code, ErrorCode::AlreadyAnswered);
        let unknown = host.approval_answer(&"0".repeat(32), Decision::Deny, Origin::launcher()).unwrap_err();
        assert_eq!(unknown.code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn a_ninth_waiting_request_is_denied() {
        let dir = tempfile::tempdir().unwrap();
        let host = test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let (id, generation, _cmds) = host.test_thread(AgentId::Claude);
        let mut waiting = Vec::new();
        for n in 0..MAX_PENDING_PER_THREAD {
            waiting.push(host.test_ask(&id, generation, command(&format!("cmd {n}"))));
        }
        let ninth = host.test_ask(&id, generation, command("one more"));
        assert_eq!(ninth.await.unwrap(), Answer::Deny(DenyReason::TooMany));
        assert_eq!(host.test_pending(&id).len(), MAX_PENDING_PER_THREAD);
        // The turn ending cancels every one of them.
        host.test_finish(&id, TurnEnd::done(None));
        for rx in waiting {
            assert_eq!(rx.await.unwrap(), Answer::Deny(DenyReason::Cancelled));
        }
        assert!(host.test_pending(&id).is_empty());
    }

    #[tokio::test]
    async fn allow_for_the_thread_is_pc_only_and_covers_later_requests() {
        let dir = tempfile::tempdir().unwrap();
        let host = test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let (id, generation, _cmds) = host.test_thread(AgentId::Codex);
        let first = host.test_ask(&id, generation, command("cargo test"));
        let same = host.test_ask(&id, generation, command("cargo test"));
        let other = host.test_ask(&id, generation, command("rm -rf target"));
        let token = host.test_pending(&id)[0].token.clone();
        let refused =
            host.approval_answer(&token, Decision::AllowThread, Origin::phone("p", "Pixel")).unwrap_err();
        assert_eq!(refused.code, ErrorCode::Denied);
        host.approval_answer(&token, Decision::AllowThread, Origin::launcher()).unwrap();
        assert_eq!(first.await.unwrap(), Answer::Allow);
        assert_eq!(same.await.unwrap(), Answer::Allow, "a request already waiting that the rule covers");
        assert_eq!(host.test_pending(&id).len(), 1, "a different command still asks");
        let later = host.test_ask(&id, generation, command("cargo test"));
        assert_eq!(later.await.unwrap(), Answer::Allow);
        assert_eq!(host.test_pending(&id).len(), 1);
        let other_token = host.test_pending(&id)[0].token.clone();
        host.approval_answer(&other_token, Decision::Deny, Origin::phone("p", "Pixel")).unwrap();
        assert_eq!(other.await.unwrap(), Answer::Deny(DenyReason::User));

        let edit_a = host.test_ask(&id, generation, edit("a.txt"));
        let token = host.test_pending(&id)[0].token.clone();
        host.approval_answer(&token, Decision::AllowThread, Origin::launcher()).unwrap();
        assert_eq!(edit_a.await.unwrap(), Answer::Allow);
        assert_eq!(host.test_ask(&id, generation, edit("b.txt")).await.unwrap(), Answer::Allow);
    }

    #[tokio::test]
    async fn an_unanswered_request_expires_as_denied() {
        let dir = tempfile::tempdir().unwrap();
        let host = test_host(dir.path(), 50);
        let (id, generation, _cmds) = host.test_thread(AgentId::Claude);
        let rx = host.test_ask(&id, generation, command("ls"));
        let token = host.test_pending(&id)[0].token.clone();
        assert_eq!(rx.await.unwrap(), Answer::Deny(DenyReason::Expired));
        assert_eq!(
            host.approval_answer(&token, Decision::Allow, Origin::launcher()).unwrap_err().code,
            ErrorCode::AlreadyAnswered
        );
        assert_eq!(host.test_state(&id).state, State::Working);
    }

    #[tokio::test]
    async fn requests_from_an_old_session_are_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let host = test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let (id, generation, _cmds) = host.test_thread(AgentId::Claude);
        let rx = host.test_ask(&id, generation + 100, command("ls"));
        assert_eq!(rx.await.unwrap(), Answer::Deny(DenyReason::Cancelled));
        assert!(host.test_pending(&id).is_empty());
    }

    /// What was posted (`Approval`/`Done` with the command or summary) or
    /// taken down, in order.
    fn notes(rx: &mut mpsc::UnboundedReceiver<Note>) -> Vec<String> {
        let mut seen = Vec::new();
        while let Ok(note) = rx.try_recv() {
            seen.push(match note {
                Note::Approval { approval, .. } => format!("ask {}", approval.command.unwrap_or_default()),
                Note::ApprovalGone { .. } => "gone".to_owned(),
                Note::Done { summary, .. } => format!("done {summary}"),
                Note::Failed { .. } => "failed".to_owned(),
                Note::PhoneStarted { .. } => "started".to_owned(),
                Note::Removed { .. } => "removed".to_owned(),
            });
        }
        seen
    }

    #[tokio::test]
    async fn a_thread_on_screen_gets_no_needs_you_or_done_notification() {
        let dir = tempfile::tempdir().unwrap();
        let (host, mut rx) = crate::host::test_host_noting(dir.path(), APPROVAL_LIFETIME_MS);
        let (id, generation, _cmds) = host.test_thread(AgentId::Codex);
        let (other, other_generation, _other_cmds) = host.test_thread(AgentId::Codex);
        // G7 Link's page shows the thread while its window is active.
        let page = host.hub.test_viewer(ClientKind::Link, Some(&id));
        let _first = host.test_ask(&id, generation, command("pnpm exec tsc --noEmit"));
        let _second = host.test_ask(&id, generation, command("rg --files"));
        let _elsewhere = host.test_ask(&other, other_generation, command("ls"));
        assert_eq!(notes(&mut rx), ["removed", "ask ls"], "only the thread not on screen asks");
        assert_eq!(host.test_state(&id).state, State::Waiting, "the list and badge still change");
        let token = host.test_pending(&id)[0].token.clone();
        host.approval_answer(&token, Decision::Allow, Origin::launcher()).unwrap();
        assert_eq!(notes(&mut rx), ["removed"], "the next request isn't posted either");
        host.test_finish(&id, TurnEnd::done(Some("Checked.".into())));
        assert_eq!(notes(&mut rx), ["gone", "removed"], "the turn ended with one still waiting");

        // The window lost focus: the next turn's request and end are posted.
        host.hub.test_view(page, None);
        let (id, generation, _cmds) = host.test_thread(AgentId::Codex);
        let _ask = host.test_ask(&id, generation, command("rg --files"));
        assert_eq!(notes(&mut rx), ["ask rg --files"]);
        let token = host.test_pending(&id)[0].token.clone();
        host.approval_answer(&token, Decision::Deny, Origin::launcher()).unwrap();
        host.test_finish(&id, TurnEnd::done(Some("Stopped there.".into())));
        assert_eq!(notes(&mut rx), ["gone", "done Stopped there."]);
    }

    #[tokio::test]
    async fn a_test_or_cli_connection_viewing_doesnt_count() {
        let dir = tempfile::tempdir().unwrap();
        let (host, mut rx) = crate::host::test_host_noting(dir.path(), APPROVAL_LIFETIME_MS);
        let (id, generation, _cmds) = host.test_thread(AgentId::Claude);
        host.hub.test_viewer(ClientKind::Cli, Some(&id));
        let _ask = host.test_ask(&id, generation, command("ls"));
        assert_eq!(notes(&mut rx), ["ask ls"]);
        host.hub.test_viewer(ClientKind::Launcher, Some(&id));
        host.test_finish(&id, TurnEnd::done(Some("Listed.".into())));
        assert_eq!(notes(&mut rx), ["gone", "removed"], "the launcher shows it");
    }

    #[test]
    fn titles_and_summaries() {
        assert_eq!(title_of("\n  Fix the build\nthen test"), "Fix the build");
        assert_eq!(title_of(&"é".repeat(50)).len(), 80);
        assert_eq!(summary_of("Done.\n\n  All\tgood \u{202e}"), "Done. All good");
        assert_eq!(random_hex(16).len(), 32);
        assert!(is_valid_folder_id(&format!("f{}", random_hex(6))));
    }
}
