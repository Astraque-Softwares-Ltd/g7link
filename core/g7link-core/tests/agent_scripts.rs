// SPDX-License-Identifier: MPL-2.0
//! Project scripts on a PC's agent threads, from a phone
//! (`docs/protocol/agents.md` §12): two real nodes over localhost, a PC
//! whose platform plays the agent host, and a phone that lists, runs, stops
//! and reads runs. The folder's "Phones can run these" is the host's to
//! check; here the host refuses one script as a switched-off folder would.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use g7link_core::{
    AgentCatalog, AgentCatalogAgent, AgentCatalogFolder, AgentJobError, AgentReply, AgentRequest,
    AgentRunBrief, AgentScript, AgentScriptRun, AgentThread, DeviceId, DeviceInfo, DeviceKind, Error,
    LinkState, Node, NodeConfig, NodeEvent, PairingEvent, PlainKeyProtector, Platform, PowerLevel, agent,
    scripts,
};
use g7link_protocol::messages::types;
use tempfile::TempDir;
use tokio::sync::broadcast::Receiver;

const WAIT: Duration = Duration::from_secs(20);
const OPEN: &str = "f0123456789ab";
const SHUT: &str = "fba9876543210";
const THREAD: &str = "0123456789abcdef0123456789abcdef";
const RUN: &str = "0123456789abcdef";
/// Scripts of the folder whose switch is on.
const DEV: &str = "s0000000a";
const TEST: &str = "s0000000b";
/// Scripts the host refuses: switched off, already running, too many,
/// gone, and a folder missing on the PC.
const OFF: &str = "s000000f0";
const BUSY: &str = "s000000f1";
const MANY: &str = "s000000f2";
const GONE: &str = "s000000f3";
const MISSING: &str = "s000000f4";
/// A command a phone must never see from the switched-off folder.
const SECRET_COMMAND: &str = "deploy --prod --token hunter2";

/// A PC's agent host, as far as these tests need one.
#[derive(Debug, Default)]
struct HostPlatform {
    requests: Mutex<Vec<(DeviceId, AgentRequest)>>,
}

impl HostPlatform {
    fn requests(&self) -> Vec<(DeviceId, AgentRequest)> {
        self.requests.lock().unwrap().clone()
    }

    fn script_requests(&self) -> Vec<(DeviceId, AgentRequest)> {
        self.requests()
            .into_iter()
            .filter(|(_, r)| {
                matches!(
                    r,
                    AgentRequest::ScriptRun { .. }
                        | AgentRequest::ScriptStop { .. }
                        | AgentRequest::ScriptRuns { .. }
                        | AgentRequest::ScriptOutput { .. }
                )
            })
            .collect()
    }
}

fn run(id: &str, script: &str, phase: &str, lines: usize) -> AgentScriptRun {
    AgentScriptRun {
        id: id.into(),
        thread: THREAD.into(),
        script: script.into(),
        name: "dev".into(),
        phase: phase.into(),
        started_ms: 1_795_000_000_000,
        lines: (0..lines).map(|i| format!("ready on :5173 line {i}")).collect(),
        ..Default::default()
    }
}

fn refused(reason: &str) -> AgentReply {
    AgentReply::Refused(AgentJobError { reason: reason.into(), message: "no".into(), names: Vec::new() })
}

impl Platform for HostPlatform {
    fn agent_request(
        &self,
        from: &DeviceId,
        request: AgentRequest,
    ) -> Result<AgentReply, g7link_core::AgentFailure> {
        self.requests.lock().unwrap().push((*from, request.clone()));
        let by = Some(*from);
        Ok(match request {
            AgentRequest::ScriptRun { script, .. } => match script.as_str() {
                OFF => refused(scripts::refused::PHONE_SCRIPTS_OFF),
                BUSY => refused(scripts::refused::SCRIPT_RUNNING),
                MANY => refused(scripts::refused::RUNS),
                GONE => refused(scripts::refused::NO_SCRIPT),
                MISSING => refused(scripts::refused::FOLDER_MISSING),
                // A run that says it's another thread's isn't an answer.
                TEST => AgentReply::ScriptRun {
                    run: AgentScriptRun { thread: "ffff".into(), ..run(RUN, TEST, "running", 0) },
                    by,
                },
                _ => AgentReply::ScriptRun {
                    run: AgentScriptRun { by: Some("Pixel".into()), ..run(RUN, &script, "running", 300) },
                    by,
                },
            },
            AgentRequest::ScriptStop { run: id } => AgentReply::ScriptRun {
                run: AgentScriptRun { ended_ms: Some(1_795_000_009_000), ..run(&id, DEV, "stopped", 3) },
                by,
            },
            AgentRequest::ScriptRuns { .. } => AgentReply::ScriptRuns(
                (0..20u32)
                    .map(|i| {
                        let mut r = run(&format!("{i:016x}"), DEV, "done", 10);
                        r.exit = Some(0);
                        (r, if i == 0 { by } else { None })
                    })
                    .chain(std::iter::once((
                        AgentScriptRun { thread: "ffff".into(), ..run("ffffffffffffffff", DEV, "done", 0) },
                        None,
                    )))
                    .collect(),
            ),
            AgentRequest::ScriptOutput { run: id } => AgentReply::ScriptRun {
                run: AgentScriptRun { from: 5, dropped: 5, ..run(&id, DEV, "running", 2_000) },
                by: None,
            },
            _ => return Err(g7link_core::AgentFailure::Unsupported),
        })
    }
}

struct Device {
    node: Node,
    events: Receiver<NodeEvent>,
    host: Arc<HostPlatform>,
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
    let host = Arc::new(HostPlatform::default());
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

fn script(id: &str, name: &str, command: &str) -> AgentScript {
    AgentScript { id: id.into(), name: name.into(), command: command.into() }
}

/// The catalog as the PC's platform gives it: one folder with its switch
/// on, and one with it off whose scripts (and their commands) the platform
/// lists anyway.
fn catalog() -> AgentCatalog {
    AgentCatalog {
        agents: vec![AgentCatalogAgent {
            id: "claude".into(),
            name: "Claude".into(),
            installed: true,
            signed_in: true,
            login_hint: "claude auth login".into(),
            ..Default::default()
        }],
        folders: vec![
            AgentCatalogFolder {
                id: OPEN.into(),
                label: "g7linux".into(),
                phone_scripts: true,
                script_count: 2,
                scripts: vec![script(DEV, "dev", "npm run dev"), script(TEST, "test", "cargo test")],
                ..Default::default()
            },
            AgentCatalogFolder {
                id: SHUT.into(),
                label: "infra".into(),
                phone_scripts: false,
                script_count: 1,
                scripts: vec![script(OFF, "deploy", SECRET_COMMAND)],
                ..Default::default()
            },
        ],
        push: false,
        add_folders: false,
    }
}

/// A PC with the agent host paired with a phone whose `agents` toggle is on,
/// and the catalog the phone got.
async fn pair() -> (Device, Device, AgentCatalog) {
    let mut pc = start("Desktop", DeviceKind::Desktop, &[agent::HOST, agent::CONTROL]).await;
    pc.node.set_agent_catalog(Some(catalog())).await.unwrap();
    let mut phone = start("Pixel", DeviceKind::Phone, &[agent::ITEMS2, agent::CONTROL]).await;
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
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, true).unwrap();
    let got = wait_for(&mut phone, "catalog", |e| match e {
        NodeEvent::AgentCatalog { device, catalog: Some(c), .. } if *device == pc_id => Some(c.clone()),
        _ => None,
    })
    .await;
    (pc, phone, got)
}

async fn timed<T>(what: &str, fut: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, fut).await.unwrap_or_else(|_| panic!("timed out: {what}"))
}

fn line_bytes(run: &AgentScriptRun) -> usize {
    run.lines.iter().map(String::len).sum()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_switched_off_folder_sends_no_script_and_no_command() {
    let (_pc, _phone, got) = pair().await;
    let open = got.folders.iter().find(|f| f.id == OPEN).unwrap();
    assert!(open.phone_scripts);
    assert_eq!(open.script_count, 2);
    assert_eq!(
        open.scripts.iter().map(|s| s.command.as_str()).collect::<Vec<_>>(),
        ["npm run dev", "cargo test"]
    );
    let shut = got.folders.iter().find(|f| f.id == SHUT).unwrap();
    assert!(!shut.phone_scripts);
    assert_eq!(shut.script_count, 1, "the phone knows there is one");
    assert!(shut.scripts.is_empty(), "the core fed one with a command; none reached the phone");
    assert!(!format!("{got:?}").contains("hunter2"));
}

#[tokio::test(flavor = "multi_thread")]
async fn every_op_reaches_the_host_as_the_phones_and_its_answer_fits() {
    let (pc, phone, _) = pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let started = timed("run", phone.node.agent_script_run(pc_id, THREAD.into(), DEV.into())).await.unwrap();
    assert_eq!((started.id.as_str(), started.phase.as_str(), started.mine), (RUN, "running", true));
    assert_eq!(started.by.as_deref(), Some("Pixel"));
    assert_eq!(started.lines.len(), scripts::MAX_LIVE_LINES, "an answer to a run carries its newest 40");
    assert!(line_bytes(&started) <= scripts::MAX_LIVE_BYTES);
    assert_eq!(started.from, 260);
    assert_eq!(started.lines[39], "ready on :5173 line 299");

    let stopped = timed("stop", phone.node.agent_script_stop(pc_id, RUN.into())).await.unwrap();
    assert_eq!((stopped.phase.as_str(), stopped.mine, stopped.lines.len()), ("stopped", true, 3));

    let runs = timed("runs", phone.node.agent_script_runs(pc_id, THREAD.into())).await.unwrap();
    assert_eq!(runs.len(), scripts::MAX_RUNS);
    assert!(
        runs.iter().all(|r| r.lines.is_empty() && r.thread == THREAD),
        "no lines, and only this thread's"
    );
    assert!(runs[0].mine && runs[1..].iter().all(|r| !r.mine));
    assert_eq!(runs[0].from, 10, "where the next line would start");

    let output = timed("output", phone.node.agent_script_output(pc_id, RUN.into())).await.unwrap();
    assert_eq!(output.lines.len(), scripts::MAX_OUTPUT_LINES);
    assert!(line_bytes(&output) <= scripts::MAX_OUTPUT_BYTES);
    assert_eq!(output.from, 5 + 1_800);
    assert_eq!(output.dropped, 5 + 1_800);
    assert_eq!(output.lines[199], "ready on :5173 line 1999");
    assert!(!output.mine, "the PC started it");

    let asked = pc.host.script_requests();
    assert_eq!(asked.len(), 4);
    assert!(asked.iter().all(|(from, _)| *from == phone_id), "every request is the phone's");
    assert!(
        matches!(&asked[0].1, AgentRequest::ScriptRun { thread, script } if thread == THREAD && script == DEV)
    );
    assert!(matches!(&asked[1].1, AgentRequest::ScriptStop { run } if run == RUN));
    assert!(matches!(&asked[2].1, AgentRequest::ScriptRuns { thread } if thread == THREAD));
    assert!(matches!(&asked[3].1, AgentRequest::ScriptOutput { run } if run == RUN));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_hosts_refusals_reach_the_phone_with_their_words() {
    let (pc, phone, _) = pair().await;
    let pc_id = pc.node.device_id();
    let run = |script: &'static str| phone.node.agent_script_run(pc_id, THREAD.into(), script.into());
    let off = timed("off", run(OFF)).await;
    assert!(matches!(off, Err(Error::Denied)), "{off:?}");
    let busy = timed("busy", run(BUSY)).await;
    assert!(matches!(&busy, Err(Error::Protocol(m)) if m == "BUSY: script_running"), "{busy:?}");
    let many = timed("many", run(MANY)).await;
    assert!(matches!(&many, Err(Error::Protocol(m)) if m == "BUSY: runs"), "{many:?}");
    let gone = timed("gone", run(GONE)).await;
    assert!(matches!(gone, Err(Error::NotFound)), "{gone:?}");
    let missing = timed("missing", run(MISSING)).await;
    assert!(matches!(&missing, Err(Error::Protocol(m)) if m.contains("folder_missing")), "{missing:?}");
    let other = timed("another thread's", run(TEST)).await;
    assert!(matches!(&other, Err(Error::Protocol(m)) if m.contains("INTERNAL")), "{other:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_live_tail_is_40_lines_and_8_kib_and_mine_only_for_its_phone() {
    let (pc, mut phone, _) = pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let first = AgentScriptRun { lines: vec!["é".repeat(300); 120], from: 0, ..run(RUN, DEV, "running", 0) };
    pc.node.agent_script_progress(first, Some(phone_id)).await.unwrap();
    let got = wait_for(&mut phone, "the first progress", |e| match e {
        NodeEvent::AgentScriptRun { device, run } if *device == pc_id && run.phase == "running" => {
            Some(run.clone())
        }
        _ => None,
    })
    .await;
    assert!(got.mine);
    assert!(got.lines.len() <= scripts::MAX_LIVE_LINES && line_bytes(&got) <= scripts::MAX_LIVE_BYTES);
    assert_eq!(got.lines.len(), 13, "600 bytes a line: 13 fit in 8 KiB");
    assert_eq!(got.from, 107);

    let last = AgentScriptRun {
        exit: Some(0),
        ended_ms: Some(1_795_000_009_000),
        from: 120,
        ..run(RUN, DEV, "done", 300)
    };
    pc.node.agent_script_progress(last, None).await.unwrap();
    let got = wait_for(&mut phone, "the last progress", |e| match e {
        NodeEvent::AgentScriptRun { run, .. } if run.phase == "done" => Some(run.clone()),
        _ => None,
    })
    .await;
    assert!(!got.mine, "the PC's own run");
    assert_eq!((got.lines.len(), got.exit, got.from), (scripts::MAX_LIVE_LINES, Some(0), 380));

    // The PC's core refuses to send a run out of its limits at all.
    let bad = AgentScriptRun { id: "nope".into(), ..run(RUN, DEV, "running", 1) };
    assert!(pc.node.agent_script_progress(bad, None).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_drops_an_agent_script_out_of_its_limits() {
    let (pc, mut phone, _) = pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let push = |run: AgentScriptRun| {
        pc.node.debug_agent_push(phone_id, types::AGENT_SCRIPT, &serde_json::json!({ "run": run })).unwrap()
    };
    push(run(RUN, DEV, "paused", 1));
    push(AgentScriptRun { lines: vec!["x".into(); 41], ..run(RUN, DEV, "running", 0) });
    push(AgentScriptRun { script: "npm run dev".into(), ..run(RUN, DEV, "running", 1) });
    push(AgentScriptRun { name: "marker".into(), ..run(RUN, DEV, "failed", 1) });
    let got = wait_for(&mut phone, "the one that fits", |e| match e {
        NodeEvent::AgentScriptRun { device, run } if *device == pc_id => Some(run.clone()),
        _ => None,
    })
    .await;
    assert_eq!(
        (got.name.as_str(), got.phase.as_str()),
        ("marker", "failed"),
        "the three before were dropped"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn with_agents_off_or_out_of_limits_nothing_reaches_the_host() {
    let (pc, phone, _) = pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let ask = |t: &'static str, body: serde_json::Value| {
        let node = &phone.node;
        async move { node.debug_agent_request(pc_id, t, &body).await }
    };
    // Ids out of their shape, and fields a phone may not send.
    let cases = [
        (types::AGENT_SCRIPT_RUN, serde_json::json!({ "thread": THREAD, "script": "npm run dev" })),
        (types::AGENT_SCRIPT_RUN, serde_json::json!({ "thread": THREAD, "script": "s0000000A" })),
        (types::AGENT_SCRIPT_RUN, serde_json::json!({ "thread": "../etc", "script": DEV })),
        (types::AGENT_SCRIPT_RUN, serde_json::json!({ "thread": THREAD })),
        (types::AGENT_SCRIPT_STOP, serde_json::json!({ "run": "1" })),
        (types::AGENT_SCRIPT_OUTPUT, serde_json::json!({ "run": "/proc/self/environ" })),
        (types::AGENT_SCRIPT_RUNS, serde_json::json!({ "thread": "" })),
    ];
    for (t, body) in cases {
        assert!(matches!(ask(t, body.clone()).await, Err(Error::Protocol(_))), "{t} {body}");
    }
    // No message adds, edits or removes a script, or turns the switch on: they fail closed.
    let body = serde_json::json!({ "folder": OPEN, "command": "rm -rf ~", "phone_scripts": true });
    for t in [
        "agent.script.add",
        "agent.script.edit",
        "agent.scripts.set",
        "agent.folders.set",
        "agent.script.restart",
    ] {
        assert!(ask(t, body.clone()).await.is_err(), "{t}");
    }
    // The phone's own core doesn't send them either.
    assert!(phone.node.agent_script_run(pc_id, THREAD.into(), "npm run dev".into()).await.is_err());
    assert!(phone.node.agent_script_stop(pc_id, "x".into()).await.is_err());
    assert!(phone.node.agent_script_output(pc_id, String::new()).await.is_err());
    assert!(pc.host.script_requests().is_empty(), "nothing reached the host: {:?}", pc.host.requests());

    // With the `agents` toggle off, every one is refused before the host hears of it.
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, false).unwrap();
    let run = phone.node.agent_script_run(pc_id, THREAD.into(), DEV.into()).await;
    assert!(matches!(run, Err(Error::Denied)), "{run:?}");
    assert!(matches!(phone.node.agent_script_stop(pc_id, RUN.into()).await, Err(Error::Denied)));
    assert!(matches!(phone.node.agent_script_runs(pc_id, THREAD.into()).await, Err(Error::Denied)));
    assert!(matches!(phone.node.agent_script_output(pc_id, RUN.into()).await, Err(Error::Denied)));
    assert!(pc.host.script_requests().is_empty());
}

#[test]
fn a_thread_carries_its_runs_in_brief() {
    let thread = AgentThread {
        id: THREAD.into(),
        agent: "claude".into(),
        folder: "g7linux".into(),
        title: "Start the dev server".into(),
        state: "done".into(),
        mode: "ask".into(),
        runs: vec![
            AgentRunBrief { id: RUN.into(), name: "dev".into(), phase: "running".into(), exit: None },
            AgentRunBrief { id: "nope".into(), name: "x".into(), phase: "running".into(), exit: None },
        ],
        ..Default::default()
    };
    let clean = thread.sanitized().unwrap();
    assert_eq!(clean.runs.len(), 1);
    assert_eq!(clean.runs[0].id, RUN);
}
