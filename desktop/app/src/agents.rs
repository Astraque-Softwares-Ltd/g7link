// SPDX-License-Identifier: GPL-3.0-or-later
//! Agents on this PC: G7 Link connects to the agent host (`g7-agents`, a
//! private local socket), relays its threads to phones with the `agents`
//! toggle through the core (docs/protocol/agents.md), answers their
//! requests, and keeps what Settings shows of the host.
//!
//! The PC's own notifications about agents come from the host, never from
//! here. A folder's path never leaves the PC: phones get its id and label.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use g7_agents::{
    api::{
        API_VERSION, AgentId, AgentInfo, Approval, ApprovalOutcome, ClientKind, Decision, ErrorCode, Event,
        Folder, HelloParams, Item, Mode, Origin, OriginKind, ThreadInfo, ThreadReadParams, ThreadSelection,
        ThreadStartParams,
    },
    client::{self, Client, ClientError, Events},
};
use g7link_core::{
    AgentAsk, AgentCatalog, AgentCatalogAgent, AgentCatalogFolder, AgentEvent, AgentFailure, AgentItem,
    AgentItems, AgentReply, AgentRequest, AgentThread, AgentTool, DeviceId, Node, agent,
};
use serde::Serialize;
use tokio::sync::{Notify, watch};

use crate::{core_host, i18n::tr, state::Changes};

/// Longest wait between attempts to reach the host while it's wanted.
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How long a phone's request waits for the host to be reached.
const CONNECT_WAIT: Duration = Duration::from_secs(4);
/// How long a phone's request may take here; the core gives up at 15 s.
const REQUEST_LIMIT: Duration = Duration::from_secs(12);
/// Threads remembered for asks and finished turns (the host keeps at most 200).
const KNOWN_THREADS: usize = 200;

/// Whether the host can be reached, as Settings says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Nothing wants it yet.
    #[default]
    Idle,
    Connecting,
    Connected,
    /// Installed but not running (and nothing asked to start it).
    NotRunning,
    NotInstalled,
    Failed,
}

#[derive(Debug, Default)]
struct BridgeState {
    client: Option<Client>,
    status: Status,
    agents: Vec<AgentInfo>,
    folders: Vec<Folder>,
    threads: HashMap<String, ThreadInfo>,
    /// The user started the host from Settings, or added a folder.
    user_started: bool,
    /// Settings shows the Agents section.
    settings_open: bool,
}

#[derive(Debug)]
struct Bridge {
    state: Mutex<BridgeState>,
    wake: Notify,
    connected: watch::Sender<bool>,
}

impl Bridge {
    fn new() -> Self {
        Self { state: Mutex::default(), wake: Notify::new(), connected: watch::Sender::new(false) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BridgeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
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

    fn catalog(&self) -> AgentCatalog {
        let s = self.lock();
        catalog(&s.agents, &s.folders)
    }
}

fn bridge() -> &'static Bridge {
    static BRIDGE: OnceLock<Bridge> = OnceLock::new();
    BRIDGE.get_or_init(Bridge::new)
}

fn lock() -> std::sync::MutexGuard<'static, BridgeState> {
    bridge().lock()
}

fn changed() {
    if let Some(host) = core_host::try_host() {
        host.hub.update(|_| Changes::AGENTS);
    }
}

/// Whether the host binary is on this PC: G7 Link then offers `agent.host`.
pub fn host_installed() -> bool {
    client::find_host_binary().is_some()
}

/// Starts the bridge: it connects when a phone has `agents` on, or Settings
/// or a phone's request needs the host.
pub fn start() {
    core_host::spawn(run());
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
    }
}

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
        }),
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
    }
}

/// The protocol error for a failed host call (contract §6.1).
pub(crate) fn failure(e: &ClientError) -> AgentFailure {
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

// ---- The connection ----

/// What the host is wanted for right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    Nothing,
    /// Settings shows it: connect if it runs, but don't start it.
    Look,
    /// A phone may use it, or the user started it: start it if needed.
    Run,
}

/// Where the host's news goes, and what decides whether it's wanted: the
/// node, or a test.
trait Relay {
    fn want(&self) -> Want;
    async fn catalog(&self, catalog: Option<AgentCatalog>);
    async fn event(&self, event: AgentEvent, started_by: Option<DeviceId>);
    async fn ask(&self, ask: AgentAsk);
    async fn asked(&self, token: String, thread: String, outcome: String, by_name: Option<String>);
}

/// The node: phones get the host's news through the core.
struct NodeRelay(Node);

impl Relay for NodeRelay {
    fn want(&self) -> Want {
        let node = &self.0;
        let (user_started, settings_open) = {
            let s = lock();
            (s.user_started, s.settings_open)
        };
        let phones = node.paired_devices().unwrap_or_default().iter().any(|d| {
            node.device_toggles(d.id)
                .unwrap_or_default()
                .iter()
                .any(|(name, on)| *on && *name == agent::TOGGLE)
        });
        if user_started || phones {
            Want::Run
        } else if settings_open {
            Want::Look
        } else {
            Want::Nothing
        }
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

    async fn asked(&self, token: String, thread: String, outcome: String, by_name: Option<String>) {
        let _ = self.0.agent_asked(token, thread, outcome, by_name).await;
    }
}

fn hello() -> HelloParams {
    HelloParams { client: ClientKind::Link, version: API_VERSION, bridge_token: None }
}

async fn run() {
    let Some(node) = core_host::wait_for_node().await else { return };
    follow(bridge(), &NodeRelay(node), |want| async move {
        if want == Want::Run {
            Client::connect_or_spawn(ClientKind::Link).await
        } else {
            Client::connect(&client::socket_path(), hello()).await
        }
    })
    .await;
}

/// Keeps a connection to the host while `relay` wants one: reconnects after
/// 1, 2, 4 … 30 s, and tells phones there's nothing while it's away.
async fn follow<R, F, Fut>(bridge: &Bridge, relay: &R, connect: F)
where
    R: Relay,
    F: Fn(Want) -> Fut,
    Fut: Future<Output = Result<(Client, Events), ClientError>>,
{
    let mut backoff = Duration::from_secs(1);
    loop {
        let wanted = relay.want();
        if wanted == Want::Nothing {
            bridge.set_status(Status::Idle);
            bridge.wake.notified().await;
            continue;
        }
        bridge.set_status(Status::Connecting);
        match connect(wanted).await {
            Ok((client, events)) => {
                backoff = Duration::from_secs(1);
                serve(bridge, relay, client, events).await;
            }
            Err(e) => {
                let status = match &e {
                    ClientError::NotInstalled => Status::NotInstalled,
                    ClientError::Io(_) if wanted == Want::Look => Status::NotRunning,
                    _ => Status::Failed,
                };
                tracing::info!(error = %e, "can't reach the agent host");
                bridge.set_status(status);
                relay.catalog(None).await;
                if wanted == Want::Look || status == Status::NotInstalled {
                    bridge.wake.notified().await;
                } else {
                    let _ = tokio::time::timeout(backoff, bridge.wake.notified()).await;
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                }
            }
        }
    }
}

/// Follows the host until the connection closes or nothing wants it.
async fn serve<R: Relay>(bridge: &Bridge, relay: &R, client: Client, mut events: Events) {
    let seeded = async {
        client.subscribe(ThreadSelection::All, true).await?;
        let agents = client.agents_list(false).await?;
        let folders = client.folders_list().await?;
        let threads = client.threads_list(Some(100), None).await.unwrap_or_default();
        Ok::<_, ClientError>((agents, folders, threads))
    };
    let (agents, folders, threads) = match seeded.await {
        Ok(seeded) => seeded,
        Err(e) => {
            tracing::warn!(error = %e, "the agent host didn't answer");
            bridge.set_status(Status::Failed);
            return;
        }
    };
    {
        let mut s = bridge.lock();
        s.client = Some(client.clone());
        s.status = Status::Connected;
        s.agents = agents;
        s.folders = folders;
        s.threads = threads.into_iter().map(|t| (t.id.clone(), t)).collect();
    }
    bridge.connected.send_replace(true);
    changed();
    relay.catalog(Some(bridge.catalog())).await;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(event) => relay_event(bridge, relay, &client, event).await,
                None => break,
            },
            () = bridge.wake.notified() => {
                if relay.want() == Want::Nothing {
                    break;
                }
            }
        }
    }
    {
        let mut s = bridge.lock();
        s.client = None;
        s.threads.clear();
    }
    bridge.connected.send_replace(false);
    bridge.set_status(Status::NotRunning);
    relay.catalog(None).await;
}

async fn relay_event<R: Relay>(bridge: &Bridge, relay: &R, client: &Client, event: Event) {
    match event {
        Event::ThreadState(e) => {
            bridge.remember(&e.thread);
            relay.event(AgentEvent::state(thread(&e.thread, None)), started_by(&e.thread)).await;
        }
        Event::ThreadItem(e) => relay.event(AgentEvent::item(e.thread_id, item(&e.item)), None).await,
        Event::ThreadDelta(e) => {
            relay.event(AgentEvent::delta(e.thread_id, e.seq, e.item_id, e.offset, e.text), None).await;
        }
        Event::ThreadDone(e) => {
            if let Some(info) = bridge.known(&e.thread_id) {
                let outcome = word(&e.outcome);
                relay
                    .event(AgentEvent::done(thread(&info, None), &outcome, e.summary), started_by(&info))
                    .await;
            }
        }
        Event::ThreadRemoved(e) => {
            bridge.lock().threads.remove(&e.thread_id);
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
            relay.ask(ask(&e.approval, info.as_ref())).await;
        }
        Event::ApprovalResolved(e) => {
            let by_name = e.by.as_ref().and_then(|by| answered_by(by, this_pc_name));
            relay.asked(e.token, e.thread_id, asked_outcome(e.outcome), by_name).await;
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
        // For the launcher only.
        Event::UiOpen(_) => {}
    }
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
/// called on a blocking thread, after the core checked the toggle, the
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
        AgentRequest::Answer { token, allow } => {
            let decision = if allow { Decision::Allow } else { Decision::Deny };
            client.approval_answer(token, decision, origin()).await.map_err(fail)?;
            AgentReply::Ok
        }
    })
}

// ---- Settings ----

/// What Settings shows, as JSON: `{ status, agents: [...], folders: [...] }`.
/// Only Settings sees the plan, the version and folder paths.
pub fn settings_json() -> String {
    let s = lock();
    let agents: Vec<_> = s
        .agents
        .iter()
        .map(|a| {
            serde_json::json!({
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
            serde_json::json!({
                "id": f.id,
                "label": f.label,
                "path": f.path,
                "shownPath": shown_path(&f.path, dirs::home_dir().as_deref()),
                "exists": f.exists,
            })
        })
        .collect();
    serde_json::json!({ "status": word(&s.status), "agents": agents, "folders": folders }).to_string()
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

/// Settings shows the Agents section (or stopped): it looks for a running
/// host, and asks the agents again.
pub fn settings_open(open: bool) {
    let refresh = {
        let mut s = lock();
        s.settings_open = open;
        open.then(|| s.client.clone()).flatten()
    };
    wake();
    if let Some(client) = refresh {
        core_host::spawn(async move {
            if let Ok(agents) = client.agents_list(true).await {
                lock().agents = agents;
                changed();
            }
        });
    }
}

/// The user asked to start the host from Settings.
pub fn start_host() {
    lock().user_started = true;
    wake();
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
        let Some(client) = connected_client().await else {
            crate::bridge::app::show_message(tr!("Agents", "Couldn't reach the agent host"));
            return;
        };
        match client.folders_add(path, None).await {
            Ok(folder) => {
                let mut s = lock();
                if !s.folders.iter().any(|f| f.id == folder.id) {
                    s.folders.push(folder);
                }
                drop(s);
                changed();
            }
            Err(e) => crate::bridge::app::show_message(folder_error(&e)),
        }
    });
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

#[cfg(test)]
mod tests {
    use g7_agents::api::{ApprovalKind, ItemKind, State, ThreadError, Tool, ToolStatus};

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
        }
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
        assert_eq!(error_code("usageLimitExceeded"), "usagelimitexceeded");
        assert_eq!(error_code("AGENT-EXITED"), "agent_exited");
        assert!(wire.is_valid());
    }

    #[test]
    fn items_and_asks_map_to_wire_words() {
        let tool = Item {
            seq: 4,
            id: "toolu_01".into(),
            kind: ItemKind::Tool,
            ts_ms: 9,
            rev: 1,
            text: None,
            truncated: false,
            origin: None,
            tool: Some(Tool {
                name: "Bash".into(),
                title: "Run command".into(),
                detail: Some("ls".into()),
                status: ToolStatus::Denied,
                output: None,
            }),
        };
        let wire = item(&tool);
        assert_eq!((wire.kind.as_str(), wire.tool.as_ref().unwrap().status.as_str()), ("tool", "denied"));
        assert!(wire.is_valid());

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
        };
        let thread_info = info(Origin::launcher());
        let wire = ask(&approval, Some(&thread_info));
        assert_eq!(
            (wire.kind.as_str(), wire.agent.as_str(), wire.thread_title.as_str()),
            ("edit", "codex", "Fix the build")
        );
        assert_eq!(wire.expires, 7_200_001);
        assert!(wire.is_valid());
        assert_eq!(asked_outcome(ApprovalOutcome::Cancelled), "cancelled");
        let pc = || "guchu".to_owned();
        assert_eq!(answered_by(&Origin::phone("abc", "Pixel"), pc).as_deref(), Some("Pixel"));
        assert_eq!(answered_by(&Origin::launcher(), pc).as_deref(), Some("guchu"));
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

    /// A host on a socket in a test folder, speaking §2 enough for the bridge:
    /// it answers, sends one event after `threads.list`, and drops the
    /// connection when told to.
    async fn fake_host(path: std::path::PathBuf, close: std::sync::Arc<Notify>) {
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
                        let result = match method.as_str() {
                            "hello" => serde_json::json!({ "version": 1, "host_version": "test", "pid": 1 }),
                            "agents.list" => serde_json::json!({ "agents": [AgentInfo::missing(AgentId::Claude)] }),
                            "folders.list" => serde_json::json!({ "folders": [folder("f0123456789ab", true)] }),
                            "threads.list" => serde_json::json!({ "threads": [] }),
                            _ => serde_json::json!({}),
                        };
                        let reply = serde_json::json!({ "id": request["id"], "result": result });
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

    #[derive(Debug, Default)]
    struct TestRelay {
        log: Mutex<Vec<String>>,
    }

    impl TestRelay {
        fn count(&self, what: &str) -> usize {
            self.log.lock().unwrap().iter().filter(|l| *l == what).count()
        }
    }

    impl Relay for TestRelay {
        fn want(&self) -> Want {
            Want::Run
        }
        async fn catalog(&self, catalog: Option<AgentCatalog>) {
            let line = catalog.map_or("catalog none".to_owned(), |c| format!("catalog {}", c.folders.len()));
            self.log.lock().unwrap().push(line);
        }
        async fn event(&self, event: AgentEvent, _started_by: Option<DeviceId>) {
            self.log.lock().unwrap().push(format!("event {}", event.kind));
        }
        async fn ask(&self, _ask: AgentAsk) {}
        async fn asked(&self, _token: String, _thread: String, _outcome: String, _by_name: Option<String>) {}
    }

    async fn until(what: &str, done: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_bridge_follows_the_host_and_reconnects() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        // The client only trusts a socket in a folder no one else can enter.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("host.sock");
        let close = std::sync::Arc::new(Notify::new());
        let server = tokio::spawn(fake_host(path.clone(), close.clone()));
        let bridge: &'static Bridge = Box::leak(Box::new(Bridge::new()));
        let relay: &'static TestRelay = Box::leak(Box::default());
        let socket = path.clone();
        let follower = tokio::spawn(follow(bridge, relay, move |_| {
            let socket = socket.clone();
            async move { Client::connect(&socket, hello()).await }
        }));

        until("connected", || relay.count("catalog 1") == 1 && relay.count("event removed") == 1).await;
        assert_eq!(bridge.lock().status, Status::Connected);
        assert_eq!(bridge.lock().folders[0].path, "/home/me/Code/g7linux", "Settings keeps the path");

        // The host goes away: phones hear there's nothing, and the bridge comes back.
        close.notify_one();
        until("told phones", || relay.count("catalog none") == 1).await;
        until("reconnected", || relay.count("catalog 1") == 2).await;
        assert_eq!(bridge.lock().status, Status::Connected);
        follower.abort();
        server.abort();
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
    fn settings_show_paths_and_plans_but_phones_never_do() {
        {
            let mut s = lock();
            let mut claude = AgentInfo::missing(AgentId::Claude);
            claude.plan = Some("max".into());
            s.agents = vec![claude];
            s.folders = vec![folder("f1", true)];
        }
        let json: serde_json::Value = serde_json::from_str(&settings_json()).unwrap();
        assert_eq!(json["agents"][0]["plan"], "max");
        assert_eq!(json["folders"][0]["path"], "/home/me/Code/g7linux");
        assert_eq!(json["status"], "idle");
        let mut s = lock();
        s.agents.clear();
        s.folders.clear();
    }
}
