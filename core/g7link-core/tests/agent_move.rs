// SPDX-License-Identifier: MPL-2.0
//! Move to G7 (`docs/protocol/agents.md` §9.1): two real nodes over
//! localhost, a PC whose platform plays the desktop's move, and a phone that
//! confirms and moves a session started on the PC.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use g7link_core::{
    AgentReply, AgentReplyTarget, AgentRequest, AgentSessionMove, AgentSessionMoved, AgentSessionReplied,
    AgentThread, DeviceId, DeviceInfo, DeviceKind, Error, LinkState, Node, NodeConfig, NodeEvent,
    PairingEvent, PlainKeyProtector, Platform, PowerLevel, agent,
    session_move::{self, reason, result},
    session_reply,
};
use g7link_protocol::messages::types;
use tempfile::TempDir;
use tokio::sync::broadcast::{Receiver, error::TryRecvError};

const WAIT: Duration = Duration::from_secs(20);
/// A session the platform moves: it asks to confirm first.
const SESSION: &str = "sc0123456789abcdef0123456789abcdef";
/// The nonce the platform hands out, and takes once.
const NONCE: &str = "fedcba9876543210fedcba9876543210";
/// Sessions the platform answers in one way each.
const CODEX: &str = "sccodex";
const GONE: &str = "scgone";
const BUSY_PC: &str = "scbusy";
const RATE: &str = "scrate";
const DRAFT: &str = "scdraft";
const FOLDER: &str = "scfolder";
const STRAY_THREAD: &str = "scstraythread";
const NO_THREAD: &str = "scnothread";
const UNKNOWN_RESULT: &str = "scunknownresult";
const UNKNOWN_REASON: &str = "scunknownreason";
const BARE_CONFIRM: &str = "scbareconfirm";
const A_REPLY: &str = "scareply";

/// The desktop's move, as far as these tests need one.
#[derive(Debug, Default)]
struct MovePlatform {
    requests: Mutex<Vec<(DeviceId, AgentRequest)>>,
}

impl MovePlatform {
    fn requests(&self) -> Vec<(DeviceId, AgentRequest)> {
        self.requests.lock().unwrap().clone()
    }
}

/// Where the move happens, as a careless platform might say it: a path for
/// a folder, a control in the app's name.
fn careless_target() -> AgentReplyTarget {
    AgentReplyTarget {
        app: "Terminal\u{1b}".into(),
        agent: "Claude".into(),
        folder: "/home/me/src/g7linux".into(),
        kind: session_reply::kind::TERMINAL.into(),
    }
}

/// The thread the session became, in plan mode with a model chosen: what
/// only a phone with `agent.control` reads as it is.
fn thread() -> AgentThread {
    AgentThread {
        id: "th0123".into(),
        agent: "claude".into(),
        folder: "g7linux".into(),
        title: "Fix the build\u{1b}".into(),
        state: agent::state::DONE.into(),
        mode: agent::mode::PLAN.into(),
        model: Some("sonnet".into()),
        from_phone: true,
        updated: 1_700_000_000_000,
        ..Default::default()
    }
}

impl Platform for MovePlatform {
    fn agent_request(
        &self,
        from: &DeviceId,
        request: AgentRequest,
    ) -> Result<AgentReply, g7link_core::AgentFailure> {
        use g7link_core::AgentFailure;
        self.requests.lock().unwrap().push((*from, request.clone()));
        let AgentRequest::SessionMove { session, confirm } = request else {
            return Err(AgentFailure::Unsupported);
        };
        let answer = match (session.as_str(), confirm.as_deref()) {
            (GONE, _) => return Err(AgentFailure::NotFound),
            (BUSY_PC, _) => return Err(AgentFailure::Busy),
            (A_REPLY, _) => return Ok(AgentReply::Replied(AgentSessionReplied::sent(careless_target()))),
            (CODEX, _) => AgentSessionMoved::closed(),
            (RATE, _) => AgentSessionMoved::refused(reason::RATE).retry_after(3_600_000),
            (DRAFT, _) => AgentSessionMoved::failed(reason::DRAFT).with_target(careless_target()),
            (FOLDER, _) => AgentSessionMoved::refused(reason::FOLDER),
            (STRAY_THREAD, _) => AgentSessionMoved { thread: Some(thread()), ..AgentSessionMoved::closed() },
            (NO_THREAD, _) => AgentSessionMoved { thread: None, ..AgentSessionMoved::moved(thread()) },
            (UNKNOWN_RESULT, _) => AgentSessionMoved { result: "parked".into(), ..Default::default() },
            (UNKNOWN_REASON, _) => AgentSessionMoved::refused("too_sunny"),
            (BARE_CONFIRM, _) => AgentSessionMoved {
                nonce: None,
                ..AgentSessionMoved::confirm(careless_target(), NONCE.into())
            },
            (SESSION, Some(NONCE)) => AgentSessionMoved::moved(thread()),
            (SESSION, _) => AgentSessionMoved::confirm(careless_target(), NONCE.into()),
            _ => return Err(AgentFailure::NotFound),
        };
        Ok(AgentReply::Moved(answer))
    }
}

struct Device {
    node: Node,
    events: Receiver<NodeEvent>,
    host: Arc<MovePlatform>,
    _dir: TempDir,
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

async fn start(name: &'static str, kind: DeviceKind, offers: &[&str]) -> Device {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let mut config = NodeConfig::new(dir.path(), info(name, kind), "0.0.1-test");
    config.lan_discovery = false;
    config.away_mode = false;
    config.key_protector = Some(Arc::new(PlainKeyProtector));
    let host = Arc::new(MovePlatform::default());
    let node = Node::start(config, host.clone()).await.expect("node starts");
    node.update_power(PowerLevel::NotApplicable, offers.iter().map(|c| (*c).to_owned()).collect()).await;
    let events = node.events();
    Device { node, events, host, _dir: dir, name }
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

/// Every event `dev` has had so far, without waiting.
fn drain(dev: &mut Device) -> Vec<NodeEvent> {
    let mut events = Vec::new();
    loop {
        match dev.events.try_recv() {
            Ok(event) => events.push(event),
            Err(TryRecvError::Lagged(_)) => continue,
            Err(TryRecvError::Empty | TryRecvError::Closed) => return events,
        }
    }
}

async fn timed<T>(what: &str, fut: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, fut).await.unwrap_or_else(|_| panic!("timed out: {what}"))
}

/// The phone of most tests: it follows sessions and speaks `agent.control`.
const PHONE: &[&str] = &[agent::ITEMS2, agent::CONTROL, agent::FOLLOW];

/// A PC offering `offers`, paired with a phone offering `phone_offers`,
/// every toggle off. Waits until each knows the other is there.
async fn pair_with(offers: &[&str], phone_offers: &[&str]) -> (Device, Device) {
    let mut pc = start("Desktop", DeviceKind::Desktop, offers).await;
    let mut phone = start("Pixel", DeviceKind::Phone, phone_offers).await;
    let uri = pc.node.pairing_start_qr().await.unwrap();
    tokio::time::timeout(WAIT, phone.node.pairing_join(&uri.to_uri())).await.unwrap().expect("paired");
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    wait_for(&mut pc, "paired", |e| match e {
        NodeEvent::Pairing(PairingEvent::Paired(d)) if d.id == phone_id => Some(()),
        _ => None,
    })
    .await;
    wait_for(&mut phone, "peer online", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Online { .. } } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    (pc, phone)
}

/// A PC that can type replies and move sessions.
const MOVING_PC: &[&str] =
    &[agent::HOST, agent::CONTROL, agent::SESSIONS, session_reply::CAP, session_move::CAP];

const TOGGLES: [&str; 3] = [agent::TOGGLE, agent::SESSIONS_TOGGLE, session_reply::TOGGLE];

/// A PC that moves sessions, paired with a phone offering `phone_offers`,
/// all three toggles on.
async fn pair_allowed_with(phone_offers: &[&str]) -> (Device, Device) {
    let (pc, phone) = pair_with(MOVING_PC, phone_offers).await;
    let phone_id = phone.node.device_id();
    for toggle in TOGGLES {
        pc.node.set_device_toggle(phone_id, toggle, true).unwrap();
    }
    (pc, phone)
}

fn is_bad_message(result: Result<(), Error>) -> bool {
    matches!(result, Err(Error::Protocol(ref reason)) if reason.starts_with("BAD_MESSAGE"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_that_doesnt_offer_moves_is_never_asked() {
    // An older PC: it says nothing about replies either.
    let (pc, phone) = pair_with(&[agent::HOST, agent::CONTROL, agent::SESSIONS], PHONE).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    for toggle in TOGGLES {
        pc.node.set_device_toggle(phone_id, toggle, true).unwrap();
    }
    assert!(!phone.node.agent_move_offered(pc_id));
    assert_eq!(phone.node.agent_reply_needs(pc_id), None, "it said nothing: show nothing");
    assert!(matches!(
        phone.node.agent_session_move(pc_id, SESSION.into(), None).await,
        Err(Error::Unsupported)
    ));
    // Asked anyway, the PC says so itself.
    let raw = AgentSessionMove::new(SESSION.into(), None);
    assert!(matches!(
        timed("raw", phone.node.debug_agent_request(pc_id, types::AGENT_SESSION_MOVE, &raw)).await,
        Err(Error::Unsupported)
    ));
    assert!(pc.host.requests().is_empty(), "nothing reaches the platform");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_that_only_replies_doesnt_take_moves() {
    let replying = &[agent::HOST, agent::CONTROL, agent::SESSIONS, session_reply::CAP];
    let (pc, phone) = pair_with(replying, PHONE).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    for toggle in TOGGLES {
        pc.node.set_device_toggle(phone_id, toggle, true).unwrap();
    }
    assert!(!phone.node.agent_move_offered(pc_id));
    assert_eq!(phone.node.agent_reply_needs(pc_id), None, "it can reply");
    let raw = AgentSessionMove::new(SESSION.into(), None);
    assert!(matches!(
        timed("raw", phone.node.debug_agent_request(pc_id, types::AGENT_SESSION_MOVE, &raw)).await,
        Err(Error::Unsupported)
    ));
    assert!(pc.host.requests().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_needs_the_reply_toggles_and_shares_their_prompt() {
    let (mut pc, phone) = pair_with(MOVING_PC, PHONE).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let move_it = || phone.node.agent_session_move(pc_id, SESSION.into(), None);

    // `agents` off: its own prompt.
    assert!(matches!(timed("move", move_it()).await, Err(Error::Denied)));
    wait_for(&mut pc, "agents prompt", |e| match e {
        NodeEvent::AgentsRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    // `agent_sessions` off: its own prompt.
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, true).unwrap();
    assert!(matches!(timed("move", move_it()).await, Err(Error::Denied)));
    wait_for(&mut pc, "sessions prompt", |e| match e {
        NodeEvent::AgentSessionsRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert!(!drain(&mut pc).iter().any(|e| matches!(e, NodeEvent::AgentReplyRequested { .. })));
    // `agent_reply` off: its prompt, once, for a move and a reply alike.
    pc.node.set_device_toggle(phone_id, agent::SESSIONS_TOGGLE, true).unwrap();
    assert!(matches!(timed("move", move_it()).await, Err(Error::Denied)));
    wait_for(&mut pc, "reply prompt", |e| match e {
        NodeEvent::AgentReplyRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert!(matches!(timed("move", move_it()).await, Err(Error::Denied)));
    assert!(matches!(
        timed("reply", phone.node.agent_session_reply(pc_id, SESSION.into(), "Go on".into(), None)).await,
        Err(Error::Denied)
    ));
    assert!(
        !drain(&mut pc).iter().any(|e| matches!(e, NodeEvent::AgentReplyRequested { .. })),
        "the prompt shows once"
    );
    assert!(pc.host.requests().is_empty(), "none reached the platform");

    pc.node.set_device_toggle(phone_id, session_reply::TOGGLE, true).unwrap();
    let asked = timed("move", move_it()).await.unwrap();
    assert_eq!(asked.reason.as_deref(), Some(reason::CONFIRM));
    // Turning `agent_sessions` off again is enough to stop it.
    pc.node.set_device_toggle(phone_id, agent::SESSIONS_TOGGLE, false).unwrap();
    assert!(matches!(timed("move", move_it()).await, Err(Error::Denied)));
    assert_eq!(pc.host.requests().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_confirms_and_moves_a_session() {
    let (pc, phone) = pair_allowed_with(PHONE).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    assert!(phone.node.agent_move_offered(pc_id));
    assert_eq!(phone.node.agent_reply_needs(pc_id), None);

    // The first move asks first: nothing is typed.
    let asked = timed("move", phone.node.agent_session_move(pc_id, SESSION.into(), None)).await.unwrap();
    assert_eq!((asked.result.as_str(), asked.reason.as_deref()), (result::REFUSED, Some(reason::CONFIRM)));
    assert_eq!(asked.nonce.as_deref(), Some(NONCE));
    let target = asked.target.expect("the sheet shows where");
    assert_eq!(
        (target.app.as_str(), target.folder.as_str()),
        ("Terminal", "g7linux"),
        "a name, never a path"
    );
    assert_eq!(asked.thread, None);

    let moved = timed("confirmed", phone.node.agent_session_move(pc_id, SESSION.into(), asked.nonce.clone()))
        .await
        .unwrap();
    assert_eq!((moved.result.as_str(), moved.reason, moved.nonce), (result::MOVED, None, None));
    let thread = moved.thread.expect("the thread to open");
    assert_eq!((thread.id.as_str(), thread.from_phone), ("th0123", true));
    assert_eq!(thread.title, "Fix the build", "made to fit");
    assert_eq!(
        (thread.mode.as_str(), thread.model.as_deref()),
        (agent::mode::PLAN, Some("sonnet")),
        "as it is for a phone with agent.control"
    );

    assert_eq!(
        pc.host.requests(),
        [
            (phone_id, AgentRequest::SessionMove { session: SESSION.into(), confirm: None }),
            (phone_id, AgentRequest::SessionMove { session: SESSION.into(), confirm: Some(NONCE.into()) }),
        ],
        "each went once"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_without_control_gets_the_threads_older_form() {
    let (pc, phone) = pair_allowed_with(&[agent::ITEMS2, agent::FOLLOW]).await;
    let pc_id = pc.node.device_id();
    let moved = timed("move", phone.node.agent_session_move(pc_id, SESSION.into(), Some(NONCE.into())))
        .await
        .unwrap();
    let thread = moved.thread.expect("the thread to open");
    // Sent as `supervised` (the old form); this build reads the old word as `ask`.
    assert_eq!((thread.mode.as_str(), thread.model), (agent::mode::ASK, None));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_pc_refuses_a_move_out_of_its_limits_before_its_platform() {
    let (pc, phone) = pair_allowed_with(PHONE).await;
    let pc_id = pc.node.device_id();
    let raw_bodies = [
        AgentSessionMove::new(String::new(), None),
        AgentSessionMove::new("../etc".into(), None),
        AgentSessionMove::new("a".repeat(41), None),
        AgentSessionMove::new(SESSION.into(), Some(NONCE.to_uppercase())),
        AgentSessionMove::new(SESSION.into(), Some("abc".into())),
    ];
    for body in &raw_bodies {
        let refused =
            timed("raw", phone.node.debug_agent_request(pc_id, types::AGENT_SESSION_MOVE, body)).await;
        assert!(is_bad_message(refused), "{body:?}");
    }
    // The phone doesn't send them at all.
    for (session, confirm) in
        [("../etc", None), ("", None), (SESSION, Some("abc")), (SESSION, Some(&*NONCE.to_uppercase()))]
    {
        let refused = phone.node.agent_session_move(pc_id, session.into(), confirm.map(str::to_owned)).await;
        assert!(matches!(refused, Err(Error::Protocol(ref r)) if r.starts_with("invalid")), "{refused:?}");
    }
    assert!(pc.host.requests().is_empty(), "nothing reached the platform");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_whole_answer_reaches_the_phone() {
    let (pc, phone) = pair_allowed_with(PHONE).await;
    let pc_id = pc.node.device_id();
    let move_it = |session: &str| phone.node.agent_session_move(pc_id, session.into(), None);

    let closed = timed("closed", move_it(CODEX)).await.unwrap();
    assert_eq!((closed.result.as_str(), closed.reason, closed.thread), (result::CLOSED, None, None));
    let stray = timed("stray", move_it(STRAY_THREAD)).await.unwrap();
    assert_eq!((stray.result.as_str(), stray.thread), (result::CLOSED, None), "a thread only with moved");

    let rate = timed("rate", move_it(RATE)).await.unwrap();
    assert_eq!((rate.result.as_str(), rate.reason.as_deref()), (result::REFUSED, Some(reason::RATE)));
    assert_eq!(rate.retry_ms, Some(session_move::MAX_RETRY_MS), "at most a minute");
    let draft = timed("draft", move_it(DRAFT)).await.unwrap();
    assert_eq!((draft.result.as_str(), draft.reason.as_deref()), (result::FAILED, Some(reason::DRAFT)));
    assert_eq!(draft.target.map(|t| t.folder), Some("g7linux".into()));
    let folder = timed("folder", move_it(FOLDER)).await.unwrap();
    assert_eq!(folder.reason.as_deref(), Some(reason::FOLDER));

    assert!(matches!(timed("gone", move_it(GONE)).await, Err(Error::NotFound)));
    let busy = timed("busy", move_it(BUSY_PC)).await;
    assert!(matches!(busy, Err(Error::Protocol(ref r)) if r.starts_with("BUSY")), "{busy:?}");

    // A platform answer that isn't whole: the PC answers INTERNAL, which a
    // phone can't read as anything: not known whether it was moved.
    for session in [NO_THREAD, UNKNOWN_RESULT, UNKNOWN_REASON, BARE_CONFIRM, A_REPLY] {
        assert!(matches!(timed(session, move_it(session)).await, Err(Error::Timeout)), "{session}");
    }
    assert_eq!(pc.host.requests().len(), 12, "each went once");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_that_cant_type_replies_says_why() {
    // A PC offers at most one; one that offers two is read as the first it checks.
    let needing = &[
        agent::HOST,
        agent::CONTROL,
        agent::SESSIONS,
        session_reply::needs::KEYBOARD,
        session_reply::needs::DESKTOP,
    ];
    let (pc, phone) = pair_with(needing, PHONE).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    for toggle in TOGGLES {
        pc.node.set_device_toggle(phone_id, toggle, true).unwrap();
    }
    assert_eq!(phone.node.agent_reply_needs(pc_id), Some(session_reply::needs::DESKTOP));
    assert!(!phone.node.agent_move_offered(pc_id));
    assert!(matches!(
        phone.node.agent_session_move(pc_id, SESSION.into(), None).await,
        Err(Error::Unsupported)
    ));
    assert_eq!(pc.node.agent_reply_needs(phone_id), None, "a phone says nothing of the kind");
    assert!(pc.host.requests().is_empty());
}
