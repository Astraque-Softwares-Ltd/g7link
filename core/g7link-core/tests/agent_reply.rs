// SPDX-License-Identifier: MPL-2.0
//! Reply on the PC (`docs/protocol/agents.md` §9): two real nodes over
//! localhost, a PC whose platform plays the desktop's reply sequence, and a
//! phone that checks, confirms and sends a reply into a session started on
//! the PC.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use g7link_core::{
    AgentReply, AgentReplyTarget, AgentRequest, AgentSessionReplied, DeviceId, DeviceInfo, DeviceKind, Error,
    LinkState, Node, NodeConfig, NodeEvent, PairingEvent, PlainKeyProtector, Platform, PowerLevel, agent,
    agent_reply::AgentSessionReply,
    session_reply::{self, reason, result},
};
use g7link_protocol::messages::types;
use tempfile::TempDir;
use tokio::sync::broadcast::{Receiver, error::TryRecvError};

const WAIT: Duration = Duration::from_secs(20);
/// A session the platform replies into: it asks to confirm first.
const SESSION: &str = "sc0123456789abcdef0123456789abcdef";
/// The nonce the platform hands out, and takes once.
const NONCE: &str = "fedcba9876543210fedcba9876543210";
/// Sessions the platform answers in one way each.
const GONE: &str = "scgone";
const BUSY_PC: &str = "scbusy";
const RATE: &str = "scrate";
const INTERRUPTED: &str = "scinterrupted";
const NO_TARGET: &str = "scnotarget";
const READY_TO_TEXT: &str = "screadytotext";
const UNKNOWN_REASON: &str = "scunknownreason";

/// The desktop's reply sequence, as far as these tests need one.
#[derive(Debug, Default)]
struct ReplyPlatform {
    requests: Mutex<Vec<(DeviceId, AgentRequest)>>,
}

impl ReplyPlatform {
    fn requests(&self) -> Vec<(DeviceId, AgentRequest)> {
        self.requests.lock().unwrap().clone()
    }
}

/// Where the reply goes, as a careless platform might say it: a path for
/// a folder, a control in the app's name.
fn careless_target() -> AgentReplyTarget {
    AgentReplyTarget {
        app: "Terminal\u{1b}".into(),
        agent: "Claude".into(),
        folder: "/home/me/src/g7linux".into(),
        kind: session_reply::kind::TERMINAL.into(),
    }
}

impl Platform for ReplyPlatform {
    fn agent_request(
        &self,
        from: &DeviceId,
        request: AgentRequest,
    ) -> Result<AgentReply, g7link_core::AgentFailure> {
        use g7link_core::AgentFailure;
        self.requests.lock().unwrap().push((*from, request.clone()));
        let AgentRequest::SessionReply { session, text, confirm } = request else {
            return Err(AgentFailure::Unsupported);
        };
        let target = careless_target();
        let answer = match (session.as_str(), text.is_some(), confirm.as_deref()) {
            (GONE, ..) => return Err(AgentFailure::NotFound),
            (BUSY_PC, ..) => return Err(AgentFailure::Busy),
            (RATE, ..) => AgentSessionReplied::refused(reason::RATE).retry_after(3_600_000),
            (INTERRUPTED, ..) => AgentSessionReplied::failed(reason::INTERRUPTED).with_target(target),
            (NO_TARGET, ..) => AgentSessionReplied { target: None, ..AgentSessionReplied::sent(target) },
            (READY_TO_TEXT, ..) => AgentSessionReplied::ready(target),
            (UNKNOWN_REASON, ..) => AgentSessionReplied::refused("too_sunny"),
            (SESSION, false, _) => AgentSessionReplied::ready(target),
            (SESSION, true, Some(NONCE)) => AgentSessionReplied::sent(target),
            (SESSION, true, _) => AgentSessionReplied::confirm(target, NONCE.into()),
            _ => return Err(AgentFailure::NotFound),
        };
        Ok(AgentReply::Replied(answer))
    }
}

struct Device {
    node: Node,
    events: Receiver<NodeEvent>,
    host: Arc<ReplyPlatform>,
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
    let host = Arc::new(ReplyPlatform::default());
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

/// A PC offering `offers`, paired with a phone that follows sessions, every
/// toggle off.
async fn pair(offers: &[&str]) -> (Device, Device) {
    let mut pc = start("Desktop", DeviceKind::Desktop, offers).await;
    let mut phone = start("Pixel", DeviceKind::Phone, &[agent::ITEMS2, agent::CONTROL, agent::FOLLOW]).await;
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

const REPLYING_PC: &[&str] = &[agent::HOST, agent::CONTROL, agent::SESSIONS, session_reply::CAP];

/// A PC that offers replies, paired with a phone with all three toggles on.
async fn pair_allowed() -> (Device, Device) {
    let (pc, phone) = pair(REPLYING_PC).await;
    let phone_id = phone.node.device_id();
    for toggle in [agent::TOGGLE, agent::SESSIONS_TOGGLE, session_reply::TOGGLE] {
        pc.node.set_device_toggle(phone_id, toggle, true).unwrap();
    }
    (pc, phone)
}

fn is_bad_message(result: Result<(), Error>) -> bool {
    matches!(result, Err(Error::Protocol(ref reason)) if reason.starts_with("BAD_MESSAGE"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_that_doesnt_offer_replies_is_never_asked() {
    let (pc, phone) = pair(&[agent::HOST, agent::CONTROL, agent::SESSIONS]).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    for toggle in [agent::TOGGLE, agent::SESSIONS_TOGGLE, session_reply::TOGGLE] {
        pc.node.set_device_toggle(phone_id, toggle, true).unwrap();
    }
    assert!(!phone.node.peer_offers(pc_id, session_reply::CAP));
    assert!(matches!(
        phone.node.agent_session_reply_check(pc_id, SESSION.into()).await,
        Err(Error::Unsupported)
    ));
    assert!(matches!(
        phone.node.agent_session_reply(pc_id, SESSION.into(), "Go on".into(), None).await,
        Err(Error::Unsupported)
    ));
    // Asked anyway, the PC says so itself.
    let raw = AgentSessionReply::text(SESSION.into(), "Go on".into(), None);
    assert!(matches!(
        timed("raw", phone.node.debug_agent_request(pc_id, types::AGENT_SESSION_REPLY, &raw)).await,
        Err(Error::Unsupported)
    ));
    assert!(pc.host.requests().is_empty(), "nothing reaches the platform");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_needs_three_toggles_each_with_its_prompt() {
    let (mut pc, phone) = pair(REPLYING_PC).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    assert!(
        pc.node.device_toggles(phone_id).unwrap().contains(&(session_reply::TOGGLE, false)),
        "off by default"
    );
    let check = || phone.node.agent_session_reply_check(pc_id, SESSION.into());

    // `agents` off: its own prompt.
    assert!(matches!(timed("check", check()).await, Err(Error::Denied)));
    wait_for(&mut pc, "agents prompt", |e| match e {
        NodeEvent::AgentsRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    // `agent_sessions` off: its own prompt.
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, true).unwrap();
    assert!(matches!(timed("check", check()).await, Err(Error::Denied)));
    wait_for(&mut pc, "sessions prompt", |e| match e {
        NodeEvent::AgentSessionsRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert!(!drain(&mut pc).iter().any(|e| matches!(e, NodeEvent::AgentReplyRequested { .. })));
    // `agent_reply` off: the third prompt, once.
    pc.node.set_device_toggle(phone_id, agent::SESSIONS_TOGGLE, true).unwrap();
    assert!(matches!(timed("check", check()).await, Err(Error::Denied)));
    wait_for(&mut pc, "reply prompt", |e| match e {
        NodeEvent::AgentReplyRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
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
    let ready = timed("check", check()).await.unwrap();
    assert_eq!(ready.result, result::READY);
    // Turning `agent_sessions` off again is enough to stop it.
    pc.node.set_device_toggle(phone_id, agent::SESSIONS_TOGGLE, false).unwrap();
    assert!(matches!(timed("check", check()).await, Err(Error::Denied)));
    assert_eq!(pc.host.requests().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_checks_confirms_and_sends_a_reply() {
    let (pc, phone) = pair_allowed().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let ready = timed("check", phone.node.agent_session_reply_check(pc_id, SESSION.into())).await.unwrap();
    assert_eq!((ready.result.as_str(), ready.reason.as_deref()), (result::READY, None));
    let target = ready.target.expect("a check says where");
    assert_eq!(
        (target.app.as_str(), target.agent.as_str(), target.folder.as_str()),
        ("Terminal", "Claude", "g7linux"),
        "a name, never a path"
    );

    // The first reply to this window asks first: nothing is typed.
    let typed = "Yes, go on.\r\nAnd run the tests.\u{1b}";
    let asked = timed("reply", phone.node.agent_session_reply(pc_id, SESSION.into(), typed.into(), None))
        .await
        .unwrap();
    assert_eq!((asked.result.as_str(), asked.reason.as_deref()), (result::REFUSED, Some(reason::CONFIRM)));
    assert_eq!(asked.nonce.as_deref(), Some(NONCE));
    assert!(asked.target.is_some(), "the sheet shows where");

    let sent = timed(
        "confirmed",
        phone.node.agent_session_reply(pc_id, SESSION.into(), typed.into(), asked.nonce.clone()),
    )
    .await
    .unwrap();
    assert_eq!((sent.result.as_str(), sent.reason, sent.nonce), (result::SENT, None, None));
    assert_eq!(sent.target.map(|t| t.folder), Some("g7linux".into()));

    let cleaned = "Yes, go on.\nAnd run the tests.";
    assert_eq!(
        pc.host.requests(),
        [
            (phone_id, AgentRequest::SessionReply { session: SESSION.into(), text: None, confirm: None }),
            (
                phone_id,
                AgentRequest::SessionReply {
                    session: SESSION.into(),
                    text: Some(cleaned.into()),
                    confirm: None
                }
            ),
            (
                phone_id,
                AgentRequest::SessionReply {
                    session: SESSION.into(),
                    text: Some(cleaned.into()),
                    confirm: Some(NONCE.into())
                }
            ),
        ],
        "each went once, the text cleaned on the phone"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_pc_refuses_a_reply_out_of_its_limits_before_its_platform() {
    let (pc, phone) = pair_allowed().await;
    let pc_id = pc.node.device_id();
    let text = |t: &str| AgentSessionReply::text(SESSION.into(), t.into(), None);
    let raw_bodies = [
        AgentSessionReply { check: true, ..text("Go") },
        AgentSessionReply { session: SESSION.into(), ..Default::default() },
        text(&"x".repeat(session_reply::MAX_TEXT_BYTES + 1)),
        text(""),
        text(" \n\t"),
        text("Go\u{1b}[201~rm -rf ~"),
        text("Go\u{202E}"),
        AgentSessionReply { confirm: Some(NONCE.to_uppercase()), ..text("Go") },
        AgentSessionReply { confirm: Some(NONCE.into()), ..AgentSessionReply::check(SESSION.into()) },
        AgentSessionReply { session: "../etc".into(), ..text("Go") },
    ];
    for body in &raw_bodies {
        let refused =
            timed("raw", phone.node.debug_agent_request(pc_id, types::AGENT_SESSION_REPLY, body)).await;
        assert!(is_bad_message(refused), "{body:?}");
    }
    // The phone doesn't send them at all.
    for (session, text) in [
        (SESSION, String::new()),
        (SESSION, " \r\n ".into()),
        (SESSION, "x".repeat(8193)),
        ("../etc", "Go".into()),
    ] {
        let refused = phone.node.agent_session_reply(pc_id, session.into(), text, None).await;
        assert!(matches!(refused, Err(Error::Protocol(ref r)) if r.starts_with("invalid")), "{refused:?}");
    }
    let bad_nonce =
        phone.node.agent_session_reply(pc_id, SESSION.into(), "Go".into(), Some("abc".into())).await;
    assert!(matches!(bad_nonce, Err(Error::Protocol(_))), "{bad_nonce:?}");
    assert!(pc.host.requests().is_empty(), "nothing reached the platform");
    // At the limit it goes.
    let long = "x".repeat(session_reply::MAX_TEXT_BYTES);
    let asked =
        timed("long", phone.node.agent_session_reply(pc_id, SESSION.into(), long, None)).await.unwrap();
    assert_eq!(asked.reason.as_deref(), Some(reason::CONFIRM));
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_whole_answer_reaches_the_phone() {
    let (pc, phone) = pair_allowed().await;
    let pc_id = pc.node.device_id();
    let reply = |session: &str| phone.node.agent_session_reply(pc_id, session.into(), "Go on".into(), None);

    let rate = timed("rate", reply(RATE)).await.unwrap();
    assert_eq!((rate.result.as_str(), rate.reason.as_deref()), (result::REFUSED, Some(reason::RATE)));
    assert_eq!(rate.retry_ms, Some(session_reply::MAX_RETRY_MS), "at most a minute");

    let interrupted = timed("interrupted", reply(INTERRUPTED)).await.unwrap();
    assert_eq!(
        (interrupted.result.as_str(), interrupted.reason.as_deref()),
        (result::FAILED, Some(reason::INTERRUPTED))
    );

    assert!(matches!(timed("gone", reply(GONE)).await, Err(Error::NotFound)));
    let busy = timed("busy", reply(BUSY_PC)).await;
    assert!(matches!(busy, Err(Error::Protocol(ref r)) if r.starts_with("BUSY")), "{busy:?}");

    // A platform answer that isn't whole: the PC answers INTERNAL, which a
    // phone can't read as anything: not known whether it was typed.
    for session in [NO_TARGET, READY_TO_TEXT, UNKNOWN_REASON] {
        assert!(matches!(timed(session, reply(session)).await, Err(Error::Timeout)), "{session}");
    }
    // A check isn't answered `sent`, and never `failed`.
    let check = timed("check", phone.node.agent_session_reply_check(pc_id, INTERRUPTED.into())).await;
    assert!(matches!(check, Err(Error::Timeout)), "{check:?}");
    assert_eq!(pc.host.requests().len(), 8, "each went once");
}
