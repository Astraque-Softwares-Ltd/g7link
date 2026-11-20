// SPDX-License-Identifier: MPL-2.0
//! The host end to end: an in-process host on a socket in a temp dir, the
//! real `mcp-approve` bridge, and `g7-agents-fake-cli` replaying the
//! captured Claude and Codex sessions.
#![cfg(feature = "fake-cli")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use g7_agents::api::*;
use g7_agents::client::{Client, ClientError, Events, When};
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
            claude_home: dir.join("home/.claude"),
            codex_home: dir.join("home/.codex"),
            proc_root: dir.join("proc"),
            sessions: false,
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
        Client::connect(&self.socket, HelloParams::new(kind, &[])).await.unwrap()
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

    let err =
        Client::connect(&host.socket, HelloParams { version: 2, ..HelloParams::new(ClientKind::Test, &[]) })
            .await
            .unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::UnsupportedVersion));
    let err = Client::connect(
        &host.socket,
        HelloParams { bridge_token: Some("ab".repeat(16)), ..HelloParams::new(ClientKind::Bridge, &[]) },
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
    assert_eq!(
        asked.diff.as_deref(),
        Some(
            "diff --git a//work/notes.txt b//work/notes.txt\n--- /dev/null\n+++ b//work/notes.txt\n@@ -0,0 +1,1 @@\n+a"
        )
    );
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
    let (client, _events) = Client::connect(&socket, HelloParams::new(ClientKind::Test, &[])).await.unwrap();
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

// ---- Phase 1: thinking, turns, phases, denials with words ----

impl TestHost {
    /// A client saying `caps`, subscribed to every thread with deltas.
    async fn subscribed_with(&self, caps: &[&str]) -> (Client, Events) {
        let (client, events) =
            Client::connect(&self.socket, HelloParams::new(ClientKind::Test, caps)).await.unwrap();
        client.subscribe(ThreadSelection::All, true).await.unwrap();
        (client, events)
    }
}

fn items_of(events: &[Event]) -> Vec<&Item> {
    events.iter().filter_map(|e| if let Event::ThreadItem(i) = e { Some(&i.item) } else { None }).collect()
}

/// The text the deltas for `seq` spell.
fn streamed(events: &[Event], seq: u64) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ThreadDelta(d) if d.seq == seq => Some(d.text.as_str()),
            _ => None,
        })
        .collect()
}

async fn read_all(client: &Client, thread_id: &str) -> Vec<Item> {
    client
        .thread_read(ThreadReadParams {
            thread_id: thread_id.to_owned(),
            limit: Some(200),
            ..Default::default()
        })
        .await
        .unwrap()
        .items
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_says_what_the_host_offers() {
    let host = TestHost::start(Options::default()).await;
    let (client, _events) = host.client(ClientKind::Test).await;
    assert_eq!(client.host().caps, [cap::ITEMS2, cap::DENY_MESSAGE]);
    let too_many = HelloParams::new(ClientKind::Test, &["x"; MAX_CAPS + 1]);
    let err = Client::connect(&host.socket, too_many).await.unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::BadRequest));
    let (later, _events) = host.subscribed_with(&[cap::ITEMS2, "something-later"]).await;
    assert!(later.folders_list().await.is_ok(), "a cap this host doesn't know is fine");
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thinking_reaches_old_clients_as_a_step_and_new_ones_as_it_is() {
    let host =
        TestHost::start(Options { claude: Some("claude-06-thinking-text.jsonl"), ..Default::default() })
            .await;
    let (old, mut old_events) = host.subscribed().await;
    let (new, mut new_events) = host.subscribed_with(&[cap::ITEMS2]).await;
    let folder = host.folder(&old).await;
    let thread = old.thread_start(start(AgentId::Claude, &folder, "What is here?")).await.unwrap();
    let old_seen = until_done(&mut old_events, &thread.id).await;
    let new_seen = until_done(&mut new_events, &thread.id).await;
    const THOUGHT: &str = "**Check the folder**\nList it first.";

    // An old client: a "Thinking" step, no text, no deltas, no gap.
    let old_items = items_of(&old_seen);
    assert!(old_items.iter().all(|i| i.kind != ItemKind::Reasoning));
    let thinking: Vec<(u64, u32, ToolStatus, &str, bool)> = old_items
        .iter()
        .filter_map(|i| {
            let tool = i.tool.as_ref().filter(|t| t.name == "reasoning")?;
            Some((i.seq, i.rev, tool.status, tool.title.as_str(), i.text.is_none()))
        })
        .collect();
    assert_eq!(
        thinking,
        [(2, 0, ToolStatus::Running, "Thinking", true), (2, 1, ToolStatus::Ok, "Thinking", true)]
    );
    assert_eq!(streamed(&old_seen, 2), "", "no deltas of thinking");
    let mut seqs: Vec<u64> = old_items.iter().map(|i| i.seq).collect();
    seqs.dedup();
    assert_eq!(seqs, (1..=4).collect::<Vec<u64>>(), "every number, in order");
    assert!(!format!("{old_seen:?}").contains(THOUGHT));

    // A new client: the thinking, its text and its deltas.
    let reasoning: Vec<&Item> =
        items_of(&new_seen).into_iter().filter(|i| i.kind == ItemKind::Reasoning).collect();
    assert_eq!(reasoning.len(), 2);
    assert_eq!((reasoning[0].rev, reasoning[0].end_ms), (0, None));
    assert_eq!(reasoning[1].text.as_deref(), Some(THOUGHT));
    assert!(reasoning[1].end_ms.is_some() && reasoning[1].turn == Some(1));
    assert_eq!(streamed(&new_seen, 2), THOUGHT);
    assert!(!format!("{new_seen:?}").contains("SIGNATURE"), "the signature never leaves the host");

    let old_read = read_all(&old, &thread.id).await;
    let new_read = read_all(&new, &thread.id).await;
    assert_eq!(old_read[1].kind, ItemKind::Tool);
    assert_eq!(old_read[1].tool.as_ref().unwrap().title, "Thinking");
    assert_eq!((new_read[1].kind, new_read[1].text.as_deref()), (ItemKind::Reasoning, Some(THOUGHT)));
    let tool = new_read[2].tool.as_ref().unwrap();
    assert_eq!((tool.class, tool.status), (Some(ToolClass::Command), ToolStatus::Ok));
    assert!(new_read[2].end_ms.is_some());
    let answer = &new_read[3];
    assert_eq!((answer.kind, answer.phase, answer.turn), (ItemKind::Assistant, Some(Phase::Final), Some(1)));
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn items_carry_their_turn_and_what_they_are() {
    let host =
        TestHost::start(Options { claude: Some("claude-02-followup-approve.jsonl"), ..Default::default() })
            .await;
    let (client, mut events) = host.subscribed_with(&[cap::ITEMS2]).await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    until_done(&mut events, &thread.id).await;
    client.thread_send(thread.id.clone(), "Create hello.txt".into(), Origin::launcher()).await.unwrap();
    let asked = approval(&mut events).await;
    assert_eq!(asked.item_id.as_deref(), Some("toolu_019665aUVn5LSZP1U45iVqz1"));
    client.approval_answer(asked.token, Decision::Allow, Origin::launcher()).await.unwrap();
    until_done(&mut events, &thread.id).await;
    let items = read_all(&client, &thread.id).await;
    let shape: Vec<(ItemKind, Option<u32>)> = items.iter().map(|i| (i.kind, i.turn)).collect();
    assert_eq!(
        shape,
        [
            (ItemKind::User, Some(1)),
            (ItemKind::Tool, Some(1)),
            (ItemKind::Assistant, Some(1)),
            (ItemKind::User, Some(2)),
            (ItemKind::Reasoning, Some(2)),
            (ItemKind::Tool, Some(2)),
            (ItemKind::Assistant, Some(2)),
        ]
    );
    assert!(items.iter().filter(|i| i.kind != ItemKind::User).all(|i| i.end_ms.is_some()), "{items:#?}");
    assert_eq!(items[2].phase, Some(Phase::Final));
    assert_eq!(items[6].phase, Some(Phase::Final));
    assert_eq!(items[4].text.as_deref(), Some(""), "the CLI sent no thinking text");
    let bash = items[1].tool.as_ref().unwrap();
    assert_eq!((bash.class, bash.path.as_deref()), (Some(ToolClass::Command), None));
    let write = items[5].tool.as_ref().unwrap();
    assert_eq!((write.class, write.path.as_deref()), (Some(ToolClass::Edit), Some("/work/hello.txt")));
    assert!(!write.by_rule, "allowed by hand");
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_reasoning_summaries_reach_new_clients() {
    let host =
        TestHost::start(Options { codex: Some("codex-04-reasoning-summary.jsonl"), ..Default::default() })
            .await;
    let (client, mut events) = host.subscribed_with(&[cap::ITEMS2]).await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Codex, &folder, "Look, then answer.")).await.unwrap();
    let seen = until_done(&mut events, &thread.id).await;
    assert_eq!(done_of(&seen).outcome, Outcome::Done);
    let items = read_all(&client, &thread.id).await;
    let thoughts: Vec<(&str, Option<&str>)> = items
        .iter()
        .filter(|i| i.kind == ItemKind::Reasoning)
        .map(|i| (i.id.as_str(), i.text.as_deref()))
        .collect();
    assert_eq!(
        thoughts,
        [("rs_c4_1", Some("**Look first**\n\nThen answer.")), ("rs_c4_2", Some("Short check."))]
    );
    let first = items.iter().find(|i| i.id == "rs_c4_1").unwrap();
    assert_eq!(streamed(&seen, first.seq), "**Look first**\n\nThen answer.");
    assert!(!format!("{seen:?}").contains("raw, never shown"));
    assert_eq!(items.last().unwrap().phase, Some(Phase::Final));
    assert!(host.log().contains("summary auto"), "{}", host.log());
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_denied_with_words_goes_on_in_the_same_turn() {
    let host =
        TestHost::start(Options { claude: Some("claude-03-resume-deny.jsonl"), ..Default::default() }).await;
    let (client, mut events) = host.subscribed_with(&[cap::ITEMS2]).await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "Run: touch denied.txt")).await.unwrap();
    let asked = approval(&mut events).await;
    let answer = |decision, message: &str| ApprovalAnswerParams {
        token: asked.token.clone(),
        decision,
        by: Origin::phone("abc", "Pixel"),
        message: Some(message.to_owned()),
    };
    for bad in [answer(Decision::Allow, "fine"), answer(Decision::Deny, &"x".repeat(MAX_DENY_MESSAGE + 1))] {
        let err = client.approval_answer_with(bad).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::BadRequest));
    }
    client.approval_answer_with(answer(Decision::Deny, "Use a temp dir")).await.unwrap();
    let rest = until_done(&mut events, &thread.id).await;
    assert!(rest.iter().any(|e| matches!(e, Event::ApprovalResolved(r)
        if r.token == asked.token && r.outcome == ApprovalOutcome::Denied
            && r.message.as_deref() == Some("Use a temp dir"))));
    assert_eq!(rest.iter().filter(|e| matches!(e, Event::ThreadDone(_))).count(), 1);
    assert_eq!(done_of(&rest).outcome, Outcome::Done);
    let log = host.log();
    assert!(
        log.contains(
            r#"bridge {"behavior":"deny","message":"The user denied this and said: Use a temp dir"}"#
        ),
        "{log}"
    );
    let items = read_all(&client, &thread.id).await;
    let said = items.iter().find(|i| i.deny_of.is_some()).expect("the words are in the thread");
    assert_eq!(said.kind, ItemKind::User);
    assert_eq!(said.text.as_deref(), Some("Use a temp dir"));
    assert_eq!(said.deny_of, asked.item_id);
    assert_eq!(said.turn, Some(1), "the same turn");
    assert_eq!(said.origin, Some(Origin::phone("abc", "Pixel")));
    let tool = items.iter().find(|i| Some(&i.id) == asked.item_id.as_ref()).unwrap();
    assert!(tool.seq < said.seq);
    assert_eq!(tool.tool.as_ref().unwrap().status, ToolStatus::Denied);
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_denied_with_words_gets_them_as_the_next_turn() {
    let host =
        TestHost::start(Options { codex: Some("codex-05-deny-say-why.jsonl"), ..Default::default() }).await;
    let (client, mut events) = host.subscribed_with(&[cap::ITEMS2]).await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Codex, &folder, "Create notes.txt")).await.unwrap();
    let asked = approval(&mut events).await;
    assert_eq!(asked.item_id.as_deref(), Some("fc_c5"));
    client
        .approval_answer_with(ApprovalAnswerParams {
            token: asked.token.clone(),
            decision: Decision::Deny,
            by: Origin::launcher(),
            message: Some("Use a temp dir".into()),
        })
        .await
        .unwrap();
    let rest = until_done(&mut events, &thread.id).await;
    assert_eq!(rest.iter().filter(|e| matches!(e, Event::ThreadDone(_))).count(), 1, "the stop isn't an end");
    let done = done_of(&rest);
    assert_eq!((done.outcome, done.summary.as_str()), (Outcome::Done, "Wrote it to a temp dir instead."));
    let states: Vec<State> = rest
        .iter()
        .filter_map(|e| if let Event::ThreadState(s) = e { Some(s.thread.state) } else { None })
        .collect();
    assert!(!states[..states.len() - 1].iter().any(|s| !s.is_running()), "{states:?}");
    assert!(
        rest.iter().any(
            |e| matches!(e, Event::ApprovalResolved(r) if r.message.as_deref() == Some("Use a temp dir"))
        )
    );
    let log = host.log();
    assert!(log.contains("decision decline") && log.contains("method turn/interrupt"), "{log}");
    assert_eq!(log.lines().filter(|l| *l == "method turn/start").count(), 2, "{log}");
    let items = read_all(&client, &thread.id).await;
    let said = items.iter().find(|i| i.deny_of.is_some()).expect("the words are in the thread");
    assert_eq!(
        (said.kind, said.turn, said.text.as_deref()),
        (ItemKind::User, Some(2), Some("Use a temp dir"))
    );
    assert_eq!(said.deny_of.as_deref(), Some("fc_c5"));
    let change = items.iter().find(|i| i.id == "fc_c5").unwrap();
    assert_eq!((change.turn, change.tool.as_ref().unwrap().status), (Some(1), ToolStatus::Denied));
    let answer = items.last().unwrap();
    assert_eq!((answer.turn, answer.phase), (Some(2), Some(Phase::Final)));
    let read = client
        .thread_read(ThreadReadParams { thread_id: thread.id.clone(), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(read.thread.state, State::Done);
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thread_from_before_turns_continues_with_the_next_one() {
    let mut host = TestHost::start(Options::default()).await;
    let folder = {
        let (client, _events) = host.client(ClientKind::Test).await;
        host.folder(&client).await
    };
    let _ = host.stop.take().unwrap().send(());
    host.task.take().unwrap().await.unwrap().unwrap();
    let dir = host.dir.path().to_owned();
    let id = "d".repeat(32);
    let thread_dir = dir.join("data/g7-agents/threads").join(&id);
    std::fs::create_dir_all(&thread_dir).unwrap();
    let meta = json!({"version": 1, "next_seq": 6, "thread": {
        "id": id, "agent": "claude", "folder_id": folder.id, "folder_label": "work", "title": "t",
        "state": "done", "mode": "supervised", "origin": {"kind": "launcher"},
        "created_ms": 1, "updated_ms": 1, "last_seq": 5, "pending": 0}});
    std::fs::write(thread_dir.join("meta.json"), meta.to_string()).unwrap();
    let lines = [
        json!({"seq": 1, "id": "user-1", "kind": "user", "ts_ms": 1, "rev": 0, "text": "a", "origin": {"kind": "launcher"}}),
        json!({"seq": 2, "id": "m:0", "kind": "assistant", "ts_ms": 2, "rev": 0, "text": "b"}),
        json!({"seq": 3, "id": "user-3", "kind": "user", "ts_ms": 3, "rev": 0, "text": "c", "origin": {"kind": "launcher"}}),
        json!({"seq": 4, "id": "m:1", "kind": "assistant", "ts_ms": 4, "rev": 0, "text": "d"}),
        json!({"seq": 5, "id": "later", "kind": "hologram", "ts_ms": 5, "rev": 0}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(thread_dir.join("items.jsonl"), text).unwrap();
    let opts = Options::default();
    let config = TestHost::config(&dir, &opts, &host.log);
    let socket = config.socket.clone();
    let (stop, stopped) = oneshot::channel::<()>();
    let mut task = tokio::spawn(host::serve(config, async move {
        let _ = stopped.await;
    }));
    wait_for(&socket, &mut task).await;
    host.stop = Some(stop);
    host.task = Some(task);
    let (client, mut events) = host.subscribed_with(&[cap::ITEMS2]).await;
    let before = read_all(&client, &id).await;
    assert_eq!(before.iter().map(|i| i.seq).collect::<Vec<_>>(), [1, 2, 3, 4], "a later kind is left out");
    assert!(before.iter().all(|i| i.turn.is_none()));
    client.thread_send(id.clone(), "Again.".into(), Origin::launcher()).await.unwrap();
    assert_eq!(done_of(&until_done(&mut events, &id).await).outcome, Outcome::Done);
    let after = read_all(&client, &id).await;
    let new: Vec<(u64, ItemKind, Option<u32>)> =
        after.iter().filter(|i| i.seq > 5).map(|i| (i.seq, i.kind, i.turn)).collect();
    assert_eq!(
        new,
        [(6, ItemKind::User, Some(3)), (7, ItemKind::Tool, Some(3)), (8, ItemKind::Assistant, Some(3))],
        "the next turn after two"
    );
    host.stop().await;
}

// ---- Replacing an outdated host ----

/// Real `g7-agents serve` processes, kept apart from the user's own (socket,
/// data, home and agents all in a test folder), stopped when dropped.
#[derive(Clone)]
struct RealHosts {
    dir: PathBuf,
    codex: Option<PathBuf>,
    started: std::sync::Arc<std::sync::Mutex<Vec<std::process::Child>>>,
}

impl RealHosts {
    fn new(dir: &Path, codex: Option<PathBuf>) -> RealHosts {
        for sub in ["run", "home", "work"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::set_permissions(dir.join("run"), std::fs::Permissions::from_mode(0o700)).unwrap();
        RealHosts { dir: dir.to_owned(), codex, started: Default::default() }
    }

    fn socket(&self) -> PathBuf {
        self.dir.join("run/host.sock")
    }

    /// Starts `exe serve`; its pid.
    fn start(&self, exe: &Path) -> std::io::Result<u32> {
        use std::os::unix::process::CommandExt;
        let codex = self.codex.clone().unwrap_or_else(|| PathBuf::from("/bin/false"));
        let child = std::process::Command::new(exe)
            .arg("serve")
            .env_clear()
            .env("HOME", self.dir.join("home"))
            .env("PATH", "/usr/bin:/bin")
            .env("G7_AGENTS_SOCKET", self.socket())
            .env("G7_AGENTS_DATA_DIR", self.dir.join("data"))
            .env("G7_AGENTS_NO_NOTIFY", "1")
            .env("G7_AGENTS_NO_SESSIONS", "1")
            .env("G7_AGENTS_CLAUDE_BIN", "/bin/false")
            .env("G7_AGENTS_CODEX_BIN", codex)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()?;
        let pid = child.id();
        self.started.lock().unwrap().push(child);
        Ok(pid)
    }

    /// How a client of `version` starts this build's host here.
    fn launch(&self, version: &str) -> g7_agents::client::Launch {
        let hosts = self.clone();
        g7_agents::client::Launch::new(self.socket(), version, move || {
            hosts.start(Path::new(env!("CARGO_BIN_EXE_g7-agents"))).map(drop)
        })
    }

    async fn listening(&self) {
        let deadline = tokio::time::Instant::now() + WAIT;
        while UnixStream::connect(self.socket()).await.is_err() {
            assert!(tokio::time::Instant::now() < deadline, "the host didn't listen");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Waits until `pid` has exited; its status.
    async fn exited(&self, pid: u32) -> std::process::ExitStatus {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            {
                let mut started = self.started.lock().unwrap();
                let child = started.iter_mut().find(|c| c.id() == pid).expect("a host started here");
                if let Some(status) = child.try_wait().unwrap() {
                    return status;
                }
            }
            assert!(tokio::time::Instant::now() < deadline, "the host didn't exit");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn count(&self) -> usize {
        self.started.lock().unwrap().len()
    }
}

impl Drop for RealHosts {
    fn drop(&mut self) {
        // The last clone (the test's own) stops whatever still runs.
        if std::sync::Arc::strong_count(&self.started) > 1 {
            return;
        }
        for child in self.started.lock().unwrap_or_else(|e| e.into_inner()).iter_mut() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

fn private_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

const NEWER: &str = "99.0.0";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_outdated_host_gives_way_to_the_current_one() {
    let dir = private_dir();
    let hosts = RealHosts::new(dir.path(), None);
    let old = hosts.start(Path::new(env!("CARGO_BIN_EXE_g7-agents"))).unwrap();
    hosts.listening().await;
    let launch = hosts.launch(NEWER);
    let (client, _events) = Client::connect_or_spawn_with(&launch, ClientKind::Test).await.unwrap();
    assert!(hosts.exited(old).await.success(), "the old host stopped cleanly on SIGTERM");
    assert_eq!(hosts.count(), 2, "one new host was started");
    assert_ne!(client.host().pid, old);
    assert!(client.folders_list().await.is_ok(), "the new host serves");
    // It is no newer than this test pretends to be, but it isn't replaced again.
    assert_eq!(client.outdated(NEWER), Some(g7_agents::client::Outdated::Older));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_busy_outdated_host_is_kept_until_its_threads_finish() {
    let dir = private_dir();
    let log = dir.path().join("cli.log");
    let codex = wrapper(dir.path(), "codex", "codex-01-create-approve.jsonl", None, &log);
    let hosts = RealHosts::new(dir.path(), Some(codex));
    let old = hosts.start(Path::new(env!("CARGO_BIN_EXE_g7-agents"))).unwrap();
    hosts.listening().await;
    let (user, mut events) =
        Client::connect(&hosts.socket(), HelloParams::new(ClientKind::Test, &[])).await.unwrap();
    user.subscribe(ThreadSelection::All, false).await.unwrap();
    let work = dir.path().join("work").to_string_lossy().into_owned();
    let folder = user.folders_add(work, Some("work".into())).await.unwrap();
    let thread =
        user.thread_start(start(AgentId::Codex, &folder, "Create hello.txt, then run ls")).await.unwrap();
    let asked = approval(&mut events).await;

    let launch = hosts.launch(NEWER);
    let (client, _events) = Client::connect_or_spawn_with(&launch, ClientKind::Test).await.unwrap();
    assert_eq!(client.host().pid, old, "a host with a thread waiting is kept");
    assert_eq!(hosts.count(), 1);
    assert_eq!(client.outdated(NEWER), Some(g7_agents::client::Outdated::Older));
    assert_eq!(client.running_threads().await.unwrap(), 1);
    assert!(client.replace(&launch, When::Idle).await.unwrap().is_none(), "still busy");

    user.approval_answer(asked.token, Decision::Allow, Origin::launcher()).await.unwrap();
    assert_eq!(done_of(&until_done(&mut events, &thread.id).await).outcome, Outcome::Done);
    let (replaced, _events) = client.replace(&launch, When::Idle).await.unwrap().expect("replaced once idle");
    assert!(hosts.exited(old).await.success());
    assert_ne!(replaced.host().pid, old);
    let threads = replaced.threads_list(None, None).await.unwrap();
    assert_eq!(threads.len(), 1, "the new host has the same threads");
    assert_eq!(threads[0].state, State::Done);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_user_can_restart_a_busy_outdated_host() {
    let dir = private_dir();
    let log = dir.path().join("cli.log");
    let codex = wrapper(dir.path(), "codex", "codex-01-create-approve.jsonl", None, &log);
    let hosts = RealHosts::new(dir.path(), Some(codex));
    let old = hosts.start(Path::new(env!("CARGO_BIN_EXE_g7-agents"))).unwrap();
    hosts.listening().await;
    let (client, mut events) =
        Client::connect(&hosts.socket(), HelloParams::new(ClientKind::Test, &[])).await.unwrap();
    client.subscribe(ThreadSelection::All, false).await.unwrap();
    let work = dir.path().join("work").to_string_lossy().into_owned();
    let folder = client.folders_add(work, Some("work".into())).await.unwrap();
    let thread =
        client.thread_start(start(AgentId::Codex, &folder, "Create hello.txt, then run ls")).await.unwrap();
    approval(&mut events).await;
    let launch = hosts.launch(NEWER);
    assert!(client.replace(&launch, When::Idle).await.unwrap().is_none());
    let (replaced, _events) = client.replace(&launch, When::Now).await.unwrap().expect("replaced");
    assert!(hosts.exited(old).await.success());
    let threads = replaced.threads_list(None, None).await.unwrap();
    assert_eq!(threads[0].id, thread.id);
    assert_eq!(threads[0].state, State::Interrupted, "its running thread stopped");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_whose_program_was_upgraded_gives_way_at_the_same_version() {
    let dir = private_dir();
    let hosts = RealHosts::new(dir.path(), None);
    let bin = dir.path().join("old/g7-agents");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_g7-agents"), &bin).unwrap();
    let old = hosts.start(&bin).unwrap();
    hosts.listening().await;
    // The package manager replaces the file.
    std::fs::remove_file(&bin).unwrap();
    let launch = hosts.launch(g7_agents::client::VERSION);
    let (client, _events) = Client::connect_or_spawn_with(&launch, ClientKind::Test).await.unwrap();
    assert!(hosts.exited(old).await.success());
    assert_ne!(client.host().pid, old);
    assert_eq!(client.outdated(g7_agents::client::VERSION), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_current_host_is_left_alone() {
    let dir = private_dir();
    let hosts = RealHosts::new(dir.path(), None);
    let pid = hosts.start(Path::new(env!("CARGO_BIN_EXE_g7-agents"))).unwrap();
    hosts.listening().await;
    let launch = hosts.launch(g7_agents::client::VERSION);
    let (client, _events) = Client::connect_or_spawn_with(&launch, ClientKind::Test).await.unwrap();
    assert_eq!(client.host().pid, pid);
    assert_eq!(client.process().map(|p| p.pid()), Some(pid), "the host's own process was found");
    assert!(client.replace(&launch, When::Now).await.unwrap().is_none(), "never for a current host");
    assert_eq!(hosts.count(), 1);
}
