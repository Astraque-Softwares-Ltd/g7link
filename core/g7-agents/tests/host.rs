// SPDX-License-Identifier: MPL-2.0
//! The host end to end: an in-process host on a socket in a temp dir, the
//! real `mcp-approve` bridge, and `g7-agents-fake-cli` replaying the
//! captured Claude and Codex sessions.
#![cfg(feature = "fake-cli")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use g7_agents::api::*;
use g7_agents::client::{Client, ClientError, Events};
use g7_agents::host::{self, Config, Served};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::oneshot;

const WAIT: Duration = Duration::from_secs(20);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

/// A shell script standing in for `claude` or `codex`.
fn wrapper(dir: &Path, agent: &str, script: &str, resume_script: Option<&str>, log: &Path) -> PathBuf {
    let mut n = 0;
    let mut path = dir.join(format!("{agent}-cli"));
    while path.exists() {
        n += 1;
        path = dir.join(format!("{agent}-cli-{n}"));
    }
    let mut body = String::from("#!/bin/sh\n");
    body.push_str(&format!("export FAKE_CLI_SCRIPT={}\n", quote(&fixture(script))));
    if let Some(resume) = resume_script {
        body.push_str(&format!("export FAKE_CLI_RESUME_SCRIPT={}\n", quote(&fixture(resume))));
    }
    body.push_str(&format!("export FAKE_CLI_LOG={}\n", quote(log)));
    body.push_str(&format!(
        "exec {} {agent} \"$@\"\n",
        quote(Path::new(env!("CARGO_BIN_EXE_g7-agents-fake-cli")))
    ));
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

struct TestHost {
    dir: tempfile::TempDir,
    socket: PathBuf,
    work: PathBuf,
    log: PathBuf,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<std::io::Result<Served>>>,
}

#[derive(Default)]
struct Options {
    claude: Option<&'static str>,
    claude_resume: Option<&'static str>,
    codex: Option<&'static str>,
    agent_idle: Option<Duration>,
    approval_lifetime_ms: Option<u64>,
}

impl TestHost {
    fn config(dir: &Path, opts: &Options, log: &Path) -> Config {
        let run = dir.join("run");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        Config {
            socket: run.join("host.sock"),
            runtime_dir: run,
            data_dir: dir.join("data/g7-agents"),
            claude_bin: Some(wrapper(
                dir,
                "claude",
                opts.claude.unwrap_or("claude-01-list.jsonl"),
                opts.claude_resume,
                log,
            )),
            codex_bin: Some(wrapper(
                dir,
                "codex",
                opts.codex.unwrap_or("codex-01-create-approve.jsonl"),
                None,
                log,
            )),
            bridge_exe: PathBuf::from(env!("CARGO_BIN_EXE_g7-agents")),
            notify: false,
            env: vec![("HOME".into(), home.into_os_string()), ("PATH".into(), "/usr/bin:/bin".into())],
            idle_exit: host::IDLE_EXIT,
            agent_idle: opts.agent_idle.unwrap_or(Duration::from_secs(600)),
            approval_lifetime_ms: opts.approval_lifetime_ms.unwrap_or(APPROVAL_LIFETIME_MS),
        }
    }

    async fn start(opts: Options) -> TestHost {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let log = dir.path().join("cli.log");
        let config = Self::config(dir.path(), &opts, &log);
        let socket = config.socket.clone();
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let (stop, stopped) = oneshot::channel::<()>();
        let mut task = tokio::spawn(host::serve(config, async move {
            let _ = stopped.await;
        }));
        wait_for(&socket, &mut task).await;
        TestHost { dir, socket, work, log, stop: Some(stop), task: Some(task) }
    }

    async fn client(&self, kind: ClientKind) -> (Client, Events) {
        Client::connect(&self.socket, HelloParams { client: kind, version: 1, bridge_token: None })
            .await
            .unwrap()
    }

    async fn subscribed(&self) -> (Client, Events) {
        let (client, events) = self.client(ClientKind::Test).await;
        client.subscribe(ThreadSelection::All, true).await.unwrap();
        (client, events)
    }

    async fn folder(&self, client: &Client) -> Folder {
        client.folders_add(self.work.to_string_lossy().into_owned(), Some("work".into())).await.unwrap()
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    async fn stop(mut self) -> Served {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let served = self.task.take().unwrap().await.unwrap().unwrap();
        drop(self.dir);
        served
    }
}

/// Waits until the host listens on `socket`.
async fn wait_for(socket: &Path, task: &mut tokio::task::JoinHandle<std::io::Result<Served>>) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !socket.exists() {
        if task.is_finished() {
            panic!("the host stopped before it listened: {:?}", task.await);
        }
        assert!(tokio::time::Instant::now() < deadline, "the host didn't listen");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn next(events: &mut Events, what: &str, pred: impl Fn(&Event) -> bool) -> Event {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(event)) if pred(&event) => return event,
            Ok(Some(_)) => {}
            Ok(None) => panic!("connection closed while waiting for {what}"),
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

/// Every event up to and including the thread's `thread.done`.
async fn until_done(events: &mut Events, thread_id: &str) -> Vec<Event> {
    let deadline = tokio::time::Instant::now() + WAIT;
    let mut seen = Vec::new();
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(event)) => {
                let done = matches!(&event, Event::ThreadDone(d) if d.thread_id == thread_id);
                seen.push(event);
                if done {
                    return seen;
                }
            }
            Ok(None) => panic!("connection closed"),
            Err(_) => panic!("timed out waiting for thread.done; saw {seen:#?}"),
        }
    }
}

async fn approval(events: &mut Events) -> Approval {
    match next(events, "approval.requested", |e| matches!(e, Event::ApprovalRequested(_))).await {
        Event::ApprovalRequested(a) => a.approval,
        _ => unreachable!(),
    }
}

fn start(agent: AgentId, folder: &Folder, prompt: &str) -> ThreadStartParams {
    ThreadStartParams {
        agent,
        folder_id: folder.id.clone(),
        prompt: prompt.into(),
        mode: Mode::Supervised,
        origin: Origin::launcher(),
    }
}

fn done_of(events: &[Event]) -> &ThreadDoneEvent {
    events
        .iter()
        .find_map(|e| if let Event::ThreadDone(d) = e { Some(d) } else { None })
        .expect("thread.done")
}

fn tool_items(events: &[Event]) -> Vec<&Item> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ThreadItem(i) if i.item.kind == ItemKind::Tool => Some(&i.item),
            _ => None,
        })
        .collect()
}

async fn raw_line(stream: &mut BufReader<UnixStream>) -> Option<serde_json::Value> {
    let mut line = String::new();
    match tokio::time::timeout(WAIT, stream.read_line(&mut line)).await {
        Ok(Ok(0)) | Err(_) => None,
        Ok(Ok(_)) => serde_json::from_str(&line).ok(),
        Ok(Err(_)) => None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_comes_first() {
    let host = TestHost::start(Options::default()).await;
    let mut raw = BufReader::new(UnixStream::connect(&host.socket).await.unwrap());
    raw.get_mut().write_all(b"{\"id\":7,\"method\":\"folders.list\",\"params\":{}}\n").await.unwrap();
    let reply = raw_line(&mut raw).await.unwrap();
    assert_eq!(reply["id"], 7);
    assert_eq!(reply["error"]["code"], "BAD_REQUEST");
    assert!(raw_line(&mut raw).await.is_none(), "closed after a request before hello");

    let err = Client::connect(
        &host.socket,
        HelloParams { client: ClientKind::Test, version: 2, bridge_token: None },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::UnsupportedVersion));
    let err = Client::connect(
        &host.socket,
        HelloParams { client: ClientKind::Bridge, version: 1, bridge_token: Some("ab".repeat(16)) },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Denied));

    // A line over the limit closes the connection.
    let (client, _events) = host.client(ClientKind::Test).await;
    assert_eq!(client.host().version, 1);
    let mut raw = BufReader::new(UnixStream::connect(&host.socket).await.unwrap());
    raw.get_mut()
        .write_all(b"{\"id\":1,\"method\":\"hello\",\"params\":{\"client\":\"test\",\"version\":1}}\n")
        .await
        .unwrap();
    assert_eq!(raw_line(&mut raw).await.unwrap()["result"]["version"], 1);
    let long = format!(
        "{{\"id\":2,\"method\":\"folders.list\",\"params\":{{\"x\":\"{}\"}}}}\n",
        "a".repeat(MAX_LINE)
    );
    let _ = raw.get_mut().write_all(long.as_bytes()).await;
    assert!(raw_line(&mut raw).await.is_none());
    // An unknown method is refused but keeps the connection.
    let err = client.call_value("nope", json!({}), Some(WAIT)).await.unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::BadRequest));
    assert!(client.folders_list().await.unwrap().is_empty());
    assert_eq!(host.stop().await, Served::Stopped);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folders_add_list_remove() {
    let host = TestHost::start(Options::default()).await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    assert!(is_valid_folder_id(&folder.id));
    assert_eq!(folder.label, "work");
    assert!(matches!(next(&mut events, "folders.changed", |e| matches!(e, Event::FoldersChanged(_))).await,
        Event::FoldersChanged(f) if f.folders.len() == 1));
    // The same folder by another spelling is the same folder.
    let again = client.folders_add(format!("{}/../work", host.work.display()), None).await.unwrap();
    assert_eq!(again.id, folder.id);
    let file = host.work.join("file.txt");
    std::fs::write(&file, "x").unwrap();
    for bad in [
        "relative/path".to_owned(),
        "/".to_owned(),
        host.work.join("missing").display().to_string(),
        file.display().to_string(),
    ] {
        let err = client.folders_add(bad.clone(), None).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::BadRequest), "{bad}");
    }
    let listed = client.folders_list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].exists);
    client.folders_remove(folder.id.clone()).await.unwrap();
    assert_eq!(client.folders_remove(folder.id).await.unwrap_err().code(), Some(ErrorCode::NotFound));
    assert!(client.folders_list().await.unwrap().is_empty());
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_streams_a_turn_to_done() {
    let host = TestHost::start(Options::default()).await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let agents = client.agents_list(false).await.unwrap();
    assert_eq!(agents.iter().map(|a| a.id).collect::<Vec<_>>(), [AgentId::Claude, AgentId::Codex]);
    assert!(agents.iter().all(|a| a.ready()), "{agents:?}");
    assert_eq!(agents[0].plan.as_deref(), Some("max"));
    let thread = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    assert_eq!(thread.state, State::Working);
    assert_eq!(thread.title, "List the files here.");
    let seen = until_done(&mut events, &thread.id).await;
    let done = done_of(&seen);
    assert_eq!(done.outcome, Outcome::Done);
    assert_eq!(done.summary, "There's one file here, `README.md`, plus the `.git` folder.");

    let streamed: String = seen
        .iter()
        .filter_map(|e| if let Event::ThreadDelta(d) = e { Some(d.text.as_str()) } else { None })
        .collect();
    assert_eq!(streamed, done.summary, "the deltas spell the message");
    let tools = tool_items(&seen);
    assert_eq!(tools.len(), 2, "the step starts and ends");
    assert_eq!((tools[0].rev, tools[0].tool.as_ref().unwrap().status), (0, ToolStatus::Running));
    assert_eq!((tools[1].rev, tools[1].tool.as_ref().unwrap().status), (1, ToolStatus::Ok));
    // thread.state with the new state comes before thread.done.
    let state_pos = seen
        .iter()
        .rposition(|e| matches!(e, Event::ThreadState(s) if s.thread.state == State::Done))
        .unwrap();
    assert!(state_pos < seen.len() - 1);

    let read = client
        .thread_read(ThreadReadParams { thread_id: thread.id.clone(), ..Default::default() })
        .await
        .unwrap();
    let kinds: Vec<_> = read.items.iter().map(|i| i.kind).collect();
    assert_eq!(kinds, [ItemKind::User, ItemKind::Tool, ItemKind::Assistant]);
    assert_eq!(read.items[1].tool.as_ref().unwrap().output.as_deref(), Some(".git\nREADME.md"));
    assert!(!read.more_before);
    assert_eq!(read.thread.state, State::Done);
    let older = client
        .thread_read(ThreadReadParams {
            thread_id: thread.id.clone(),
            before_seq: Some(3),
            limit: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(older.items.iter().map(|i| i.seq).collect::<Vec<_>>(), [2]);
    assert!(older.more_before);

    let terminal = client.thread_terminal_command(thread.id.clone()).await.unwrap();
    assert_eq!(terminal.argv[..2], ["claude", "--resume"]);
    assert_eq!(Some(terminal.argv[2].clone()), read.thread.native_id);
    assert_eq!(terminal.cwd, std::fs::canonicalize(&host.work).unwrap().to_string_lossy());
    let log = host.log();
    assert!(log.contains("--session-id"), "{log}");
    assert!(log.contains("--strict-mcp-config") && log.contains("--permission-prompt-tool mcp__g7__approve"));
    assert!(!log.contains("--bare"));

    let listed = client.threads_list(None, None).await.unwrap();
    assert_eq!(listed[0].id, thread.id);
    client.thread_delete(thread.id.clone()).await.unwrap();
    next(&mut events, "thread.removed", |e| matches!(e, Event::ThreadRemoved(_))).await;
    assert!(client.threads_list(None, None).await.unwrap().is_empty());
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_asks_through_the_bridge_and_the_user_allows() {
    let host =
        TestHost::start(Options { claude: Some("claude-02-followup-approve.jsonl"), ..Default::default() })
            .await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    let first = until_done(&mut events, &thread.id).await;
    assert_eq!(done_of(&first).outcome, Outcome::Done);
    // BUSY while working is checked on the next turn; here it's done.
    client
        .thread_send(thread.id.clone(), "Create hello.txt containing exactly: hi".into(), Origin::launcher())
        .await
        .unwrap();
    let busy = client.thread_send(thread.id.clone(), "again".into(), Origin::launcher()).await.unwrap_err();
    assert_eq!(busy.code(), Some(ErrorCode::Busy));
    let mut seen = Vec::new();
    let asked = loop {
        let event = next(&mut events, "approval", |_| true).await;
        if let Event::ApprovalRequested(a) = &event {
            break a.approval.clone();
        }
        seen.push(event);
    };
    assert_eq!(asked.kind, ApprovalKind::Edit);
    assert_eq!(asked.title, "Write /work/hello.txt", "the capture ran in /work, outside the test folder");
    assert_eq!(asked.diff.as_deref(), Some("+hi"));
    assert!(asked.can_allow_thread);
    // The step was shown before its approval.
    assert!(tool_items(&seen).iter().any(|i| i.id == "toolu_019665aUVn5LSZP1U45iVqz1"));
    let waiting = client.threads_list(None, None).await.unwrap();
    assert_eq!((waiting[0].state, waiting[0].pending), (State::Waiting, 1));
    let read = client
        .thread_read(ThreadReadParams { thread_id: thread.id.clone(), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(read.pending.len(), 1);
    client.approval_answer(asked.token.clone(), Decision::Allow, Origin::launcher()).await.unwrap();
    let rest = until_done(&mut events, &thread.id).await;
    assert!(rest.iter().any(|e| matches!(e, Event::ApprovalResolved(r)
        if r.token == asked.token && r.outcome == ApprovalOutcome::Allowed && r.by.as_ref().is_some_and(|b| b.kind == OriginKind::Launcher))));
    assert_eq!(done_of(&rest).outcome, Outcome::Done);
    assert!(tool_items(&rest).iter().any(|i| i.tool.as_ref().unwrap().status == ToolStatus::Ok));
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_denied_and_the_race_for_an_answer() {
    let host =
        TestHost::start(Options { claude: Some("claude-03-resume-deny.jsonl"), ..Default::default() }).await;
    let (client, mut events) = host.subscribed().await;
    let (other, _other_events) = host.client(ClientKind::Link).await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "Run: touch denied.txt")).await.unwrap();
    let asked = approval(&mut events).await;
    assert_eq!(asked.kind, ApprovalKind::Command);
    assert_eq!(asked.command.as_deref(), Some("touch denied.txt"));
    let phone = Origin::phone("abc", "Pixel");
    let refused =
        other.approval_answer(asked.token.clone(), Decision::AllowThread, phone.clone()).await.unwrap_err();
    assert_eq!(refused.code(), Some(ErrorCode::Denied));
    let (a, b) = tokio::join!(
        client.approval_answer(asked.token.clone(), Decision::Deny, Origin::launcher()),
        other.approval_answer(asked.token.clone(), Decision::Allow, phone),
    );
    let results = [a, b];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1, "{results:?}");
    let loser = results.iter().find_map(|r| r.as_ref().err()).unwrap();
    assert_eq!(loser.code(), Some(ErrorCode::AlreadyAnswered));
    let rest = until_done(&mut events, &thread.id).await;
    // The fixture's Claude reports the step denied whichever answer won.
    assert!(tool_items(&rest).iter().any(|i| i.tool.as_ref().unwrap().status == ToolStatus::Denied));
    assert_eq!(done_of(&rest).outcome, Outcome::Done);
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_interrupted_while_waiting() {
    let host = TestHost::start(Options {
        claude: Some("claude-05-interrupt-while-waiting.jsonl"),
        ..Default::default()
    })
    .await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "Run: touch held.txt")).await.unwrap();
    let asked = approval(&mut events).await;
    client.thread_interrupt(thread.id.clone()).await.unwrap();
    let rest = until_done(&mut events, &thread.id).await;
    assert!(rest.iter().any(|e| matches!(e, Event::ApprovalResolved(r)
        if r.token == asked.token && r.outcome == ApprovalOutcome::Cancelled)));
    assert_eq!(done_of(&rest).outcome, Outcome::Interrupted);
    let answer = client.approval_answer(asked.token, Decision::Allow, Origin::launcher()).await.unwrap_err();
    assert_eq!(answer.code(), Some(ErrorCode::AlreadyAnswered));
    // Interrupting a thread that isn't running is fine.
    client.thread_interrupt(thread.id.clone()).await.unwrap();
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_asks_before_a_command_and_finishes() {
    let host = TestHost::start(Options::default()).await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let thread =
        client.thread_start(start(AgentId::Codex, &folder, "Create hello.txt, then run ls")).await.unwrap();
    let asked = approval(&mut events).await;
    assert_eq!(asked.kind, ApprovalKind::Command);
    assert_eq!(asked.command.as_deref(), Some("printf 'hi.' > hello.txt\nls"));
    client.approval_answer(asked.token, Decision::Allow, Origin::launcher()).await.unwrap();
    let rest = until_done(&mut events, &thread.id).await;
    let done = done_of(&rest);
    assert_eq!(done.outcome, Outcome::Done);
    assert!(done.summary.starts_with("Created `hello.txt` containing exactly `hi.`"));
    let read = client
        .thread_read(ThreadReadParams { thread_id: thread.id.clone(), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(read.thread.native_id.as_deref(), Some("01a1f093-66a5-7131-8017-6c07427593eb"));
    let terminal = client.thread_terminal_command(thread.id).await.unwrap();
    assert_eq!(terminal.argv, ["codex", "resume", "01a1f093-66a5-7131-8017-6c07427593eb"]);
    assert!(host.log().contains("decision accept"));
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_decline_and_interrupt() {
    let host = TestHost::start(Options {
        codex: Some("codex-02-resume-filechange-decline.jsonl"),
        ..Default::default()
    })
    .await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Codex, &folder, "Create notes.txt")).await.unwrap();
    let asked = approval(&mut events).await;
    assert_eq!(asked.kind, ApprovalKind::Edit);
    assert_eq!(asked.path.as_deref(), Some("/work/notes.txt"));
    assert_eq!(asked.diff.as_deref(), Some("/work/notes.txt (add)\na\n"));
    client.approval_answer(asked.token, Decision::Deny, Origin::launcher()).await.unwrap();
    let rest = until_done(&mut events, &thread.id).await;
    assert!(tool_items(&rest).iter().any(|i| i.tool.as_ref().unwrap().status == ToolStatus::Denied));
    assert!(host.log().contains("decision decline"));
    host.stop().await;

    let host = TestHost::start(Options {
        codex: Some("codex-03-resume-interrupt-while-waiting.jsonl"),
        ..Default::default()
    })
    .await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Codex, &folder, "Run: touch held.txt")).await.unwrap();
    let asked = approval(&mut events).await;
    client.thread_interrupt(thread.id.clone()).await.unwrap();
    let rest = until_done(&mut events, &thread.id).await;
    assert_eq!(done_of(&rest).outcome, Outcome::Interrupted);
    assert!(rest.iter().any(|e| matches!(e, Event::ApprovalResolved(r) if r.token == asked.token && r.outcome == ApprovalOutcome::Cancelled)));
    // The step that never ran shows as denied, not still running.
    assert!(
        tool_items(&rest).iter().any(|i| i.rev == 1 && i.tool.as_ref().unwrap().status == ToolStatus::Denied)
    );
    assert!(host.log().contains("method turn/interrupt"));
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thread_resumes_after_its_process_idled_out() {
    let host = TestHost::start(Options {
        claude_resume: Some("claude-01-list.jsonl"),
        agent_idle: Some(Duration::from_millis(300)),
        ..Default::default()
    })
    .await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    until_done(&mut events, &thread.id).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    client.thread_send(thread.id.clone(), "Again.".into(), Origin::cli()).await.unwrap();
    let rest = until_done(&mut events, &thread.id).await;
    assert_eq!(done_of(&rest).outcome, Outcome::Done);
    let native = client.threads_list(None, None).await.unwrap()[0].native_id.clone().unwrap();
    let log = host.log();
    let runs: Vec<&str> = log.lines().filter(|l| l.starts_with("args -p")).collect();
    assert_eq!(runs.len(), 2, "{log}");
    assert!(runs[0].contains(&format!("--session-id {native}")));
    assert!(runs[1].contains(&format!("--resume {native}")));

    // Codex: the second process resumes the thread instead of starting one.
    let thread = client.thread_start(start(AgentId::Codex, &folder, "Create hello.txt")).await.unwrap();
    let asked = approval(&mut events).await;
    client.approval_answer(asked.token, Decision::Allow, Origin::launcher()).await.unwrap();
    until_done(&mut events, &thread.id).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    client.thread_send(thread.id.clone(), "Again.".into(), Origin::cli()).await.unwrap();
    let asked = approval(&mut events).await;
    client.approval_answer(asked.token, Decision::Allow, Origin::launcher()).await.unwrap();
    until_done(&mut events, &thread.id).await;
    let log = host.log();
    assert_eq!(log.lines().filter(|l| *l == "method thread/start").count(), 1, "{log}");
    assert_eq!(log.lines().filter(|l| *l == "method thread/resume").count(), 1, "{log}");
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn limits_on_starting() {
    let host = TestHost::start(Options {
        claude: Some("claude-05-interrupt-while-waiting.jsonl"),
        codex: Some("codex-03-resume-interrupt-while-waiting.jsonl"),
        ..Default::default()
    })
    .await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let phone = ThreadStartParams {
        mode: Mode::AutoEdit,
        origin: Origin::phone("abc", "Pixel"),
        ..start(AgentId::Claude, &folder, "x")
    };
    assert_eq!(client.thread_start(phone).await.unwrap_err().code(), Some(ErrorCode::Denied));
    let empty = start(AgentId::Claude, &folder, "");
    assert_eq!(client.thread_start(empty).await.unwrap_err().code(), Some(ErrorCode::BadRequest));
    // Codex runs commands in its sandbox without asking under "on-request".
    let codex_auto = ThreadStartParams { mode: Mode::AutoEdit, ..start(AgentId::Codex, &folder, "x") };
    assert_eq!(client.thread_start(codex_auto).await.unwrap_err().code(), Some(ErrorCode::Unavailable));
    let missing =
        ThreadStartParams { folder_id: "f000000000000".into(), ..start(AgentId::Claude, &folder, "x") };
    assert_eq!(client.thread_start(missing).await.unwrap_err().code(), Some(ErrorCode::NotFound));

    let mut ids = Vec::new();
    for _ in 0..MAX_RUNNING_PER_AGENT {
        ids.push(
            client.thread_start(start(AgentId::Claude, &folder, "Run: touch held.txt")).await.unwrap().id,
        );
    }
    let per_agent = client.thread_start(start(AgentId::Claude, &folder, "x")).await.unwrap_err();
    assert_eq!(per_agent.code(), Some(ErrorCode::Busy));
    let ClientError::Remote(e) = &per_agent else { panic!() };
    assert!(
        e.message.ends_with("is already running 3 Claude threads. Stop one to start another."),
        "{}",
        e.message
    );
    ids.push(client.thread_start(start(AgentId::Codex, &folder, "Run: touch held.txt")).await.unwrap().id);
    let total = client.thread_start(start(AgentId::Codex, &folder, "x")).await.unwrap_err();
    let ClientError::Remote(e) = &total else { panic!() };
    assert_eq!(e.code, ErrorCode::Busy);
    assert!(e.message.ends_with("is already running 4 threads. Stop one to start another."), "{}", e.message);
    for _ in 0..ids.len() {
        approval(&mut events).await;
    }
    for id in &ids {
        client.thread_interrupt(id.clone()).await.unwrap();
    }
    let mut done = std::collections::HashSet::new();
    while done.len() < ids.len() {
        if let Event::ThreadDone(d) =
            next(&mut events, "thread.done", |e| matches!(e, Event::ThreadDone(_))).await
        {
            assert_eq!(d.outcome, Outcome::Interrupted);
            done.insert(d.thread_id);
        }
    }
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_host_exits_and_a_restart_interrupts_running_threads() {
    let host = TestHost::start(Options::default()).await;
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_g7-agents"))
        .arg("serve")
        .env("G7_AGENTS_SOCKET", &host.socket)
        .env("XDG_DATA_HOME", host.dir.path().join("data"))
        .env("XDG_STATE_HOME", host.dir.path().join("state"))
        .env("G7_AGENTS_NO_NOTIFY", "1")
        .status()
        .unwrap();
    assert!(status.success());
    let (client, _events) = host.client(ClientKind::Test).await;
    assert!(client.folders_list().await.is_ok(), "the first host still serves");
    drop(client);
    let dir_path = host.dir.path().to_owned();
    let opts = Options { claude: Some("claude-01-list.jsonl"), ..Default::default() };
    // Stop without deleting the directory, then plant a thread that was running.
    let mut host = host;
    let _ = host.stop.take().unwrap().send(());
    host.task.take().unwrap().await.unwrap().unwrap();
    let id = "c".repeat(32);
    let thread_dir = dir_path.join("data/g7-agents/threads").join(&id);
    std::fs::create_dir_all(&thread_dir).unwrap();
    let meta = json!({"version": 1, "next_seq": 2, "thread": {
        "id": id, "agent": "claude", "folder_id": "f000000000001", "folder_label": "w", "title": "t",
        "state": "waiting", "mode": "supervised", "origin": {"kind": "launcher"},
        "created_ms": 1, "updated_ms": 1, "last_seq": 1, "pending": 1}});
    std::fs::write(thread_dir.join("meta.json"), meta.to_string()).unwrap();
    std::fs::write(dir_path.join("data/g7-agents/threads/not-hex"), "x").unwrap();
    let log = dir_path.join("cli.log");
    let config = TestHost::config(&dir_path, &opts, &log);
    let socket = config.socket.clone();
    let (stop, stopped) = oneshot::channel::<()>();
    let mut task = tokio::spawn(host::serve(config, async move {
        let _ = stopped.await;
    }));
    wait_for(&socket, &mut task).await;
    let (client, _events) =
        Client::connect(&socket, HelloParams { client: ClientKind::Test, version: 1, bridge_token: None })
            .await
            .unwrap();
    let threads = client.threads_list(None, None).await.unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!((threads[0].state, threads[0].pending), (State::Interrupted, 0));
    let read =
        client.thread_read(ThreadReadParams { thread_id: id.clone(), ..Default::default() }).await.unwrap();
    assert_eq!(read.items.last().unwrap().text.as_deref(), Some("Stopped: the agent host restarted."));
    let _ = stop.send(());
    task.await.unwrap().unwrap();
    drop(host.dir);
}
