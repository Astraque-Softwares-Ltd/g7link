// SPDX-License-Identifier: MPL-2.0
//! The agent host: accepts clients on the private socket (§2), checks each
//! peer is this user, dispatches requests, and fans events out to
//! subscribers.

pub mod agents;
pub mod attachments;
pub mod awake;
pub mod bridge;
pub mod catalog;
pub mod claude;
pub mod codex;
pub mod commands;
pub mod db;
pub mod dispatch;
pub mod drafts;
pub mod env;
pub mod export;
pub mod files;
pub mod fork;
pub mod git;
pub mod marks;
pub mod notify;
pub mod phone;
pub mod plan;
pub mod ports;
pub mod presence;
pub mod questions;
pub mod scm;
pub mod scripts;
pub mod sessions;
pub mod source;
pub mod store;
pub mod threads;
pub mod undo;
pub mod usage;

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, Semaphore, mpsc};

use crate::api::*;
use crate::client;
use crate::host::agents::{AgentEvent, EventTx, Probe};
use crate::host::notify::{NoteAction, Notifier};
use crate::host::store::{Store, StoreHandle};
use crate::host::threads::Registry;
use crate::lines::{self, LineRead};

/// No client and nothing running for this long: the host exits.
pub const IDLE_EXIT: Duration = Duration::from_secs(15 * 60);
/// Requests one connection may have in flight.
const MAX_IN_FLIGHT: usize = 32;
/// Remembered answered tokens, for `ALREADY_ANSWERED`.
const ANSWERED_KEPT: usize = 4096;

/// Where the host keeps things and which binaries it runs.
#[derive(Debug, Clone)]
pub struct Config {
    pub socket: PathBuf,
    /// The socket's directory: the lock, and the MCP configs.
    pub runtime_dir: PathBuf,
    pub data_dir: PathBuf,
    pub claude_bin: Option<PathBuf>,
    pub codex_bin: Option<PathBuf>,
    /// This binary, which Claude starts as `mcp-approve`.
    pub bridge_exe: PathBuf,
    pub notify: bool,
    /// The environment children are built from.
    pub env: Vec<(OsString, OsString)>,
    pub idle_exit: Duration,
    pub agent_idle: Duration,
    pub approval_lifetime_ms: u64,
    /// `$CLAUDE_CONFIG_DIR`, else `~/.claude`: read for sessions started elsewhere.
    pub claude_home: PathBuf,
    /// `$CODEX_HOME`, else `~/.codex`.
    pub codex_home: PathBuf,
    /// `/proc`, or a fake tree in tests.
    pub proc_root: PathBuf,
    /// Follow sessions started outside G7 (off with `G7_AGENTS_NO_SESSIONS=1`).
    pub sessions: bool,
    /// Take logind's inhibitor while an agent works (off with
    /// `G7_AGENTS_NO_KEEP_AWAKE=1`, and in tests).
    pub keep_awake_bus: bool,
}

impl Config {
    /// Where the session reader looks.
    pub fn session_roots(&self) -> sessions::paths::Roots {
        let config = env::lookup(&self.env, "XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| env::lookup(&self.env, "HOME").map(|h| PathBuf::from(h).join(".config")));
        sessions::paths::Roots {
            claude_home: self.claude_home.clone(),
            codex_home: self.codex_home.clone(),
            proc_root: self.proc_root.clone(),
            claude_app_dir: config.map(|c| c.join("Claude")),
        }
    }
}

impl Config {
    /// From the process environment (§1.3, §2.1, §3).
    pub fn from_env() -> io::Result<Config> {
        let socket = client::socket_path();
        let runtime_dir =
            socket.parent().map(PathBuf::from).ok_or_else(|| io::Error::other("bad socket path"))?;
        let abs = |name: &str| std::env::var_os(name).map(PathBuf::from).filter(|p| p.is_absolute());
        // G7_AGENTS_DATA_DIR keeps a test host's threads apart without
        // changing the XDG_DATA_HOME the agents themselves see.
        let data_dir = match abs("G7_AGENTS_DATA_DIR") {
            Some(dir) => dir,
            None => dirs::data_dir().ok_or_else(|| io::Error::other("no data directory"))?.join("g7-agents"),
        };
        let env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        let roots = sessions::paths::Roots::from_env(&env);
        let sessions = roots.is_some() && std::env::var("G7_AGENTS_NO_SESSIONS").map_or(true, |v| v != "1");
        let roots = roots.unwrap_or(sessions::paths::Roots {
            claude_home: PathBuf::from("/nonexistent/.claude"),
            codex_home: PathBuf::from("/nonexistent/.codex"),
            proc_root: PathBuf::from("/proc"),
            claude_app_dir: None,
        });
        Ok(Config {
            socket,
            runtime_dir,
            data_dir,
            claude_bin: abs("G7_AGENTS_CLAUDE_BIN"),
            codex_bin: abs("G7_AGENTS_CODEX_BIN"),
            bridge_exe: std::env::current_exe()?,
            notify: std::env::var("G7_AGENTS_NO_NOTIFY").map_or(true, |v| v != "1"),
            env,
            idle_exit: IDLE_EXIT,
            agent_idle: Duration::from_millis(AGENT_IDLE_MS),
            approval_lifetime_ms: APPROVAL_LIFETIME_MS,
            claude_home: roots.claude_home,
            codex_home: roots.codex_home,
            proc_root: roots.proc_root,
            sessions,
            keep_awake_bus: std::env::var("G7_AGENTS_NO_KEEP_AWAKE").map_or(true, |v| v != "1"),
        })
    }
}

/// How `serve` ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Served {
    /// Another host holds the lock.
    AlreadyRunning,
    /// Stopped by the caller or a signal.
    Stopped,
    /// Nothing to do for [`Config::idle_exit`].
    Idle,
}

// ---- Outbound queues ----

struct OutLine {
    text: String,
    delta: bool,
}

/// One client's outbound queue: at most [`CLIENT_QUEUE`] lines; when full,
/// deltas go first, and a client still full is closed.
struct Outbox {
    queue: Mutex<VecDeque<OutLine>>,
    /// Wakes the writer.
    notify: Notify,
    closed: std::sync::atomic::AtomicBool,
    /// Tells the reader the connection is closing.
    closing: tokio::sync::watch::Sender<bool>,
}

impl Outbox {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
            closed: Default::default(),
            closing: tokio::sync::watch::Sender::new(false),
        }
    }

    fn push(&self, text: String, delta: bool) {
        if self.is_closed() {
            return;
        }
        {
            let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            if queue.len() >= CLIENT_QUEUE {
                if delta {
                    return;
                }
                match queue.iter().position(|l| l.delta) {
                    Some(pos) => {
                        queue.remove(pos);
                    }
                    None => {
                        drop(queue);
                        self.close();
                        return;
                    }
                }
            }
            queue.push_back(OutLine { text, delta });
        }
        self.notify.notify_one();
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.closing.send_replace(true);
        self.notify.notify_one();
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn pop(&self) -> Option<OutLine> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front()
    }
}

async fn write_loop(outbox: Arc<Outbox>, mut writer: tokio::net::unix::OwnedWriteHalf) {
    loop {
        while let Some(line) = outbox.pop() {
            let mut bytes = line.text.into_bytes();
            bytes.push(b'\n');
            if writer.write_all(&bytes).await.is_err() {
                outbox.close();
                return;
            }
        }
        if outbox.is_closed() {
            let _ = writer.shutdown().await;
            return;
        }
        outbox.notify.notified().await;
    }
}

// ---- Subscribers ----

#[derive(Debug, Clone)]
struct Subscription {
    threads: ThreadSelection,
    deltas: bool,
    sessions: bool,
}

/// What a client said it understands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Caps {
    /// Thinking items as they are.
    pub items2: bool,
    /// Plan and question items, and mode `plan`, as they are.
    pub items3: bool,
}

impl Caps {
    fn of(hello: &HelloParams) -> Caps {
        Caps { items2: hello.has(cap::ITEMS2), items3: hello.has(cap::ITEMS3) }
    }
}

struct ClientSlot {
    kind: ClientKind,
    /// It said `items2`: it gets thinking items as they are.
    items2: bool,
    /// It said `items3`: it gets plan and question items as they are.
    items3: bool,
    outbox: Arc<Outbox>,
    subscription: Option<Subscription>,
    viewing: Option<String>,
    viewing_session: Option<String>,
    /// When `viewing` and `viewing_session` last changed ([`Hub::next_seq`]).
    viewing_since: u64,
    session_since: u64,
    /// It said `presence`: it gets the `ui.presence` event (a launcher or G7 Link).
    presence: bool,
    /// G7 Link that said `handoff`: it gets the `ui.handoff` event.
    handoff: bool,
    /// G7 Link that said `photo`: it may get the `ui.photo` event.
    photo: bool,
    /// G7 Link that said `beside`: it gets the `ui.beside` event.
    beside: bool,
    /// It said `scripts`: it gets the `script.progress` event.
    scripts: bool,
    /// The one session whose items this connection gets.
    watching: Option<String>,
    /// It called `sessions.scan`: it gets `sessions.scanned`.
    scans: bool,
}

impl ClientSlot {
    fn new(kind: ClientKind, caps: Caps, outbox: Arc<Outbox>) -> Self {
        Self {
            kind,
            items2: caps.items2,
            items3: caps.items3,
            outbox,
            subscription: None,
            viewing: None,
            viewing_session: None,
            viewing_since: 0,
            session_since: 0,
            presence: false,
            handoff: false,
            photo: false,
            beside: false,
            scripts: false,
            watching: None,
            scans: false,
        }
    }

    /// A launcher or G7 Link: a client that shows threads to a person.
    fn shows_threads(&self) -> bool {
        matches!(self.kind, ClientKind::Launcher | ClientKind::Link)
    }
}

/// Connected clients and what each wants to hear, and what G7 Link said of
/// presence. The presence lock is never taken while the clients' is held.
#[derive(Default)]
pub struct Hub {
    clients: Mutex<HashMap<u64, ClientSlot>>,
    presence: Mutex<presence::Presence>,
    /// Orders what connections view, newest first.
    seq: AtomicU64,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub").field("clients", &self.lock().len()).finish()
    }
}

impl Hub {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, ClientSlot>> {
        self.clients.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sends `event` to every subscribed client it concerns. Session events
    /// go only to clients that asked for them: `session.state` and
    /// `session.removed` to subscriptions with `sessions`, `session.item` to
    /// the connections watching that session. A thinking item reaches a
    /// client without `items2` as [`Item::legacy`].
    pub fn broadcast(&self, event: &Event) {
        self.send(event, false);
    }

    /// [`Hub::broadcast`] to the clients that said `items2` only: a delta of
    /// thinking, which the others never see.
    pub fn broadcast_items2(&self, event: &Event) {
        self.send(event, true);
    }

    fn send(&self, event: &Event, items2_only: bool) {
        let line = event.to_line();
        if matches!(event, Event::SessionsScanned(_)) {
            for slot in self.lock().values().filter(|s| s.scans) {
                slot.outbox.push(line.clone(), false);
            }
            return;
        }
        if let Some(session) = event.session_id() {
            let item = matches!(event, Event::SessionItem(_));
            for slot in self.lock().values() {
                let wants = if item {
                    slot.watching.as_deref() == Some(session)
                } else {
                    slot.subscription.as_ref().is_some_and(|s| s.sessions)
                };
                if wants {
                    slot.outbox.push(line.clone(), false);
                }
            }
            return;
        }
        if matches!(event, Event::ThreadItem(e) if e.item.kind == ItemKind::Unknown) {
            return;
        }
        // The forms clients that didn't say `items2` or `items3` get, made
        // once each, when they differ.
        let mut forms: HashMap<Caps, Option<String>> = HashMap::new();
        let delta = matches!(event, Event::ThreadDelta(_));
        let script = matches!(event, Event::ScriptProgress(_));
        let thread = event.thread_id();
        for slot in self.lock().values() {
            let Some(sub) = &slot.subscription else { continue };
            if (delta && !sub.deltas) || (script && !slot.scripts) {
                continue;
            }
            if items2_only && !slot.items2 {
                continue;
            }
            if let Some(thread) = thread
                && !sub.threads.contains(thread)
            {
                continue;
            }
            let caps = Caps { items2: slot.items2, items3: slot.items3 };
            let form = forms.entry(caps).or_insert_with(|| event_for(event, caps).map(|e| e.to_line()));
            slot.outbox.push(form.as_deref().unwrap_or(&line).to_owned(), delta);
        }
    }

    /// Sends `event` to every subscribed connection but `except`.
    pub fn broadcast_except(&self, event: &Event, except: u64) {
        let line = event.to_line();
        for (id, slot) in self.lock().iter() {
            if *id != except && slot.subscription.is_some() {
                slot.outbox.push(line.clone(), false);
            }
        }
    }

    /// Sends `ui.open` to every launcher and G7 Link; how many got it.
    pub fn ui_open(&self, thread_id: &str) -> usize {
        let line = Event::UiOpen(ThreadIdParams { thread_id: thread_id.to_owned() }).to_line();
        let mut sent = 0;
        for slot in self.lock().values().filter(|s| s.shows_threads()) {
            slot.outbox.push(line.clone(), false);
            sent += 1;
        }
        sent
    }

    /// Sends `ui.open` to G7 Link's connections but `except`; how many got it.
    pub fn ui_open_in_link(&self, thread_id: &str, except: u64) -> usize {
        let line = Event::UiOpen(ThreadIdParams { thread_id: thread_id.to_owned() }).to_line();
        let mut sent = 0;
        for (_, slot) in self.lock().iter().filter(|(id, s)| **id != except && s.kind == ClientKind::Link) {
            slot.outbox.push(line.clone(), false);
            sent += 1;
        }
        sent
    }

    /// Sends `ui.handoff` to G7 Link's connections that said `handoff`; how many got it.
    pub(crate) fn send_handoff(&self, event: &HandoffEvent) -> usize {
        let line = Event::UiHandoff(event.clone()).to_line();
        let mut sent = 0;
        for slot in self.lock().values().filter(|s| s.handoff) {
            slot.outbox.push(line.clone(), false);
            sent += 1;
        }
        sent
    }

    /// Sends `ui.beside` to G7 Link's connections that said `beside`; how many got it.
    pub(crate) fn send_beside(&self, event: &BesideEvent) -> usize {
        let line = Event::UiBeside(event.clone()).to_line();
        let mut sent = 0;
        for slot in self.lock().values().filter(|s| s.beside) {
            slot.outbox.push(line.clone(), false);
            sent += 1;
        }
        sent
    }

    /// The G7 Link connection a photo request goes to: the newest that said `photo`.
    pub(crate) fn photo_link(&self) -> Option<u64> {
        self.lock().iter().filter(|(_, s)| s.photo).map(|(id, _)| *id).max()
    }

    /// Sends `event` to connection `client` alone; whether it is still connected.
    pub(crate) fn send_to(&self, client: u64, event: &Event) -> bool {
        let clients = self.lock();
        let Some(slot) = clients.get(&client) else { return false };
        slot.outbox.push(event.to_line(), false);
        true
    }

    /// A launcher or G7 Link says its window shows this thread to the user
    /// right now (`ui.viewing`), or a phone does (a fresh `ui.presence`).
    pub fn viewed(&self, thread_id: &str) -> bool {
        self.viewed_at(thread_id, Instant::now())
    }

    pub(crate) fn viewed_at(&self, thread_id: &str, now: Instant) -> bool {
        let on_pc =
            self.lock().values().any(|s| s.shows_threads() && s.viewing.as_deref() == Some(thread_id));
        on_pc || self.lock_presence().views(thread_id, now)
    }

    pub(crate) fn lock_presence(&self) -> std::sync::MutexGuard<'_, presence::Presence> {
        self.presence.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// The `ui.presence` event as things are at `now`.
    pub(crate) fn presence_event(&self, now: Instant) -> PresenceEvent {
        let (viewing, sessions) = {
            let clients = self.lock();
            let shown = || clients.values().filter(|s| s.shows_threads());
            let threads = shown().filter_map(|s| Some((s.viewing_since, s.viewing.clone()?)));
            let sessions = shown().filter_map(|s| Some((s.session_since, s.viewing_session.clone()?)));
            (
                newest(threads.filter(|(_, id)| is_valid_thread_id(id))),
                newest(sessions.filter(|(_, id)| is_valid_session_id(id))),
            )
        };
        self.lock_presence().event(viewing, sessions, now)
    }

    /// Sends the `ui.presence` event to the launchers and G7 Links that said
    /// `presence`, or to connection `only`.
    pub(crate) fn send_presence(&self, event: &PresenceEvent, only: Option<u64>) {
        let line = Event::UiPresence(event.clone()).to_line();
        for (id, slot) in self.lock().iter() {
            if slot.presence && slot.shows_threads() && only.is_none_or(|o| o == *id) {
                slot.outbox.push(line.clone(), false);
            }
        }
    }

    /// The threads a launcher or G7 Link shows now, or a phone does.
    pub(crate) fn viewed_threads(&self, now: Instant) -> std::collections::HashSet<String> {
        let mut viewed: std::collections::HashSet<String> =
            self.lock().values().filter(|s| s.shows_threads()).filter_map(|s| s.viewing.clone()).collect();
        if let Some(presence) = self.lock_presence().fresh(now) {
            viewed.extend(presence.remote.iter().filter_map(|r| r.thread_id.clone()));
        }
        viewed.retain(|id| is_valid_thread_id(id));
        viewed
    }

    /// `client` is a launcher or G7 Link: what it views counts as read.
    fn viewed_by(&self, client: u64) -> bool {
        self.lock().get(&client).is_some_and(ClientSlot::shows_threads)
    }

    /// A connection of `kind` that views `thread_id`; its id.
    #[cfg(test)]
    pub(crate) fn test_viewer(&self, kind: ClientKind, thread_id: Option<&str>) -> u64 {
        let mut clients = self.lock();
        let id = clients.keys().max().map_or(1, |n| n + 1);
        let mut slot = ClientSlot::new(kind, Caps::default(), Arc::new(Outbox::new()));
        slot.viewing = thread_id.map(str::to_owned);
        clients.insert(id, slot);
        id
    }

    /// A connection subscribed to every thread with `caps`; its id.
    #[cfg(test)]
    pub(crate) fn test_subscriber(&self, caps: Caps) -> u64 {
        let mut clients = self.lock();
        let id = clients.keys().max().map_or(1, |n| n + 1);
        let mut slot = ClientSlot::new(ClientKind::Test, caps, Arc::new(Outbox::new()));
        slot.subscription =
            Some(Subscription { threads: ThreadSelection::All, deltas: true, sessions: false });
        clients.insert(id, slot);
        id
    }

    /// A G7 Link connection that said `handoff` and/or `photo`; its id.
    #[cfg(test)]
    pub(crate) fn test_link(&self, handoff: bool, photo: bool) -> u64 {
        let mut clients = self.lock();
        let id = clients.keys().max().map_or(1, |n| n + 1);
        let mut slot = ClientSlot::new(ClientKind::Link, Caps::default(), Arc::new(Outbox::new()));
        slot.handoff = handoff;
        slot.photo = photo;
        clients.insert(id, slot);
        id
    }

    /// A test connection goes.
    #[cfg(test)]
    pub(crate) fn test_close(&self, client: u64) {
        self.lock().remove(&client);
    }

    /// The lines a test connection was sent, oldest first.
    #[cfg(test)]
    pub(crate) fn test_lines(&self, client: u64) -> Vec<String> {
        let clients = self.lock();
        let Some(slot) = clients.get(&client) else { return Vec::new() };
        std::iter::from_fn(|| slot.outbox.pop()).map(|l| l.text).collect()
    }

    #[cfg(test)]
    pub(crate) fn test_view(&self, client: u64, thread_id: Option<&str>) {
        self.set(client, |slot| slot.viewing = thread_id.map(str::to_owned));
    }

    /// Some connection subscribed with `sessions` or watches a session.
    pub fn wants_sessions(&self) -> bool {
        self.lock()
            .values()
            .any(|s| s.watching.is_some() || s.subscription.as_ref().is_some_and(|sub| sub.sessions))
    }

    /// The sessions some connection watches.
    pub fn watched_sessions(&self) -> std::collections::HashSet<String> {
        self.lock().values().filter_map(|s| s.watching.clone()).collect()
    }

    /// No connection watches a session any more.
    fn clear_watches(&self) {
        for slot in self.lock().values_mut() {
            slot.watching = None;
        }
    }

    /// Sets what `client` watches, unless that would make more than
    /// [`MAX_WATCHED_SESSIONS`] sessions watched.
    fn set_watch(&self, client: u64, session: Option<String>) -> bool {
        let mut clients = self.lock();
        if let Some(wanted) = &session {
            let others: std::collections::HashSet<&String> = clients
                .iter()
                .filter(|(id, _)| **id != client)
                .filter_map(|(_, s)| s.watching.as_ref())
                .collect();
            if !others.contains(wanted) && others.len() >= MAX_WATCHED_SESSIONS {
                return false;
            }
        }
        if let Some(slot) = clients.get_mut(&client) {
            slot.watching = session;
        }
        true
    }

    fn set(&self, client: u64, f: impl FnOnce(&mut ClientSlot)) {
        if let Some(slot) = self.lock().get_mut(&client) {
            f(slot);
        }
    }
}

/// Ids by when they were last chosen, newest first, each once, at most
/// [`MAX_PRESENCE_VIEWING`].
fn newest(views: impl Iterator<Item = (u64, String)>) -> Vec<String> {
    let mut views: Vec<(u64, String)> = views.collect();
    views.sort_by_key(|(since, _)| std::cmp::Reverse(*since));
    let mut out: Vec<String> = Vec::with_capacity(MAX_PRESENCE_VIEWING);
    for (_, id) in views {
        if !out.contains(&id) {
            out.push(id);
            if out.len() == MAX_PRESENCE_VIEWING {
                break;
            }
        }
    }
    out
}

/// `event` as a client with `caps` gets it, when that differs from the
/// event itself; `None` when it doesn't.
fn event_for(event: &Event, caps: Caps) -> Option<Event> {
    match event {
        Event::ThreadItem(e) => {
            let item = e.item.for_caps(caps.items2, caps.items3)?;
            (item != e.item)
                .then(|| Event::ThreadItem(ThreadItemEvent { thread_id: e.thread_id.clone(), item }))
        }
        Event::ThreadState(e) if !caps.items3 && e.thread.mode == Mode::Plan => {
            Some(Event::ThreadState(ThreadResult { thread: e.thread.legacy() }))
        }
        _ => None,
    }
}

// ---- The host ----

/// Probe results, refreshed at most every [`PROBE_CACHE_MS`].
#[derive(Debug, Default)]
pub struct ProbeCache {
    pub at: Option<Instant>,
    pub claude: Option<Probe>,
    pub codex: Option<Probe>,
    pub shown: Option<Vec<AgentInfo>>,
    /// Claude's models and commands, and the CLI version they came from.
    pub claude_catalog: Option<(Option<String>, catalog::ClaudeCatalog)>,
}

pub struct Host {
    pub config: Config,
    pub hub: Hub,
    pub store: StoreHandle,
    pub reg: Mutex<Registry>,
    pub probes: tokio::sync::Mutex<ProbeCache>,
    /// Serializes folder changes so each one is saved before the next.
    pub folders_lock: tokio::sync::Mutex<()>,
    pub events: EventTx,
    pub notifier: Option<Notifier>,
    pub hostname: String,
    pub sessions: sessions::Sessions,
    /// `settings.keep_awake`.
    keep_awake: std::sync::atomic::AtomicBool,
    /// `settings.limit_alarm`.
    limit_alarm: std::sync::atomic::AtomicBool,
    /// The phone photo on its way (`ui.photo`).
    pub(crate) photos: Mutex<phone::Photos>,
    /// The threads whose ports are looked at (§V9.2).
    pub(crate) ports: Mutex<ports::Watch>,
    pub awake: awake::Awake,
    pub usage: Mutex<usage::Usage>,
    pub drafts: Mutex<drafts::Drafts>,
    /// Bumped on each draft change; a save waits for it to settle.
    pub drafts_generation: AtomicU64,
    pub history: Mutex<drafts::History>,
    pub files: files::FileIndex,
    pub commands: Mutex<commands::Commands>,
    /// `None` when the image store couldn't be opened.
    pub images: Option<attachments::ImageStore>,
    /// Each connection's image upload (one at a time).
    pub uploads: Mutex<HashMap<u64, attachments::Upload>>,
    /// Rewind plans handed out and not yet applied.
    pub rewinds: Mutex<undo::RewindTokens>,
    /// Each connection's running search, stopped when it asks again.
    pub searches: Mutex<HashMap<u64, db::Interrupt>>,
    /// The user's `git`, for restore points; `None` when there is none.
    pub git: Option<Arc<git::Git>>,
    /// Source control (phase 4), with the same `git`; `None` without one.
    pub scm: Option<Arc<scm::Scm>>,
    pub source: source::Source,
    /// Project scripts' runs (phase 6).
    pub scripts: scripts::Scripts,
    clients: AtomicUsize,
    next_client: AtomicU64,
    idle_since: Mutex<Instant>,
    shutdown: Notify,
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host").field("socket", &self.config.socket).finish_non_exhaustive()
    }
}

impl Host {
    pub(crate) fn lock_reg(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.reg.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn lock_commands(&self) -> std::sync::MutexGuard<'_, commands::Commands> {
        self.commands.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Marks activity for the idle exit.
    pub fn touch(&self) {
        *self.idle_since.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    /// Asks `serve` to stop.
    pub fn stop(&self) {
        self.shutdown.notify_one();
    }

    /// The keep-awake setting.
    pub fn keep_awake_on(&self) -> bool {
        self.keep_awake.load(Ordering::SeqCst)
    }

    /// The limit alarm setting.
    pub fn limit_alarm_on(&self) -> bool {
        self.limit_alarm.load(Ordering::SeqCst)
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "This PC".to_owned())
}

/// Runs the host until `stop` resolves, a signal, or the idle timeout.
pub async fn serve(config: Config, stop: impl std::future::Future<Output = ()>) -> io::Result<Served> {
    client::ensure_private_dir(&config.runtime_dir).map_err(|e| io::Error::other(e.to_string()))?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(config.runtime_dir.join("host.lock"))?;
    // A host that is stopping still holds the lock for a moment.
    let mut tries = 0;
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if tries < 10 => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(std::fs::TryLockError::WouldBlock) => return Ok(Served::AlreadyRunning),
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
    }
    // Cut copies a previous host left (§U6.2 step 4); only once the lock is
    // ours, so a second host never removes a running one's.
    fork::sweep(&config.runtime_dir);
    let _ = std::fs::remove_file(&config.socket);
    let listener = UnixListener::bind(&config.socket)?;
    std::fs::set_permissions(&config.socket, std::fs::Permissions::from_mode(0o600))?;

    // The thread database first: it brings in a file store left by an earlier release.
    let mut db = match db::Db::open(&config.data_dir) {
        Ok(db) => Some(db),
        Err(e) => {
            tracing::warn!("couldn't open the thread store: {e}");
            None
        }
    };
    let store = Store::open(config.data_dir.clone())?;
    let registry = Registry::load(&store, db.as_mut(), config.approval_lifetime_ms);
    let settings = store.load_settings();
    let drafts = drafts::Drafts::load(store.load_drafts());
    let history = drafts::History::load(store.load_history());
    let store = StoreHandle::spawn(store, db)?;
    let images = open_images(&config);
    let awake_source = if config.keep_awake_bus { awake::Source::System } else { awake::Source::Off };
    let (events_tx, mut events_rx) = mpsc::unbounded_channel::<(String, u64, AgentEvent)>();
    let (actions_tx, mut actions_rx) = mpsc::unbounded_channel::<NoteAction>();
    let notifier = config.notify.then(|| Notifier::spawn(actions_tx));
    let git = git::Git::new(&config.env);
    let host = Arc::new(Host {
        hub: Hub::default(),
        store,
        reg: Mutex::new(registry),
        probes: tokio::sync::Mutex::new(ProbeCache::default()),
        folders_lock: tokio::sync::Mutex::new(()),
        events: events_tx,
        notifier,
        hostname: hostname(),
        sessions: sessions::Sessions::new(&config),
        keep_awake: std::sync::atomic::AtomicBool::new(settings.keep_awake),
        limit_alarm: std::sync::atomic::AtomicBool::new(settings.limit_alarm),
        photos: Mutex::new(phone::Photos::default()),
        ports: Mutex::new(ports::Watch::default()),
        awake: awake::Awake::spawn(awake_source, settings.keep_awake, awake::release_delay()),
        usage: Mutex::new(usage::Usage::new(Duration::from_millis(LIMITS_EVENT_MS))),
        drafts: Mutex::new(drafts),
        drafts_generation: AtomicU64::new(0),
        history: Mutex::new(history),
        files: files::FileIndex::default(),
        commands: Mutex::new(commands::Commands::default()),
        images,
        uploads: Mutex::new(HashMap::new()),
        rewinds: Mutex::new(undo::RewindTokens::default()),
        searches: Mutex::new(HashMap::new()),
        scm: git.clone().map(|git| Arc::new(scm::Scm::new(git, &config.env))),
        git,
        source: source::Source::load(&config.data_dir),
        scripts: scripts::Scripts::default(),
        clients: AtomicUsize::new(0),
        next_client: AtomicU64::new(1),
        idle_since: Mutex::new(Instant::now()),
        shutdown: Notify::new(),
        config,
    });
    host.attach_worktrees();
    host.sessions.set_following_loaded(settings.follow_sessions);
    if host.store.has_db() {
        host.sessions.attach_store(Arc::new(FoundStoreOf { host: Arc::downgrade(&host) }));
    }
    // Turns cut short by the last stop, and refs of threads that are gone.
    let recovered = std::mem::take(&mut host.lock_reg().recovered);
    host.recover_turns(recovered);
    host.sweep_orphan_refs();
    host.arm_limit_alarms();
    tracing::info!(pid = std::process::id(), "agent host serving");

    // Agent events, in order, one at a time.
    let pump = {
        let host = host.clone();
        tokio::spawn(async move {
            while let Some((thread_id, generation, event)) = events_rx.recv().await {
                host.on_agent_event(&thread_id, generation, event);
            }
        })
    };
    let actions = {
        let host = Arc::downgrade(&host);
        tokio::spawn(async move {
            while let Some(action) = actions_rx.recv().await {
                let Some(host) = host.upgrade() else { break };
                host.on_note_action(action);
            }
        })
    };
    // The folders of the threads a client shows (§SC3.2).
    let git_poll = {
        let host = Arc::downgrade(&host);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(source::POLL).await;
                let Some(host) = host.upgrade() else { break };
                host.git_poll().await;
            }
        })
    };
    // The ports each running agent listens on (§V9.2).
    let ports_poll = {
        let host = Arc::downgrade(&host);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(ports::FAST_EVERY).await;
                let Some(host) = host.upgrade() else { break };
                host.ports_tick().await;
            }
        })
    };
    // Probe in the background so the first agents.list is quick.
    {
        let host = host.clone();
        tokio::spawn(async move {
            host.agents(false).await;
        });
    }

    let accept = {
        let host = host.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { continue };
                let host = host.clone();
                tokio::spawn(async move { host.connection(stream).await });
            }
        })
    };

    let outcome = {
        let idle = async {
            let mut ticks: u64 = 0;
            loop {
                tokio::time::sleep(Duration::from_secs(5).min(host.config.idle_exit)).await;
                host.check_folders();
                host.wake_snoozed();
                host.expire_presence();
                host.ring_limit_alarms(store::now_ms());
                host.expire_undos();
                host.git_tick();
                host.scripts_tick();
                host.files.sweep();
                host.drop_stale_uploads();
                ticks += 1;
                // Pending images older than a day go, checked every 10 minutes.
                if ticks.is_multiple_of(120)
                    && let Some(images) = host.images.clone()
                {
                    tokio::spawn(async move { images.sweep().await });
                }
                let busy = host.clients.load(Ordering::SeqCst) > 0
                    || host.any_running()
                    || host.source.running_jobs() > 0
                    || host.scripts.running() > 0;
                if busy {
                    host.touch();
                    continue;
                }
                let since = *host.idle_since.lock().unwrap_or_else(|e| e.into_inner());
                if since.elapsed() >= host.config.idle_exit {
                    return Served::Idle;
                }
            }
        };
        let signals = async {
            let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
            let mut int = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).ok();
            tokio::select! {
                _ = async { match term.as_mut() { Some(s) => { s.recv().await; } None => std::future::pending().await } } => {}
                _ = async { match int.as_mut() { Some(s) => { s.recv().await; } None => std::future::pending().await } } => {}
            }
        };
        tokio::select! {
            outcome = idle => outcome,
            () = stop => Served::Stopped,
            () = signals => Served::Stopped,
            () = host.shutdown.notified() => Served::Stopped,
        }
    };
    accept.abort();
    git_poll.abort();
    ports_poll.abort();
    let _ = std::fs::remove_file(&host.config.socket);
    host.sessions.stop();
    host.stop_runs().await;
    host.stop_jobs().await;
    host.shutdown_threads().await;
    host.files.shutdown().await;
    host.save_drafts_now();
    host.store.flush().await;
    pump.abort();
    actions.abort();
    drop(lock);
    tracing::info!("agent host stopped");
    Ok(outcome)
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params).map_err(|e| RpcError::new(ErrorCode::BadRequest, e.to_string()))
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, RpcError> {
    serde_json::to_value(value).map_err(|e| RpcError::new(ErrorCode::Internal, e.to_string()))
}

/// Methods that make or read threads beyond what is in memory: refused
/// when the thread database couldn't be opened.
const NEEDS_STORE: [&str; 6] = [
    method::THREAD_START,
    method::SESSION_CONTINUE,
    method::THREADS_SEARCH,
    method::STORE_INFO,
    method::THREAD_FORK,
    method::SESSIONS_IMPORT,
];

/// What a connection is after `hello`.
#[derive(Debug, Clone)]
enum Role {
    /// A client, and what it understands.
    Client(ClientKind, Caps),
    Bridge {
        thread_id: String,
        generation: u64,
    },
}

impl Role {
    /// It may say who is at this PC (`ui.presence`): G7 Link only.
    fn sets_presence(&self) -> bool {
        matches!(self, Role::Client(ClientKind::Link, _))
    }
}

impl Host {
    fn reply(outbox: &Outbox, id: u64, result: Result<Value, RpcError>) {
        let response = match result {
            Ok(value) => Response::ok(id, value),
            Err(error) => Response::err(id, error),
        };
        outbox.push(serde_json::to_string(&response).unwrap_or_default(), false);
    }

    async fn connection(self: Arc<Self>, stream: UnixStream) {
        match stream.peer_cred() {
            Ok(cred) if cred.uid() == client::euid() => {}
            _ => return,
        }
        let (read, write) = stream.into_split();
        let outbox = Arc::new(Outbox::new());
        let writer = tokio::spawn(write_loop(outbox.clone(), write));
        if self.clients.fetch_add(1, Ordering::SeqCst) >= MAX_CLIENTS {
            self.clients.fetch_sub(1, Ordering::SeqCst);
            Self::reply(&outbox, 0, Err(RpcError::new(ErrorCode::Busy, "Too many clients")));
            outbox.close();
            let _ = writer.await;
            return;
        }
        self.touch();
        let client_id = self.next_client.fetch_add(1, Ordering::SeqCst);
        let mut reader = BufReader::new(read);
        let mut buf = Vec::new();
        let in_flight = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
        let mut role: Option<Role> = None;
        let mut closing = outbox.closing.subscribe();
        loop {
            let read = tokio::select! {
                read = lines::read_line(&mut reader, &mut buf, MAX_LINE) => read,
                _ = closing.wait_for(|closed| *closed) => break,
            };
            match read {
                Ok(LineRead::Line) => {}
                Ok(LineRead::Eof | LineRead::TooLong) | Err(_) => break,
            }
            let request = match serde_json::from_slice::<Request>(&buf) {
                Ok(request) => request,
                Err(_) => {
                    let id = serde_json::from_slice::<Value>(&buf)
                        .ok()
                        .and_then(|v| v.get("id")?.as_u64())
                        .unwrap_or(0);
                    Self::reply(&outbox, id, Err(RpcError::new(ErrorCode::BadRequest, "not a request")));
                    if role.is_none() {
                        break;
                    }
                    continue;
                }
            };
            let Some(current) = role.clone() else {
                match self.hello(client_id, &outbox, request) {
                    Some(r) => {
                        // A person's client: sessions on this PC are looked for, at most hourly.
                        if matches!(r, Role::Client(ClientKind::Launcher | ClientKind::Link, _)) {
                            sessions::scan::hourly(&self);
                        }
                        role = Some(r);
                        continue;
                    }
                    None => break,
                }
            };
            // Connection state changes run in order; everything else may overlap.
            match (&current, request.method.as_str()) {
                (Role::Client(..), method::SUBSCRIBE) => {
                    let result = parse::<SubscribeParams>(request.params).and_then(|p| {
                        if let ThreadSelection::Ids(ids) = &p.threads
                            && (ids.len() > MAX_SUBSCRIBE_THREADS || ids.iter().any(|id| !is_valid_id(id)))
                        {
                            return Err(RpcError::new(ErrorCode::BadRequest, "bad thread list"));
                        }
                        let sessions = p.sessions && self.config.sessions;
                        self.hub.set(client_id, |slot| {
                            slot.subscription =
                                Some(Subscription { threads: p.threads, deltas: p.deltas, sessions });
                        });
                        if sessions {
                            self.sessions_wake();
                        }
                        Ok(json!({}))
                    });
                    Self::reply(&outbox, request.id, result);
                }
                (Role::Client(..), method::SESSION_WATCH) => {
                    let result = match parse::<SessionWatchParams>(request.params) {
                        Ok(p) => self.session_watch(client_id, p.session_id).await.map(|()| json!({})),
                        Err(e) => Err(e),
                    };
                    Self::reply(&outbox, request.id, result);
                }
                (Role::Client(..), method::UNSUBSCRIBE) => {
                    self.hub.set(client_id, |slot| slot.subscription = None);
                    Self::reply(&outbox, request.id, Ok(json!({})));
                }
                (Role::Client(..), method::UI_VIEWING) => {
                    let result = parse::<UiViewingParams>(request.params).map(|p| {
                        let shown = p.thread_id.clone();
                        let seq = self.hub.next_seq();
                        self.hub.set(client_id, |slot| {
                            let session = p.session_id.filter(|id| is_valid_session_id(id));
                            if slot.viewing != p.thread_id {
                                slot.viewing_since = seq;
                            }
                            if slot.viewing_session != session {
                                slot.session_since = seq;
                            }
                            slot.viewing = p.thread_id;
                            slot.viewing_session = session;
                        });
                        if let Some(thread_id) = shown.filter(|_| self.hub.viewed_by(client_id)) {
                            self.thread_viewed(&thread_id);
                        }
                        json!({})
                    });
                    Self::reply(&outbox, request.id, result);
                    self.presence_changed();
                }
                (_, method::UI_PRESENCE) => {
                    let result = self.ui_presence(current.sets_presence(), client_id, request.params);
                    Self::reply(&outbox, request.id, result);
                }
                _ => {
                    let Ok(permit) = in_flight.clone().acquire_owned().await else { break };
                    let host = self.clone();
                    let outbox = outbox.clone();
                    tokio::spawn(async move {
                        let result =
                            host.dispatch(&current, client_id, request.method.as_str(), request.params).await;
                        Self::reply(&outbox, request.id, result);
                        drop(permit);
                    });
                }
            }
        }
        outbox.close();
        self.hub.lock().remove(&client_id);
        self.photo_closed(client_id);
        // What it said of presence goes with it, and what it viewed.
        self.hub.lock_presence().closed(client_id);
        self.presence_changed();
        self.lock_uploads().remove(&client_id);
        self.rewinds.lock().unwrap_or_else(|e| e.into_inner()).forget_client(client_id);
        if let Some(search) = self.searches.lock().unwrap_or_else(|e| e.into_inner()).remove(&client_id) {
            search.stop();
        }
        if let Some(Role::Bridge { thread_id, generation }) = role {
            self.bridge_closed(&thread_id, generation);
        }
        let _ = writer.await;
        self.clients.fetch_sub(1, Ordering::SeqCst);
        self.touch();
    }

    /// The first request. `None` closes the connection.
    fn hello(&self, client_id: u64, outbox: &Arc<Outbox>, request: Request) -> Option<Role> {
        if request.method != method::HELLO {
            Self::reply(outbox, request.id, Err(RpcError::new(ErrorCode::BadRequest, "say hello first")));
            return None;
        }
        let params = match parse::<HelloParams>(request.params) {
            Ok(p) => p,
            Err(e) => {
                Self::reply(outbox, request.id, Err(e));
                return None;
            }
        };
        if !params.caps_valid() {
            Self::reply(outbox, request.id, Err(RpcError::new(ErrorCode::BadRequest, "too many caps")));
            return None;
        }
        if params.version != API_VERSION {
            Self::reply(
                outbox,
                request.id,
                Err(RpcError::new(ErrorCode::UnsupportedVersion, "this host speaks version 1")),
            );
            return None;
        }
        let role = match (&params.bridge_token, params.client) {
            (Some(token), _) => match self.bridge_lookup(token) {
                Some((thread_id, generation)) => Role::Bridge { thread_id, generation },
                None => {
                    Self::reply(
                        outbox,
                        request.id,
                        Err(RpcError::new(ErrorCode::Denied, "unknown bridge token")),
                    );
                    return None;
                }
            },
            (None, ClientKind::Bridge) => {
                Self::reply(
                    outbox,
                    request.id,
                    Err(RpcError::new(ErrorCode::Denied, "a bridge needs its token")),
                );
                return None;
            }
            (None, kind) => Role::Client(kind, Caps::of(&params)),
        };
        let (kind, caps) = match &role {
            Role::Client(kind, caps) => (*kind, *caps),
            Role::Bridge { .. } => (ClientKind::Bridge, Caps::default()),
        };
        let mut slot = ClientSlot::new(kind, caps, outbox.clone());
        slot.presence = matches!(role, Role::Client(..)) && params.has(cap::PRESENCE);
        slot.handoff = kind == ClientKind::Link && params.has(cap::HANDOFF);
        slot.photo = kind == ClientKind::Link && params.has(cap::PHOTO);
        slot.beside = kind == ClientKind::Link && params.has(cap::BESIDE);
        slot.scripts = params.has(cap::SCRIPTS);
        let presence = slot.presence && slot.shows_threads();
        self.hub.lock().insert(client_id, slot);
        let result = HelloResult {
            version: API_VERSION,
            host_version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            caps: cap::HOST.map(str::to_owned).to_vec(),
        };
        Self::reply(outbox, request.id, to_value(&result));
        // The state now, so it needn't wait for a change.
        if presence {
            let event = self.hub.presence_event(Instant::now());
            self.hub.send_presence(&event, Some(client_id));
        }
        Some(role)
    }

    async fn dispatch(
        self: &Arc<Self>,
        role: &Role,
        client_id: u64,
        method_name: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        if let Role::Bridge { thread_id, generation } = role {
            return match method_name {
                method::BRIDGE_ASK => {
                    let p: BridgeAskParams = parse(params)?;
                    if p.tool_name.len() > MAX_BRIDGE_TOOL_NAME
                        || p.tool_use_id.len() > MAX_BRIDGE_TOOL_USE_ID
                    {
                        return Err(RpcError::new(ErrorCode::BadRequest, "field too long"));
                    }
                    let result = self.bridge_ask(thread_id, *generation, p).await;
                    to_value(&result)
                }
                method::BRIDGE_CANCEL => {
                    let p: BridgeCancelParams = parse(params)?;
                    self.bridge_cancel(thread_id, *generation, &p.tool_use_id);
                    Ok(json!({}))
                }
                _ => Err(RpcError::new(ErrorCode::BadRequest, "a bridge may only ask")),
            };
        }
        let (kind, caps) = match role {
            Role::Client(kind, caps) => (*kind, *caps),
            Role::Bridge { .. } => (ClientKind::Bridge, Caps::default()),
        };
        // Whose pending images a request may use.
        let owner = client_owner(kind);
        if !self.store.has_db() && NEEDS_STORE.contains(&method_name) {
            return Err(RpcError::new(ErrorCode::Unavailable, "G7 couldn't open its thread store."));
        }
        match method_name {
            method::AGENTS_LIST => {
                let p: AgentsListParams = parse(params)?;
                let agents = self.agents(p.refresh).await;
                to_value(&AgentsListResult { agents })
            }
            method::FOLDERS_LIST => to_value(&FoldersListResult { folders: self.folders_list() }),
            method::FOLDERS_ADD => {
                let p: FoldersAddParams = parse(params)?;
                let folder = self.folders_add(p.path, p.label).await?;
                to_value(&FolderResult { folder })
            }
            method::FOLDERS_REMOVE => {
                let p: FoldersRemoveParams = parse(params)?;
                self.folders_remove(&p.id).await?;
                Ok(json!({}))
            }
            method::THREADS_LIST => {
                let p: ThreadsListParams = parse(params)?;
                let threads = self.threads_list(p).iter().map(|t| t.for_client(caps.items3)).collect();
                to_value(&ThreadsListResult { threads })
            }
            method::THREAD_START => {
                let p: ThreadStartParams = parse(params)?;
                let p = self.review_start(kind, p).await?;
                if p.worktree.is_some() {
                    let mut result = self.thread_start_worktree(kind, p, owner).await?;
                    result.thread = result.thread.for_client(caps.items3);
                    return to_value(&result);
                }
                let (thread, duplicate) = self.thread_start_keyed(p, owner).await?;
                to_value(&ThreadStartResult { thread: thread.for_client(caps.items3), duplicate, job: None })
            }
            method::THREAD_SEND => {
                let p: ThreadSendParams = parse(params)?;
                let p = self.ask_params(p).await?;
                to_value(&self.thread_send(p, owner).await?)
            }
            method::THREAD_INTERRUPT => {
                let p: ThreadIdParams = parse(params)?;
                self.thread_interrupt(&p.thread_id)?;
                Ok(json!({}))
            }
            method::THREAD_READ => {
                let p: ThreadReadParams = parse(params)?;
                to_value(&self.thread_read(p, caps).await?)
            }
            method::THREAD_DELETE => {
                let p: ThreadDeleteParams = parse(params)?;
                to_value(&self.thread_delete_git(kind, p).await?)
            }
            method::THREAD_TERMINAL_COMMAND => {
                let p: ThreadIdParams = parse(params)?;
                to_value(&self.thread_terminal_command(&p.thread_id)?)
            }
            method::APPROVAL_ANSWER => {
                let p: ApprovalAnswerParams = parse(params)?;
                self.approval_answer(&p.token, p.decision, p.by, p.message)?;
                Ok(json!({}))
            }
            method::SESSIONS_LIST => {
                let p: SessionsListParams = parse(params)?;
                let limit = p.limit.map_or(MAX_SESSIONS, |l| (l as usize).clamp(1, MAX_SESSIONS));
                to_value(&SessionsListResult { sessions: self.sessions_list(limit).await })
            }
            method::SESSION_READ => {
                let p: SessionReadParams = parse(params)?;
                to_value(&self.session_read(p).await?)
            }
            method::SESSION_CONTINUE => {
                let p: SessionContinueParams = parse(params)?;
                let thread = self.session_continue(p).await?;
                to_value(&ThreadResult { thread: thread.for_client(caps.items3) })
            }
            method::SETTINGS_GET => to_value(&self.settings()),
            method::THREAD_EXPORT => {
                let p: ThreadExportParams = parse(params)?;
                to_value(&self.thread_export(p).await?)
            }
            method::SESSIONS_SET_FOLLOWING => {
                let p: SessionsSetFollowingParams = parse(params)?;
                to_value(&self.sessions_set_following(p.on).await?)
            }
            method::THREAD_CONFIGURE => {
                let p: ThreadConfigureParams = parse(params)?;
                let thread = self.thread_configure(p).await?;
                to_value(&ThreadResult { thread: thread.for_client(caps.items3) })
            }
            method::QUEUE_EDIT => {
                let p: QueueEditParams = parse(params)?;
                self.queue_edit(p)?;
                Ok(json!({}))
            }
            method::QUEUE_REMOVE => {
                let p: QueueItemParams = parse(params)?;
                self.queue_remove(p)?;
                Ok(json!({}))
            }
            method::QUEUE_SEND_NOW => {
                let p: QueueItemParams = parse(params)?;
                self.queue_send_now(p).await?;
                Ok(json!({}))
            }
            method::QUESTION_ANSWER => {
                let p: QuestionAnswerParams = parse(params)?;
                self.question_answer(p)?;
                Ok(json!({}))
            }
            method::QUESTION_DISMISS => {
                let p: QuestionDismissParams = parse(params)?;
                self.question_dismiss(&p.token, p.by)?;
                Ok(json!({}))
            }
            method::PLAN_IMPLEMENT => {
                let p: PlanImplementParams = parse(params)?;
                let thread_id = p.thread_id.clone();
                self.plan_implement(p).await?;
                self.sent_to(&thread_id);
                Ok(json!({}))
            }
            method::FILES_SEARCH => {
                let p: FilesSearchParams = parse(params)?;
                to_value(&self.files_search(p).await?)
            }
            method::COMMANDS_LIST => {
                let p: CommandsListParams = parse(params)?;
                to_value(&CommandsListResult { commands: self.commands_list(p.agent, &p.folder_id).await? })
            }
            method::IMAGE_ADD_PATH => {
                let p: ImageAddPathParams = parse(params)?;
                let images = self.image_store()?;
                let info = images.add_path(&p.path, owner).await.map_err(image_error)?;
                to_value(&ImageResult { image: image_ref(info) })
            }
            method::IMAGE_BEGIN => {
                let p: ImageBeginParams = parse(params)?;
                self.image_store()?;
                let upload = attachments::Upload::begin(&p.name, p.bytes).map_err(image_error)?;
                let id = upload.id().to_owned();
                self.lock_uploads().insert(client_id, upload);
                to_value(&ImageBeginResult { upload: id })
            }
            method::IMAGE_CHUNK => {
                let p: ImageChunkParams = parse(params)?;
                let mut uploads = self.lock_uploads();
                let upload = uploads
                    .get_mut(&client_id)
                    .filter(|u| u.id() == p.upload)
                    .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "No such upload."))?;
                if let Err(e) = upload.chunk(p.seq, &p.data) {
                    uploads.remove(&client_id);
                    return Err(image_error(e));
                }
                Ok(json!({}))
            }
            method::IMAGE_END => {
                let p: ImageEndParams = parse(params)?;
                let images = self.image_store()?;
                let upload = {
                    let mut uploads = self.lock_uploads();
                    match uploads.get(&client_id) {
                        Some(u) if u.id() == p.upload => uploads.remove(&client_id),
                        _ => None,
                    }
                };
                let upload = upload.ok_or_else(|| RpcError::new(ErrorCode::NotFound, "No such upload."))?;
                let (name, bytes) = upload.finish().map_err(image_error)?;
                let info = images.add_bytes(&name, bytes, owner).await.map_err(image_error)?;
                to_value(&ImageResult { image: image_ref(info) })
            }
            method::IMAGE_DROP => {
                let p: ImageIdParams = parse(params)?;
                self.image_store()?.drop_pending(&p.id, Some(owner)).await.map_err(image_error)?;
                Ok(json!({}))
            }
            method::IMAGE_READ => {
                let p: ImageReadParams = parse(params)?;
                to_value(&self.image_store()?.read(&p.id, p.max).await.map_err(image_error)?)
            }
            method::DRAFT_SET => {
                let p: DraftSetParams = parse(params)?;
                if p.images.len() > MAX_IMAGES {
                    return Err(RpcError::new(ErrorCode::BadRequest, "Up to 4 images go with a message."));
                }
                let mut images = Vec::with_capacity(p.images.len());
                for id in &p.images {
                    let info = self.images.as_ref().and_then(|store| store.info(id)).ok_or_else(|| {
                        RpcError::new(ErrorCode::NotFound, "That image isn't there any more.")
                    })?;
                    images.push(image_ref(info));
                }
                to_value(&DraftSetResult { rev: self.draft_set(p, images, client_id)? })
            }
            method::DRAFT_GET => {
                let p: DraftGetParams = parse(params)?;
                to_value(&DraftGetResult { draft: self.draft_get(&p.key)? })
            }
            method::HISTORY_LIST => {
                let p: HistoryListParams = parse(params)?;
                to_value(&HistoryListResult { prompts: self.history_list(p.limit) })
            }
            method::FOLDERS_SET => {
                let p: FoldersSetParams = parse(params)?;
                if p.follow_remote.is_some() {
                    source::check_may(kind, None, source::Ask::FollowRemote)?;
                }
                // A folder's scripts are typed in G7 Link on this PC, nowhere else.
                if (p.scripts.is_some() || p.phone_scripts.is_some()) && kind != ClientKind::Link {
                    return Err(RpcError::new(ErrorCode::Denied, source::DONE_ON_LINK));
                }
                to_value(&FolderResult { folder: self.folders_set(&p).await? })
            }
            method::SETTINGS_SET => {
                let p: SettingsSetParams = parse(params)?;
                to_value(&self.settings_set(p).await?)
            }
            // History (phase 3).
            method::THREAD_MARK => {
                let p: ThreadMarkParams = parse(params)?;
                let mut result = self.thread_mark(p).await?;
                for thread in &mut result.threads {
                    *thread = thread.for_client(caps.items3);
                }
                to_value(&result)
            }
            method::THREAD_RENAME => {
                let p: ThreadRenameParams = parse(params)?;
                let thread = self.thread_rename(p).await?;
                to_value(&ThreadResult { thread: thread.for_client(caps.items3) })
            }
            method::THREADS_SEARCH => {
                let p: ThreadsSearchParams = parse(params)?;
                to_value(&self.threads_search(p, client_id).await?)
            }
            method::SESSIONS_SCAN => {
                let p: SessionsScanParams = parse(params)?;
                self.hub.set(client_id, |slot| slot.scans = true);
                to_value(&self.sessions_scan(p).await?)
            }
            method::SESSIONS_IMPORT => {
                let p: SessionsImportParams = parse(params)?;
                to_value(&self.sessions_import(p).await?)
            }
            method::IMPORT_CANCEL => {
                let p: ImportCancelParams = parse(params)?;
                self.import_cancel(&p.job).await?;
                Ok(json!({}))
            }
            method::SESSION_PREVIEW => {
                let p: SessionPreviewParams = parse(params)?;
                let items = self.session_preview(p).await?;
                let items = items.iter().filter_map(|i| i.for_caps(caps.items2, caps.items3)).collect();
                to_value(&SessionPreviewResult { items })
            }
            method::STORE_INFO => to_value(&self.store_info().await?),
            method::UI_OPEN => {
                let p: ThreadIdParams = parse(params)?;
                if !self.lock_reg().threads.contains_key(&p.thread_id) {
                    return Err(RpcError::new(ErrorCode::NotFound, "No such thread."));
                }
                if self.hub.ui_open_in_link(&p.thread_id, client_id) == 0 {
                    return Err(RpcError::new(ErrorCode::Unavailable, "G7 Link isn't open."));
                }
                Ok(json!({}))
            }
            // Undo (phase 3).
            method::THREAD_TURNS => {
                let p: ThreadTurnsParams = parse(params)?;
                to_value(&ThreadTurnsResult { turns: self.thread_turns(p, kind).await? })
            }
            method::TURN_FILES => {
                let p: TurnFilesParams = parse(params)?;
                to_value(&self.turn_files(p).await?)
            }
            method::TURN_PATCH => {
                let p: TurnPatchParams = parse(params)?;
                to_value(&self.turn_patch(p).await?)
            }
            method::REWIND_PREVIEW => {
                let p: RewindPreviewParams = parse(params)?;
                to_value(&RewindPreviewResult { plan: self.rewind_preview(p, client_id, kind).await? })
            }
            method::REWIND_APPLY => {
                let p: RewindApplyParams = parse(params)?;
                let mut result = self.rewind_apply(p, client_id, kind).await?;
                result.thread = result.thread.for_client(caps.items3);
                to_value(&result)
            }
            method::REWIND_UNDO => {
                let p: RewindUndoParams = parse(params)?;
                let thread = self.rewind_undo(p, kind).await?;
                to_value(&ThreadResult { thread: thread.for_client(caps.items3) })
            }
            method::THREAD_FORK => {
                let p: ThreadForkParams = parse(params)?;
                let thread = self.thread_fork(p).await?;
                to_value(&ThreadResult { thread: thread.for_client(caps.items3) })
            }
            // Reply on the PC (0.1.21).
            method::SESSION_PLACE => {
                let p: SessionPlaceParams = parse(params)?;
                to_value(&self.session_place(&p.session_id).await?)
            }
            // Hand-off and the phone's camera (phase 5, wave 2).
            method::UI_HANDOFF => {
                let p: UiHandoffParams = parse(params)?;
                to_value(&self.ui_handoff(kind, p)?)
            }
            method::UI_PHOTO => {
                let p: UiPhotoParams = parse(params)?;
                to_value(&self.ui_photo(kind, client_id, p).await?)
            }
            method::UI_PHOTO_DONE => {
                let p: UiPhotoDoneParams = parse(params)?;
                self.ui_photo_done(kind, client_id, p).await?;
                Ok(json!({}))
            }
            method::UI_PHOTO_STOP => {
                self.ui_photo_stop(kind, client_id)?;
                Ok(json!({}))
            }
            // The phone beside the PC (phase 5, wave 3).
            method::UI_BESIDE => {
                let p: UiBesideParams = parse(params)?;
                self.ui_beside(kind, p)?;
                Ok(json!({}))
            }
            m if source::METHODS.contains(&m) => self.git_call(kind, m, params).await,
            m if scripts::METHODS.contains(&m) => self.script_call(kind, m, params).await,
            method::HELLO => Err(RpcError::new(ErrorCode::BadRequest, "already said hello")),
            _ => Err(RpcError::new(ErrorCode::BadRequest, UNKNOWN_METHOD)),
        }
    }

    fn on_note_action(self: &Arc<Self>, action: NoteAction) {
        match action {
            NoteAction::Answer { token, allow } => {
                let decision = if allow { Decision::Allow } else { Decision::Deny };
                if let Err(e) = self.approval_answer(&token, decision, Origin::launcher(), None) {
                    tracing::debug!("a notification answer was refused: {}", e.code);
                }
            }
            NoteAction::Open { thread_id } => self.open_ui(&thread_id),
            NoteAction::Stop { thread_id } => {
                let _ = self.thread_interrupt(&thread_id);
            }
        }
    }

    /// Shows a thread: `ui.open` to the launchers, or the launcher's deep
    /// link when none is connected.
    pub fn open_ui(&self, thread_id: &str) {
        if !is_valid_thread_id(thread_id) || self.hub.ui_open(thread_id) > 0 {
            return;
        }
        let child_env = env::child_env(AgentId::Codex, self.config.env.iter().cloned());
        let Some(bin) = agents::resolve_cli("g7linux", None, &child_env) else { return };
        let spawned = tokio::process::Command::new(bin)
            .arg("deeplink")
            .arg(format!("g7linux://agents/thread/{thread_id}"))
            .env_clear()
            .envs(self.config.env.iter().map(|(k, v)| (k, v)))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut child) = spawned {
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
    }
}

/// Whose pending images a connection may use: images added through one kind
/// of client (G7 Link's page and the phones it relays, or the launcher) are
/// that kind's, so a draft's images survive a reconnect.
fn client_owner(kind: ClientKind) -> &'static str {
    match kind {
        ClientKind::Launcher => "launcher",
        ClientKind::Link => "link",
        ClientKind::Bridge => "bridge",
        ClientKind::Cli => "cli",
        ClientKind::Test => "test",
    }
}

fn image_error(e: attachments::ImageError) -> RpcError {
    RpcError::new(e.code(), e.message())
}

fn image_ref(info: attachments::ImageInfo) -> ImageRef {
    info.image_ref()
}

/// The image store under the data directory. `image.add_path` never opens
/// anything in the agents' own configuration or a keyring.
fn open_images(config: &Config) -> Option<attachments::ImageStore> {
    let mut refused = vec![config.claude_home.clone(), config.codex_home.clone(), config.data_dir.clone()];
    if let Some(home) = env::lookup(&config.env, "HOME").map(PathBuf::from).filter(|p| p.is_absolute()) {
        refused.push(home.join(".local/share/keyrings"));
        refused.push(home.join(".local/share/kwalletd"));
        refused.push(home.join(".claude.json"));
        refused.push(home.join(".ssh"));
        refused.push(home.join(".gnupg"));
    }
    match attachments::ImageStore::open(config.data_dir.join("images"), refused) {
        Ok(store) => Some(store),
        Err(e) => {
            tracing::warn!("couldn't open the image store: {e}");
            None
        }
    }
}

impl Host {
    fn lock_uploads(&self) -> std::sync::MutexGuard<'_, HashMap<u64, attachments::Upload>> {
        self.uploads.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn image_store(&self) -> Result<&attachments::ImageStore, RpcError> {
        self.images
            .as_ref()
            .ok_or_else(|| RpcError::new(ErrorCode::Unavailable, "Images aren't available here."))
    }

    /// Uploads not ended in time are dropped.
    fn drop_stale_uploads(&self) {
        self.lock_uploads().retain(|_, upload| !upload.expired());
    }

    /// `files.search` in a folder, or a thread's folder.
    async fn files_search(&self, p: FilesSearchParams) -> Result<FilesSearchResult, RpcError> {
        if p.query.len() > MAX_FILES_QUERY * 4 || p.query.chars().count() > MAX_FILES_QUERY {
            return Err(RpcError::new(ErrorCode::BadRequest, "The query is too long."));
        }
        // A worktree thread's files are in its worktree (§SC3.8).
        let folder = {
            let reg = self.lock_reg();
            let (folder_id, worktree) = match (&p.folder_id, &p.thread_id) {
                (Some(id), _) => (id.clone(), None),
                (None, Some(thread)) => {
                    let entry = reg
                        .threads
                        .get(thread)
                        .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "No such thread."))?;
                    let worktree = entry.info.worktree.is_some().then(|| entry.worktree_dir.clone());
                    (entry.info.folder_id.clone(), worktree)
                }
                (None, None) => {
                    return Err(RpcError::new(ErrorCode::BadRequest, "Name a folder or a thread."));
                }
            };
            match worktree {
                Some(dir) => dir
                    .filter(|d| d.is_dir())
                    .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "Its worktree is gone."))?,
                None => reg
                    .folders
                    .iter()
                    .find(|f| f.id == folder_id)
                    .map(|f| PathBuf::from(&f.path))
                    .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "No such folder."))?,
            }
        };
        let found = self.files.search(&folder, &p.query, p.limit).await?;
        Ok(FilesSearchResult {
            files: found.files.into_iter().map(|h| FileHit { path: h.path, dir: h.dir }).collect(),
            more: found.more,
        })
    }

    /// `commands.list`: Claude's from its catalog and the folder's last
    /// `system/init`; Codex's skills in the folder, asked once.
    async fn commands_list(
        &self,
        agent: AgentId,
        folder_id: &str,
    ) -> Result<Vec<crate::api::Command>, RpcError> {
        let folder = {
            let reg = self.lock_reg();
            reg.folders
                .iter()
                .find(|f| f.id == folder_id)
                .map(|f| PathBuf::from(&f.path))
                .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "No such folder."))?
        };
        let mut commands = match agent {
            AgentId::Claude => {
                self.probe(false).await;
                self.lock_commands().list(AgentId::Claude, folder_id)
            }
            AgentId::Codex => {
                // Asked once per folder; `commands.rs` filters them (G7's own
                // names) and adds `compact`.
                if !self.lock_commands().has_codex_skills(folder_id) {
                    let probe = self.probe(false).await[AgentId::Codex as usize].clone();
                    let Some(bin) = probe.bin.filter(|_| probe.info.ready()) else { return Ok(Vec::new()) };
                    let child_env = env::child_env(AgentId::Codex, self.config.env.iter().cloned());
                    let skills = catalog::codex_skills(&bin, &child_env, &folder).await;
                    self.lock_commands().set_codex_skills(folder_id, &skills);
                }
                self.lock_commands().list(AgentId::Codex, folder_id)
            }
        };
        commands.truncate(MAX_COMMANDS);
        Ok(commands)
    }

    /// `settings.set`: stored, announced, and keep-awake follows at once.
    async fn settings_set(&self, p: SettingsSetParams) -> Result<HostSettings, RpcError> {
        let _serial = self.folders_lock.lock().await;
        let now = self.settings();
        let settings = HostSettings {
            keep_awake: p.keep_awake.unwrap_or(now.keep_awake),
            limit_alarm: p.limit_alarm.unwrap_or(now.limit_alarm),
            ..now
        };
        if settings == now {
            return Ok(now);
        }
        self.store
            .save_settings(settings)
            .await
            .map_err(|_| RpcError::new(ErrorCode::Internal, "Couldn't save the setting."))?;
        if settings.keep_awake != now.keep_awake {
            self.keep_awake.store(settings.keep_awake, Ordering::SeqCst);
            self.awake.set_enabled(settings.keep_awake);
        }
        self.limit_alarm.store(settings.limit_alarm, Ordering::SeqCst);
        self.hub.broadcast(&Event::SettingsChanged(settings));
        Ok(settings)
    }

    // ---- Sessions found on this PC (§U4), served by `sessions` ----

    /// `sessions.scan`: starts or continues the scan, answers at once.
    async fn sessions_scan(self: &Arc<Self>, p: SessionsScanParams) -> Result<SessionsScanResult, RpcError> {
        sessions::scan::answer(self, p).await
    }

    /// `sessions.import`: one job at a time.
    async fn sessions_import(
        self: &Arc<Self>,
        p: SessionsImportParams,
    ) -> Result<SessionsImportResult, RpcError> {
        let job = sessions::import::import(self, p.ids, p.by).await?;
        Ok(SessionsImportResult { job: job.job, total: job.total })
    }

    async fn import_cancel(&self, job: &str) -> Result<(), RpcError> {
        sessions::import::cancel(self, job)
    }

    /// `session.preview`: read on demand, nothing stored.
    async fn session_preview(&self, p: SessionPreviewParams) -> Result<Vec<Item>, RpcError> {
        let limit = p.limit.unwrap_or(DEFAULT_PREVIEW_LIMIT).clamp(1, MAX_PREVIEW_LIMIT);
        sessions::import::preview(self, &p.id, limit).await
    }

    /// A session brought in becomes a thread (`sessions::FoundStore::bring_in`):
    /// saved with its items and turns and linked to its session in one go,
    /// then listed. It is not unread and raises no notification.
    fn bring_in(&self, b: sessions::import::BroughtIn) -> Result<String, RpcError> {
        let failed = |_| RpcError::new(ErrorCode::Internal, "Couldn't save the thread.");
        let id = {
            let reg = self.lock_reg();
            let mut id = threads::random_hex(16);
            while reg.threads.contains_key(&id) {
                id = threads::random_hex(16);
            }
            id
        };
        let info = brought_in_info(&id, &b);
        let cols = db::ThreadCols {
            next_seq: b.next_seq(),
            next_turn: b.next_turn(),
            cwd: if b.folder.is_none() { b.cwd.clone() } else { None },
            read_seq: b.last_seq(),
            found_id: Some(b.found_id.clone()),
            ..db::ThreadCols::default()
        };
        let thread = id.clone();
        let loaded = self
            .store
            .db_blocking(move |db| {
                db.save_thread(&info, &cols)?;
                for item in &b.items {
                    db.append_item(&thread, item)?;
                }
                for turn in &b.turns {
                    db.save_turn(&thread, turn, None)?;
                }
                db.flush()?;
                db.set_found_thread(&b.found_id, Some(&thread))?;
                db.load_thread(&thread)
            })
            .map_err(failed)?
            .ok_or_else(|| RpcError::new(ErrorCode::Internal, "Couldn't save the thread."))?;
        let mut reg = self.lock_reg();
        let folders = reg.folders.clone();
        let mut entry = threads::Registry::entry_from(loaded, &folders);
        threads::emit_state(self, &mut entry);
        reg.threads.insert(id.clone(), entry);
        Ok(id)
    }
}

/// The thread a session brought in becomes: done, read, `found`, and
/// read-only while no agent folder holds its working directory.
fn brought_in_info(id: &str, b: &sessions::import::BroughtIn) -> ThreadInfo {
    ThreadInfo {
        id: id.to_owned(),
        agent: b.agent,
        folder_id: b.folder.as_ref().map(|f| f.0.clone()).unwrap_or_default(),
        folder_label: b.folder_label.clone(),
        title: b.title.clone(),
        state: State::Done,
        mode: Mode::Supervised,
        origin: b.origin.clone(),
        created_ms: b.created_ms,
        updated_ms: b.updated_ms,
        last_seq: b.last_seq(),
        pending: 0,
        summary: None,
        error: None,
        native_id: Some(b.native_id.clone()),
        step: None,
        plan: None,
        state_since_ms: b.updated_ms,
        subdir: b.folder.as_ref().and_then(|f| f.1.clone()),
        continued: false,
        model: None,
        effort: None,
        answered_model: None,
        queued: Vec::new(),
        queue_held: false,
        context: None,
        mcp: None,
        asking: 0,
        pinned: false,
        archived: false,
        unread: false,
        snoozed_until_ms: None,
        turns: b.turns.last().map_or(0, |t| t.turn),
        forked_from: None,
        found: true,
        readonly: b.folder.is_none().then_some(Readonly::OutsideFolders),
        undo: None,
        branch: None,
        worktree: None,
        changed: None,
        wrote: None,
        ports: Vec::new(),
        runs: Vec::new(),
    }
}

/// The thread database as the session reader uses it (`sessions::FoundStore`),
/// from its blocking worker.
struct FoundStoreOf {
    host: std::sync::Weak<Host>,
}

impl std::fmt::Debug for FoundStoreOf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FoundStoreOf")
    }
}

impl FoundStoreOf {
    fn store(&self) -> io::Result<(Arc<Host>, StoreHandle)> {
        let host = self.host.upgrade().ok_or_else(|| io::Error::other("the host stopped"))?;
        let store = host.store.clone();
        Ok((host, store))
    }
}

impl sessions::FoundStore for FoundStoreOf {
    fn found_files(&self) -> io::Result<Vec<(String, db::FoundFile)>> {
        self.store()?.1.db_blocking(|db| db.found_files())
    }

    fn found(&self, q: &db::FoundQuery) -> io::Result<db::FoundPage> {
        let q = q.clone();
        self.store()?.1.db_blocking(move |db| db.found(&q))
    }

    fn found_row(&self, id: &str) -> io::Result<Option<db::FoundRow>> {
        let id = id.to_owned();
        self.store()?.1.db_blocking(move |db| db.found_row(&id))
    }

    fn upsert_found(&self, rows: &[db::FoundRow]) -> io::Result<()> {
        let rows = rows.to_vec();
        self.store()?.1.db_blocking(move |db| db.upsert_found(&rows))
    }

    fn forget_found_files(&self, files: &[(String, String)]) -> io::Result<()> {
        let files = files.to_vec();
        self.store()?.1.db_blocking(move |db| db.forget_found_files(&files))
    }

    fn bring_in(&self, thread: sessions::import::BroughtIn) -> Result<String, RpcError> {
        let (host, _) =
            self.store().map_err(|_| RpcError::new(ErrorCode::Unavailable, "The host stopped."))?;
        host.bring_in(thread)
    }
}

/// Keeps the newest answered tokens so a late answer gets `ALREADY_ANSWERED`.
#[derive(Debug, Default)]
pub struct Answered {
    order: VecDeque<String>,
    set: std::collections::HashSet<String>,
}

impl Answered {
    pub fn insert(&mut self, token: String) {
        if self.set.insert(token.clone()) {
            self.order.push_back(token);
            if self.order.len() > ANSWERED_KEPT
                && let Some(old) = self.order.pop_front()
            {
                self.set.remove(&old);
            }
        }
    }

    pub fn contains(&self, token: &str) -> bool {
        self.set.contains(token)
    }
}

/// A host with no socket and no agents, for tests of the registry.
#[cfg(test)]
pub(crate) fn test_host(dir: &std::path::Path, approval_lifetime_ms: u64) -> Arc<Host> {
    test_host_with(dir, approval_lifetime_ms, None, Vec::new())
}

/// A test host whose environment is `env` (it finds `git` on its `PATH`).
#[cfg(test)]
pub(crate) fn test_host_env(dir: &std::path::Path, env: Vec<(OsString, OsString)>) -> Arc<Host> {
    test_host_with(dir, APPROVAL_LIFETIME_MS, None, env)
}

/// [`test_host`] whose notifications land in the receiver.
#[cfg(test)]
pub(crate) fn test_host_noting(
    dir: &std::path::Path,
    approval_lifetime_ms: u64,
) -> (Arc<Host>, mpsc::UnboundedReceiver<notify::Note>) {
    let (notifier, notes) = Notifier::recording();
    (test_host_with(dir, approval_lifetime_ms, Some(notifier), Vec::new()), notes)
}

#[cfg(test)]
fn test_host_with(
    dir: &std::path::Path,
    approval_lifetime_ms: u64,
    notifier: Option<Notifier>,
    env: Vec<(OsString, OsString)>,
) -> Arc<Host> {
    let config = Config {
        socket: dir.join("host.sock"),
        runtime_dir: dir.to_owned(),
        data_dir: dir.join("data"),
        claude_bin: None,
        codex_bin: None,
        bridge_exe: PathBuf::from("/bin/false"),
        notify: false,
        env,
        idle_exit: IDLE_EXIT,
        agent_idle: Duration::from_millis(AGENT_IDLE_MS),
        approval_lifetime_ms,
        claude_home: dir.join("claude"),
        codex_home: dir.join("codex"),
        proc_root: dir.join("proc"),
        sessions: true,
        keep_awake_bus: false,
    };
    let mut db = db::Db::open(&config.data_dir).unwrap();
    let store = Store::open(config.data_dir.clone()).unwrap();
    let registry = Registry::load(&store, Some(&mut db), approval_lifetime_ms);
    let (events, _) = mpsc::unbounded_channel();
    let images = open_images(&config);
    Arc::new(Host {
        hub: Hub::default(),
        store: StoreHandle::spawn(store, Some(db)).unwrap(),
        reg: Mutex::new(registry),
        probes: tokio::sync::Mutex::new(ProbeCache::default()),
        folders_lock: tokio::sync::Mutex::new(()),
        events,
        notifier,
        hostname: "test-pc".into(),
        sessions: sessions::Sessions::new(&config),
        keep_awake: std::sync::atomic::AtomicBool::new(true),
        limit_alarm: std::sync::atomic::AtomicBool::new(true),
        photos: Mutex::new(phone::Photos::default()),
        ports: Mutex::new(ports::Watch::default()),
        awake: awake::Awake::spawn(awake::Source::Off, true, awake::release_delay()),
        usage: Mutex::new(usage::Usage::new(Duration::from_millis(LIMITS_EVENT_MS))),
        drafts: Mutex::new(drafts::Drafts::default()),
        drafts_generation: AtomicU64::new(0),
        history: Mutex::new(drafts::History::default()),
        files: files::FileIndex::default(),
        commands: Mutex::new(commands::Commands::default()),
        images,
        uploads: Mutex::new(HashMap::new()),
        rewinds: Mutex::new(undo::RewindTokens::default()),
        searches: Mutex::new(HashMap::new()),
        git: git::Git::new(&config.env),
        scm: None,
        source: source::Source::load(&config.data_dir),
        scripts: scripts::Scripts::default(),
        clients: AtomicUsize::new(0),
        next_client: AtomicU64::new(1),
        idle_since: Mutex::new(Instant::now()),
        shutdown: Notify::new(),
        config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_reaches_each_client_in_the_form_it_understands() {
        let hub = Hub::default();
        let mut boxes = Vec::new();
        for (id, items2) in [(1, false), (2, true)] {
            let outbox = Arc::new(Outbox::new());
            let mut slot = ClientSlot::new(ClientKind::Test, Caps { items2, items3: true }, outbox.clone());
            slot.subscription =
                Some(Subscription { threads: ThreadSelection::All, deltas: true, sessions: false });
            hub.lock().insert(id, slot);
            boxes.push(outbox);
        }
        let item = Item { text: Some("Look.".into()), ..Item::new(3, "r", ItemKind::Reasoning, 1) };
        hub.broadcast(&Event::ThreadItem(ThreadItemEvent { thread_id: "t".into(), item: item.clone() }));
        hub.broadcast_items2(&Event::ThreadDelta(ThreadDeltaEvent {
            thread_id: "t".into(),
            seq: 3,
            item_id: "r".into(),
            offset: 0,
            text: "Look.".into(),
        }));
        let unknown = Item::new(4, "x", ItemKind::Unknown, 1);
        hub.broadcast(&Event::ThreadItem(ThreadItemEvent { thread_id: "t".into(), item: unknown }));
        let events = |outbox: &Outbox| {
            let mut seen = Vec::new();
            while let Some(line) = outbox.pop() {
                let ServerLine::Event(line) = ServerLine::parse(&line.text).unwrap() else { panic!() };
                seen.push(Event::from_line(&line).unwrap());
            }
            seen
        };
        let old = events(&boxes[0]);
        assert_eq!(old, [Event::ThreadItem(ThreadItemEvent { thread_id: "t".into(), item: item.legacy() })]);
        let new = events(&boxes[1]);
        assert_eq!(new.len(), 2);
        assert_eq!(new[0], Event::ThreadItem(ThreadItemEvent { thread_id: "t".into(), item }));
        assert!(matches!(&new[1], Event::ThreadDelta(d) if d.seq == 3));
    }

    #[test]
    fn a_full_outbox_drops_deltas_first_then_closes() {
        let outbox = Outbox::new();
        for i in 0..CLIENT_QUEUE {
            outbox.push(format!("d{i}"), i % 2 == 0);
        }
        outbox.push("delta".into(), true);
        assert_eq!(outbox.queue.lock().unwrap().len(), CLIENT_QUEUE, "a delta into a full queue is dropped");
        outbox.push("item".into(), false);
        assert_eq!(outbox.queue.lock().unwrap().back().unwrap().text, "item");
        assert!(!outbox.is_closed());
        assert_eq!(outbox.queue.lock().unwrap().front().unwrap().text, "d1", "the oldest delta went first");
        for i in 0..CLIENT_QUEUE {
            outbox.push(format!("x{i}"), false);
        }
        assert!(outbox.is_closed(), "full of items: closed");
    }

    fn phone(device: &str, thread: Option<&str>) -> RemoteView {
        RemoteView {
            device_id: device.into(),
            device_name: "Pixel".into(),
            thread_id: thread.map(str::to_owned),
            session_id: None,
            pane: None,
            dnd: false,
        }
    }

    #[test]
    fn only_g7_link_says_who_is_at_the_pc() {
        assert!(Role::Client(ClientKind::Link, Caps::default()).sets_presence());
        for kind in [ClientKind::Launcher, ClientKind::Bridge, ClientKind::Cli, ClientKind::Test] {
            assert!(!Role::Client(kind, Caps::default()).sets_presence(), "{kind:?}");
        }
        assert!(!Role::Bridge { thread_id: "a".repeat(32), generation: 1 }.sets_presence());
    }

    #[tokio::test]
    async fn presence_from_anyone_but_g7_link_is_denied() {
        let dir = tempfile::tempdir().unwrap();
        let host = test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let params = serde_json::to_value(UiPresenceParams { at_pc: true, ..Default::default() }).unwrap();
        let err = host.ui_presence(false, 1, params.clone()).unwrap_err();
        assert_eq!(err.code, ErrorCode::Denied);
        assert_eq!(host.hub.presence_event(Instant::now()), PresenceEvent::default(), "nothing kept");
        host.ui_presence(true, 1, params).unwrap();
        assert!(host.hub.presence_event(Instant::now()).at_pc);
        let nine = (0..9).map(|n| phone(&n.to_string(), None)).collect();
        let nine = serde_json::to_value(UiPresenceParams { remote: nine, ..Default::default() }).unwrap();
        assert_eq!(host.ui_presence(true, 1, nine).unwrap_err().code, ErrorCode::BadRequest);
        assert!(host.hub.presence_event(Instant::now()).at_pc, "a refused one changes nothing");
    }

    #[test]
    fn a_thread_a_phone_shows_is_viewed_while_fresh_and_its_connection_is_open() {
        let hub = Hub::default();
        let t = "a".repeat(32);
        let t0 = Instant::now();
        assert!(!hub.viewed_at(&t, t0));
        let link = hub.test_viewer(ClientKind::Link, None);
        let presence = UiPresenceParams { remote: vec![phone("p", Some(&t))], ..Default::default() };
        hub.lock_presence().set(link, presence.clone(), t0);
        assert!(hub.viewed_at(&t, t0 + Duration::from_secs(59)));
        assert!(!hub.viewed_at(&"b".repeat(32), t0));
        assert!(!hub.viewed_at(&t, t0 + Duration::from_secs(61)), "stale");
        hub.lock_presence().set(link, presence, t0);
        hub.lock().remove(&link);
        assert!(hub.lock_presence().closed(link));
        assert!(!hub.viewed_at(&t, t0), "its connection closed");
    }

    #[test]
    fn the_presence_event_lists_views_newest_first_to_those_that_asked() {
        let hub = Hub::default();
        let ids: Vec<String> = (1..=6).map(|n| format!("{n:032x}")).collect();
        let launcher = hub.test_viewer(ClientKind::Launcher, None);
        let link = hub.test_viewer(ClientKind::Link, None);
        let quiet = hub.test_viewer(ClientKind::Launcher, None);
        let test = hub.test_viewer(ClientKind::Test, Some(&ids[5]));
        let view = |client: u64, id: &str| {
            let seq = hub.next_seq();
            hub.set(client, |s| {
                s.viewing = Some(id.to_owned());
                s.viewing_since = seq;
            });
        };
        view(launcher, &ids[0]);
        view(link, &ids[1]);
        view(quiet, &ids[1]);
        view(launcher, &ids[2]);
        let extra: Vec<u64> = (0..3).map(|_| hub.test_viewer(ClientKind::Link, None)).collect();
        for (client, id) in extra.iter().zip(&ids[3..5]) {
            view(*client, id);
        }
        view(extra[2], "not a thread id");
        let event = hub.presence_event(Instant::now());
        let newest = [&ids[4], &ids[3], &ids[2], &ids[1]].map(String::clone);
        assert_eq!(event.viewing, newest, "newest first, each once, at most 4");
        assert!(!event.viewing.contains(&ids[5]), "a test connection shows nobody anything");
        for client in [launcher, link, test] {
            hub.set(client, |s| s.presence = true);
        }
        hub.send_presence(&event, None);
        let line = Event::UiPresence(event.clone()).to_line();
        assert_eq!(hub.test_lines(launcher), std::slice::from_ref(&line));
        assert_eq!(hub.test_lines(link), [line]);
        assert!(hub.test_lines(quiet).is_empty(), "it didn't say presence");
        assert!(hub.test_lines(test).is_empty(), "not a launcher or G7 Link");
        hub.send_presence(&event, Some(link));
        assert!(hub.test_lines(launcher).is_empty());
        assert_eq!(hub.test_lines(link).len(), 1);
    }

    #[test]
    fn scan_progress_reaches_only_the_connections_that_scan() {
        let hub = Hub::default();
        let subscribed = hub.test_subscriber(Caps::default());
        let scanning = hub.test_viewer(ClientKind::Link, None);
        hub.set(scanning, |slot| slot.scans = true);
        let scanned =
            Event::SessionsScanned(SessionsScannedEvent { total: 12, scanning: true, partial: false });
        hub.broadcast(&scanned);
        let progress = Event::ImportProgress(ImportProgressEvent {
            job: "0123456789abcdef".into(),
            done: 1,
            failed: 0,
            total: 2,
            thread_id: None,
        });
        hub.broadcast(&progress);
        let cut = Event::ThreadCut(ThreadCutEvent {
            thread_id: "t".into(),
            from_seq: 3,
            to_seq: 9,
            restored: false,
        });
        hub.broadcast(&cut);
        assert_eq!(hub.test_lines(scanning), [scanned.to_line()], "unsubscribed, but it asked to scan");
        assert_eq!(hub.test_lines(subscribed), [progress.to_line(), cut.to_line()]);
    }
}
