// SPDX-License-Identifier: MPL-2.0
//! Presence (`docs/protocol/agents.md` §7): two real nodes over localhost, a
//! PC that tells its phone whether someone is at it and what's on its
//! screen, and a phone that tells its PC what it shows.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use g7link_core::{
    AgentPcPresence, AgentPresence, AgentView, AgentViewing, DeviceId, DeviceInfo, DeviceKind, Error,
    LinkState, Node, NodeConfig, NodeEvent, NoopPlatform, PairingEvent, PlainKeyProtector, PowerLevel, agent,
    presence,
};
use g7link_protocol::{
    ble,
    lanes::{Lane, lane_of, rpc_of, svc},
    messages::types,
};
use tempfile::TempDir;
use tokio::sync::broadcast::{Receiver, error::TryRecvError};

const WAIT: Duration = Duration::from_secs(20);
const FOLDER: &str = "f0123456789ab";
const A: &str = "0123456789abcdef0123456789abcdef";
const B: &str = "aaaa0000bbbb1111cccc2222dddd3333";
const C: &str = "eeee4444ffff55556666777788889999";
const D: &str = "99998888777766665555444433332222";
const SESSION: &str = "sc0123456789abcdef0123456789abcdef";

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
        }],
        modes: vec!["supervised".into()],
        push: false,
    }
}

async fn wait_catalog(phone: &mut Device, pc: DeviceId) {
    wait_for(phone, "agent catalog", |e| match e {
        NodeEvent::AgentCatalog { device, catalog: Some(_), .. } if *device == pc => Some(()),
        _ => None,
    })
    .await
}

/// A PC with the agent host and sessions, paired with a phone that follows
/// sessions too, with the `agents` toggle on and `agent_sessions` off.
async fn pair() -> (Device, Device) {
    let mut pc = start("Desktop", DeviceKind::Desktop, &[agent::HOST, agent::CONTROL, agent::SESSIONS]).await;
    let mut phone = start("Pixel", DeviceKind::Phone, &[agent::ITEMS2, agent::CONTROL, agent::FOLLOW]).await;
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
    wait_catalog(&mut phone, pc_id).await;
    (pc, phone)
}

fn now_ms() -> i64 {
    i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis())
        .unwrap()
}

fn said(viewing: &[&str], sessions: &[&str]) -> AgentPresence {
    AgentPresence {
        at_pc: true,
        locked: false,
        dnd: false,
        viewing: viewing.iter().map(|id| (*id).to_owned()).collect(),
        sessions: sessions.iter().map(|id| (*id).to_owned()).collect(),
        ttl_ms: presence::TTL_MS,
    }
}

async fn next_presence(phone: &mut Device, pc: DeviceId) -> AgentPcPresence {
    wait_for(phone, "agent.presence", |e| match e {
        NodeEvent::AgentPresence { device, presence } if *device == pc => Some(presence.clone()),
        _ => None,
    })
    .await
}

/// The PC's next word of what `phone` shows, and when it came.
async fn next_view(pc: &mut Device, phone: DeviceId) -> (Option<AgentView>, Instant) {
    wait_for(pc, "agent.viewing", |e| match e {
        NodeEvent::AgentViewing { device, view } if *device == phone => Some((view.clone(), Instant::now())),
        _ => None,
    })
    .await
}

fn shows(thread: &str, pane: Option<&str>) -> AgentViewing {
    AgentViewing { thread: Some(thread.into()), session: None, visible: true, pane: pane.map(Into::into) }
}

fn leaves(thread: Option<&str>) -> AgentViewing {
    AgentViewing { thread: thread.map(Into::into), session: None, visible: false, pane: None }
}

fn thread_of(view: &Option<AgentView>) -> Option<&str> {
    view.as_ref().and_then(|v| v.thread.as_deref())
}

#[test]
fn presence_rides_the_agent_lane_viewing_is_an_rpc_and_neither_rides_bluetooth() {
    assert_eq!(lane_of(types::AGENT_PRESENCE), Some(Lane::Service(svc::AGENT)));
    assert_eq!(lane_of(types::AGENT_VIEWING), Some(Lane::Rpc { svc: svc::AGENT, op: "viewing" }));
    assert!(rpc_of(types::AGENT_VIEWING).is_some_and(|op| !op.paged));
    assert!(!ble::carries(types::AGENT_PRESENCE));
    assert!(!ble::carries(types::AGENT_VIEWING));
}

#[tokio::test(flavor = "multi_thread")]
async fn presence_reaches_a_phone_with_agents_on_and_sessions_only_with_agent_sessions() {
    let (pc, mut phone) = pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    assert_eq!(phone.node.agent_presence_of(pc_id), None, "nothing said yet");

    let before = now_ms();
    pc.node.agent_presence(said(&[A, B], &[SESSION])).unwrap();
    let got = next_presence(&mut phone, pc_id).await;
    assert!(got.at_pc && !got.locked && !got.dnd);
    assert_eq!(got.viewing, [A, B]);
    assert!(got.sessions.is_empty(), "agent_sessions is off for this phone");
    assert!(got.until_ms >= before + 60_000 && got.until_ms <= now_ms() + 60_000, "{}", got.until_ms);
    assert_eq!(phone.node.agent_presence_of(pc_id), Some(got));

    pc.node.set_device_toggle(phone_id, agent::SESSIONS_TOGGLE, true).unwrap();
    pc.node.agent_presence(said(&[A], &[SESSION])).unwrap();
    assert_eq!(next_presence(&mut phone, pc_id).await.sessions, [SESSION]);

    // Made to fit on the PC: no path, four at most, nobody at a locked PC.
    let mut many = said(&["/home/me/g7linux", A, A, B, C, D, "1111"], &[]);
    many.locked = true;
    pc.node.agent_presence(many).unwrap();
    let got = next_presence(&mut phone, pc_id).await;
    assert_eq!(got.viewing, [A, B, C, D]);
    assert!(got.locked && !got.at_pc);

    // Agents off: the last, empty catalog, and the presence goes with it.
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, false).unwrap();
    wait_for(&mut phone, "the empty catalog", |e| match e {
        NodeEvent::AgentCatalog { device, catalog: None, .. } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    assert_eq!(phone.node.agent_presence_of(pc_id), None);
    let mut quiet = said(&[], &[]);
    quiet.dnd = true;
    pc.node.agent_presence(quiet).unwrap();

    // Nothing reached it while off. Let back in, it gets the presence that
    // still holds, after its catalog, with what's left of its time.
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, true).unwrap();
    let mut early = false;
    wait_for(&mut phone, "the catalog", |e| match e {
        NodeEvent::AgentPresence { device, .. } if *device == pc_id => {
            early = true;
            None
        }
        NodeEvent::AgentCatalog { device, catalog: Some(_), .. } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    assert!(!early, "a presence reached a phone with agents off");
    let got = next_presence(&mut phone, pc_id).await;
    assert!(got.dnd && got.viewing.is_empty(), "the one given while it was off: {got:?}");
    assert!(got.until_ms <= now_ms() + 60_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_presence_out_of_its_limits_is_dropped_and_one_that_lapsed_or_lost_its_link_says_nothing() {
    let (mut pc, mut phone) = pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let push =
        |body: &AgentPresence| pc.node.debug_agent_push(phone_id, types::AGENT_PRESENCE, body).unwrap();
    push(&AgentPresence { ttl_ms: presence::MAX_TTL_MS + 1, ..said(&[], &[]) });
    push(&AgentPresence { ttl_ms: presence::MIN_TTL_MS - 1, ..said(&[], &[]) });
    push(&said(&[A, B, C, D, "1111"], &[]));
    push(&said(&[A, A], &[]));
    push(&said(&["../etc"], &[]));
    // At the PC while locked reads as not at the PC.
    let mut short = said(&[A], &[]);
    short.locked = true;
    short.ttl_ms = presence::MIN_TTL_MS;
    push(&short);
    let got = next_presence(&mut phone, pc_id).await;
    assert!(got.locked && !got.at_pc, "the first to arrive is the one within its limits: {got:?}");
    assert_eq!(got.viewing, [A]);
    assert!(phone.node.agent_presence_of(pc_id).is_some());
    tokio::time::sleep(Duration::from_millis(1_300)).await;
    assert_eq!(phone.node.agent_presence_of(pc_id), None, "after until_ms");

    pc.node.agent_presence(said(&[A], &[])).unwrap();
    next_presence(&mut phone, pc_id).await;
    assert!(phone.node.agent_presence_of(pc_id).is_some());
    phone.node.agent_viewing(pc_id, shows(A, None)).unwrap();
    assert_eq!(thread_of(&next_view(&mut pc, phone_id).await.0), Some(A));
    assert_eq!(pc.node.agent_views().len(), 1);

    // The link drops: the phone's presence and the PC's view of it go.
    phone.node.set_reconnect(pc_id, false).unwrap();
    wait_link(&mut pc, phone_id, false).await;
    wait_link(&mut phone, pc_id, false).await;
    assert_eq!(phone.node.agent_presence_of(pc_id), None, "offline");
    assert!(pc.node.agent_views().is_empty(), "offline");
    assert!(matches!(phone.node.agent_viewing(pc_id, shows(B, None)), Err(Error::Offline)), "never queued");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_says_what_it_shows_in_order_and_at_most_every_two_seconds() {
    let (mut pc, phone) = pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Twice within 2 s: one goes.
    phone.node.agent_viewing(pc_id, shows(A, Some(presence::pane::CHANGES))).unwrap();
    phone.node.agent_viewing(pc_id, shows(A, Some(presence::pane::CHANGES))).unwrap();
    let (view, _) = next_view(&mut pc, phone_id).await;
    let view = view.expect("A shows");
    assert_eq!(view.thread.as_deref(), Some(A));
    assert_eq!(view.pane.as_deref(), Some(presence::pane::CHANGES));
    tokio::time::sleep(Duration::from_millis(2_500)).await;

    // The repeat never went: the next word is B's.
    phone.node.agent_viewing(pc_id, shows(B, None)).unwrap();
    let (view, shown_at) = next_view(&mut pc, phone_id).await;
    assert_eq!(thread_of(&view), Some(B), "{view:?}");
    // `visible: false` goes at once, though B went a moment ago.
    phone.node.agent_viewing(pc_id, leaves(Some(B))).unwrap();
    let (view, left_at) = next_view(&mut pc, phone_id).await;
    assert_eq!(view, None);
    assert!(left_at - shown_at < Duration::from_millis(1_500), "{:?}", left_at - shown_at);

    // A change within 2 s waits for its turn; a late "left A" doesn't undo it.
    phone.node.agent_viewing(pc_id, shows(C, None)).unwrap();
    phone.node.agent_viewing(pc_id, leaves(Some(A))).unwrap();
    let (view, shown_at) = next_view(&mut pc, phone_id).await;
    assert_eq!(thread_of(&view), Some(C), "{view:?}");
    assert!(shown_at - left_at >= Duration::from_millis(1_500), "{:?}", shown_at - left_at);
    let views = pc.node.agent_views();
    assert_eq!(views.len(), 1);
    assert_eq!((views[0].0, views[0].1.thread.as_deref()), (phone_id, Some(C)));

    // On the PC a late "left A" changes nothing either, and a pane it
    // doesn't know reads as absent: the next word is D's.
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    phone.node.agent_viewing(pc_id, leaves(Some(A))).unwrap();
    let raw = serde_json::json!({ "thread": D, "visible": true, "pane": "diff_beside" });
    phone.node.debug_agent_request(pc_id, types::AGENT_VIEWING, &raw).await.unwrap();
    let (view, _) = next_view(&mut pc, phone_id).await;
    let view = view.expect("D shows");
    assert_eq!((view.thread.as_deref(), view.pane.as_deref()), (Some(D), None));
    assert_eq!(pc.node.agent_views()[0].1.thread.as_deref(), Some(D));

    // Agents off: what it showed goes.
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, false).unwrap();
    assert_eq!(next_view(&mut pc, phone_id).await.0, None);
    assert!(pc.node.agent_views().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_refuses_views_it_may_not_take_and_never_prompts_for_them() {
    let (mut pc, phone) = pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let ask = |body: serde_json::Value| {
        let node = &phone.node;
        async move { node.debug_agent_request(pc_id, types::AGENT_VIEWING, &body).await }
    };
    let session = serde_json::json!({ "session": SESSION, "visible": true });
    assert!(matches!(ask(session.clone()).await, Err(Error::Denied)), "agent_sessions is off");
    let nothing = serde_json::json!({ "visible": true });
    assert!(matches!(ask(nothing).await, Err(Error::Protocol(_))));
    let both = serde_json::json!({ "thread": A, "session": SESSION, "visible": true });
    assert!(matches!(ask(both).await, Err(Error::Protocol(_))));
    let path = serde_json::json!({ "thread": "/home/me", "visible": true });
    assert!(matches!(ask(path).await, Err(Error::Protocol(_))));
    // The phone doesn't send them at all.
    let both = AgentViewing { session: Some(SESSION.into()), ..shows(A, None) };
    assert!(matches!(phone.node.agent_viewing(pc_id, both), Err(Error::Protocol(_))));
    assert!(matches!(phone.node.agent_viewing(pc_id, leaves(None)), Ok(())), "leaving whatever it showed");
    assert_eq!(next_view(&mut pc, phone_id).await.0, None);

    // With agent_sessions on, a session view goes.
    pc.node.set_device_toggle(phone_id, agent::SESSIONS_TOGGLE, true).unwrap();
    ask(session).await.unwrap();
    let view = next_view(&mut pc, phone_id).await.0.expect("the session shows");
    assert_eq!((view.thread.as_deref(), view.session.as_deref()), (None, Some(SESSION)));
    // Off again: it goes.
    pc.node.set_device_toggle(phone_id, agent::SESSIONS_TOGGLE, false).unwrap();
    assert_eq!(next_view(&mut pc, phone_id).await.0, None);

    // Agents off: refused too.
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, false).unwrap();
    assert!(matches!(ask(serde_json::json!({ "thread": A, "visible": true })).await, Err(Error::Denied)));

    // None of it asked the PC's user anything.
    let prompts: Vec<NodeEvent> = drain(&mut pc)
        .into_iter()
        .filter(|e| matches!(e, NodeEvent::AgentsRequested { .. } | NodeEvent::AgentSessionsRequested { .. }))
        .collect();
    assert!(prompts.is_empty(), "{prompts:?}");
}
