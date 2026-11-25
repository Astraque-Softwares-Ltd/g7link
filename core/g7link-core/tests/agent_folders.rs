// SPDX-License-Identifier: MPL-2.0
//! A new thread in any folder (`docs/protocol/agents.md` §15): two real
//! nodes over localhost, a PC whose platform plays the desktop's places and
//! adds, and a phone that browses the PC's folders and adds one.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use g7link_core::{
    AgentCatalogFolder, AgentFailure, AgentPlace, AgentPlaces, AgentReply, AgentRequest, DeviceId,
    DeviceInfo, DeviceKind, Error, LinkState, Node, NodeConfig, NodeEvent, PairingEvent, PlainKeyProtector,
    Platform, PowerLevel, agent, agent_places,
};
use tempfile::TempDir;
use tokio::sync::broadcast::{Receiver, error::TryRecvError};

const WAIT: Duration = Duration::from_secs(20);
const FOLDER_ID: &str = "f0123456789ab";

/// The desktop's places and adds, as far as these tests need them.
#[derive(Debug, Default)]
struct FolderPlatform {
    requests: Mutex<Vec<AgentRequest>>,
}

impl FolderPlatform {
    fn requests(&self) -> Vec<AgentRequest> {
        self.requests.lock().unwrap().clone()
    }
}

fn place(name: &str, repo: bool, folder: Option<&str>) -> AgentPlace {
    AgentPlace { name: name.into(), repo, folder: folder.map(str::to_owned) }
}

impl Platform for FolderPlatform {
    fn agent_request(&self, _from: &DeviceId, request: AgentRequest) -> Result<AgentReply, AgentFailure> {
        self.requests.lock().unwrap().push(request.clone());
        match request {
            AgentRequest::Places { place: at, .. } => {
                let entries = match at.as_str() {
                    "" => vec![place("Home", false, None), place("Local Disk", false, None)],
                    // A careless platform: a hidden name and a bad id.
                    "Home/code" => vec![
                        place("orbit", true, Some(FOLDER_ID)),
                        place("plain", false, Some("NOT AN ID")),
                        place(".secret", false, None),
                    ],
                    "Home/slow" => return Err(AgentFailure::Busy),
                    _ => return Err(AgentFailure::NotFound),
                };
                Ok(AgentReply::Places(AgentPlaces { place: at, entries, more: false }))
            }
            AgentRequest::FolderAdd { place: at, .. } if at.ends_with("/declined") => {
                Err(AgentFailure::Denied)
            }
            AgentRequest::FolderAdd { place: at, .. } => Ok(AgentReply::FolderAdded(AgentCatalogFolder {
                id: FOLDER_ID.into(),
                // A bidi control the core cleans before it goes.
                label: format!("{}\u{202e}", at.rsplit('/').next().unwrap_or_default()),
                repo: true,
                ..Default::default()
            })),
            _ => Err(AgentFailure::Unsupported),
        }
    }
}

struct Device {
    node: Node,
    events: Receiver<NodeEvent>,
    host: Arc<FolderPlatform>,
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
    let host = Arc::new(FolderPlatform::default());
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

const PHONE: &[&str] = &[agent::ITEMS2, agent::CONTROL];
const PLACES_PC: &[&str] = &[agent::HOST, agent::CONTROL, agent_places::CAP];

async fn pair_with(offers: &[&str]) -> (Device, Device) {
    let mut pc = start("Desktop", DeviceKind::Desktop, offers).await;
    let mut phone = start("Pixel", DeviceKind::Phone, PHONE).await;
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

/// A PC offering places with the phone's `agents` toggle on.
async fn pair_allowed() -> (Device, Device) {
    let (pc, phone) = pair_with(PLACES_PC).await;
    pc.node.set_device_toggle(phone.node.device_id(), agent::TOGGLE, true).unwrap();
    (pc, phone)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_that_doesnt_offer_places_is_never_asked() {
    let (pc, phone) = pair_with(&[agent::HOST, agent::CONTROL]).await;
    let pc_id = pc.node.device_id();
    pc.node.set_device_toggle(phone.node.device_id(), agent::TOGGLE, true).unwrap();
    assert!(!phone.node.agent_places_offered(pc_id));
    assert!(matches!(
        timed("places", phone.node.agent_places(pc_id, String::new(), None)).await,
        Err(Error::Unsupported)
    ));
    assert!(matches!(
        timed("add", phone.node.agent_folder_add(pc_id, "Home/code/orbit".into())).await,
        Err(Error::Unsupported)
    ));
    assert!(pc.host.requests().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agents_toggle_governs_browsing() {
    let (pc, phone) = pair_with(PLACES_PC).await;
    let pc_id = pc.node.device_id();
    assert!(phone.node.agent_places_offered(pc_id));
    assert!(matches!(
        timed("places", phone.node.agent_places(pc_id, String::new(), None)).await,
        Err(Error::Denied)
    ));
    assert!(pc.host.requests().is_empty(), "refused before the platform is asked");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_browses_roots_and_folders_made_to_fit() {
    let (pc, phone) = pair_allowed().await;
    let pc_id = pc.node.device_id();
    let roots = timed("roots", phone.node.agent_places(pc_id, String::new(), None)).await.unwrap();
    let names: Vec<&str> = roots.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Home", "Local Disk"]);
    let code = timed("code", phone.node.agent_places(pc_id, "Home/code".into(), Some(0))).await.unwrap();
    let names: Vec<&str> = code.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["orbit", "plain"], "a hidden name never reaches the phone");
    assert!(code.entries[0].repo);
    assert_eq!(code.entries[0].folder.as_deref(), Some(FOLDER_ID));
    assert_eq!(code.entries[1].folder, None, "a bad id is dropped");
    assert!(matches!(
        timed("missing", phone.node.agent_places(pc_id, "Home/missing".into(), None)).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        timed("slow", phone.node.agent_places(pc_id, "Home/slow".into(), None)).await,
        Err(Error::Protocol(reason)) if reason.starts_with("BUSY")
    ));
    let asked = pc.host.requests().len();
    for bad in ["/Home", "Home/../etc", "Home/.ssh"] {
        assert!(phone.node.agent_places(pc_id, bad.into(), None).await.is_err(), "{bad}");
        assert!(phone.node.agent_folder_add(pc_id, bad.into()).await.is_err(), "{bad}");
    }
    assert!(phone.node.agent_folder_add(pc_id, "Home".into()).await.is_err(), "never a root itself");
    assert_eq!(pc.host.requests().len(), asked, "a bad place never leaves the phone");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_add_asks_the_pc_until_the_phone_may_add() {
    let (pc, mut phone) = pair_allowed().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    // `agent_folders` off: the phone hears that the PC is asking, and the
    // platform is told to ask.
    let folder = timed("add", phone.node.agent_folder_add(pc_id, "Home/code/barber".into())).await.unwrap();
    assert_eq!((folder.id.as_str(), folder.label.as_str()), (FOLDER_ID, "barber"), "the label is cleaned");
    assert!(folder.repo);
    let place = wait_for(&mut phone, "prompt", |e| match e {
        NodeEvent::AgentFolderPrompt { device, place } if *device == pc_id => Some(place.clone()),
        _ => None,
    })
    .await;
    assert_eq!(place, "Home/code/barber");
    assert!(matches!(pc.host.requests().last(), Some(AgentRequest::FolderAdd { ask: true, .. })));
    // Said no (or didn't answer): `DENIED`.
    assert!(matches!(
        timed("declined", phone.node.agent_folder_add(pc_id, "Home/code/declined".into())).await,
        Err(Error::Denied)
    ));
    // On: added at once, nobody asked.
    pc.node.set_device_toggle(phone_id, agent_places::TOGGLE, true).unwrap();
    drain(&mut phone);
    timed("add again", phone.node.agent_folder_add(pc_id, "Home/code/other".into())).await.unwrap();
    assert!(matches!(pc.host.requests().last(), Some(AgentRequest::FolderAdd { ask: false, .. })));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !drain(&mut phone).iter().any(|e| matches!(e, NodeEvent::AgentFolderPrompt { .. })),
        "no prompt while the phone may add"
    );
}
