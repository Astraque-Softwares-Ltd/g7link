// SPDX-License-Identifier: MPL-2.0
//! History and undo of a PC's agent threads, from a phone
//! (`docs/protocol/agents.md` §6): two real nodes over localhost, a PC whose
//! platform plays the agent host, and a phone with its saved threads and
//! held messages.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use g7link_core::{
    AgentFailure, AgentReply, AgentRequest, AgentSend, AgentThread, DeviceId, DeviceInfo, DeviceKind, Error,
    LinkState, Node, NodeConfig, NodeEvent, PairingEvent, PlainKeyProtector, Platform, PowerLevel, agent,
    agent_history::{
        AgentCut, AgentFound, AgentFoundList, AgentFoundRequest, AgentHit, AgentHits, AgentImportJob,
        AgentImportProgress, AgentMark, AgentPatchPage, AgentRewindPlan, AgentSearchRequest,
        AgentThreadsRequest, AgentTurn, AgentTurnFile, AgentTurnFiles, HeldState, HeldWhy, TurnRange,
        history,
    },
};
use tempfile::TempDir;
use tokio::sync::broadcast::Receiver;

const WAIT: Duration = Duration::from_secs(20);
const FOLDER: &str = "f0123456789ab";
const THREAD: &str = "0123456789abcdef0123456789abcdef";
/// A thread waiting for an answer to a request.
const WAITING: &str = "aaaa0000bbbb1111cccc2222dddd3333";
/// A thread with a turn running.
const RUNNING: &str = "eeee4444ffff55556666777788889999";
/// A thread the PC no longer has.
const GONE: &str = "99998888777766665555444433332222";
const FOUND: &str = "sc0123456789abcdef0123456789abcdef";
const JOB: &str = "00112233445566ff";
const TOKEN: &str = "ffff0000ffff0000ffff0000ffff0000";
/// A path whose patch never ends: the PC stops at 1 MiB.
const ENDLESS: &str = "big/endless.txt";
const PAGE: usize = 196_608;

/// A PC's agent host, as far as these tests need one.
#[derive(Debug, Default)]
struct HostPlatform {
    requests: Mutex<Vec<AgentRequest>>,
    /// Keys taken by sends: one sent again is a duplicate.
    keys: Mutex<HashSet<String>>,
    /// What the host sent the agent: (thread, text, dispatch).
    sent: Mutex<Vec<(String, String, Option<String>)>>,
    /// Threads by id, as `agent.read` answers them.
    threads: Mutex<HashMap<String, AgentThread>>,
}

fn thread(id: &str, state: &str) -> AgentThread {
    AgentThread {
        id: id.into(),
        agent: "claude".into(),
        folder: "g7linux".into(),
        title: "Fix the login test".into(),
        state: state.into(),
        mode: "ask".into(),
        updated: 1_795_000_000_000,
        ..Default::default()
    }
}

fn item(seq: u64, text: &str) -> g7link_core::AgentItem {
    g7link_core::AgentItem {
        seq,
        id: format!("m:{seq}"),
        kind: "assistant".into(),
        when: 1_795_000_000_000,
        text: Some(text.into()),
        turn: Some(1),
        ..Default::default()
    }
}

fn turn(n: u32) -> AgentTurn {
    AgentTurn {
        turn: n,
        prompt_seq: u64::from(n) * 10,
        started_ms: 1_795_000_000_000,
        checkpoint: Some(history::checkpoint::READY.into()),
        files: 2,
        adds: 5,
        dels: 1,
        can_fork: true,
        can_rewind: true,
        ..Default::default()
    }
}

impl HostPlatform {
    fn with_threads() -> Self {
        let host = Self::default();
        {
            let mut threads = host.threads.lock().unwrap();
            threads.insert(THREAD.into(), thread(THREAD, "done"));
            let mut waiting = thread(WAITING, "waiting");
            waiting.asks = 1;
            threads.insert(WAITING.into(), waiting);
            threads.insert(RUNNING.into(), thread(RUNNING, "working"));
        }
        host
    }

    fn requests(&self) -> Vec<AgentRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn sent(&self) -> Vec<(String, String, Option<String>)> {
        self.sent.lock().unwrap().clone()
    }
}

impl Platform for HostPlatform {
    fn agent_request(&self, _from: &DeviceId, request: AgentRequest) -> Result<AgentReply, AgentFailure> {
        self.requests.lock().unwrap().push(request.clone());
        let known = |id: &str| self.threads.lock().unwrap().get(id).cloned().ok_or(AgentFailure::NotFound);
        Ok(match request {
            AgentRequest::Threads | AgentRequest::ThreadsPage { .. } => {
                let mut threads: Vec<AgentThread> = self.threads.lock().unwrap().values().cloned().collect();
                threads.sort_by(|a, b| a.id.cmp(&b.id));
                AgentReply::Threads(threads)
            }
            AgentRequest::Read { thread, .. } => AgentReply::Items(g7link_core::AgentItems {
                thread: known(&thread)?,
                items: vec![item(1, "Hello"), item(2, "Done.")],
                ..Default::default()
            }),
            AgentRequest::Send { thread, text, dispatch, key, .. } => {
                known(&thread)?;
                if let Some(key) = key
                    && !self.keys.lock().unwrap().insert(key)
                {
                    return Ok(AgentReply::Duplicate);
                }
                self.sent.lock().unwrap().push((thread, text, dispatch));
                AgentReply::Ok
            }
            AgentRequest::Turns { .. } => AgentReply::Turns(vec![turn(2), turn(1)]),
            AgentRequest::DiffFiles { .. } => AgentReply::TurnFiles(AgentTurnFiles {
                files: vec![
                    AgentTurnFile { path: "src/login.rs".into(), adds: 4, dels: 1, ..Default::default() },
                    AgentTurnFile { path: "/home/me/.ssh/id_ed25519".into(), ..Default::default() },
                    AgentTurnFile { path: "../../etc/passwd".into(), ..Default::default() },
                ],
                more: false,
                adds: 4,
                dels: 1,
            }),
            AgentRequest::DiffPatch { path, offset, .. } => {
                let at = offset.unwrap_or(0);
                if path == ENDLESS {
                    AgentReply::Patch(AgentPatchPage {
                        patch: format!("+{}\n", "x".repeat(PAGE - 2)),
                        next: Some(at + PAGE as u64),
                        ..Default::default()
                    })
                } else if at == 0 {
                    AgentReply::Patch(AgentPatchPage {
                        patch: "@@ -1 +1 @@\n".into(),
                        next: Some(12),
                        ..Default::default()
                    })
                } else {
                    AgentReply::Patch(AgentPatchPage { patch: "-a\n+b\n".into(), ..Default::default() })
                }
            }
            AgentRequest::Search { .. } => AgentReply::Hits(AgentHits {
                hits: vec![AgentHit {
                    thread: THREAD.into(),
                    title: "Fix the login test".into(),
                    agent: "claude".into(),
                    folder: "/home/me/g7linux".into(),
                    state: "done".into(),
                    seq: 2,
                    kind: Some("assistant".into()),
                    when: 1,
                    snippet: "the\nlogin test".into(),
                    marks: vec![(4, 5)],
                    ..Default::default()
                }],
                next: Some("c2".into()),
                titles_only: false,
            }),
            AgentRequest::Mark { threads, pinned, .. } => AgentReply::Threads(
                threads
                    .iter()
                    .filter_map(|id| known(id).ok())
                    .map(|mut t| {
                        t.pinned = pinned.unwrap_or(t.pinned);
                        t
                    })
                    .collect(),
            ),
            AgentRequest::Rename { thread, title } => {
                let mut t = known(&thread)?;
                t.title = title;
                AgentReply::Thread(t)
            }
            AgentRequest::Fork { thread, turn } => {
                let mut t = known(&thread)?;
                t.id = "f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0".into();
                t.forked_from = Some(g7link_core::agent_history::AgentForkedFrom {
                    thread,
                    turn,
                    title: t.title.clone(),
                });
                AgentReply::Thread(t)
            }
            AgentRequest::RewindPreview { thread, .. } => {
                known(&thread)?;
                AgentReply::RewindPlan(AgentRewindPlan {
                    token: TOKEN.into(),
                    expires_ms: 1_795_000_120_000,
                    turns: 2,
                    messages: 7,
                    blocked: None,
                })
            }
            AgentRequest::RewindApply { token, .. } if token == TOKEN => {
                let mut t = known(THREAD)?;
                t.undo =
                    Some(g7link_core::agent_history::AgentUndo { until_ms: 1_795_086_400_000, files: false });
                AgentReply::Thread(t)
            }
            AgentRequest::RewindApply { .. } => return Err(AgentFailure::NotFound),
            AgentRequest::RewindUndo { thread } => AgentReply::Thread(known(&thread)?),
            AgentRequest::Found { .. } => AgentReply::Found(AgentFoundList {
                found: vec![AgentFound {
                    id: FOUND.into(),
                    agent: "codex".into(),
                    title: "Old work".into(),
                    folder: "/home/me/src/g7linux".into(),
                    updated_ms: 1,
                    bytes: 2_000,
                    packed: true,
                    source: "terminal".into(),
                    ..Default::default()
                }],
                next_ms: None,
                total: 1,
                scanning: true,
                partial: false,
            }),
            AgentRequest::FoundPreview { .. } => AgentReply::Items(g7link_core::AgentItems {
                items: vec![item(0, "Earlier"), item(64, "Later")],
                ..Default::default()
            }),
            AgentRequest::Import { ids } => {
                AgentReply::Job(AgentImportJob { job: JOB.into(), total: ids.len() as u32 })
            }
            AgentRequest::ImportCancel { .. } => AgentReply::Ok,
            _ => return Err(AgentFailure::Unsupported),
        })
    }
}

struct Device {
    node: Node,
    events: Receiver<NodeEvent>,
    host: Arc<HostPlatform>,
    dir: TempDir,
    name: &'static str,
}

fn info(name: &str, kind: DeviceKind) -> DeviceInfo {
    DeviceInfo {
        name: name.into(),
        kind,
        os: if kind == DeviceKind::Phone { "android" } else { "linux" }.into(),
        os_ver: "test".into(),
        model: None,
        accent: None,
        screen: None,
        install_id: None,
    }
}

async fn start_in(dir: TempDir, name: &'static str, kind: DeviceKind, offers: &[&str]) -> Device {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let mut config = NodeConfig::new(dir.path(), info(name, kind), "0.0.1-test");
    config.lan_discovery = false;
    config.away_mode = false;
    config.key_protector = Some(Arc::new(PlainKeyProtector));
    let host = Arc::new(HostPlatform::with_threads());
    let node = Node::start(config, host.clone()).await.expect("node starts");
    node.update_power(PowerLevel::NotApplicable, offers.iter().map(|c| (*c).to_owned()).collect()).await;
    let events = node.events();
    Device { node, events, host, dir, name }
}

async fn wait_for<T>(dev: &mut Device, what: &str, mut pick: impl FnMut(&NodeEvent) -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, dev.events.recv()).await {
            Ok(Ok(event)) => {
                if let Some(found) = pick(&event) {
                    return found;
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(e)) => panic!("{}: event stream ended while waiting for {what}: {e}", dev.name),
            Err(_) => panic!("{}: timed out waiting for {what}", dev.name),
        }
    }
}

async fn wait_link(dev: &mut Device, peer: DeviceId, online: bool) {
    wait_for(dev, if online { "peer online" } else { "peer offline" }, |e| match e {
        NodeEvent::LinkChanged { device, link } if *device == peer => {
            (matches!(link, LinkState::Online { .. }) == online).then_some(())
        }
        _ => None,
    })
    .await
}

fn catalog() -> g7link_core::AgentCatalog {
    g7link_core::AgentCatalog {
        agents: vec![g7link_core::AgentCatalogAgent {
            id: "claude".into(),
            name: "Claude".into(),
            installed: true,
            signed_in: true,
            login_hint: "claude auth login".into(),
            ..Default::default()
        }],
        folders: vec![g7link_core::AgentCatalogFolder {
            id: FOLDER.into(),
            label: "g7linux".into(),
            checkpoints: true,
            follow_remote: false,
            ..Default::default()
        }],
        push: false,
        add_folders: false,
    }
}

async fn wait_catalog(phone: &mut Device, pc: DeviceId) {
    wait_for(phone, "agent catalog", |e| match e {
        NodeEvent::AgentCatalog { device, catalog: Some(_), .. } if *device == pc => Some(()),
        _ => None,
    })
    .await
}

/// A PC with the agent host (and §5, and sessions) paired with a phone that
/// follows its agents, with the `agents` toggle on (and `agent_sessions`
/// with `sessions`).
async fn pair(sessions: bool) -> (Device, Device) {
    let pc_caps = [agent::HOST, agent::CONTROL, agent::SESSIONS];
    let mut pc = start_in(tempfile::tempdir().unwrap(), "Desktop", DeviceKind::Desktop, &pc_caps).await;
    let phone_caps = [agent::ITEMS2, agent::CONTROL, agent::FOLLOW];
    let mut phone = start_in(tempfile::tempdir().unwrap(), "Pixel", DeviceKind::Phone, &phone_caps).await;
    pc.node.set_agent_catalog(Some(catalog())).await.unwrap();
    let uri = pc.node.pairing_start_qr().await.unwrap();
    tokio::time::timeout(WAIT, phone.node.pairing_join(&uri.to_uri())).await.unwrap().expect("paired");
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    wait_for(&mut pc, "paired", |e| match e {
        NodeEvent::Pairing(PairingEvent::Paired(d)) if d.id == phone_id => Some(()),
        _ => None,
    })
    .await;
    wait_link(&mut phone, pc_id, true).await;
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, true).unwrap();
    if sessions {
        pc.node.set_device_toggle(phone_id, agent::SESSIONS_TOGGLE, true).unwrap();
    }
    wait_catalog(&mut phone, pc_id).await;
    (pc, phone)
}

async fn timed<T>(what: &str, fut: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, fut).await.unwrap_or_else(|_| panic!("timed out: {what}"))
}

/// Takes the phone off the PC, and waits until both say so.
async fn go_offline(pc: &mut Device, phone: &mut Device) {
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    phone.node.set_reconnect(pc_id, false).unwrap();
    wait_link(pc, phone_id, false).await;
}

/// Brings the phone back, and waits for the PC's catalog (held messages go then).
async fn come_back(pc: &Device, phone: &mut Device) {
    let pc_id = pc.node.device_id();
    phone.node.set_reconnect(pc_id, true).unwrap();
    wait_catalog(phone, pc_id).await;
}

/// Waits until the phone's held messages for `pc` are as `done` says.
async fn wait_outbox(
    phone: &mut Device,
    pc: DeviceId,
    what: &str,
    done: impl Fn(&[g7link_core::AgentHeld]) -> bool,
) {
    if done(&phone.node.agent_outbox(pc).await.unwrap()) {
        return;
    }
    wait_for(phone, what, |e| match e {
        NodeEvent::AgentOutbox { device, held } if *device == pc && done(held) => Some(()),
        _ => None,
    })
    .await
}

fn send(thread: &str, text: &str) -> AgentSend {
    AgentSend { thread: thread.into(), text: text.into(), ..Default::default() }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_request_reaches_the_host_and_its_answer_fits_the_phone() {
    let (pc, phone) = pair(true).await;
    let pc_id = pc.node.device_id();

    let turns = timed("turns", phone.node.agent_turns(pc_id, THREAD.into(), None, Some(10))).await.unwrap();
    assert_eq!(turns.iter().map(|t| t.turn).collect::<Vec<_>>(), [2, 1]);

    let files =
        timed("files", phone.node.agent_diff_files(pc_id, THREAD.into(), TurnRange::Turn(2))).await.unwrap();
    assert_eq!(
        files.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        ["src/login.rs"],
        "no path out of the folder"
    );
    assert_eq!((files.adds, files.dels), (4, 1));

    let patch = phone
        .node
        .agent_diff_patch(pc_id, THREAD.into(), TurnRange::Span { from: 1, to: 2 }, "src/login.rs".into())
        .await
        .unwrap();
    assert_eq!(patch.patch, "@@ -1 +1 @@\n-a\n+b\n", "the host's pages, joined");
    assert!(!patch.truncated);

    let request = AgentSearchRequest { query: "login\ttest".into(), ..Default::default() };
    let hits = timed("search", phone.node.agent_search(pc_id, request)).await.unwrap();
    assert_eq!(hits.hits.len(), 1);
    assert_eq!(hits.hits[0].snippet, "the login test");
    assert_eq!(hits.hits[0].folder, "g7linux", "a folder's name, never its path");
    assert_eq!(hits.next.as_deref(), Some("c2"));

    let mark =
        AgentMark { threads: vec![THREAD.into(), GONE.into()], pinned: Some(true), ..Default::default() };
    let marked = timed("mark", phone.node.agent_mark(pc_id, mark)).await.unwrap();
    assert_eq!(marked.len(), 1);
    assert!(marked[0].pinned);

    let renamed = phone.node.agent_rename(pc_id, THREAD.into(), " Login\nfix ".into()).await.unwrap();
    assert_eq!(renamed.title, "Login fix", "one line");

    let fork = phone.node.agent_fork(pc_id, THREAD.into(), Some(1)).await.unwrap();
    assert_eq!(fork.forked_from.as_ref().map(|f| (f.thread.as_str(), f.turn)), Some((THREAD, Some(1))));

    let plan = phone.node.agent_rewind_preview(pc_id, THREAD.into(), 2).await.unwrap();
    assert_eq!((plan.turns, plan.messages), (2, 7));
    let rewound = phone.node.agent_rewind_apply(pc_id, plan.token, true).await.unwrap();
    assert_eq!(rewound.undo.map(|u| u.files), Some(false));
    assert!(matches!(phone.node.agent_rewind_apply(pc_id, "0000".into(), false).await, Err(Error::NotFound)));
    phone.node.agent_rewind_undo(pc_id, THREAD.into()).await.unwrap();

    let found = timed("found", phone.node.agent_found(pc_id, AgentFoundRequest::default())).await.unwrap();
    assert_eq!(found.found[0].folder, "g7linux");
    assert!(found.scanning);
    let preview = phone.node.agent_found_preview(pc_id, FOUND.into(), None).await.unwrap();
    assert_eq!(preview.iter().map(|i| i.seq).collect::<Vec<_>>(), [0, 64]);
    let job = phone.node.agent_import(pc_id, vec![FOUND.into()]).await.unwrap();
    assert_eq!((job.job.as_str(), job.total), (JOB, 1));
    phone.node.agent_import_cancel(pc_id, JOB.into()).await.unwrap();

    let shown = AgentThreadsRequest { show: Some(history::show::ARCHIVED.into()), before_ms: Some(5) };
    assert_eq!(phone.node.agent_threads_shown(pc_id, shown).await.unwrap().len(), 3);

    let requests = pc.host.requests();
    assert!(requests.iter().any(|r| matches!(r, AgentRequest::Turns { limit: 10, .. })));
    assert!(requests.iter().any(|r| matches!(r, AgentRequest::DiffFiles { range: TurnRange::Turn(2), .. })));
    assert!(requests.iter().any(|r| matches!(r, AgentRequest::DiffPatch { offset: Some(12), .. })));
    assert!(
        requests.iter().any(|r| matches!(r, AgentRequest::Search { query, .. } if query == "login test"))
    );
    assert!(requests.iter().any(|r| matches!(r, AgentRequest::Rename { title, .. } if title == "Login fix")));
    assert!(requests.iter().any(|r| matches!(r, AgentRequest::RewindApply { refill: true, .. })));
    assert!(requests.iter().any(|r| matches!(r, AgentRequest::ImportCancel { job } if job == JOB)));
    assert!(requests.iter().any(|r| matches!(
        r,
        AgentRequest::ThreadsPage { show: Some(show), before_ms: Some(5) } if show == "archived"
    )));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_patch_stops_at_a_megabyte() {
    let (pc, phone) = pair(false).await;
    let pc_id = pc.node.device_id();
    let patch =
        timed("patch", phone.node.agent_diff_patch(pc_id, THREAD.into(), TurnRange::Turn(1), ENDLESS.into()))
            .await
            .unwrap();
    assert_eq!(patch.patch.len(), history::MAX_PATCH_BYTES);
    assert!(patch.truncated);
    let pages = pc.host.requests().iter().filter(|r| matches!(r, AgentRequest::DiffPatch { .. })).count();
    assert_eq!(pages, history::MAX_PATCH_BYTES.div_ceil(PAGE));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_pc_refuses_what_a_phone_may_not_ask_before_the_host_hears_of_it() {
    let (mut pc, phone) = pair(false).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let ask = |t: &'static str, body: serde_json::Value| {
        let node = &phone.node;
        async move { node.debug_agent_request(pc_id, t, &body).await }
    };
    let preview = serde_json::json!({ "thread": THREAD, "turn": 1, "files": true });
    assert!(matches!(ask("agent.rewind.preview", preview).await, Err(Error::Denied)));
    let apply = serde_json::json!({ "token": TOKEN, "include_later": true });
    assert!(matches!(ask("agent.rewind.apply", apply).await, Err(Error::Denied)));
    let ids: Vec<String> = (0..51).map(|i| format!("{i:032x}")).collect();
    let many = serde_json::json!({ "threads": ids, "pinned": true });
    assert!(matches!(ask("agent.mark", many).await, Err(Error::Protocol(_))));
    let later = serde_json::json!({ "threads": [THREAD], "snooze": u64::MAX / 2 });
    assert!(matches!(ask("agent.mark", later).await, Err(Error::Protocol(_))));
    let long = serde_json::json!({ "query": "x".repeat(201) });
    assert!(matches!(ask("agent.search", long).await, Err(Error::Protocol(_))));
    let path = serde_json::json!({ "thread": THREAD, "turn": 1, "path": "d/".repeat(256) + "x" });
    assert!(matches!(ask("agent.diff.patch", path).await, Err(Error::Protocol(_))));
    // Found sessions need `agent_sessions` too: refused, and the PC asks its user once.
    assert!(matches!(phone.node.agent_found(pc_id, AgentFoundRequest::default()).await, Err(Error::Denied)));
    wait_for(&mut pc, "sessions prompt", |e| match e {
        NodeEvent::AgentSessionsRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert!(pc.host.requests().is_empty(), "nothing reached the host: {:?}", pc.host.requests());

    // The phone doesn't send them at all.
    let mark = AgentMark { threads: vec![THREAD.into()], snooze: Some(1), ..Default::default() };
    assert!(matches!(phone.node.agent_mark(pc_id, mark).await, Err(Error::Protocol(_))));
    assert!(matches!(
        phone.node.agent_rename(pc_id, THREAD.into(), " \n ".into()).await,
        Err(Error::Protocol(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_found_on_the_pc_need_their_own_toggle_and_a_phone_has_no_way_to_the_pcs_screen() {
    let (mut pc, phone) = pair(false).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    // `agents` on, `agent_sessions` off: what was found on the PC is refused,
    // every request of it, before the host hears of it.
    assert!(matches!(phone.node.agent_found(pc_id, AgentFoundRequest::default()).await, Err(Error::Denied)));
    assert!(matches!(phone.node.agent_found_preview(pc_id, FOUND.into(), None).await, Err(Error::Denied)));
    assert!(matches!(phone.node.agent_import(pc_id, vec![FOUND.into()]).await, Err(Error::Denied)));
    assert!(matches!(phone.node.agent_import_cancel(pc_id, JOB.into()).await, Err(Error::Denied)));
    wait_for(&mut pc, "sessions prompt", |e| match e {
        NodeEvent::AgentSessionsRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    // The host's `ui.open` (show a thread on the PC's screen) has no message
    // on the wire: a phone can't ask for it, not even raw.
    let open = serde_json::json!({ "thread_id": THREAD });
    assert!(phone.node.debug_agent_request(pc_id, "agent.ui.open", &open).await.is_err());
    assert!(phone.node.debug_agent_request(pc_id, "agent.open", &open).await.is_err());
    assert!(pc.host.requests().is_empty(), "nothing reached the host: {:?}", pc.host.requests());

    // With the toggle on they go, and a found session's folder comes as its name.
    pc.node.set_device_toggle(phone_id, agent::SESSIONS_TOGGLE, true).unwrap();
    let found = phone.node.agent_found(pc_id, AgentFoundRequest::default()).await.unwrap();
    assert_eq!(found.found[0].folder, "g7linux");
    phone.node.agent_import_cancel(pc_id, JOB.into()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn turns_cuts_and_imports_reach_the_phone_and_a_cut_leaves_the_saved_thread() {
    let (pc, mut phone) = pair(true).await;
    let pc_id = pc.node.device_id();
    let page = phone.node.agent_read(pc_id, THREAD.into(), None, 50).await.unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(phone.node.agent_cached_items(pc_id, THREAD.into()).await.unwrap().value.items.len(), 2);

    pc.node.agent_turn(THREAD.into(), turn(3)).await.unwrap();
    let got = wait_for(&mut phone, "agent.turn", |e| match e {
        NodeEvent::AgentTurn { device, thread, turn } if *device == pc_id && thread == THREAD => {
            Some(turn.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(got.turn, 3);

    pc.node
        .agent_cut(AgentCut { thread: THREAD.into(), from_seq: 2, to_seq: 9, restored: false })
        .await
        .unwrap();
    wait_for(&mut phone, "agent.cut", |e| match e {
        NodeEvent::AgentCut { cut, .. } if cut.thread == THREAD => Some(()),
        _ => None,
    })
    .await;
    let saved = phone.node.agent_cached_items(pc_id, THREAD.into()).await.unwrap().value.items;
    assert_eq!(saved.iter().map(|i| i.seq).collect::<Vec<_>>(), [1], "the cut range left the saved thread");

    let progress =
        AgentImportProgress { job: JOB.into(), done: 1, failed: 0, total: 1, thread: Some(THREAD.into()) };
    pc.node.agent_import_progress(progress.clone()).await.unwrap();
    let got = wait_for(&mut phone, "agent.import.progress", |e| match e {
        NodeEvent::AgentImport { progress, .. } => Some(progress.clone()),
        _ => None,
    })
    .await;
    assert_eq!(got, progress);
    assert!(
        pc.node
            .agent_cut(AgentCut { thread: THREAD.into(), from_seq: 5, to_seq: 4, restored: false })
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn saved_threads_read_back_after_a_restart_with_the_pc_gone() {
    let (pc, phone) = pair(false).await;
    let pc_id = pc.node.device_id();
    phone.node.agent_threads(pc_id).await.unwrap();
    phone.node.agent_read(pc_id, THREAD.into(), None, 50).await.unwrap();
    pc.node.shutdown().await;
    phone.node.shutdown().await;
    let Device { node, dir, .. } = phone;
    drop(node);
    let phone = start_in(dir, "Pixel", DeviceKind::Phone, &[agent::CONTROL]).await;
    let threads = phone.node.agent_cached_threads(pc_id).await.unwrap();
    assert_eq!(threads.value.len(), 3);
    assert!(threads.saved_ms.is_some());
    let items = phone.node.agent_cached_items(pc_id, THREAD.into()).await.unwrap();
    assert_eq!(items.value.items.iter().map(|i| i.seq).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(items.value.thread.map(|t| t.id), Some(THREAD.to_owned()));
    assert!(phone.node.agent_cache_bytes().await.unwrap() > 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_stays_saved_once_the_pc_turns_agents_off_or_is_unpaired() {
    let (pc, phone) = pair(false).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    phone.node.agent_threads(pc_id).await.unwrap();
    assert_eq!(phone.node.agent_cached_threads(pc_id).await.unwrap().value.len(), 3);
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, false).unwrap();
    assert!(matches!(phone.node.agent_threads(pc_id).await, Err(Error::Denied)));
    assert!(phone.node.agent_cached_threads(pc_id).await.unwrap().value.is_empty());

    pc.node.set_device_toggle(phone_id, agent::TOGGLE, true).unwrap();
    phone.node.agent_threads(pc_id).await.unwrap();
    assert_eq!(phone.node.agent_cached_threads(pc_id).await.unwrap().value.len(), 3);
    phone.node.unpair(pc_id).await.unwrap();
    assert!(phone.node.agent_cached_threads(pc_id).await.unwrap().value.is_empty());

    // "Keep threads on this phone" off: nothing is saved.
    phone.node.agent_cache_keep(false).await.unwrap();
    assert!(!phone.node.agent_cache_keeps().await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_written_offline_is_held_and_sent_once_when_the_pc_is_back() {
    let (mut pc, mut phone) = pair(false).await;
    let pc_id = pc.node.device_id();
    go_offline(&mut pc, &mut phone).await;

    let held = phone.node.agent_send_held(pc_id, send(THREAD, "Then run the tests."), true).await.unwrap();
    let held = held.expect("held while the PC is away");
    assert_eq!(held.state, HeldState::Waiting);
    assert!(g7link_core::agent_history::is_send_key(&held.key));
    // A picture can't wait.
    let mut picture = send(THREAD, "");
    picture.images = vec!["a".repeat(32)];
    assert!(phone.node.agent_send_held(pc_id, picture, true).await.is_err());
    // Without `hold` nothing waits.
    assert!(phone.node.agent_send_held(pc_id, send(THREAD, "now"), false).await.is_err());
    assert_eq!(phone.node.agent_outbox(pc_id).await.unwrap().len(), 1);

    come_back(&pc, &mut phone).await;
    wait_outbox(&mut phone, pc_id, "held message sent", <[_]>::is_empty).await;
    assert_eq!(pc.host.sent(), [(THREAD.to_owned(), "Then run the tests.".to_owned(), None)]);
    let keyed = pc
        .host
        .requests()
        .iter()
        .any(|r| matches!(r, AgentRequest::Send { key: Some(k), .. } if *k == held.key));
    assert!(keyed, "sent with its key");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_the_pc_took_before_its_answer_was_lost_is_taken_once() {
    let (mut pc, mut phone) = pair(false).await;
    let pc_id = pc.node.device_id();
    go_offline(&mut pc, &mut phone).await;
    let held = phone.node.agent_send_held(pc_id, send(THREAD, "Once."), true).await.unwrap().unwrap();
    // The PC took it already; its answer never came back.
    pc.host.keys.lock().unwrap().insert(held.key.clone());
    come_back(&pc, &mut phone).await;
    wait_outbox(&mut phone, pc_id, "duplicate cleared", <[_]>::is_empty).await;
    assert!(pc.host.sent().is_empty(), "the host sent nothing a second time");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_message_waits_for_an_answer_and_queues_behind_a_running_turn() {
    let (mut pc, mut phone) = pair(false).await;
    let pc_id = pc.node.device_id();
    go_offline(&mut pc, &mut phone).await;
    for (thread, text) in
        [(WAITING, "After the answer."), (RUNNING, "Next."), (RUNNING, "And then."), (GONE, "Lost.")]
    {
        phone.node.agent_send_held(pc_id, send(thread, text), true).await.unwrap().unwrap();
    }
    come_back(&pc, &mut phone).await;
    wait_outbox(&mut phone, pc_id, "first flush", |held| {
        held.len() == 2
            && held.iter().any(|h| {
                h.thread == WAITING && h.state == HeldState::Blocked && h.why == Some(HeldWhy::AnswerFirst)
            })
            && held
                .iter()
                .any(|h| h.thread == GONE && h.state == HeldState::NotSent && h.why == Some(HeldWhy::Gone))
    })
    .await;
    let queue = Some(agent::dispatch::QUEUE.to_owned());
    assert_eq!(
        pc.host.sent(),
        [
            (RUNNING.to_owned(), "Next.".to_owned(), queue.clone()),
            (RUNNING.to_owned(), "And then.".to_owned(), queue),
        ],
        "in order, each queued behind the running turn"
    );

    // The request is answered: the thread changes, and the held message goes.
    let done = thread(WAITING, "done");
    pc.host.threads.lock().unwrap().insert(WAITING.into(), done.clone());
    pc.node.agent_event(g7link_core::AgentEvent::state(done), None).await.unwrap();
    wait_outbox(&mut phone, pc_id, "sent after the answer", |held| held.iter().all(|h| h.thread == GONE))
        .await;
    assert!(pc.host.sent().contains(&(WAITING.to_owned(), "After the answer.".to_owned(), None)));

    // One that won't go by itself is sent when the user says, or discarded.
    let lost = phone.node.agent_outbox(pc_id).await.unwrap().remove(0);
    phone.node.agent_outbox_discard(pc_id, lost.key.clone()).await.unwrap();
    assert!(phone.node.agent_outbox(pc_id).await.unwrap().is_empty());
    assert!(matches!(phone.node.agent_outbox_send(pc_id, lost.key).await, Err(Error::NotFound)));
}
