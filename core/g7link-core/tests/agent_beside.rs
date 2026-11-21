// SPDX-License-Identifier: MPL-2.0
//! The phone beside the PC (`docs/protocol/agents.md` §11): real nodes over
//! localhost, a PC that switches the pane a phone shows and lists the ports
//! a thread's agent listens on.

use std::{sync::Arc, time::Duration};

use g7link_core::{
    AgentBeside, AgentEvent, AgentPort, AgentThread, AgentViewing, DeviceId, DeviceInfo, DeviceKind, Error,
    LinkState, Node, NodeConfig, NodeEvent, NoopPlatform, PairingEvent, PlainKeyProtector, PowerLevel, agent,
    presence,
};
use g7link_protocol::{
    ble,
    lanes::{Lane, lane_of},
    messages::types,
};
use tempfile::TempDir;
use tokio::sync::broadcast::{Receiver, error::TryRecvError};

const WAIT: Duration = Duration::from_secs(20);
const THREAD: &str = "0123456789abcdef0123456789abcdef";
const OTHER: &str = "fedcba9876543210fedcba9876543210";

struct Device {
    node: Node,
    events: Receiver<NodeEvent>,
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
    let node = Node::start(config, Arc::new(NoopPlatform)).await.expect("node starts");
    node.update_power(PowerLevel::NotApplicable, offers.iter().map(|c| (*c).to_owned()).collect()).await;
    let events = node.events();
    Device { node, events, _dir: dir, name }
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

fn beside(thread: &str, pane: &str) -> AgentBeside {
    AgentBeside { thread: thread.into(), pane: pane.into() }
}

/// The pane `pc` asked `phone` to show beside `thread`.
async fn asked(phone: &mut Device, pc: DeviceId, thread: &str) -> String {
    wait_for(phone, &format!("agent.beside of {thread}"), |e| match e {
        NodeEvent::AgentBeside { device, thread: t, pane } if *device == pc && t == thread => {
            Some(pane.clone())
        }
        _ => None,
    })
    .await
}

fn besides(events: &[NodeEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            NodeEvent::AgentBeside { thread, pane, .. } => Some((thread.clone(), pane.clone())),
            _ => None,
        })
        .collect()
}

fn thread_with(ports: Vec<AgentPort>) -> AgentThread {
    AgentThread {
        id: THREAD.into(),
        agent: "claude".into(),
        folder: "g7linux".into(),
        title: "Start the dev server".into(),
        state: "working".into(),
        mode: "supervised".into(),
        updated: 1_795_000_000_000,
        ports,
        ..Default::default()
    }
}

/// The ports of the `state` event for `title` that `phone` got from `pc`.
async fn ports_of(phone: &mut Device, pc: DeviceId, title: &str) -> Vec<(u16, bool)> {
    wait_for(phone, &format!("the state of {title}"), |e| match e {
        NodeEvent::AgentEvent { device, event } if *device == pc && event.kind == "state" => {
            let state = event.state.as_ref()?;
            (state.title == title).then(|| state.ports.iter().map(|p| (p.port, p.is_lan())).collect())
        }
        _ => None,
    })
    .await
}

#[test]
fn the_beside_rides_the_agent_lane_and_never_bluetooth() {
    assert_eq!(lane_of(types::AGENT_BESIDE), Some(Lane::Service(g7link_protocol::lanes::svc::AGENT)));
    assert!(!ble::carries(types::AGENT_BESIDE));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_switches_the_pane_of_a_phone_with_agents_on() {
    let (mut pc, mut pixel) = pair().await;
    let (pc_id, pixel_id) = (pc.node.device_id(), pixel.node.device_id());

    // Agents off: nothing goes, and the PC says why.
    assert!(matches!(
        timed("off", pc.node.agent_beside(pixel_id, beside(THREAD, presence::pane::PLAN))).await,
        Err(Error::Denied)
    ));

    pc.node.set_device_toggle(pixel_id, agent::TOGGLE, true).unwrap();
    for pane in
        [presence::pane::PLAN, presence::pane::REQUESTS, presence::pane::CHANGES, presence::pane::THREAD]
    {
        timed(pane, pc.node.agent_beside(pixel_id, beside(THREAD, pane))).await.unwrap();
        assert_eq!(asked(&mut pixel, pc_id, THREAD).await, pane);
    }

    // The other way: the phone says the pane it shows, and the PC has it.
    let showing =
        AgentViewing { thread: Some(THREAD.into()), session: None, visible: true, pane: Some("plan".into()) };
    pixel.node.agent_viewing(pc_id, showing).unwrap();
    let shown = wait_for(&mut pc, "the phone's plan pane", |e| match e {
        NodeEvent::AgentViewing { device, view: Some(view) }
            if *device == pixel_id && view.pane.as_deref() == Some(presence::pane::PLAN) =>
        {
            Some(view.thread.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(shown.as_deref(), Some(THREAD));

    // A pane the PC doesn't know, or a thread that isn't an id, never goes.
    assert!(matches!(
        timed("later pane", pc.node.agent_beside(pixel_id, beside(THREAD, "diff_beside"))).await,
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        timed("a path", pc.node.agent_beside(pixel_id, beside("/home/me", presence::pane::PLAN))).await,
        Err(Error::Protocol(_))
    ));
    // A phone the PC never paired.
    assert!(matches!(
        timed("a stranger", pc.node.agent_beside(DeviceId([9; 32]), beside(THREAD, presence::pane::PLAN)))
            .await,
        Err(Error::NotPaired)
    ));
    // The marker after the refused ones: it is the next to arrive.
    timed("marker", pc.node.agent_beside(pixel_id, beside(OTHER, presence::pane::CHANGES))).await.unwrap();
    assert_eq!(asked(&mut pixel, pc_id, OTHER).await, presence::pane::CHANGES);
    let rest = besides(&drain(&mut pixel));
    assert!(rest.is_empty(), "{rest:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_drops_a_beside_out_of_its_limits_and_a_pc_drops_any() {
    let (mut pc, mut phone) = pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, true).unwrap();

    let push = |body: &AgentBeside| pc.node.debug_agent_push(phone_id, types::AGENT_BESIDE, body).unwrap();
    push(&beside(THREAD, "diff_beside"));
    push(&beside(THREAD, "Plan!"));
    push(&beside("../etc", presence::pane::PLAN));
    push(&beside("", presence::pane::PLAN));
    // The first to arrive is the one within its limits.
    push(&beside(OTHER, presence::pane::REQUESTS));
    let first = wait_for(&mut phone, "a beside", |e| match e {
        NodeEvent::AgentBeside { device, thread, pane } if *device == pc_id => {
            Some((thread.clone(), pane.clone()))
        }
        _ => None,
    })
    .await;
    assert_eq!(first, (OTHER.to_owned(), presence::pane::REQUESTS.to_owned()));

    // A PC takes none.
    phone.node.debug_agent_push(pc_id, types::AGENT_BESIDE, &beside(THREAD, presence::pane::PLAN)).unwrap();
    phone.node.set_reconnect(pc_id, false).unwrap();
    wait_link(&mut pc, phone_id, false).await;
    assert!(besides(&drain(&mut pc)).is_empty(), "a PC takes no beside");

    // Offline: nothing goes.
    wait_link(&mut phone, pc_id, false).await;
    assert!(matches!(
        timed("offline", pc.node.agent_beside(phone_id, beside(THREAD, presence::pane::PLAN))).await,
        Err(Error::Offline)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_threads_ports_reach_the_phone_within_their_limits() {
    let (pc, mut phone) = pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, true).unwrap();

    // The PC makes them fit before they go: under 1024, a repeat, the fifth.
    let sent = thread_with(vec![
        AgentPort::new(80, true),
        AgentPort::new(5173, true),
        AgentPort::new(5173, false),
        AgentPort::new(8080, false),
        AgentPort::new(3000, true),
        AgentPort::new(3001, true),
        AgentPort::new(3002, true),
    ]);
    timed("state", pc.node.agent_event(AgentEvent::state(sent), None)).await.unwrap();
    let ports = ports_of(&mut phone, pc_id, "Start the dev server").await;
    assert_eq!(ports, [(5173, true), (8080, false), (3000, true), (3001, true)]);

    // A thread over its limits is dropped; a reach this build doesn't know is left out.
    let five = thread_with((0..5).map(|n| AgentPort::new(4000 + n, true)).collect());
    let five = AgentThread { title: "Five".into(), ..five };
    pc.node.debug_agent_push(phone_id, types::AGENT_EVENT, &AgentEvent::state(five)).unwrap();
    let later = AgentThread {
        title: "Later".into(),
        ports: vec![AgentPort { port: 4100, reach: "tunnel".into() }, AgentPort::new(4101, false)],
        ..thread_with(Vec::new())
    };
    pc.node.debug_agent_push(phone_id, types::AGENT_EVENT, &AgentEvent::state(later)).unwrap();
    assert_eq!(ports_of(&mut phone, pc_id, "Later").await, [(4101, false)]);
    let titles: Vec<String> = drain(&mut phone)
        .iter()
        .filter_map(|e| match e {
            NodeEvent::AgentEvent { event, .. } => event.state.as_ref().map(|s| s.title.clone()),
            _ => None,
        })
        .collect();
    assert!(!titles.contains(&"Five".to_owned()), "{titles:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_port_has_no_address_here_without_a_lan_path() {
    let (mut pc, mut phone) = pair().await;
    let pc_id = pc.node.device_id();
    // Over loopback a browser here would open the phone itself: no address.
    // A test link may also take the machine's own network address, which a
    // phone could open. (Which path wins is in the core's unit tests.)
    if let Some(url) = phone.node.agent_port_url(pc_id, 5173) {
        assert!(url.starts_with("http://") && url.ends_with(":5173/"), "{url}");
        assert!(!url.contains("127.0.0.1") && !url.contains("[::1]"), "{url}");
    }
    assert_eq!(phone.node.agent_port_url(pc_id, 80), None, "under 1024");
    assert_eq!(phone.node.agent_port_url(DeviceId([9; 32]), 5173), None, "not a paired PC");
    // And the PC's side, for "Open on <phone>".
    let phone_id = phone.node.device_id();
    if let Some(url) = pc.node.agent_port_url_for_phone(phone_id, 5173) {
        assert!(url.starts_with("http://") && url.ends_with(":5173/"), "{url}");
        assert!(!url.contains("127.0.0.1") && !url.contains("[::1]"), "{url}");
    }
    assert_eq!(pc.node.agent_port_url_for_phone(phone_id, 80), None, "under 1024");
    assert_eq!(pc.node.agent_port_url_for_phone(DeviceId([9; 32]), 5173), None, "not a paired phone");
    phone.node.set_reconnect(pc_id, false).unwrap();
    wait_link(&mut phone, pc_id, false).await;
    assert_eq!(phone.node.agent_port_url(pc_id, 5173), None, "offline");
    wait_link(&mut pc, phone_id, false).await;
    assert_eq!(pc.node.agent_port_url_for_phone(phone_id, 5173), None, "offline");
}
