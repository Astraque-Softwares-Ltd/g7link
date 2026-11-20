// SPDX-License-Identifier: MPL-2.0
//! The agent host: accepts clients on the private socket (§2), checks each
//! peer is this user, dispatches requests, and fans events out to
//! subscribers.

pub mod agents;
pub mod bridge;
pub mod claude;
pub mod codex;
pub mod env;
pub mod export;
pub mod notify;
pub mod sessions;
pub mod store;
pub mod threads;

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

struct ClientSlot {
    kind: ClientKind,
    outbox: Arc<Outbox>,
    subscription: Option<Subscription>,
    viewing: Option<String>,
    viewing_session: Option<String>,
    /// The one session whose items this connection gets.
    watching: Option<String>,
}

impl ClientSlot {
    fn new(kind: ClientKind, outbox: Arc<Outbox>) -> Self {
        Self { kind, outbox, subscription: None, viewing: None, viewing_session: None, watching: None }
    }

    /// A launcher or G7 Link: a client that shows threads to a person.
    fn shows_threads(&self) -> bool {
        matches!(self.kind, ClientKind::Launcher | ClientKind::Link)
    }
}

/// Connected clients and what each wants to hear.
#[derive(Default)]
pub struct Hub {
    clients: Mutex<HashMap<u64, ClientSlot>>,
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
    /// the connections watching that session.
    pub fn broadcast(&self, event: &Event) {
        let line = event.to_line();
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
        let delta = matches!(event, Event::ThreadDelta(_));
        let thread = event.thread_id();
        for slot in self.lock().values() {
            let Some(sub) = &slot.subscription else { continue };
            if delta && !sub.deltas {
                continue;
            }
            if let Some(thread) = thread
                && !sub.threads.contains(thread)
            {
                continue;
            }
            slot.outbox.push(line.clone(), delta);
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

    /// A launcher or G7 Link says its window shows this thread to the user
    /// right now (`ui.viewing`).
    pub fn viewed(&self, thread_id: &str) -> bool {
        self.lock().values().any(|s| s.shows_threads() && s.viewing.as_deref() == Some(thread_id))
    }

    /// A connection of `kind` that views `thread_id`; its id.
    #[cfg(test)]
    pub(crate) fn test_viewer(&self, kind: ClientKind, thread_id: Option<&str>) -> u64 {
        let mut clients = self.lock();
        let id = clients.keys().max().map_or(1, |n| n + 1);
        let mut slot = ClientSlot::new(kind, Arc::new(Outbox::new()));
        slot.viewing = thread_id.map(str::to_owned);
        clients.insert(id, slot);
        id
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

// ---- The host ----

/// Probe results, refreshed at most every [`PROBE_CACHE_MS`].
#[derive(Debug)]
pub struct ProbeCache {
    pub at: Option<Instant>,
    pub claude: Option<Probe>,
    pub codex: Option<Probe>,
    pub shown: Option<Vec<AgentInfo>>,
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
    fn lock_reg(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.reg.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Marks activity for the idle exit.
    pub fn touch(&self) {
        *self.idle_since.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    /// Asks `serve` to stop.
    pub fn stop(&self) {
        self.shutdown.notify_one();
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
    let _ = std::fs::remove_file(&config.socket);
    let listener = UnixListener::bind(&config.socket)?;
    std::fs::set_permissions(&config.socket, std::fs::Permissions::from_mode(0o600))?;

    let mut store = Store::open(config.data_dir.clone())?;
    let registry = Registry::load(&mut store, config.approval_lifetime_ms);
    let settings = store.load_settings();
    let store = StoreHandle::spawn(store)?;
    let (events_tx, mut events_rx) = mpsc::unbounded_channel::<(String, u64, AgentEvent)>();
    let (actions_tx, mut actions_rx) = mpsc::unbounded_channel::<NoteAction>();
    let notifier = config.notify.then(|| Notifier::spawn(actions_tx));
    let host = Arc::new(Host {
        hub: Hub::default(),
        store,
        reg: Mutex::new(registry),
        probes: tokio::sync::Mutex::new(ProbeCache { at: None, claude: None, codex: None, shown: None }),
        folders_lock: tokio::sync::Mutex::new(()),
        events: events_tx,
        notifier,
        hostname: hostname(),
        sessions: sessions::Sessions::new(&config),
        clients: AtomicUsize::new(0),
        next_client: AtomicU64::new(1),
        idle_since: Mutex::new(Instant::now()),
        shutdown: Notify::new(),
        config,
    });
    host.sessions.set_following_loaded(settings.follow_sessions);
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
            loop {
                tokio::time::sleep(Duration::from_secs(5).min(host.config.idle_exit)).await;
                host.check_folders();
                let busy = host.clients.load(Ordering::SeqCst) > 0 || host.any_running();
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
    let _ = std::fs::remove_file(&host.config.socket);
    host.sessions.stop();
    host.shutdown_threads().await;
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

/// What a connection is after `hello`.
#[derive(Debug, Clone)]
enum Role {
    Client(ClientKind),
    Bridge { thread_id: String, generation: u64 },
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
                        role = Some(r);
                        continue;
                    }
                    None => break,
                }
            };
            // Connection state changes run in order; everything else may overlap.
            match (&current, request.method.as_str()) {
                (Role::Client(_), method::SUBSCRIBE) => {
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
                (Role::Client(_), method::SESSION_WATCH) => {
                    let result = match parse::<SessionWatchParams>(request.params) {
                        Ok(p) => self.session_watch(client_id, p.session_id).await.map(|()| json!({})),
                        Err(e) => Err(e),
                    };
                    Self::reply(&outbox, request.id, result);
                }
                (Role::Client(_), method::UNSUBSCRIBE) => {
                    self.hub.set(client_id, |slot| slot.subscription = None);
                    Self::reply(&outbox, request.id, Ok(json!({})));
                }
                (Role::Client(_), method::UI_VIEWING) => {
                    let result = parse::<UiViewingParams>(request.params).map(|p| {
                        self.hub.set(client_id, |slot| {
                            slot.viewing = p.thread_id;
                            slot.viewing_session = p.session_id.filter(|id| is_valid_session_id(id));
                        });
                        json!({})
                    });
                    Self::reply(&outbox, request.id, result);
                }
                _ => {
                    let Ok(permit) = in_flight.clone().acquire_owned().await else { break };
                    let host = self.clone();
                    let outbox = outbox.clone();
                    tokio::spawn(async move {
                        let result = host.dispatch(&current, request.method.as_str(), request.params).await;
                        Self::reply(&outbox, request.id, result);
                        drop(permit);
                    });
                }
            }
        }
        outbox.close();
        self.hub.lock().remove(&client_id);
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
            (None, kind) => Role::Client(kind),
        };
        let kind = match &role {
            Role::Client(kind) => *kind,
            Role::Bridge { .. } => ClientKind::Bridge,
        };
        self.hub.lock().insert(client_id, ClientSlot::new(kind, outbox.clone()));
        let result = HelloResult {
            version: API_VERSION,
            host_version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
        };
        Self::reply(outbox, request.id, to_value(&result));
        Some(role)
    }

    async fn dispatch(
        self: &Arc<Self>,
        role: &Role,
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
                to_value(&ThreadsListResult { threads: self.threads_list(p) })
            }
            method::THREAD_START => {
                let p: ThreadStartParams = parse(params)?;
                let thread = self.thread_start(p).await?;
                to_value(&ThreadResult { thread })
            }
            method::THREAD_SEND => {
                let p: ThreadSendParams = parse(params)?;
                self.thread_send(p).await?;
                Ok(json!({}))
            }
            method::THREAD_INTERRUPT => {
                let p: ThreadIdParams = parse(params)?;
                self.thread_interrupt(&p.thread_id)?;
                Ok(json!({}))
            }
            method::THREAD_READ => {
                let p: ThreadReadParams = parse(params)?;
                to_value(&self.thread_read(p).await?)
            }
            method::THREAD_DELETE => {
                let p: ThreadIdParams = parse(params)?;
                self.thread_delete(&p.thread_id)?;
                Ok(json!({}))
            }
            method::THREAD_TERMINAL_COMMAND => {
                let p: ThreadIdParams = parse(params)?;
                to_value(&self.thread_terminal_command(&p.thread_id)?)
            }
            method::APPROVAL_ANSWER => {
                let p: ApprovalAnswerParams = parse(params)?;
                self.approval_answer(&p.token, p.decision, p.by)?;
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
                to_value(&ThreadResult { thread })
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
            method::HELLO => Err(RpcError::new(ErrorCode::BadRequest, "already said hello")),
            _ => Err(RpcError::new(ErrorCode::BadRequest, UNKNOWN_METHOD)),
        }
    }

    fn on_note_action(self: &Arc<Self>, action: NoteAction) {
        match action {
            NoteAction::Answer { token, allow } => {
                let decision = if allow { Decision::Allow } else { Decision::Deny };
                if let Err(e) = self.approval_answer(&token, decision, Origin::launcher()) {
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
    test_host_with(dir, approval_lifetime_ms, None)
}

/// [`test_host`] whose notifications land in the receiver.
#[cfg(test)]
pub(crate) fn test_host_noting(
    dir: &std::path::Path,
    approval_lifetime_ms: u64,
) -> (Arc<Host>, mpsc::UnboundedReceiver<notify::Note>) {
    let (notifier, notes) = Notifier::recording();
    (test_host_with(dir, approval_lifetime_ms, Some(notifier)), notes)
}

#[cfg(test)]
fn test_host_with(dir: &std::path::Path, approval_lifetime_ms: u64, notifier: Option<Notifier>) -> Arc<Host> {
    let config = Config {
        socket: dir.join("host.sock"),
        runtime_dir: dir.to_owned(),
        data_dir: dir.join("data"),
        claude_bin: None,
        codex_bin: None,
        bridge_exe: PathBuf::from("/bin/false"),
        notify: false,
        env: Vec::new(),
        idle_exit: IDLE_EXIT,
        agent_idle: Duration::from_millis(AGENT_IDLE_MS),
        approval_lifetime_ms,
        claude_home: dir.join("claude"),
        codex_home: dir.join("codex"),
        proc_root: dir.join("proc"),
        sessions: true,
    };
    let mut store = Store::open(config.data_dir.clone()).unwrap();
    let registry = Registry::load(&mut store, approval_lifetime_ms);
    let (events, _) = mpsc::unbounded_channel();
    Arc::new(Host {
        hub: Hub::default(),
        store: StoreHandle::spawn(store).unwrap(),
        reg: Mutex::new(registry),
        probes: tokio::sync::Mutex::new(ProbeCache { at: None, claude: None, codex: None, shown: None }),
        folders_lock: tokio::sync::Mutex::new(()),
        events,
        notifier,
        hostname: "test-pc".into(),
        sessions: sessions::Sessions::new(&config),
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
}
