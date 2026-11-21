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
    /// `PATH` holds only `git` (never the user's `gh`), and git reads no system configuration.
    git: bool,
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
            env: if opts.git {
                let bin = dir.join("bin");
                std::fs::create_dir_all(&bin).unwrap();
                if let Some(git) = real_git() {
                    std::os::unix::fs::symlink(git, bin.join("git")).unwrap();
                }
                vec![
                    ("HOME".into(), home.into_os_string()),
                    ("PATH".into(), bin.into_os_string()),
                    ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
                ]
            } else {
                vec![("HOME".into(), home.into_os_string()), ("PATH".into(), "/usr/bin:/bin".into())]
            },
            idle_exit: host::IDLE_EXIT,
            agent_idle: opts.agent_idle.unwrap_or(Duration::from_secs(600)),
            approval_lifetime_ms: opts.approval_lifetime_ms.unwrap_or(APPROVAL_LIFETIME_MS),
            claude_home: dir.join("home/.claude"),
            codex_home: dir.join("home/.codex"),
            proc_root: dir.join("proc"),
            sessions: false,
            keep_awake_bus: false,
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

/// Removes the thread database a host made, so the next one brings in a
/// file store planted from an earlier release (it is read only then).
fn forget_database(dir: &Path) {
    for name in ["agents.db", "agents.db-wal", "agents.db-shm"] {
        let _ = std::fs::remove_file(dir.join("data/g7-agents").join(name));
    }
}

async fn approval(events: &mut Events) -> Approval {
    match next(events, "approval.requested", |e| matches!(e, Event::ApprovalRequested(_))).await {
        Event::ApprovalRequested(a) => a.approval,
        _ => unreachable!(),
    }
}

fn start(agent: AgentId, folder: &Folder, prompt: &str) -> ThreadStartParams {
    ThreadStartParams::new(agent, folder.id.clone(), prompt.into(), Mode::Supervised, Origin::launcher())
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
    assert!(log.contains("--mcp-config") && log.contains("--permission-prompt-tool mcp__g7__approve"));
    assert!(!log.contains("--strict-mcp-config"), "the folder uses the user's MCP servers by default");
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
    client
        .thread_send(thread.id.clone(), "Create hello.txt containing exactly: hi".into(), Origin::launcher())
        .await
        .unwrap();
    // While it works a message waits its turn; while it waits on a request, BUSY.
    let again = ThreadSendParams::new(thread.id.clone(), "again".into(), Origin::launcher());
    match client.thread_send_with(again).await {
        Ok(sent) => {
            let queued = sent.queued.expect("it waits its turn");
            let remove = QueueItemParams { thread_id: thread.id.clone(), id: queued.id, by: None };
            client.queue_remove(remove).await.unwrap();
        }
        Err(e) => assert_eq!(e.code(), Some(ErrorCode::Busy)),
    }
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
    forget_database(&dir_path);
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
    assert_eq!(
        client.host().caps,
        [
            cap::ITEMS2,
            cap::DENY_MESSAGE,
            cap::CONTROL,
            cap::HISTORY,
            cap::UNDO,
            cap::PRESENCE,
            cap::GIT,
            cap::PLACE,
            cap::HANDOFF,
            cap::PHOTO
        ]
    );
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
    forget_database(&dir);
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
        let mut tries = 0;
        let spawn = || {
            std::process::Command::new(exe)
                .arg("serve")
                .env_clear()
                .env("HOME", self.dir.join("home"))
                .env("PATH", "/usr/bin:/bin")
                .env("G7_AGENTS_SOCKET", self.socket())
                .env("G7_AGENTS_DATA_DIR", self.dir.join("data"))
                .env("G7_AGENTS_NO_NOTIFY", "1")
                .env("G7_AGENTS_NO_SESSIONS", "1")
                .env("G7_AGENTS_CLAUDE_BIN", "/bin/false")
                .env("G7_AGENTS_CODEX_BIN", &codex)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .process_group(0)
                .spawn()
        };
        // A binary just copied can be busy for a moment: another test's child, forked while
        // the copy was open for writing, holds it until that child execs.
        let child = loop {
            match spawn() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 100 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(20));
                }
                other => break other?,
            }
        };
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_lists_come_from_the_clis_and_the_control_methods_answer() {
    let host = TestHost::start(Options::default()).await;
    let (client, _events) = host.subscribed().await;
    let agents = client.agents_list(false).await.unwrap();
    let claude = agents.iter().find(|a| a.id == AgentId::Claude).unwrap();
    let codex = agents.iter().find(|a| a.id == AgentId::Codex).unwrap();
    assert!(claude.model("default").is_some_and(|m| m.default), "Claude's own list: {:?}", claude.models);
    assert!(claude.can(Can::Steer) && claude.can(Can::Plan) && claude.can(Can::McpSwitch));
    assert_eq!(codex.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["gpt-5.5-codex"]);
    assert!(codex.can(Can::Steer) && !codex.can(Can::McpSwitch));
    assert_eq!(codex.limits.first().map(|l| (l.window.as_str(), l.used_pct)), Some(("300", Some(41))));
    let log = host.log();
    assert!(log.contains("probe control initialize"), "Claude was asked on its control channel: {log}");
    assert!(
        !log.lines().any(|l| l.starts_with("args") && l.contains("--no-session-persistence")),
        "the probe is no thread"
    );

    let folder = host.folder(&client).await;
    let commands = client.commands_list(AgentId::Claude, folder.id.clone()).await.unwrap();
    let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"review") && !names.contains(&"login"), "{names:?}");
    let skills = client.commands_list(AgentId::Codex, folder.id.clone()).await.unwrap();
    let skill_names: Vec<&str> = skills.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(skill_names, ["release-notes", "compact"], "G7's own names left out, compact once");

    // A model the list doesn't have needs `custom_model`; a typed one may not be a flag.
    let unknown = ThreadStartParams { model: Some("opus-9".into()), ..start(AgentId::Claude, &folder, "x") };
    assert_eq!(client.thread_start(unknown).await.unwrap_err().code(), Some(ErrorCode::BadRequest));
    let flag = ThreadStartParams {
        model: Some("--dangerously".into()),
        custom_model: true,
        ..start(AgentId::Claude, &folder, "x")
    };
    assert_eq!(client.thread_start(flag).await.unwrap_err().code(), Some(ErrorCode::BadRequest));

    // Drafts, the folder's MCP switch, keep-awake and the history.
    let key = format!("new:{}", folder.id);
    let set = |text: &str, rev: u64| DraftSetParams {
        key: key.clone(),
        text: text.into(),
        images: vec![],
        rev,
        by: None,
    };
    assert_eq!(client.draft_set(set("Fix the", 0)).await.unwrap(), 1);
    let conflict = client.draft_set(set("Fix it", 0)).await.unwrap_err();
    let ClientError::Remote(error) = conflict else { panic!("not the host's error") };
    assert_eq!(error.conflict_draft().map(|d| d.text), Some("Fix the".into()));
    assert!(!client.folders_set(folder.id.clone(), false).await.unwrap().mcp);
    assert!(!client.folders_list().await.unwrap()[0].mcp, "stored");
    let settings = client
        .settings_set(SettingsSetParams { keep_awake: Some(false), ..Default::default() })
        .await
        .unwrap();
    assert!(!settings.keep_awake && settings.follow_sessions && settings.limit_alarm);
    assert!(!client.settings_get().await.unwrap().keep_awake);
    let settings = client
        .settings_set(SettingsSetParams { limit_alarm: Some(false), ..Default::default() })
        .await
        .unwrap();
    assert!(!settings.limit_alarm && !settings.keep_awake, "what is absent is unchanged");
    assert!(!client.settings_get().await.unwrap().limit_alarm);
    client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    assert_eq!(client.history_list(None).await.unwrap(), ["List the files here."]);
    assert_eq!(client.draft_get(key.clone()).await.unwrap().map(|d| d.text), Some(String::new()), "sent");
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_history_and_undo_methods_are_named_and_the_folder_switch_is_kept() {
    let host = TestHost::start(Options::default()).await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    assert!(folder.checkpoints, "restore points are on for a new folder");
    let thread =
        client.thread_start_with(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    assert!(!thread.duplicate);
    let id = thread.thread.id.clone();
    let mut bad_key = start(AgentId::Claude, &folder, "x");
    bad_key.key = Some("NOT-HEX".into());
    assert_eq!(client.thread_start(bad_key).await.unwrap_err().code(), Some(ErrorCode::BadRequest));

    // Never "unknown method": a client tells this host from an older one by
    // its caps, not by guessing.
    let by = Origin::launcher();
    let not_yet = |e: ClientError| {
        assert!(!e.is_unknown_method(), "{e}");
        e.code()
    };
    let unavailable = Some(ErrorCode::Unavailable);
    // Built: answers of their own, never "not yet".
    until_done(&mut events, &id).await;
    let turns = ThreadTurnsParams { thread_id: id.clone(), ..Default::default() };
    let listed = client.thread_turns(turns).await.unwrap();
    assert_eq!(listed.iter().map(|t| t.turn).collect::<Vec<_>>(), [1]);
    assert_eq!(listed[0].checkpoint, Checkpoint::NotGit, "the work folder isn't a git repository");
    let files = TurnFilesParams::new(id.clone(), TurnRange::One(1));
    assert_eq!(client.turn_files(files).await.unwrap_err().code(), unavailable, "nothing was recorded");
    let apply =
        RewindApplyParams { token: "ab".repeat(16), include_later: false, refill: false, by: by.clone() };
    assert_eq!(client.rewind_apply(apply).await.unwrap_err().code(), Some(ErrorCode::NotFound));
    assert_eq!(
        client.rewind_undo(id.clone(), by.clone()).await.unwrap_err().code(),
        Some(ErrorCode::NotFound)
    );
    // Sessions: served now (each answer is the method's own, never "unknown method").
    client.sessions_scan(SessionsScanParams::default()).await.unwrap();
    let found = format!("sc{}", "ab".repeat(16));
    let _ = client.sessions_import(vec![found.clone()], by.clone()).await.map_err(not_yet);
    let _ = client.import_cancel("ab".repeat(8)).await.map_err(not_yet);
    let _ = client.session_preview(found, None).await.map_err(not_yet);
    // A request of the wrong shape is still the client's mistake.
    let wrong =
        client.call_value(method::THREAD_MARK, json!({"thread_ids": 3}), Some(WAIT)).await.unwrap_err();
    assert!(!wrong.is_unknown_method() && wrong.code() == Some(ErrorCode::BadRequest), "{wrong}");

    // The shelves: an unmarked thread is on the main one only.
    let main = client.threads_list_with(ThreadsListParams::default()).await.unwrap();
    assert_eq!(main.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), [id.as_str()]);
    for show in [ThreadShow::Archived, ThreadShow::Snoozed] {
        let shelf = ThreadsListParams { show: Some(show), ..Default::default() };
        assert!(client.threads_list_with(shelf).await.unwrap().is_empty(), "{show:?}");
    }

    // The restore points switch, apart from the MCP one, and stored.
    let off = FoldersSetParams { id: folder.id.clone(), checkpoints: Some(false), ..Default::default() };
    let folder_now = client.folders_set_with(off).await.unwrap();
    assert!(!folder_now.checkpoints && folder_now.mcp);
    let listed = client.folders_list().await.unwrap();
    assert!(!listed[0].checkpoints && listed[0].mcp, "stored");
    let neither = FoldersSetParams { id: folder.id.clone(), ..Default::default() };
    let same = client.folders_set_with(neither).await.unwrap();
    assert_eq!((same.mcp, same.checkpoints), (true, false), "nothing named, nothing changed");
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn marks_move_threads_between_shelves_and_the_host_keeps_them() {
    // A follow-up starts a new process that resumes (the first idles out).
    let host = TestHost::start(Options {
        claude_resume: Some("claude-01-list.jsonl"),
        agent_idle: Some(Duration::from_millis(300)),
        ..Default::default()
    })
    .await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let mut ids = Vec::new();
    for prompt in ["One", "Two"] {
        let thread = client.thread_start(start(AgentId::Claude, &folder, prompt)).await.unwrap();
        until_done(&mut events, &thread.id).await;
        ids.push(thread.id);
    }
    let (one, two) = (ids[0].clone(), ids[1].clone());
    let by = Origin::launcher();
    let marks =
        |ids: &[&String]| ThreadMarkParams::new(ids.iter().map(|s| (*s).clone()).collect(), by.clone());
    let main = |client: Client| async move {
        let list = client.threads_list_with(ThreadsListParams::default()).await.unwrap();
        list.into_iter().map(|t| t.id).collect::<Vec<_>>()
    };
    // Nobody looked: both finished threads are unread.
    let listed = client.threads_list_with(ThreadsListParams::default()).await.unwrap();
    assert!(listed.iter().all(|t| t.unread), "{listed:#?}");
    assert_eq!(main(client.clone()).await, [two.clone(), one.clone()], "newest first");

    // Pinned comes before the rest.
    let pinned = client.thread_mark(ThreadMarkParams { pinned: Some(true), ..marks(&[&one]) }).await.unwrap();
    assert!(pinned.threads[0].pinned && pinned.busy.is_empty());
    assert_eq!(main(client.clone()).await, [one.clone(), two.clone()]);
    let state = next(
        &mut events,
        "thread.state for the pin",
        |e| matches!(e, Event::ThreadState(s) if s.thread.id == one && s.thread.pinned),
    )
    .await;
    assert!(matches!(state, Event::ThreadState(_)));

    // Looking at a thread reads it; "Mark unread" brings it back.
    let (viewer, _viewer_events) = host.client(ClientKind::Link).await;
    viewer.ui_viewing(Some(two.clone())).await.unwrap();
    next(
        &mut events,
        "two read",
        |e| matches!(e, Event::ThreadState(s) if s.thread.id == two && !s.thread.unread),
    )
    .await;
    viewer.ui_viewing(None).await.unwrap();
    let unread = client.thread_mark(ThreadMarkParams { unread: Some(true), ..marks(&[&two]) }).await.unwrap();
    assert!(unread.threads[0].unread);

    // Archived: off the main list, on its own shelf; sending brings it back.
    client.thread_mark(ThreadMarkParams { archived: Some(true), ..marks(&[&two]) }).await.unwrap();
    assert_eq!(main(client.clone()).await, std::slice::from_ref(&one));
    let archived = ThreadsListParams { show: Some(ThreadShow::Archived), ..Default::default() };
    let shelf = client.threads_list_with(archived.clone()).await.unwrap();
    assert_eq!(shelf.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), [two.as_str()]);
    tokio::time::sleep(Duration::from_millis(1200)).await;
    client.thread_send(two.clone(), "And again.".into(), Origin::launcher()).await.unwrap();
    assert!(client.threads_list_with(archived).await.unwrap().is_empty(), "sent to: unarchived");
    until_done(&mut events, &two).await;
    // A thread that waits on a request can't be archived; the others in the call are.
    let waiting = client.thread_start(start(AgentId::Codex, &folder, "Create hello.txt")).await.unwrap();
    approval(&mut events).await;
    let both = ThreadMarkParams { archived: Some(true), ..marks(&[&one, &waiting.id]) };
    let result = client.thread_mark(both).await.unwrap();
    assert_eq!(result.threads.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), [one.as_str()]);
    assert_eq!(result.busy, std::slice::from_ref(&waiting.id));
    let alone = ThreadMarkParams { archived: Some(true), ..marks(&[&waiting.id]) };
    assert_eq!(client.thread_mark(alone).await.unwrap_err().code(), Some(ErrorCode::Busy));
    let back =
        client.thread_mark(ThreadMarkParams { archived: Some(false), ..marks(&[&one]) }).await.unwrap();
    assert!(!back.threads[0].archived);

    // Snoozed: a time ahead, within 30 days; 0 ends it.
    let soon = now_ms() + 3_600_000;
    let snoozed =
        client.thread_mark(ThreadMarkParams { snooze_until_ms: Some(soon), ..marks(&[&one]) }).await.unwrap();
    assert_eq!(snoozed.threads[0].snoozed_until_ms, Some(soon));
    let shelf = ThreadsListParams { show: Some(ThreadShow::Snoozed), ..Default::default() };
    assert_eq!(client.threads_list_with(shelf).await.unwrap().len(), 1);
    let past = ThreadMarkParams { snooze_until_ms: Some(1), ..marks(&[&one]) };
    assert_eq!(client.thread_mark(past).await.unwrap_err().code(), Some(ErrorCode::BadRequest));
    let unknown = ThreadMarkParams { pinned: Some(true), ..marks(&[&"f".repeat(32)]) };
    assert_eq!(client.thread_mark(unknown).await.unwrap_err().code(), Some(ErrorCode::NotFound));

    // A name of its own, one clean line.
    let rename = ThreadRenameParams { thread_id: one.clone(), title: "  Tidy\nthe build ".into(), by: None };
    assert_eq!(client.thread_rename(rename).await.unwrap().title, "Tidy the build");
    let empty = ThreadRenameParams { thread_id: one.clone(), title: " ".into(), by: None };
    assert_eq!(client.thread_rename(empty).await.unwrap_err().code(), Some(ErrorCode::BadRequest));
    host.stop().await;

    fn now_ms() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
    }
}

#[test]
fn the_fake_claude_forks_as_the_cli_does() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("cli.log");
    let run = |resume: &Path| {
        std::process::Command::new(env!("CARGO_BIN_EXE_g7-agents-fake-cli"))
            .args(["claude", "-p", "--resume"])
            .arg(resume)
            .arg("--fork-session")
            .env("FAKE_CLI_SCRIPT", fixture("claude-01-list.jsonl"))
            .env("FAKE_CLI_LOG", &log)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    };
    let first_line = |out: &std::process::Output| -> serde_json::Value {
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        serde_json::from_str(text.lines().next().unwrap_or("null")).unwrap()
    };
    // Refused as the CLI refuses it: a result, exit 1.
    let missing = run(&dir.path().join("missing.jsonl"));
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(first_line(&missing)["subtype"], "error_during_execution");
    let garbage = dir.path().join("garbage.jsonl");
    std::fs::write(&garbage, "{}\n{}\n{}\n").unwrap();
    assert_eq!(run(&garbage).status.code(), Some(1));
    // A copy that loads: nothing but its answers, under the fork's id, until stdin closes.
    let cut = dir.path().join("cut.jsonl");
    std::fs::write(&cut, "{\"type\":\"user\",\"uuid\":\"u-1\"}\n").unwrap();
    let loaded = run(&cut);
    assert_eq!(loaded.status.code(), Some(0));
    for line in String::from_utf8_lossy(&loaded.stdout).lines() {
        let line: serde_json::Value = serde_json::from_str(line).unwrap();
        if let Some(id) = line.get("session_id") {
            assert_eq!(id, "f0f0f0f0-0000-4000-8000-00000000f0c5");
        }
    }
    let log = std::fs::read_to_string(&log).unwrap();
    assert!(log.contains(&format!("fork {}", cut.display())), "{log}");
}

/// Runs git in `dir` with a throwaway identity; never the user's configuration.
fn git_in(dir: &Path, home: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@localhost", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// The newest turn once its restore point settled.
async fn settled_turn(client: &Client, thread_id: &str) -> TurnInfo {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let params = ThreadTurnsParams { thread_id: thread_id.to_owned(), ..Default::default() };
        let turns = client.thread_turns(params).await.unwrap();
        if let Some(turn) = turns.first().filter(|t| t.checkpoint != Checkpoint::Pending && t.ended.is_some())
        {
            return turn.clone();
        }
        assert!(tokio::time::Instant::now() < deadline, "the restore point never settled: {turns:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_go_back_to_a_restore_point_and_undo_brings_them_again() {
    if std::process::Command::new("git").arg("--version").output().is_err() {
        assert!(std::env::var_os("CI").is_none(), "CI has git");
        eprintln!("skipped: no git on PATH");
        return;
    }
    let host = TestHost::start(Options::default()).await;
    let home = host.dir.path().join("home");
    git_in(&host.work, &home, &["init", "-q"]);
    std::fs::write(host.work.join("a.txt"), "1\n").unwrap();
    git_in(&host.work, &home, &["add", "-A"]);
    git_in(&host.work, &home, &["commit", "-qm", "one"]);
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    until_done(&mut events, &thread.id).await;
    let turn = settled_turn(&client, &thread.id).await;
    assert_eq!(turn.checkpoint, Checkpoint::Ready, "{turn:?}");
    // Changed after the agent finished: listed apart, left alone unless asked.
    std::fs::write(host.work.join("a.txt"), "2\n").unwrap();
    let ask = RewindPreviewParams {
        thread_id: thread.id.clone(),
        turn: 1,
        conversation: false,
        files: true,
        by: None,
    };
    assert_eq!(
        client.rewind_preview(ask.clone()).await.unwrap_err().code(),
        Some(ErrorCode::Denied),
        "not G7 Link"
    );
    let (link, _link_events) = host.client(ClientKind::Link).await;
    let plan = link.rewind_preview(ask).await.unwrap();
    let files = plan.files.clone().expect("a files part");
    assert_eq!(files.change.len(), 1, "{files:?}");
    assert_eq!(
        (files.change[0].path.as_str(), files.change[0].action, files.change[0].later),
        ("a.txt", RewindAction::Restore, true)
    );
    assert_eq!(std::fs::read_to_string(host.work.join("a.txt")).unwrap(), "2\n", "a preview changes nothing");
    // Single use, and only for the connection that asked.
    let apply = RewindApplyParams {
        token: plan.token.clone(),
        include_later: true,
        refill: false,
        by: Origin::launcher(),
    };
    assert_eq!(client.rewind_apply(apply.clone()).await.unwrap_err().code(), Some(ErrorCode::NotFound));
    let done = link.rewind_apply(apply.clone()).await.unwrap();
    assert!(done.left.is_empty(), "{:?}", done.left);
    assert_eq!(std::fs::read_to_string(host.work.join("a.txt")).unwrap(), "1\n");
    assert_eq!(done.thread.undo.map(|u| u.files), Some(true));
    assert_eq!(link.rewind_apply(apply).await.unwrap_err().code(), Some(ErrorCode::NotFound), "used once");
    let items = read_all(&client, &thread.id).await;
    assert_eq!(items.last().and_then(|i| i.notice), Some(NoticeKind::FilesRestored));
    // Undo: the files as they were, and nothing left to undo.
    let back = link.rewind_undo(thread.id.clone(), Origin::launcher()).await.unwrap();
    assert_eq!(back.undo, None);
    assert_eq!(std::fs::read_to_string(host.work.join("a.txt")).unwrap(), "2\n");
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_conversation_rewinds_and_comes_back_and_a_fork_copies_it() {
    let host = TestHost::start(Options::default()).await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    until_done(&mut events, &thread.id).await;
    settled_turn(&client, &thread.id).await;
    let before = read_all(&client, &thread.id).await;
    assert!(!before.is_empty());
    let native = before_native(&client, &thread.id).await;
    assert!(native.is_some(), "the session has an id");

    // A fork: the items so far, then its notice; nothing runs yet.
    let fork = ThreadForkParams { thread_id: thread.id.clone(), turn: None, by: Origin::phone("p", "Pixel") };
    let forked = client.thread_fork(fork).await.unwrap();
    assert_ne!(forked.id, thread.id);
    assert_eq!(forked.forked_from.as_ref().map(|f| f.thread_id.as_str()), Some(thread.id.as_str()));
    assert_eq!((forked.state, forked.native_id.as_deref()), (State::Done, None));
    let copied = read_all(&client, &forked.id).await;
    assert_eq!(copied.len(), before.len() + 1);
    assert_eq!(copied.last().and_then(|i| i.notice), Some(NoticeKind::Forked), "after the copied items");

    // Back to before the first message: the thread's items are cut, and come back.
    let ask = RewindPreviewParams {
        thread_id: thread.id.clone(),
        turn: 1,
        conversation: true,
        files: false,
        by: None,
    };
    let plan = client.rewind_preview(ask).await.unwrap();
    assert_eq!(plan.cut.map(|c| c.turns), Some(1));
    let apply =
        RewindApplyParams { token: plan.token, include_later: false, refill: true, by: Origin::launcher() };
    let rewound = client.rewind_apply(apply).await.unwrap().thread;
    assert_eq!((rewound.native_id.as_deref(), rewound.undo.map(|u| u.files)), (None, Some(false)));
    let cut = next(&mut events, "thread.cut", |e| matches!(e, Event::ThreadCut(c) if !c.restored)).await;
    let Event::ThreadCut(cut) = cut else { unreachable!() };
    let after = read_all(&client, &thread.id).await;
    assert!(after.iter().all(|i| i.seq < cut.from_seq || i.seq > cut.to_seq), "cut items aren't read");
    assert_eq!(after.last().and_then(|i| i.notice), Some(NoticeKind::EditFromHere));
    let draft = client.draft_get(thread.id.clone()).await.unwrap().map(|d| d.text);
    assert_eq!(draft.as_deref(), Some("List the files here."), "the message back in the composer");
    let undone = client.rewind_undo(thread.id.clone(), Origin::launcher()).await.unwrap();
    assert_eq!(undone.native_id, native, "the session as it was");
    assert_eq!(
        read_all(&client, &thread.id).await.len(),
        before.len(),
        "the items are back, the notice gone"
    );
    host.stop().await;
}

/// A thread's `native_id` as listed (tests).
async fn before_native(client: &Client, thread_id: &str) -> Option<String> {
    let threads = client.threads_list(None, None).await.unwrap();
    threads.into_iter().find(|t| t.id == thread_id).and_then(|t| t.native_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_key_is_taken_once_and_a_start_key_too() {
    let host = TestHost::start(Options {
        claude_resume: Some("claude-01-list.jsonl"),
        agent_idle: Some(Duration::from_millis(300)),
        ..Default::default()
    })
    .await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let keyed =
        ThreadStartParams { key: Some("00000000000000a1".into()), ..start(AgentId::Claude, &folder, "One") };
    let first = client.thread_start_with(keyed.clone()).await.unwrap();
    let again = client.thread_start_with(keyed).await.unwrap();
    assert!(!first.duplicate && again.duplicate);
    assert_eq!(again.thread.id, first.thread.id, "the retry names the thread it started");
    let id = first.thread.id;
    until_done(&mut events, &id).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let send = ThreadSendParams {
        key: Some("00000000000000b2".into()),
        ..ThreadSendParams::new(id.clone(), "Again.".into(), Origin::phone("p", "Pixel"))
    };
    assert!(!client.thread_send_with(send.clone()).await.unwrap().duplicate);
    until_done(&mut events, &id).await;
    let users = |items: &[Item]| items.iter().filter(|i| i.kind == ItemKind::User).count();
    let sent = users(&read_all(&client, &id).await);
    assert!(client.thread_send_with(send).await.unwrap().duplicate, "a retry after a lost reply");
    assert_eq!(users(&read_all(&client, &id).await), sent, "nothing more was sent");
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_launcher_asks_g7_link_to_show_a_thread() {
    let host = TestHost::start(Options::default()).await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    until_done(&mut events, &thread.id).await;
    let (launcher, _launcher_events) = host.client(ClientKind::Launcher).await;
    assert_eq!(
        launcher.ui_open(thread.id.clone()).await.unwrap_err().code(),
        Some(ErrorCode::Unavailable),
        "no G7 Link"
    );
    let (_link, mut link_events) = host.client(ClientKind::Link).await;
    assert_eq!(launcher.ui_open("f".repeat(32)).await.unwrap_err().code(), Some(ErrorCode::NotFound));
    launcher.ui_open(thread.id.clone()).await.unwrap();
    let shown = next(&mut link_events, "ui.open", |e| matches!(e, Event::UiOpen(_))).await;
    assert_eq!(shown, Event::UiOpen(ThreadIdParams { thread_id: thread.id }));
    host.stop().await;
}

fn presence_of(event: Event) -> PresenceEvent {
    match event {
        Event::UiPresence(p) => p,
        other => panic!("not ui.presence: {other:?}"),
    }
}

async fn heard(events: &mut Events, what: &str, pred: impl Fn(&PresenceEvent) -> bool) -> PresenceEvent {
    presence_of(next(events, what, |e| matches!(e, Event::UiPresence(p) if pred(p))).await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presence_is_g7_links_to_say_and_reaches_those_that_asked() {
    let host = TestHost::start(Options::default()).await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    until_done(&mut events, &thread.id).await;
    let listed = client.threads_list(None, None).await.unwrap();
    assert!(listed.iter().any(|t| t.id == thread.id && t.unread), "nobody has looked");

    let connect =
        |kind: ClientKind, caps: &[&str]| Client::connect(&host.socket, HelloParams::new(kind, caps));
    let (launcher, mut launcher_events) = connect(ClientKind::Launcher, &[cap::PRESENCE]).await.unwrap();
    let now = heard(&mut launcher_events, "the state at hello", |_| true).await;
    assert_eq!(now, PresenceEvent::default(), "nobody there");
    let (quiet, mut quiet_events) = connect(ClientKind::Launcher, &[]).await.unwrap();
    let (test, mut test_events) = connect(ClientKind::Test, &[cap::PRESENCE]).await.unwrap();
    let (link, mut link_events) = connect(ClientKind::Link, &[cap::PRESENCE]).await.unwrap();
    heard(&mut link_events, "the state at hello", |_| true).await;

    let view = |device: &str, thread_id: Option<String>| RemoteView {
        device_id: device.into(),
        device_name: "Pixel".into(),
        thread_id,
        session_id: None,
        pane: Some(Pane::Thread),
        dnd: false,
    };
    let shown = UiPresenceParams {
        at_pc: false,
        locked: true,
        dnd: false,
        remote: vec![view("7", Some(thread.id.clone()))],
    };
    for (who, kind) in [(&launcher, "launcher"), (&quiet, "launcher"), (&test, "test"), (&client, "test")] {
        let err = who.ui_presence(shown.clone()).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::Denied), "{kind}");
    }
    let nine = (0..9).map(|n| view(&n.to_string(), None)).collect();
    let err = link.ui_presence(UiPresenceParams { remote: nine, ..shown.clone() }).await.unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::BadRequest), "more than 8 phones");

    link.ui_presence(shown.clone()).await.unwrap();
    let now = heard(&mut launcher_events, "the phone's view", |p| !p.remote.is_empty()).await;
    assert!(now.locked && !now.at_pc && now.viewing.is_empty());
    assert_eq!(now.remote, shown.remote);
    heard(&mut link_events, "the phone's view", |p| !p.remote.is_empty()).await;
    let read = next(
        &mut events,
        "read on the phone",
        |e| matches!(e, Event::ThreadState(s) if s.thread.id == thread.id && !s.thread.unread),
    )
    .await;
    assert!(matches!(read, Event::ThreadState(_)));

    launcher.ui_viewing(Some(thread.id.clone())).await.unwrap();
    let now = heard(&mut launcher_events, "the launcher's view", |p| !p.viewing.is_empty()).await;
    assert_eq!(now.viewing, std::slice::from_ref(&thread.id));

    // The ones that didn't ask heard nothing: a reply comes after any event sent before it.
    quiet.folders_list().await.unwrap();
    test.folders_list().await.unwrap();
    client.folders_list().await.unwrap();
    for (events, who) in
        [(&mut quiet_events, "quiet"), (&mut test_events, "test"), (&mut events, "subscribed")]
    {
        while let Some(event) = events.try_recv() {
            assert!(!matches!(event, Event::UiPresence(_)), "{who} heard {event:?}");
        }
    }

    // G7 Link goes: what it said goes with it.
    drop(link);
    let now = heard(&mut launcher_events, "G7 Link gone", |p| p.remote.is_empty()).await;
    assert!(!now.locked && !now.at_pc);
    assert_eq!(now.viewing, [thread.id], "the launcher still shows it");
    drop(link_events);
    host.stop().await;
}

// ---- Hand-off and the phone's camera (phase 5, wave 2, §V8.4, §V8.5) ----

/// What G7 Link's page connection says: it takes hand-offs and photos.
const LINK_CAPS: [&str; 5] = [cap::ITEMS2, cap::ITEMS3, cap::PRESENCE, cap::HANDOFF, cap::PHOTO];

/// The events that came before a reply to a request sent now.
async fn heard_so_far(client: &Client, events: &mut Events) -> Vec<Event> {
    client.folders_list().await.unwrap();
    std::iter::from_fn(|| events.try_recv()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thread_is_handed_to_a_phone_through_g7_link_after_its_draft() {
    let host = TestHost::start(Options::default()).await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    until_done(&mut events, &thread.id).await;
    let connect =
        |kind: ClientKind, caps: &[&str]| Client::connect(&host.socket, HelloParams::new(kind, caps));
    let (launcher, mut launcher_events) = connect(ClientKind::Launcher, &[cap::PRESENCE]).await.unwrap();
    let to = |device_id: Option<&str>, seq: Option<u64>| UiHandoffParams {
        thread_id: thread.id.clone(),
        device_id: device_id.map(str::to_owned),
        seq,
        by: Origin::launcher(),
    };
    assert_eq!(launcher.ui_handoff(to(Some("7"), Some(2))).await.unwrap(), 0, "G7 Link isn't open");

    let (link, mut link_events) = connect(ClientKind::Link, &LINK_CAPS).await.unwrap();
    let (watch, mut watch_events) = connect(ClientKind::Link, &[cap::ITEMS2, cap::ITEMS3]).await.unwrap();
    // The sender saves its draft first; the phone reads it from the host.
    let draft = DraftSetParams {
        key: thread.id.clone(),
        text: "And the tests too.".into(),
        images: Vec::new(),
        rev: 0,
        by: None,
    };
    launcher.draft_set(draft).await.unwrap();
    assert_eq!(launcher.ui_handoff(to(Some("7"), Some(2))).await.unwrap(), 1);
    let handed = next(&mut link_events, "ui.handoff", |e| matches!(e, Event::UiHandoff(_))).await;
    let expected = HandoffEvent { thread_id: thread.id.clone(), device_id: Some("7".into()), seq: Some(2) };
    assert_eq!(handed, Event::UiHandoff(expected));
    let saved = link.draft_get(thread.id.clone()).await.unwrap().expect("the draft");
    assert_eq!(saved.text, "And the tests too.");

    // The PC app may hand over through the host too: it hears its own.
    assert_eq!(link.ui_handoff(to(None, None)).await.unwrap(), 1);
    let handed = next(&mut link_events, "its own ui.handoff", |e| matches!(e, Event::UiHandoff(_))).await;
    let expected = HandoffEvent { thread_id: thread.id.clone(), device_id: None, seq: None };
    assert_eq!(handed, Event::UiHandoff(expected));

    let err = client.ui_handoff(to(None, None)).await.unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Denied), "neither the launcher nor G7 Link");
    let unknown = UiHandoffParams { thread_id: "f".repeat(32), ..to(None, None) };
    assert_eq!(launcher.ui_handoff(unknown).await.unwrap_err().code(), Some(ErrorCode::NotFound));
    for bad in [String::new(), "d".repeat(MAX_DEVICE_ID + 1)] {
        let err = launcher.ui_handoff(to(Some(&bad), None)).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::BadRequest), "{bad:?}");
    }

    // Only the connections that said `handoff` heard it, and the thread didn't move.
    for (who, events) in
        [(&watch, &mut watch_events), (&launcher, &mut launcher_events), (&client, &mut events)]
    {
        for event in heard_so_far(who, events).await {
            assert!(!matches!(event, Event::UiHandoff(_)), "{event:?}");
        }
    }
    let listed = client.threads_list(None, None).await.unwrap();
    assert!(listed.iter().any(|t| t.id == thread.id && t.state == State::Done), "nothing was sent");
    host.stop().await;
}

/// A small picture in `dir`, as a phone's photo arrives on this PC.
fn picture(dir: &Path) -> String {
    let path = dir.join("photo.png");
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 3, image::Rgb([200, 120, 40])))
        .write_with_encoder(image::codecs::png::PngEncoder::new(&mut png))
        .unwrap();
    std::fs::write(&path, png).unwrap();
    path.to_string_lossy().into_owned()
}

async fn photo_asked(events: &mut Events) -> String {
    match next(events, "ui.photo", |e| matches!(e, Event::UiPhoto(_))).await {
        Event::UiPhoto(e) => e.request,
        _ => unreachable!(),
    }
}

fn ask_photo(launcher: &Client) -> tokio::task::JoinHandle<Result<UiPhotoResult, ClientError>> {
    let launcher = launcher.clone();
    tokio::spawn(async move { launcher.ui_photo(Origin::launcher()).await })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phone_photo_comes_back_as_the_launchers_own_image() {
    let host = TestHost::start(Options::default()).await;
    let connect =
        |kind: ClientKind, caps: &[&str]| Client::connect(&host.socket, HelloParams::new(kind, caps));
    let (launcher, _launcher_events) = connect(ClientKind::Launcher, &[]).await.unwrap();
    let folder = host.folder(&launcher).await;
    let err = launcher.ui_photo(Origin::launcher()).await.unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Unavailable), "no G7 Link");
    let (link, mut link_events) = connect(ClientKind::Link, &LINK_CAPS).await.unwrap();
    let (watch, mut watch_events) = connect(ClientKind::Link, &[cap::ITEMS2, cap::ITEMS3]).await.unwrap();

    let asked = ask_photo(&launcher);
    let request = photo_asked(&mut link_events).await;
    assert!(is_valid_photo_request(&request));
    let err = launcher.ui_photo(Origin::launcher()).await.unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Busy), "one at a time");
    assert_eq!(link.ui_photo(Origin::launcher()).await.unwrap_err().code(), Some(ErrorCode::Denied));

    // G7 Link adds the picture as any image is added (decoded, encoded again), then answers.
    let file = picture(&host.work);
    let added = link.image_add_path(file.clone()).await.unwrap();
    let done = |image: Option<&str>, error: Option<PhotoError>| UiPhotoDoneParams {
        request: request.clone(),
        image: image.map(str::to_owned),
        error,
    };
    let refused = [
        (&launcher, done(Some(&added.id), None), ErrorCode::Denied, "only G7 Link answers"),
        (&watch, done(Some(&added.id), None), ErrorCode::NotFound, "not the connection asked"),
        (&link, done(Some(&added.id), Some(PhotoError::Cancelled)), ErrorCode::BadRequest, "both"),
        (&link, done(None, None), ErrorCode::BadRequest, "neither"),
        (
            &link,
            UiPhotoDoneParams { request: "0".repeat(16), ..done(Some(&added.id), None) },
            ErrorCode::NotFound,
            "not the request",
        ),
    ];
    for (who, params, code, why) in refused {
        assert_eq!(who.ui_photo_done(params).await.unwrap_err().code(), Some(code), "{why}");
    }
    let launchers = launcher.image_add_path(file).await.unwrap();
    let err = link.ui_photo_done(done(Some(&launchers.id), None)).await.unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::NotFound), "not an image G7 Link added");

    link.ui_photo_done(done(Some(&added.id), None)).await.unwrap();
    let got = asked.await.unwrap().unwrap();
    let image = got.image.clone().expect("the photo");
    assert_eq!(got, UiPhotoResult { image: Some(added.clone()), error: None });
    assert_eq!((image.w, image.h), (4, 3));
    let err = link.ui_photo_done(done(None, Some(PhotoError::Cancelled))).await.unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::NotFound), "answered already");

    // It is the launcher's now: it goes into the launcher's draft; G7 Link can't use it any more.
    assert_eq!(link.image_drop(image.id.clone()).await.unwrap_err().code(), Some(ErrorCode::NotFound));
    let key = format!("new:{}", folder.id);
    let draft = DraftSetParams {
        key: key.clone(),
        text: String::new(),
        images: vec![image.id.clone()],
        rev: 0,
        by: None,
    };
    launcher.draft_set(draft).await.unwrap();
    assert_eq!(launcher.draft_get(key).await.unwrap().expect("the draft").images, [image]);
    launcher.image_drop(launchers.id).await.unwrap();

    // Cancelled on the phone.
    let asked = ask_photo(&launcher);
    let request = photo_asked(&mut link_events).await;
    let cancelled = UiPhotoDoneParams { request, image: None, error: Some(PhotoError::Cancelled) };
    link.ui_photo_done(cancelled).await.unwrap();
    let got = asked.await.unwrap().unwrap();
    assert_eq!(got, UiPhotoResult { image: None, error: Some(PhotoError::Cancelled) });

    // The launcher goes while it waits: G7 Link is told to stop, and the next isn't busy.
    let (quitter, quitter_events) = connect(ClientKind::Launcher, &[]).await.unwrap();
    let waiting = ask_photo(&quitter);
    let request = photo_asked(&mut link_events).await;
    waiting.abort();
    let _ = waiting.await;
    drop((quitter, quitter_events));
    let stop = next(&mut link_events, "ui.photo.cancel", |e| matches!(e, Event::UiPhotoCancel(_))).await;
    assert_eq!(stop, Event::UiPhotoCancel(PhotoEvent { request: request.clone() }));
    let late = UiPhotoDoneParams { request, image: None, error: Some(PhotoError::Timeout) };
    assert_eq!(link.ui_photo_done(late).await.unwrap_err().code(), Some(ErrorCode::NotFound));

    // G7 Link goes while the launcher waits.
    let asked = ask_photo(&launcher);
    photo_asked(&mut link_events).await;
    drop(link);
    let err = asked.await.unwrap().unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Unavailable), "G7 Link went");
    let err = launcher.ui_photo(Origin::launcher()).await.unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Unavailable), "the connection left doesn't take photos");

    for event in heard_so_far(&watch, &mut watch_events).await {
        assert!(!matches!(event, Event::UiPhoto(_) | Event::UiPhotoCancel(_)), "{event:?}");
    }
    drop(link_events);
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_launcher_that_asked_stops_its_photo_and_g7_link_stops_waiting() {
    let host = TestHost::start(Options::default()).await;
    let connect =
        |kind: ClientKind, caps: &[&str]| Client::connect(&host.socket, HelloParams::new(kind, caps));
    let (launcher, _launcher_events) = connect(ClientKind::Launcher, &[]).await.unwrap();
    let (other, _other_events) = connect(ClientKind::Launcher, &[]).await.unwrap();
    let (link, mut link_events) = connect(ClientKind::Link, &LINK_CAPS).await.unwrap();
    launcher.ui_photo_stop().await.unwrap();

    let asked = ask_photo(&launcher);
    let request = photo_asked(&mut link_events).await;
    let err = other.ui_photo_stop().await.unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Denied), "another launcher's photo");
    assert_eq!(link.ui_photo_stop().await.unwrap_err().code(), Some(ErrorCode::Denied), "G7 Link");
    launcher.ui_photo_stop().await.unwrap();
    let got = asked.await.unwrap().unwrap();
    assert_eq!(got, UiPhotoResult { image: None, error: Some(PhotoError::Cancelled) });
    let stop = next(&mut link_events, "ui.photo.cancel", |e| matches!(e, Event::UiPhotoCancel(_))).await;
    assert_eq!(stop, Event::UiPhotoCancel(PhotoEvent { request: request.clone() }));
    launcher.ui_photo_stop().await.unwrap();
    other.ui_photo_stop().await.unwrap();
    let late = UiPhotoDoneParams { request, image: None, error: Some(PhotoError::Cancelled) };
    assert_eq!(link.ui_photo_done(late).await.unwrap_err().code(), Some(ErrorCode::NotFound));

    // The next one isn't busy, and anyone may ask for it.
    let asked = ask_photo(&other);
    let request = photo_asked(&mut link_events).await;
    let done = UiPhotoDoneParams { request, image: None, error: Some(PhotoError::NoPhone) };
    link.ui_photo_done(done).await.unwrap();
    let got = asked.await.unwrap().unwrap();
    assert_eq!(got, UiPhotoResult { image: None, error: Some(PhotoError::NoPhone) });
    host.stop().await;
}

// ---- Source control (phase 4, §SC5) ----

/// The `git` on this process's `PATH`.
fn real_git() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("git")).find(|p| p.is_file())
}

/// No `git` here: the test skips (CI has one).
fn no_git() -> bool {
    if real_git().is_some() {
        return false;
    }
    assert!(std::env::var_os("CI").is_none(), "CI has git");
    eprintln!("skipped: no git on PATH");
    true
}

/// What git prints in `dir`, with a throwaway identity; never the user's configuration.
fn git_out(dir: &Path, home: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@localhost", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The work folder as a repository on `main` with one commit (`a.txt`),
/// its own identity and no signing. The home folder git runs with.
fn repo(host: &TestHost) -> PathBuf {
    let home = host.dir.path().join("home");
    git_in(&host.work, &home, &["init", "-q", "-b", "main"]);
    for (key, value) in [("user.name", "t"), ("user.email", "t@localhost"), ("commit.gpgsign", "false")] {
        git_in(&host.work, &home, &["config", key, value]);
    }
    std::fs::write(host.work.join("a.txt"), "1\n").unwrap();
    git_in(&host.work, &home, &["add", "-A"]);
    git_in(&host.work, &home, &["commit", "-qm", "one"]);
    home
}

/// A refusal's word.
fn reason(e: &ClientError) -> Option<String> {
    match e {
        ClientError::Remote(r) => r.data.as_ref()?.get("reason")?.as_str().map(str::to_owned),
        _ => None,
    }
}

/// The job's last `git.progress`.
async fn job_end(events: &mut Events, job_id: &str) -> Job {
    let ended = next(
        events,
        "the job's end",
        |e| matches!(e, Event::GitProgress(p) if p.job.id == job_id && p.job.phase != JobPhase::Running),
    )
    .await;
    match ended {
        Event::GitProgress(p) => p.job,
        _ => unreachable!(),
    }
}

/// A copy of `claude-01-list.jsonl` whose answer is `answer`, written in `dir`.
fn answering(dir: &Path, answer: &str) -> &'static str {
    let text = std::fs::read_to_string(fixture("claude-01-list.jsonl")).unwrap();
    let mut out = String::new();
    let mut first = true;
    for line in text.lines() {
        let mut v: serde_json::Value = serde_json::from_str(line).unwrap();
        if let Some(delta) = v.pointer_mut("/event/delta").filter(|d| d["type"] == "text_delta") {
            delta["text"] = json!(if first { answer } else { "" });
            first = false;
        }
        if v["type"] == "assistant"
            && let Some(blocks) = v.pointer_mut("/message/content").and_then(|c| c.as_array_mut())
        {
            for block in blocks.iter_mut().filter(|b| b["type"] == "text") {
                block["text"] = json!(answer);
            }
        }
        if v["type"] == "result" && v.get("result").is_some() {
            v["result"] = json!(answer);
        }
        out.push_str(&v.to_string());
        out.push('\n');
    }
    let path = dir.join("claude-answer.jsonl");
    std::fs::write(&path, out).unwrap();
    Box::leak(path.to_string_lossy().into_owned().into_boxed_str())
}

/// §SC5.1's table, end to end: each method of phase 4, from each kind of
/// connection, as this PC and as a phone. A refused row is `DENIED` with
/// the same words; an allowed one goes on (here to "not a repository").
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn source_control_follows_who_may_ask_for_every_method_and_kind() {
    #[derive(Debug, Clone, Copy)]
    enum Row {
        Read,
        /// A person's Act: G7 Link and the launcher; a phone through G7 Link.
        Act,
        /// PC clients, never a phone.
        Pc,
        /// G7 Link on this PC only.
        Link,
    }
    let host = TestHost::start(Options { git: true, ..Default::default() }).await;
    let (client, mut events) = host.subscribed().await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    until_done(&mut events, &thread.id).await;
    let t = thread.id.clone();
    let phone = Origin::phone("dev1", "Pixel 8 Pro");
    let cases: Vec<(&str, serde_json::Value, Row, bool)> = vec![
        (method::GIT_STATUS, json!({"thread_id": t}), Row::Read, false),
        (method::GIT_PATCH, json!({"thread_id": t, "path": "a.txt"}), Row::Read, false),
        (method::GIT_REVIEW, json!({"thread_id": t, "paths": ["a.txt"]}), Row::Read, false),
        (method::GIT_BRANCHES, json!({"thread_id": t}), Row::Read, false),
        (method::GIT_JOB, json!({"job": "0123456789abcdef"}), Row::Read, false),
        (method::GH_STATUS, json!({}), Row::Read, false),
        (method::PR_VIEW, json!({"thread_id": t}), Row::Read, false),
        (method::PR_LIST, json!({"show": "mine"}), Row::Read, false),
        (
            method::GIT_COMMIT,
            json!({"thread_id": t, "paths": ["a.txt"], "message": "m", "review": "0123456789abcdef"}),
            Row::Act,
            true,
        ),
        (method::GIT_PUSH, json!({"thread_id": t}), Row::Act, true),
        (method::GIT_FETCH, json!({"thread_id": t}), Row::Act, true),
        (method::GIT_SWITCH, json!({"thread_id": t, "branch": "x", "create": true}), Row::Act, true),
        (method::PR_CREATE, json!({"thread_id": t, "title": "t"}), Row::Act, true),
        (method::GIT_SWITCH, json!({"thread_id": t, "branch": "x", "carry": true}), Row::Link, true),
        (method::WORKTREE_REMOVE, json!({"id": "0123456789ab"}), Row::Link, true),
        (
            method::PR_MERGE,
            json!({"thread_id": t, "number": 1, "method": "squash", "head": "a".repeat(40)}),
            Row::Link,
            true,
        ),
        (method::PR_READY, json!({"thread_id": t, "number": 1}), Row::Link, true),
        (method::PR_METHODS, json!({"thread_id": t}), Row::Link, false),
        (
            method::THREAD_DELETE,
            json!({"thread_id": "f".repeat(32), "remove_worktree": true}),
            Row::Link,
            false,
        ),
        (method::WORKTREE_LIST, json!({}), Row::Pc, false),
        (method::FOLDERS_SET, json!({"id": folder.id, "follow_remote": false}), Row::Pc, false),
    ];
    for kind in [ClientKind::Launcher, ClientKind::Link, ClientKind::Cli, ClientKind::Test] {
        let pc = matches!(kind, ClientKind::Launcher | ClientKind::Link);
        let (asker, _asker_events) = host.client(kind).await;
        for (method_name, params, row, takes_by) in &cases {
            let bys: Vec<Option<&Origin>> = if *takes_by { vec![None, Some(&phone)] } else { vec![None] };
            for by in bys {
                let by_phone = by.is_some();
                let allowed = match row {
                    Row::Read => true,
                    Row::Act => pc && (!by_phone || kind == ClientKind::Link),
                    Row::Pc => pc && !by_phone,
                    Row::Link => kind == ClientKind::Link && !by_phone,
                };
                let mut params = params.clone();
                if let Some(by) = by {
                    params["by"] = serde_json::to_value(by).unwrap();
                }
                let got = asker.call_value(method_name, params, None).await;
                let denied = matches!(&got, Err(ClientError::Remote(e)) if e.code == ErrorCode::Denied);
                assert_eq!(
                    !denied, allowed,
                    "{method_name} {row:?} from {kind:?}, phone {by_phone}: {got:?}"
                );
                if let Err(ClientError::Remote(e)) = &got
                    && denied
                {
                    assert_eq!(e.message, "That is done from G7 Link on this PC.");
                }
            }
        }
        // A thread in a new worktree: its origin is who asks.
        for origin in [Origin::launcher(), phone.clone()] {
            let mut p = start(AgentId::Claude, &folder, "Work apart.");
            p.origin = origin.clone();
            p.worktree = Some(WorktreeWanted::default());
            let got = asker.thread_start_with(p).await;
            let denied = matches!(&got, Err(e) if e.code() == Some(ErrorCode::Denied));
            let allowed = pc && origin.kind != OriginKind::Phone;
            assert_eq!(!denied, allowed, "a new worktree from {kind:?} by {origin:?}: {got:?}");
            assert!(got.is_err(), "the folder isn't a repository");
        }
    }
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_commit_or_switch_waits_for_its_thread_and_for_others_in_the_same_work_tree() {
    if no_git() {
        return;
    }
    let host = TestHost::start(Options { git: true, ..Default::default() }).await;
    repo(&host);
    std::fs::write(host.work.join("a.txt"), "2\n").unwrap();
    let (client, mut events) = host.subscribed().await;
    let (link, _link_events) = host.client(ClientKind::Link).await;
    let folder = host.folder(&client).await;
    let idle = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    until_done(&mut events, &idle.id).await;
    let working = client.thread_start(start(AgentId::Codex, &folder, "Create hello.txt")).await.unwrap();
    let asked = approval(&mut events).await;
    let commit = |thread: &str| GitCommitParams {
        thread_id: thread.into(),
        paths: vec!["a.txt".into()],
        message: "Change a".into(),
        ..Default::default()
    };
    let switch = |thread: &str| GitSwitchParams {
        thread_id: thread.into(),
        branch: "feat/x".into(),
        create: true,
        carry: true,
        ..Default::default()
    };
    for (who, word) in [(&working.id, "thread_busy"), (&idle.id, "shared_busy")] {
        let err = link.git_commit(commit(who)).await.unwrap_err();
        assert_eq!((err.code(), reason(&err).as_deref()), (Some(ErrorCode::Busy), Some(word)), "commit");
        let err = link.git_switch(switch(who)).await.unwrap_err();
        assert_eq!((err.code(), reason(&err).as_deref()), (Some(ErrorCode::Busy), Some(word)), "switch");
    }
    assert!(!host.work.join(".git/refs/heads/feat/x").exists(), "nothing ran");

    // Once it stops, the commit goes.
    client.approval_answer(asked.token, Decision::Deny, Origin::launcher()).await.unwrap();
    until_done(&mut events, &working.id).await;
    let job = link.git_commit(commit(&idle.id)).await.unwrap();
    assert_eq!(
        (job.kind, job.phase, job.by.kind),
        (JobKind::Commit, JobPhase::Running, OriginKind::Launcher)
    );
    let ended = job_end(&mut events, &job.id).await;
    assert_eq!(ended.phase, JobPhase::Done, "{ended:?}");
    assert_eq!(ended.result.as_ref().and_then(|r| r.files), Some(1));
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phone_commits_what_it_read_and_the_thread_says_who_did() {
    if no_git() {
        return;
    }
    let host = TestHost::start(Options { git: true, ..Default::default() }).await;
    let home = repo(&host);
    std::fs::write(host.work.join("a.txt"), "2\n").unwrap();
    std::fs::write(host.work.join("b.txt"), "new\n").unwrap();
    let (client, mut events) = host.subscribed().await;
    let (link, _link_events) = host.client(ClientKind::Link).await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    until_done(&mut events, &thread.id).await;

    // The turn's end read the folder: the thread carries its summary.
    next(&mut events, "the thread's summary", |e| {
        matches!(e, Event::ThreadState(s) if s.thread.id == thread.id
            && s.thread.branch.as_deref() == Some("main") && s.thread.changed == Some(2))
    })
    .await;
    next(
        &mut events,
        "git.changed",
        |e| matches!(e, Event::GitChanged(c) if c.thread_id == thread.id && c.changed == 2 && !c.detached),
    )
    .await;
    let status = link
        .git_status(GitStatusParams { thread_id: Some(thread.id.clone()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!((status.repo, status.branch.as_deref(), status.total), (RepoState::Ok, Some("main"), 2));
    let mut paths: Vec<&str> = status.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["a.txt", "b.txt"]);

    let phone = Origin::phone("dev1", "Pixel 8 Pro");
    let mut ask = GitCommitParams {
        thread_id: thread.id.clone(),
        paths: vec!["a.txt".into()],
        message: "Fix a".into(),
        by: Some(phone.clone()),
        ..Default::default()
    };
    let err = link.git_commit(ask.clone()).await.unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Denied), "a phone commits after a review");
    let read = GitReviewParams { thread_id: thread.id.clone(), paths: vec!["a.txt".into()], all: false };
    let review = link.git_review(read.clone()).await.unwrap();
    assert_eq!(review.files.len(), 1);

    // The file changed after it was read: nothing is committed.
    std::fs::write(host.work.join("a.txt"), "3\n").unwrap();
    ask.review = Some(review.review);
    let job = link.git_commit(ask.clone()).await.unwrap();
    let ended = job_end(&mut events, &job.id).await;
    assert_eq!(
        (ended.phase, ended.error.as_ref().map(|e| e.reason.as_str())),
        (JobPhase::Failed, Some("changed"))
    );

    // Read again, it goes; the notice names the phone.
    ask.review = Some(link.git_review(read).await.unwrap().review);
    let job = link.git_commit(ask).await.unwrap();
    assert_eq!(job.by, phone);
    let ended = job_end(&mut events, &job.id).await;
    assert_eq!(ended.phase, JobPhase::Done, "{ended:?}");
    assert_eq!(ended.result.as_ref().and_then(|r| r.subject.as_deref()), Some("Fix a"));
    let notice = next(&mut events, "the notice", |e| {
        matches!(e, Event::ThreadItem(i) if i.thread_id == thread.id && i.item.notice == Some(NoticeKind::Committed))
    })
    .await;
    let Event::ThreadItem(notice) = notice else { unreachable!() };
    assert_eq!(notice.item.kind, ItemKind::Notice, "a notice, never a message to the agent");
    assert_eq!(notice.item.text.as_deref(), Some("Committed 1 file: Fix a · from Pixel 8 Pro"));
    let last = git_out(&host.work, &home, &["log", "-n", "1", "--format=%s", "--name-only"]);
    assert_eq!(last.split_whitespace().collect::<Vec<_>>(), ["Fix", "a", "a.txt"]);
    assert!(
        git_out(&host.work, &home, &["status", "--porcelain"]).contains("?? b.txt"),
        "the rest is as it was"
    );
    assert_eq!(link.git_job(ended.id.clone()).await.unwrap().phase, JobPhase::Done, "kept to be read again");
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asking_the_agent_sends_the_fixed_text_and_keeps_its_answer_until_a_commit() {
    if no_git() {
        return;
    }
    let scratch = tempfile::tempdir().unwrap();
    let answer = answering(scratch.path(), "Here it is:\n\n```\nAdd the retry\n\nIt waits now.\n```");
    let host = TestHost::start(Options {
        claude_resume: Some(answer),
        agent_idle: Some(Duration::from_millis(300)),
        git: true,
        ..Default::default()
    })
    .await;
    repo(&host);
    std::fs::write(host.work.join("a.txt"), "2\n").unwrap();
    let (client, mut events) = host.subscribed().await;
    let (link, _link_events) = host.client(ClientKind::Link).await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    until_done(&mut events, &thread.id).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;

    let mut with_text = ThreadSendParams::ask(thread.id.clone(), Purpose::Commit, Origin::launcher());
    with_text.text = "and this".into();
    assert_eq!(link.thread_send_with(with_text).await.unwrap_err().code(), Some(ErrorCode::BadRequest));
    link.thread_send_with(ThreadSendParams::ask(thread.id.clone(), Purpose::Commit, Origin::launcher()))
        .await
        .unwrap();
    let rest = until_done(&mut events, &thread.id).await;
    let asked: Vec<&Item> = items_of(&rest).into_iter().filter(|i| i.kind == ItemKind::User).collect();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].text.as_deref(), Some(ASK_COMMIT_TEXT), "exactly the constant, as the user's");
    let wrote = client.threads_list(None, None).await.unwrap()[0].wrote.clone().expect("the answer kept");
    assert_eq!(
        (wrote.purpose, wrote.turn, wrote.text.as_str()),
        (Purpose::Commit, 2, "Add the retry\n\nIt waits now.")
    );

    // The commit made with it clears it.
    let commit = GitCommitParams {
        thread_id: thread.id.clone(),
        paths: vec!["a.txt".into()],
        message: wrote.text,
        ..Default::default()
    };
    let job = link.git_commit(commit).await.unwrap();
    assert_eq!(job_end(&mut events, &job.id).await.phase, JobPhase::Done);
    next(
        &mut events,
        "wrote cleared",
        |e| matches!(e, Event::ThreadState(s) if s.thread.id == thread.id && s.thread.wrote.is_none()),
    )
    .await;
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thread_in_a_new_worktree_works_there_and_the_worktree_stays_until_removed() {
    if no_git() {
        return;
    }
    let host = TestHost::start(Options { git: true, ..Default::default() }).await;
    let home = repo(&host);
    std::fs::write(host.work.join("u.txt"), "only in the folder\n").unwrap();
    let (client, mut events) = host.subscribed().await;
    let (link, _link_events) = host.client(ClientKind::Link).await;
    let (launcher, _launcher_events) = host.client(ClientKind::Launcher).await;
    let folder = host.folder(&client).await;
    let root = host.dir.path().join("data/g7-agents/worktrees").join(&folder.id);

    // A start that fails after the worktree was made removes it again.
    let mut failing = start(AgentId::Claude, &folder, "Fix the sync retry");
    failing.worktree = Some(WorktreeWanted::default());
    failing.images = vec!["0".repeat(32)];
    assert!(link.thread_start_with(failing).await.is_err());
    assert!(launcher.worktree_list(None).await.unwrap().is_empty());
    assert_eq!(std::fs::read_dir(&root).map(|d| d.count()).unwrap_or(0), 0, "its folder went");
    let listed = git_out(&host.work, &home, &["worktree", "list", "--porcelain"]);
    assert_eq!(listed.matches("worktree ").count(), 1, "{listed}");

    let mut p = start(AgentId::Claude, &folder, "Fix the sync retry");
    p.worktree = Some(WorktreeWanted { branch: Some("g7/sync-retry".into()), base: None });
    let started = link.thread_start_with(p).await.unwrap();
    let job = started.job.clone().expect("the worktree's job");
    assert_eq!((job.kind, job.phase), (JobKind::Worktree, JobPhase::Done), "{job:?}");
    let tag = started.thread.worktree.clone().expect("a worktree thread");
    assert_eq!((tag.branch.as_str(), tag.missing), ("g7/sync-retry", false));
    let id = started.thread.id.clone();
    until_done(&mut events, &id).await;
    let listed = launcher.worktree_list(None).await.unwrap();
    assert_eq!(listed.len(), 1);
    let path = PathBuf::from(&listed[0].path);
    assert!(path.starts_with(&root) && path.is_dir(), "{path:?}");
    assert_eq!(listed[0].thread_id.as_deref(), Some(id.as_str()));
    let real = std::fs::canonicalize(&path).unwrap();
    assert!(
        host.log().lines().any(|l| l == format!("cwd {}", real.display())),
        "the agent ran there: {}",
        host.log()
    );

    // Its files, its restore point and its status are the worktree's.
    let search =
        |q: &str| FilesSearchParams { thread_id: Some(id.clone()), query: q.into(), ..Default::default() };
    assert!(client.files_search(search("u.txt")).await.unwrap().files.is_empty(), "not copied");
    assert!(client.files_search(search("a.txt")).await.unwrap().files.iter().any(|f| f.path == "a.txt"));
    let turn = settled_turn(&client, &id).await;
    assert_eq!(turn.checkpoint, Checkpoint::Ready, "{turn:?}");
    let status =
        link.git_status(GitStatusParams { thread_id: Some(id.clone()), ..Default::default() }).await.unwrap();
    assert_eq!(status.branch.as_deref(), Some("g7/sync-retry"));
    assert_eq!(status.worktree.map(|w| w.id), Some(tag.id.clone()));
    assert_eq!(status.total, 0, "the folder's untracked file isn't there");
    let to_main = GitSwitchParams { thread_id: id.clone(), branch: "main".into(), ..Default::default() };
    assert_eq!(reason(&link.git_switch(to_main).await.unwrap_err()).as_deref(), Some("in_worktree"));

    // Deleting the thread without the box leaves the worktree, no one's.
    let left =
        link.thread_delete_with(ThreadDeleteParams { thread_id: id, remove_worktree: false }).await.unwrap();
    assert_eq!(left.worktree_left, None);
    let listed = launcher.worktree_list(None).await.unwrap();
    assert_eq!((listed.len(), listed[0].thread_id.as_deref()), (1, None));
    assert!(path.is_dir());

    // Removing it is G7 Link's; its folder goes, its branch stays.
    let remove = WorktreeRemoveParams { id: listed[0].id.clone(), ..Default::default() };
    assert_eq!(launcher.worktree_remove(remove.clone()).await.unwrap_err().code(), Some(ErrorCode::Denied));
    link.worktree_remove(remove).await.unwrap();
    assert!(!path.exists());
    assert!(launcher.worktree_list(None).await.unwrap().is_empty());
    assert!(git_out(&host.work, &home, &["branch", "--list", "g7/*"]).contains("g7/sync-retry"));
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeping_in_step_with_the_remote_fetches_only_when_its_switch_is_on() {
    if no_git() {
        return;
    }
    let host = TestHost::start(Options { git: true, ..Default::default() }).await;
    let home = repo(&host);
    let bare = host.dir.path().join("remote.git");
    let bare_str = bare.to_string_lossy().into_owned();
    git_in(host.dir.path(), &home, &["init", "-q", "--bare", &bare_str]);
    git_in(&host.work, &home, &["remote", "add", "origin", &bare_str]);
    git_in(&host.work, &home, &["push", "-q", "-u", "origin", "main"]);
    let fetched = host.work.join(".git/FETCH_HEAD");
    let (client, mut events) = host.subscribed().await;
    let (link, _link_events) = host.client(ClientKind::Link).await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    until_done(&mut events, &thread.id).await;

    // Shown on the PC: read, never fetched while the switch is off.
    link.ui_viewing(Some(thread.id.clone())).await.unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(!fetched.exists(), "a timer never fetches with the switch off");

    let on = FoldersSetParams { id: folder.id.clone(), follow_remote: Some(true), ..Default::default() };
    assert!(link.folders_set_with(on).await.unwrap().follow_remote);
    let deadline = tokio::time::Instant::now() + WAIT;
    while !fetched.exists() {
        assert!(tokio::time::Instant::now() < deadline, "never fetched");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(client.folders_list().await.unwrap()[0].follow_remote, "kept");
    host.stop().await;
}

/// A fake `gh` beside the test host's `git`: signed in to github.com (G's
/// fixtures, fictional), no pull request until one is created; it keeps
/// the create call's argv and body. Nothing is contacted. (`PATH` holds
/// only `git` and it: programs are named by their path.)
fn fake_gh(host: &TestHost) -> PathBuf {
    let calls = host.dir.path().join("gh");
    std::fs::create_dir_all(&calls).unwrap();
    let scm = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scm/gh");
    for name in ["auth-signed-in.json", "create-out.txt", "pr-created.json"] {
        std::fs::copy(scm.join(name), calls.join(name)).unwrap();
    }
    let script = format!(
        "#!/bin/sh\nd={}\ncase \"$1 $2\" in\n\
         \"--version \") echo 'gh version 2.102.0 (2026-01-01)' ;;\n\
         \"auth status\") /bin/cat \"$d/auth-signed-in.json\" ;;\n\
         \"pr list\") if [ -f \"$d/created\" ]; then /bin/cat \"$d/pr-created.json\"; else echo '[]'; fi ;;\n\
         \"pr create\") printf '%s\\n' \"$@\" > \"$d/create-argv\"; /bin/cat > \"$d/create-body\"; : > \"$d/created\"; /bin/cat \"$d/create-out.txt\" ;;\n\
         *) echo \"unexpected: $*\" >&2; exit 1 ;;\nesac\n",
        quote(&calls)
    );
    let gh = host.dir.path().join("bin/gh");
    std::fs::write(&gh, script).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    calls
}

/// The launcher names a pull request's base `main`, without its remote: the
/// host takes it as the push remote's branch, for `pr.create` and for
/// "Ask <agent> to write it".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_base_named_without_its_remote_is_the_push_remotes_branch() {
    if no_git() {
        return;
    }
    let host = TestHost::start(Options {
        claude_resume: Some("claude-01-list.jsonl"),
        agent_idle: Some(Duration::from_millis(300)),
        git: true,
        ..Default::default()
    })
    .await;
    let home = repo(&host);
    // Pushed already (as far as git knows): origin on github.com, never contacted.
    git_in(&host.work, &home, &["remote", "add", "origin", "https://github.com/fictional/repo.git"]);
    git_in(&host.work, &home, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git_in(&host.work, &home, &["switch", "-q", "-c", "feat/x"]);
    git_in(&host.work, &home, &["update-ref", "refs/remotes/origin/feat/x", "HEAD"]);
    git_in(&host.work, &home, &["branch", "-q", "--set-upstream-to=origin/feat/x", "feat/x"]);
    let calls = fake_gh(&host);
    let (client, mut events) = host.subscribed().await;
    let (launcher, _launcher_events) = host.client(ClientKind::Launcher).await;
    let folder = host.folder(&client).await;
    let thread = client.thread_start(start(AgentId::Claude, &folder, "List the files here.")).await.unwrap();
    until_done(&mut events, &thread.id).await;
    let status = launcher
        .git_status(GitStatusParams { thread_id: Some(thread.id.clone()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!((status.branch.as_deref(), status.ahead), (Some("feat/x"), Some(0)));

    // Ask: the text names the branch as the launcher did.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let mut ask = ThreadSendParams::ask(thread.id.clone(), Purpose::Pr, Origin::launcher());
    ask.base = Some("main".into());
    launcher.thread_send_with(ask).await.unwrap();
    let rest = until_done(&mut events, &thread.id).await;
    let asked = items_of(&rest).into_iter().find(|i| i.kind == ItemKind::User).expect("the ask");
    assert_eq!(
        asked.text.as_deref(),
        Some(ASK_PR_TEXT.replace("%1", "feat/x").replace("%2", "main").as_str())
    );

    // Open: `main` is origin's main.
    let open = PrCreateParams {
        thread_id: thread.id.clone(),
        title: "Fix the retry".into(),
        body: "Body with `ticks` and $(nothing)".into(),
        base: Some("main".into()),
        ..Default::default()
    };
    let job = launcher.pr_create(open).await.unwrap();
    let ended = job_end(&mut events, &job.id).await;
    assert_eq!(ended.phase, JobPhase::Done, "{ended:?}");
    assert_eq!(ended.result.as_ref().and_then(|r| r.number), Some(215));
    let argv = std::fs::read_to_string(calls.join("create-argv")).unwrap();
    let argv: Vec<&str> = argv.lines().collect();
    let after = |flag: &str| argv.iter().position(|a| *a == flag).and_then(|i| argv.get(i + 1)).copied();
    assert_eq!((after("--base"), after("--head")), (Some("main"), Some("feat/x")), "{argv:?}");
    assert_eq!(
        std::fs::read_to_string(calls.join("create-body")).unwrap(),
        "Body with `ticks` and $(nothing)"
    );
    next(&mut events, "the notice", |e| {
        matches!(e, Event::ThreadItem(i) if i.thread_id == thread.id
            && i.item.notice == Some(NoticeKind::PrOpened)
            && i.item.text.as_deref() == Some("Opened pull request #215"))
    })
    .await;

    // A base no remote has is refused by lookup, and nothing is created.
    std::fs::remove_file(calls.join("create-argv")).unwrap();
    std::fs::remove_file(calls.join("created")).unwrap();
    let missing = PrCreateParams {
        thread_id: thread.id.clone(),
        title: "t".into(),
        base: Some("no-such-branch".into()),
        ..Default::default()
    };
    let job = launcher.pr_create(missing).await.unwrap();
    let ended = job_end(&mut events, &job.id).await;
    assert_eq!(
        (ended.phase, ended.error.map(|e| e.reason)),
        (JobPhase::Failed, Some("not_found".to_owned()))
    );
    assert!(!calls.join("create-argv").exists());
    host.stop().await;
}
