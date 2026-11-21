// SPDX-License-Identifier: MPL-2.0
//! Hand-off (`docs/protocol/agents.md` §10): real nodes over localhost, a PC
//! that hands threads to its phones, and whose platform opens the ones a
//! phone hands it.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use g7link_core::{
    AgentFailure, AgentHandedOff, AgentHandoff, AgentHandoffOpen, AgentPcHandoff, AgentReply, AgentRequest,
    DeviceId, DeviceInfo, DeviceKind, Error, LinkState, Node, NodeConfig, NodeEvent, PairingEvent,
    PlainKeyProtector, Platform, PowerLevel, agent, agent_handoff::handoff,
};
use g7link_protocol::{
    ble,
    lanes::{Lane, lane_of, rpc_of, svc},
    messages::types,
};
use tempfile::TempDir;
use tokio::sync::broadcast::{Receiver, error::TryRecvError};

const WAIT: Duration = Duration::from_secs(20);
/// Threads the platform answers in one way each.
const OPEN: &str = "0123456789abcdef0123456789abcdef";
const LOCKED: &str = "aaaa0000bbbb1111cccc2222dddd3333";
const AWAY: &str = "eeee4444ffff55556666777788889999";
const ODD: &str = "99998888777766665555444433332222";
const GONE: &str = "1111222233334444";

/// The desktop's side of a hand-off, as far as these tests need one.
#[derive(Debug, Default)]
struct HandoffPlatform {
    requests: Mutex<Vec<(DeviceId, AgentRequest)>>,
}

impl HandoffPlatform {
    fn requests(&self) -> Vec<(DeviceId, AgentRequest)> {
        self.requests.lock().unwrap().clone()
    }
}

impl Platform for HandoffPlatform {
    fn agent_request(&self, from: &DeviceId, request: AgentRequest) -> Result<AgentReply, AgentFailure> {
        self.requests.lock().unwrap().push((*from, request.clone()));
        let AgentRequest::Handoff { thread, .. } = request else {
            return Err(AgentFailure::Unsupported);
        };
        Ok(AgentReply::HandedOff(match thread.as_str() {
            OPEN => AgentHandedOff::opened(),
            LOCKED => AgentHandedOff::locked(),
            AWAY => AgentHandedOff::away(),
            ODD => AgentHandedOff { result: "on_the_tv".into() },
            _ => return Err(AgentFailure::NotFound),
        }))
    }
}

struct Device {
    node: Node,
    events: Receiver<NodeEvent>,
    host: Arc<HandoffPlatform>,
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
    let host = Arc::new(HandoffPlatform::default());
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

async fn wait_link(dev: &mut Device, peer: DeviceId, online: bool) {
    wait_for(dev, if online { "peer online" } else { "peer offline" }, |e| match e {
        NodeEvent::LinkChanged { device, link } if *device == peer => {
            (matches!(link, LinkState::Online { .. }) == online).then_some(())
        }
        _ => None,
    })
    .await
}

fn phone(name: &'static str) -> impl std::future::Future<Output = Device> {
    start(name, DeviceKind::Phone, &[agent::ITEMS2, agent::CONTROL, agent::FOLLOW])
}

/// Pairs `phone` with `pc` and waits until both see the other online; every
/// toggle stays off.
async fn pair_with(pc: &mut Device, phone: &mut Device) {
    let uri = pc.node.pairing_start_qr().await.unwrap();
    timed("pairing", phone.node.pairing_join(&uri.to_uri())).await.expect("paired");
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    wait_for(pc, "paired", |e| match e {
        NodeEvent::Pairing(PairingEvent::Paired(d)) if d.id == phone_id => Some(()),
        _ => None,
    })
    .await;
    wait_link(phone, pc_id, true).await;
    wait_link(pc, phone_id, true).await;
}

/// A PC with the agent host and a phone, every toggle off.
async fn pair() -> (Device, Device) {
    let mut pc = start("Desktop", DeviceKind::Desktop, &[agent::HOST, agent::CONTROL]).await;
    let mut phone = phone("Pixel").await;
    pair_with(&mut pc, &mut phone).await;
    (pc, phone)
}

fn now_ms() -> i64 {
    i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis())
        .unwrap()
}

fn handing(thread: &str, seq: Option<u64>, offer: bool) -> AgentHandoff {
    AgentHandoff { thread: thread.into(), seq, offer, ttl_ms: handoff::TTL_MS }
}

/// `phone`'s hand-off of `thread` from `pc`, as an offer or not.
async fn handed(phone: &mut Device, pc: DeviceId, thread: &str, offer: bool) -> AgentPcHandoff {
    let what = format!("agent.handoff of {thread} (offer: {offer})");
    wait_for(phone, &what, |e| match e {
        NodeEvent::AgentHandoff { device, handoff }
            if *device == pc && handoff.thread == thread && handoff.offer == offer =>
        {
            Some(handoff.clone())
        }
        _ => None,
    })
    .await
}

fn handoffs(events: &[NodeEvent]) -> Vec<&AgentPcHandoff> {
    events
        .iter()
        .filter_map(|e| match e {
            NodeEvent::AgentHandoff { handoff, .. } => Some(handoff),
            _ => None,
        })
        .collect()
}

#[test]
fn the_handoff_rides_the_agent_lane_its_open_is_an_rpc_and_neither_rides_bluetooth() {
    assert_eq!(lane_of(types::AGENT_HANDOFF), Some(Lane::Service(svc::AGENT)));
    assert_eq!(lane_of(types::AGENT_HANDOFF_OPEN), Some(Lane::Rpc { svc: svc::AGENT, op: "handoff.open" }));
    assert!(rpc_of(types::AGENT_HANDOFF_OPEN).is_some_and(|op| !op.paged));
    assert!(!ble::carries(types::AGENT_HANDOFF));
    assert!(!ble::carries(types::AGENT_HANDOFF_OPEN));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_hands_a_thread_to_the_phone_it_names_and_offers_one_to_every_phone_with_agents_on() {
    let (mut pc, mut pixel) = pair().await;
    let mut tablet = phone("Tablet").await;
    pair_with(&mut pc, &mut tablet).await;
    let (pc_id, pixel_id, tablet_id) = (pc.node.device_id(), pixel.node.device_id(), tablet.node.device_id());

    // Agents off for both: nothing goes, and the PC says why.
    assert!(matches!(
        timed("to the pixel", pc.node.agent_handoff(Some(pixel_id), handing(OPEN, None, false))).await,
        Err(Error::Denied)
    ));
    assert_eq!(timed("offer", pc.node.agent_handoff(None, handing(OPEN, None, true))).await.unwrap(), 0);

    // On for the Pixel only.
    pc.node.set_device_toggle(pixel_id, agent::TOGGLE, true).unwrap();
    let before = now_ms();
    let sent = timed("explicit", pc.node.agent_handoff(Some(pixel_id), handing(OPEN, Some(42), false))).await;
    assert_eq!(sent.unwrap(), 1);
    let got = handed(&mut pixel, pc_id, OPEN, false).await;
    assert_eq!(got.seq, Some(42));
    let ttl = i64::try_from(handoff::TTL_MS).unwrap();
    assert!(got.expires_ms >= before + ttl && got.expires_ms <= now_ms() + ttl, "{}", got.expires_ms);
    assert!(matches!(
        timed("to the tablet", pc.node.agent_handoff(Some(tablet_id), handing(OPEN, None, false))).await,
        Err(Error::Denied)
    ));

    // An offer goes to each phone with agents on, held no more than 10 minutes.
    let long = AgentHandoff { ttl_ms: 3_600_000, ..handing(LOCKED, None, true) };
    assert_eq!(timed("offer", pc.node.agent_handoff(None, long)).await.unwrap(), 1);
    let got = handed(&mut pixel, pc_id, LOCKED, true).await;
    assert_eq!(got.seq, None);
    assert!(got.expires_ms <= now_ms() + ttl, "brought into its range: {}", got.expires_ms);

    pc.node.set_device_toggle(tablet_id, agent::TOGGLE, true).unwrap();
    assert_eq!(timed("offer", pc.node.agent_handoff(None, handing(AWAY, Some(7), true))).await.unwrap(), 2);
    assert_eq!(handed(&mut pixel, pc_id, AWAY, true).await.seq, Some(7));
    assert_eq!(handed(&mut tablet, pc_id, AWAY, true).await.seq, Some(7));
    // The tablet heard only the offer made while its toggle was on.
    let earlier: Vec<String> = handoffs(&drain(&mut tablet)).iter().map(|h| h.thread.clone()).collect();
    assert!(earlier.is_empty(), "{earlier:?}");

    // Not an id: nothing goes.
    assert!(matches!(
        timed("a path", pc.node.agent_handoff(None, handing("/home/me", None, true))).await,
        Err(Error::Protocol(_))
    ));
    // A phone the PC never paired.
    let stranger = DeviceId([9; 32]);
    assert!(matches!(
        timed("a stranger", pc.node.agent_handoff(Some(stranger), handing(OPEN, None, false))).await,
        Err(Error::NotPaired)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_drops_a_handoff_out_of_its_limits() {
    let (pc, mut phone) = pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, true).unwrap();

    let push = |body: &AgentHandoff| pc.node.debug_agent_push(phone_id, types::AGENT_HANDOFF, body).unwrap();
    push(&AgentHandoff { ttl_ms: handoff::MAX_TTL_MS + 1, ..handing(OPEN, None, false) });
    push(&AgentHandoff { ttl_ms: handoff::MIN_TTL_MS - 1, ..handing(OPEN, None, false) });
    push(&handing("../etc", None, false));
    push(&handing("", None, false));
    // The first to arrive is the one within its limits.
    let short = AgentHandoff { ttl_ms: handoff::MIN_TTL_MS, ..handing(OPEN, Some(3), false) };
    let before = now_ms();
    push(&short);
    let got = wait_for(&mut phone, "a hand-off", |e| match e {
        NodeEvent::AgentHandoff { device, handoff } if *device == pc_id => Some(handoff.clone()),
        _ => None,
    })
    .await;
    assert_eq!((got.thread.as_str(), got.seq), (OPEN, Some(3)));
    assert!(got.expires_ms >= before + 1_000 && got.expires_ms <= now_ms() + 1_000, "{}", got.expires_ms);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_hands_a_thread_to_its_pc_which_opens_it_or_keeps_it_for_its_unlock() {
    let (mut pc, phone) = pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let open = |thread: &str, seq| phone.node.agent_handoff_open(pc_id, thread.into(), seq);

    // Agents off: refused, and the PC asks its user once.
    assert!(matches!(timed("off", open(OPEN, Some(42))).await, Err(Error::Denied)));
    wait_for(&mut pc, "agents prompt", |e| match e {
        NodeEvent::AgentsRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert!(pc.host.requests().is_empty(), "nothing reached the platform");

    pc.node.set_device_toggle(phone_id, agent::TOGGLE, true).unwrap();
    let done = timed("open", open(OPEN, Some(42))).await.unwrap();
    assert_eq!(done, AgentHandedOff::opened());
    let requests = pc.host.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, phone_id);
    assert!(
        matches!(&requests[0].1, AgentRequest::Handoff { thread, seq: Some(42) } if thread == OPEN),
        "{:?}",
        requests[0].1
    );

    assert_eq!(timed("locked", open(LOCKED, None)).await.unwrap(), AgentHandedOff::locked());
    assert!(matches!(&pc.host.requests()[1].1, AgentRequest::Handoff { seq: None, .. }));
    assert_eq!(timed("away", open(AWAY, Some(0))).await.unwrap(), AgentHandedOff::away());
    assert!(matches!(timed("gone", open(GONE, None)).await, Err(Error::NotFound)));
    // A word the PC's platform made up never reaches the phone.
    match timed("odd", open(ODD, None)).await {
        Err(Error::Protocol(reason)) => assert!(reason.starts_with("INTERNAL"), "{reason}"),
        other => panic!("{other:?}"),
    }

    // The phone never sends one out of its limits; the PC refuses one anyway.
    assert!(matches!(timed("a path", open("/home/me", None)).await, Err(Error::Protocol(_))));
    let raw = AgentHandoffOpen { thread: "../x".into(), seq: None };
    match timed("raw", phone.node.debug_agent_request(pc_id, types::AGENT_HANDOFF_OPEN, &raw)).await {
        Err(Error::Protocol(reason)) => assert!(reason.starts_with("BAD_MESSAGE"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(pc.host.requests().len(), 5, "the refused ones never reached the platform");
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_is_handed_over_while_offline_and_a_phone_never_answers_an_open() {
    let (mut pc, mut phone) = pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, true).unwrap();

    // A phone has no agent host: it refuses an open and never asks its platform.
    assert!(matches!(
        timed(
            "to the phone",
            pc.node.debug_agent_request(
                phone_id,
                types::AGENT_HANDOFF_OPEN,
                &AgentHandoffOpen { thread: OPEN.into(), seq: None }
            )
        )
        .await,
        Err(Error::Unsupported)
    ));
    assert!(phone.host.requests().is_empty());
    // And a PC drops a hand-off sent to it.
    phone.node.debug_agent_push(pc_id, types::AGENT_HANDOFF, &handing(OPEN, None, false)).unwrap();

    phone.node.set_reconnect(pc_id, false).unwrap();
    wait_link(&mut pc, phone_id, false).await;
    wait_link(&mut phone, pc_id, false).await;
    assert!(matches!(
        timed("offline", pc.node.agent_handoff(Some(phone_id), handing(OPEN, None, false))).await,
        Err(Error::Offline)
    ));
    assert_eq!(timed("offer", pc.node.agent_handoff(None, handing(OPEN, None, true))).await.unwrap(), 0);
    assert!(matches!(
        timed("open", phone.node.agent_handoff_open(pc_id, OPEN.into(), None)).await,
        Err(Error::Offline)
    ));
    assert!(pc.host.requests().is_empty());
    assert!(handoffs(&drain(&mut pc)).is_empty(), "a PC takes no hand-off");
}
