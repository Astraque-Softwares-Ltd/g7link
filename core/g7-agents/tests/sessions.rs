// SPDX-License-Identifier: MPL-2.0
//! Sessions started outside G7, end to end: an in-process host whose Claude
//! and Codex homes and `/proc` are a temp dir with hand-written files, and
//! `g7-agents-fake-cli` standing in for the agents when a thread resumes.
#![cfg(feature = "fake-cli")]

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use g7_agents::api::*;
use g7_agents::client::{Client, ClientError, Events};
use g7_agents::host::{self, Config, Served};
use serde_json::json;
use tokio::sync::oneshot;

const WAIT: Duration = Duration::from_secs(20);
const MIN: u64 = 60_000;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn wrapper(dir: &Path, agent: &str, script: &str, log: &Path) -> PathBuf {
    let path = dir.join(format!("{agent}-cli"));
    let body = format!(
        "#!/bin/sh\nexport FAKE_CLI_SCRIPT={}\nexport FAKE_CLI_LOG={}\nexec {} {agent} \"$@\"\n",
        quote(&fixture(script)),
        quote(log),
        quote(Path::new(env!("CARGO_BIN_EXE_g7-agents-fake-cli")))
    );
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as u64
}

fn uuid(n: u32) -> String {
    format!("{n:08x}-0000-4000-8000-000000000000")
}

fn sc(uuid: &str) -> String {
    format!("sc{}", uuid.replace('-', ""))
}

fn sx(uuid: &str) -> String {
    format!("sx{}", uuid.replace('-', ""))
}

fn set_mtime(path: &Path, ms: u64) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_millis(ms)).unwrap();
}

struct TestHost {
    dir: tempfile::TempDir,
    socket: PathBuf,
    claude: PathBuf,
    codex: PathBuf,
    proc_root: PathBuf,
    work: PathBuf,
    log: PathBuf,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<std::io::Result<Served>>>,
}

impl TestHost {
    async fn start() -> TestHost {
        Self::start_with("claude-01-list.jsonl", |_| {}).await
    }

    /// `claude` is the fake Claude's script; `plant` runs on the temp dir
    /// before the host starts.
    async fn start_with(claude_script: &str, plant: impl FnOnce(&Path)) -> TestHost {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let run = root.join("run");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
        let home = root.join("home");
        let claude = home.join(".claude");
        let codex = home.join(".codex");
        let proc_root = root.join("proc");
        let work = root.join("work");
        for d in [
            &claude.join("projects"),
            &claude.join("sessions"),
            &codex.join("sessions"),
            &proc_root,
            &work.join("sub"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(proc_root.join("stat"), "cpu 0\nbtime 1700000000\n").unwrap();
        let log = root.join("cli.log");
        plant(&root);
        let config = Config {
            socket: run.join("host.sock"),
            runtime_dir: run,
            data_dir: root.join("data/g7-agents"),
            claude_bin: Some(wrapper(&root, "claude", claude_script, &log)),
            codex_bin: Some(wrapper(&root, "codex", "codex-01-create-approve.jsonl", &log)),
            bridge_exe: PathBuf::from(env!("CARGO_BIN_EXE_g7-agents")),
            notify: false,
            env: vec![("HOME".into(), home.into_os_string()), ("PATH".into(), "/usr/bin:/bin".into())],
            idle_exit: host::IDLE_EXIT,
            agent_idle: Duration::from_millis(300),
            approval_lifetime_ms: APPROVAL_LIFETIME_MS,
            claude_home: claude.clone(),
            codex_home: codex.clone(),
            proc_root: proc_root.clone(),
            sessions: true,
            keep_awake_bus: false,
        };
        let socket = config.socket.clone();
        let (stop, stopped) = oneshot::channel::<()>();
        let mut task = tokio::spawn(host::serve(config, async move {
            let _ = stopped.await;
        }));
        let deadline = tokio::time::Instant::now() + WAIT;
        while !socket.exists() {
            assert!(!task.is_finished(), "the host stopped: {:?}", (&mut task).await);
            assert!(tokio::time::Instant::now() < deadline, "the host didn't listen");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        TestHost { dir, socket, claude, codex, proc_root, work, log, stop: Some(stop), task: Some(task) }
    }

    async fn client(&self) -> (Client, Events) {
        Client::connect(&self.socket, HelloParams::new(ClientKind::Test, &[])).await.unwrap()
    }

    async fn sessions_client(&self) -> (Client, Events) {
        let (client, events) = self.client().await;
        client
            .subscribe_with(SubscribeParams { threads: ThreadSelection::All, deltas: false, sessions: true })
            .await
            .unwrap();
        (client, events)
    }

    async fn folder(&self, client: &Client) -> Folder {
        client.folders_add(self.work.to_string_lossy().into_owned(), Some("work".into())).await.unwrap()
    }

    /// A Claude transcript under a project folder, written in `cwd`.
    fn claude_session(&self, uuid: &str, cwd: &Path, prompt: &str) -> PathBuf {
        let folder = self.claude.join("projects").join(cwd.to_string_lossy().replace(['/', '.'], "-"));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(format!("{uuid}.jsonl"));
        let cwd = cwd.to_string_lossy();
        let line = |kind: &str, n: u32, message: serde_json::Value| {
            json!({"type": kind, "uuid": format!("{uuid}-{n}"), "parentUuid": null, "timestamp": "2026-11-19T09:00:00.000Z",
                   "sessionId": uuid, "cwd": cwd, "isSidechain": false, "version": "2.1.325", "message": message})
            .to_string()
        };
        let lines = [
            line("user", 1, json!({"role": "user", "content": prompt})),
            line(
                "assistant",
                2,
                json!({"id": "msg_1", "role": "assistant", "content": [{"type": "text", "text": "Looking."}], "stop_reason": null}),
            ),
            line(
                "assistant",
                3,
                json!({"id": "msg_1", "role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "make test", "description": "Run the tests"}}], "stop_reason": "tool_use"}),
            ),
            r#"{"type":"queue-operation","operation":"enqueue"}"#.to_owned(),
        ];
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    /// A plain Codex rollout dated today, last written two minutes ago.
    fn codex_session(&self, uuid: &str, cwd: &Path) -> PathBuf {
        self.codex_session_from(uuid, cwd, "codex-tui")
    }

    fn codex_session_from(&self, uuid: &str, cwd: &Path, originator: &str) -> PathBuf {
        let now = now_ms();
        let days = (now / 86_400_000) as i64;
        let (y, m, d) = civil(days);
        let dir = self.codex.join(format!("sessions/{y:04}/{m:02}/{d:02}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("rollout-{y:04}-{m:02}-{d:02}T09-00-00-{uuid}.jsonl"));
        let lines = [
            json!({"timestamp": "2026-11-19T09:00:00Z", "type": "session_meta", "payload": {"id": uuid, "cwd": cwd, "originator": originator, "creator_user_id": "invented"}}),
            json!({"timestamp": "2026-11-19T09:00:01Z", "type": "event_msg", "payload": {"type": "task_started", "turn_id": "t"}}),
            json!({"timestamp": "2026-11-19T09:00:01Z", "type": "event_msg", "payload": {"type": "item_completed", "item": {"type": "UserMessage", "id": "u1", "content": [{"type": "text", "text": "Explain the build."}]}}}),
            json!({"timestamp": "2026-11-19T09:00:02Z", "type": "event_msg", "payload": {"type": "item_completed", "item": {"type": "AgentMessage", "id": "a1", "phase": "final_answer", "content": [{"type": "Text", "text": "It uses make."}]}}}),
            json!({"timestamp": "2026-11-19T09:00:03Z", "type": "event_msg", "payload": {"type": "task_complete", "turn_id": "t", "last_agent_message": "It uses make."}}),
        ];
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&path, body).unwrap();
        set_mtime(&path, now - 2 * MIN);
        path
    }

    /// A process in the fake `/proc`.
    fn process(&self, pid: u32, comm: &str, argv: &[&str]) {
        let dir = self.proc_root.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
        std::fs::write(
            dir.join("stat"),
            format!("{pid} ({comm}) S 1 {pid} {pid} 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 555 1000 100 0"),
        )
        .unwrap();
        let cmdline: Vec<u8> = argv.iter().flat_map(|a| a.bytes().chain([0])).collect();
        std::fs::write(dir.join("cmdline"), cmdline).unwrap();
    }

    fn end_process(&self, pid: u32) {
        std::fs::remove_dir_all(self.proc_root.join(pid.to_string())).unwrap();
    }

    /// `pid`, a real process of the test's own, as a `claude` in the fake
    /// `/proc` started at `start`, its live status file saying `status` for
    /// session `uuid`.
    fn claude_process(&self, pid: u32, start: u64, uuid: &str, cwd: &Path, status: &str) {
        let dir = self.proc_root.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("comm"), "claude\n").unwrap();
        std::fs::write(
            dir.join("stat"),
            format!(
                "{pid} (claude) S 1 {pid} {pid} 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 {start} 1000 100 0"
            ),
        )
        .unwrap();
        std::fs::write(dir.join("cmdline"), b"claude\0").unwrap();
        let file = json!({"pid": pid, "sessionId": uuid, "cwd": cwd, "procStart": start.to_string(),
                          "entrypoint": "cli", "status": status, "updatedAt": now_ms()});
        std::fs::write(self.claude.join(format!("sessions/{pid}.json")), file.to_string()).unwrap();
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.task.take().unwrap().await.unwrap().unwrap();
        drop(self.dir);
    }
}

fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
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

fn code(e: &ClientError) -> Option<ErrorCode> {
    e.code()
}

fn continue_params(id: &str, mode: Mode, origin: Origin) -> SessionContinueParams {
    SessionContinueParams { session_id: id.to_owned(), mode, origin }
}

/// The fake CLI waiting on its input, run as `claude` (its process name),
/// and its start time from the real `/proc`. It is the test's own child: the
/// only process `session.close` may reach here.
fn own_claude(dir: &Path) -> (std::process::Child, u64) {
    let exe = dir.join("bin/claude");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_g7-agents-fake-cli"), &exe).unwrap();
    let child = std::process::Command::new(&exe)
        .args(["claude", "--no-session-persistence"])
        .env_remove("FAKE_CLI_SCRIPT")
        .env_remove("FAKE_CLI_LOG")
        .env_remove("FAKE_CLI_INITIALIZE")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", child.id())).unwrap();
    let start = stat[stat.rfind(')').unwrap() + 1..].split_whitespace().nth(19).unwrap().parse().unwrap();
    (child, start)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_list_keeps_seven_days_and_fifty() {
    let host = TestHost::start().await;
    let now = now_ms();
    for n in 0..55u32 {
        let path = host.claude_session(&uuid(n), &host.work, &format!("Task number {n}"));
        set_mtime(&path, now - (u64::from(n) + 1) * MIN);
    }
    let old = host.claude_session(&uuid(900), &host.work, "An old task");
    set_mtime(&old, now - 8 * 24 * 60 * MIN);
    let (client, _events) = host.client().await;
    let sessions = client.sessions_list(None).await.unwrap();
    assert_eq!(sessions.len(), MAX_SESSIONS);
    assert_eq!(sessions[0].id, sc(&uuid(0)), "newest first");
    assert_eq!(sessions[0].title, "Task number 0");
    assert_eq!(sessions[49].id, sc(&uuid(49)));
    assert!(sessions.iter().all(|s| s.state == SessionState::Ended && !s.open && !s.state_guess));
    assert!(!sessions.iter().any(|s| s.id == sc(&uuid(900))), "older than 7 days");
    assert_eq!(sessions[0].blocked, Some(ContinueBlock::Folder), "no agent folder yet");
    assert_eq!(sessions[0].folder_label, "work");
    assert!(sessions[0].state_since_ms > 0);
    assert_eq!(client.sessions_list(Some(3)).await.unwrap().len(), 3);
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_threads_own_session_is_hidden() {
    let ours = uuid(7);
    let host = TestHost::start_with("claude-01-list.jsonl", |root| {
        let id = "d".repeat(32);
        let dir = root.join("data/g7-agents/threads").join(&id);
        std::fs::create_dir_all(&dir).unwrap();
        let meta = json!({"version": 1, "next_seq": 1, "thread": {
            "id": id, "agent": "claude", "folder_id": "f000000000001", "folder_label": "w", "title": "t",
            "state": "done", "mode": "supervised", "origin": {"kind": "launcher"},
            "created_ms": 1, "updated_ms": 1, "last_seq": 0, "pending": 0, "native_id": uuid(7)}});
        std::fs::write(dir.join("meta.json"), meta.to_string()).unwrap();
    })
    .await;
    host.claude_session(&ours, &host.work, "Ours");
    host.claude_session(&uuid(8), &host.work, "Theirs");
    let (client, _events) = host.client().await;
    let ids: Vec<String> = client.sessions_list(None).await.unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(ids, [sc(&uuid(8))]);
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_watch_gets_appended_items_and_old_clients_get_nothing() {
    let host = TestHost::start().await;
    let id = uuid(1);
    let path = host.claude_session(&id, &host.work, "Watch me");
    let (old, mut old_events) = host.client().await;
    old.subscribe(ThreadSelection::All, true).await.unwrap();
    let (client, mut events) = host.sessions_client().await;
    let first = next(&mut events, "session.state", |e| matches!(e, Event::SessionState(_))).await;
    let Event::SessionState(first) = first else { unreachable!() };
    assert_eq!(first.session.id, sc(&id));
    client.session_watch(Some(sc(&id))).await.unwrap();
    let read =
        client.session_read(SessionReadParams { session_id: sc(&id), ..Default::default() }).await.unwrap();
    assert_eq!(read.items.len(), 3);
    assert!(!read.more_before);
    let export = client
        .thread_export(ThreadExportParams {
            thread_id: None,
            session_id: Some(sc(&id)),
            format: ExportFormat::Markdown,
        })
        .await
        .unwrap();
    assert_eq!(export.file_name, "claude-watch-me-2026-11-19.md");
    assert!(export.text.starts_with("# Watch me\n\n- Agent: Claude\n- Folder: work\n"), "{}", export.text);
    assert!(export.text.contains("\n**Ran** `make test` (not finished)\n"));
    assert!(!export.text.contains(&host.work.to_string_lossy().into_owned()), "never the path");
    assert!(!export.truncated);
    let line = json!({"type": "user", "uuid": "late", "timestamp": "2026-11-19T09:01:00Z", "cwd": host.work,
                      "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}]}});
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(format!("{line}\n").as_bytes())
        .unwrap();
    let item = next(&mut events, "session.item", |e| matches!(e, Event::SessionItem(_))).await;
    let Event::SessionItem(item) = item else { unreachable!() };
    assert_eq!(item.session_id, sc(&id));
    assert_eq!(item.item.seq, read.items[2].seq, "the step's own seq");
    assert_eq!(item.item.rev, 1);
    assert_eq!(item.item.tool.as_ref().unwrap().status, ToolStatus::Ok);
    // Exactly one item for the one line.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    while let Some(e) = events.try_recv() {
        assert!(!matches!(e, Event::SessionItem(_)), "a second item: {e:?}");
    }
    // The old client heard nothing about sessions.
    while let Some(e) = old_events.try_recv() {
        assert!(e.session_id().is_none(), "an old client got {e:?}");
    }
    // A read of an unknown session.
    let missing = client
        .session_read(SessionReadParams { session_id: sc(&uuid(99)), ..Default::default() })
        .await
        .unwrap_err();
    assert_eq!(code(&missing), Some(ErrorCode::NotFound));
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fifth_watched_session_is_refused() {
    let host = TestHost::start().await;
    for n in 0..5 {
        host.claude_session(&uuid(n), &host.work, "x");
    }
    let mut clients = Vec::new();
    for n in 0..4 {
        let (client, events) = host.client().await;
        client.session_watch(Some(sc(&uuid(n)))).await.unwrap();
        clients.push((client, events));
    }
    let (fifth, _events) = host.client().await;
    let refused = fifth.session_watch(Some(sc(&uuid(4)))).await.unwrap_err();
    assert_eq!(code(&refused), Some(ErrorCode::Limit));
    // A session already watched by someone else is fine.
    fifth.session_watch(Some(sc(&uuid(0)))).await.unwrap();
    // Dropping a watch frees a place.
    clients[1].0.session_watch(None).await.unwrap();
    let (sixth, _e) = host.client().await;
    sixth.session_watch(Some(sc(&uuid(4)))).await.unwrap();
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuing_a_claude_session() {
    let host = TestHost::start().await;
    let (client, mut events) = host.client().await;
    client.subscribe(ThreadSelection::All, false).await.unwrap();
    let folder = host.folder(&client).await;
    let sub = host.work.join("sub");
    let id = uuid(3);
    host.claude_session(&id, &sub, "Fix the flaky test");
    let outside = host.dir.path().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    host.claude_session(&uuid(4), &outside, "Somewhere else");

    // Open in a terminal: refused.
    host.process(4100, "claude", &["claude", "--resume", &id]);
    let listed = client.sessions_list(None).await.unwrap();
    let session = listed.iter().find(|s| s.id == sc(&id)).unwrap();
    assert!(session.open);
    assert_eq!(session.blocked, Some(ContinueBlock::Open));
    assert_eq!(session.source, SessionSource::Terminal);
    assert!(session.state_guess, "no status file: a guess");
    assert_eq!(session.folder_id.as_deref(), Some(folder.id.as_str()));
    let busy = client.session_continue(continue_params(&sc(&id), Mode::Supervised, Origin::launcher())).await;
    let busy = busy.unwrap_err();
    assert_eq!(code(&busy), Some(ErrorCode::Busy));
    let ClientError::Remote(e) = &busy else { panic!() };
    assert_eq!(e.message, "Still open in a terminal. Close it there to continue here.");
    host.end_process(4100);

    // Outside every folder, and a phone asking for auto_edit: refused.
    let denied =
        client.session_continue(continue_params(&sc(&uuid(4)), Mode::Supervised, Origin::launcher())).await;
    let denied = denied.unwrap_err();
    assert_eq!(code(&denied), Some(ErrorCode::Denied));
    let ClientError::Remote(e) = &denied else { panic!() };
    assert_eq!(e.message, "Add elsewhere to your agent folders first.");
    let phone = client
        .session_continue(continue_params(&sc(&id), Mode::AutoEdit, Origin::phone("p", "Pixel")))
        .await
        .unwrap_err();
    assert_eq!(code(&phone), Some(ErrorCode::Denied));

    // Closed now: it becomes a thread with the session's items, and no process starts.
    let thread = client
        .session_continue(continue_params(&sc(&id), Mode::Supervised, Origin::launcher()))
        .await
        .unwrap();
    assert!(thread.continued);
    assert_eq!(thread.state, State::Done);
    assert_eq!(thread.subdir.as_deref(), Some("sub"));
    assert_eq!(thread.native_id.as_deref(), Some(id.as_str()));
    assert_eq!(thread.title, "Fix the flaky test");
    assert_eq!(thread.folder_id, folder.id);
    let read = client
        .thread_read(ThreadReadParams { thread_id: thread.id.clone(), ..Default::default() })
        .await
        .unwrap();
    let seqs: Vec<u64> = read.items.iter().map(|i| i.seq).collect();
    assert_eq!(seqs, [1, 2, 3, 4]);
    assert_eq!(read.items[0].text.as_deref(), Some("Fix the flaky test"));
    let tool = read.items[2].tool.as_ref().unwrap();
    assert_eq!(
        (tool.status, tool.output.as_deref()),
        (ToolStatus::Error, Some("Stopped before it finished."))
    );
    assert_eq!(read.items[3].kind, ItemKind::Notice);
    assert_eq!(read.items[3].text.as_deref(), Some("Continued from a session started in a terminal."));
    assert!(!host.log().contains("--resume"), "nothing ran yet");
    assert!(!client.sessions_list(None).await.unwrap().iter().any(|s| s.id == sc(&id)), "it is a thread now");
    let again =
        client.session_continue(continue_params(&sc(&id), Mode::Supervised, Origin::launcher())).await;
    assert_eq!(code(&again.unwrap_err()), Some(ErrorCode::NotFound));

    // The first message resumes the session in folder/subdir.
    client.thread_send(thread.id.clone(), "Go on.".into(), Origin::launcher()).await.unwrap();
    until_done(&mut events, &thread.id).await;
    let log = host.log();
    assert!(log.lines().any(|l| l.starts_with("args -p") && l.contains(&format!("--resume {id}"))), "{log}");
    assert!(log.lines().any(|l| l == format!("cwd {}", sub.display())), "{log}");
    let terminal = client.thread_terminal_command(thread.id.clone()).await.unwrap();
    assert_eq!(terminal.cwd, sub.to_string_lossy());
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuing_a_codex_session_resumes_its_thread() {
    let host = TestHost::start().await;
    let (client, mut events) = host.client().await;
    client.subscribe(ThreadSelection::All, false).await.unwrap();
    host.folder(&client).await;
    let id = uuid(5);
    let path = host.codex_session(&id, &host.work);
    let app = uuid(6);
    host.codex_session_from(&app, &host.work, "Codex Desktop");
    let session = client.sessions_list(None).await.unwrap().into_iter().find(|s| s.id == sx(&id)).unwrap();
    assert_eq!(
        (session.state, session.source, session.title.as_str()),
        (SessionState::Ended, SessionSource::Terminal, "Explain the build.")
    );
    assert_eq!(session.summary.as_deref(), Some("It uses make."));
    assert!(session.can_continue, "{session:?}");
    // The Codex app's sessions can't be continued here.
    let from_app = client.sessions_list(None).await.unwrap().into_iter().find(|s| s.id == sx(&app)).unwrap();
    assert_eq!(
        (from_app.source, from_app.open, from_app.blocked),
        (SessionSource::DesktopApp, false, Some(ContinueBlock::Open))
    );
    let refused =
        client.session_continue(continue_params(&sx(&app), Mode::Supervised, Origin::launcher())).await;
    assert_eq!(code(&refused.unwrap_err()), Some(ErrorCode::Busy));
    // A Codex session written in the last 90 s is open.
    set_mtime(&path, now_ms());
    let busy = client.session_continue(continue_params(&sx(&id), Mode::Supervised, Origin::launcher())).await;
    assert_eq!(code(&busy.unwrap_err()), Some(ErrorCode::Busy));
    set_mtime(&path, now_ms() - 2 * MIN);
    let auto = client.session_continue(continue_params(&sx(&id), Mode::AutoEdit, Origin::launcher())).await;
    assert_eq!(code(&auto.unwrap_err()), Some(ErrorCode::Unavailable));
    let thread = client
        .session_continue(continue_params(&sx(&id), Mode::Supervised, Origin::launcher()))
        .await
        .unwrap();
    assert_eq!(thread.subdir, None);
    client.thread_send(thread.id.clone(), "Again.".into(), Origin::launcher()).await.unwrap();
    let asked = match next(&mut events, "approval", |e| matches!(e, Event::ApprovalRequested(_))).await {
        Event::ApprovalRequested(a) => a.approval,
        _ => unreachable!(),
    };
    client.approval_answer(asked.token, Decision::Allow, Origin::launcher()).await.unwrap();
    until_done(&mut events, &thread.id).await;
    let log = host.log();
    assert!(log.lines().any(|l| l == "method thread/resume"), "{log}");
    assert!(log.lines().any(|l| l == format!("resume {id}")), "{log}");
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_resume_guard_and_the_running_step() {
    let host = TestHost::start_with("claude-02-followup-approve.jsonl", |_| {}).await;
    let (client, mut events) = host.client().await;
    client.subscribe(ThreadSelection::All, false).await.unwrap();
    let folder = host.folder(&client).await;
    let params = ThreadStartParams::new(
        AgentId::Claude,
        folder.id.clone(),
        "List the files here.".into(),
        Mode::Supervised,
        Origin::launcher(),
    );
    let thread = client.thread_start(params).await.unwrap();
    assert!(thread.state_since_ms > 0);
    until_done(&mut events, &thread.id).await;
    // The second turn asks before writing: the step shows while it waits.
    client
        .thread_send(thread.id.clone(), "Create hello.txt containing exactly: hi".into(), Origin::launcher())
        .await
        .unwrap();
    let asked = match next(&mut events, "approval", |e| matches!(e, Event::ApprovalRequested(_))).await {
        Event::ApprovalRequested(a) => a.approval,
        _ => unreachable!(),
    };
    let waiting = client.threads_list(None, None).await.unwrap().remove(0);
    assert_eq!(waiting.state, State::Waiting);
    assert_eq!(waiting.step.as_deref(), Some("Write /work/hello.txt"), "no description: the title");
    client.approval_answer(asked.token, Decision::Allow, Origin::launcher()).await.unwrap();
    let rest = until_done(&mut events, &thread.id).await;
    let export = client
        .thread_export(ThreadExportParams {
            thread_id: Some(thread.id.clone()),
            session_id: None,
            format: ExportFormat::Markdown,
        })
        .await
        .unwrap();
    assert!(export.text.starts_with("# List the files here.\n"), "{}", export.text);
    assert!(
        export.text.contains("**Write /work/hello.txt**\n\n```diff\n+hi\n```\n\n_Allowed on "),
        "{}",
        export.text
    );
    assert!(export.file_name.starts_with("claude-list-the-files-here-"));
    let both = client
        .thread_export(ThreadExportParams {
            thread_id: Some(thread.id.clone()),
            session_id: Some(sc(&uuid(1))),
            format: ExportFormat::Markdown,
        })
        .await
        .unwrap_err();
    assert_eq!(code(&both), Some(ErrorCode::BadRequest));
    let last = rest
        .iter()
        .rev()
        .find_map(|e| if let Event::ThreadState(s) = e { Some(&s.thread) } else { None })
        .unwrap();
    assert_eq!((last.state, last.step.as_deref()), (State::Done, None), "cleared at the end");
    assert!(last.state_since_ms >= waiting.state_since_ms);
    let native = client.threads_list(None, None).await.unwrap()[0].native_id.clone().unwrap();
    // The process idles out; then the user opens the thread in a terminal.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    host.process(4200, "claude", &["claude", "--resume", &native]);
    let busy = client.thread_send(thread.id.clone(), "More.".into(), Origin::launcher()).await.unwrap_err();
    assert_eq!(code(&busy), Some(ErrorCode::Busy));
    let ClientError::Remote(e) = &busy else { panic!() };
    assert_eq!(e.message, "Still open in a terminal. Close it there to continue here.");
    host.end_process(4200);
    client.thread_send(thread.id.clone(), "More.".into(), Origin::launcher()).await.unwrap();
    until_done(&mut events, &thread.id).await;
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_files_give_exact_states_and_following_can_be_turned_off() {
    let host = TestHost::start().await;
    let id = "a0000000-0000-4000-8000-0000000000aa";
    host.claude_session(id, &host.work, "Fix the login test");
    host.process(4242, "claude", &["claude"]);
    let status = std::fs::read_to_string(fixture("sessions/claude-status-waiting.json")).unwrap();
    let status = status.replace("1234567", "555").replace("/home/me/code/app", &host.work.to_string_lossy());
    std::fs::write(host.claude.join("sessions/4242.json"), status).unwrap();
    std::fs::write(host.claude.join("sessions/4242.key"), "never read").unwrap();
    let (client, mut events) = host.sessions_client().await;
    let Event::SessionState(first) =
        next(&mut events, "session.state", |e| matches!(e, Event::SessionState(_))).await
    else {
        unreachable!()
    };
    let s = first.session;
    assert_eq!((s.state, s.state_guess), (SessionState::NeedsYou, false));
    assert_eq!(s.waiting_for.as_deref(), Some("permission prompt"));
    assert_eq!(s.source, SessionSource::DesktopApp);
    assert_eq!(s.title, "Fix the login test");
    assert_eq!(s.blocked, Some(ContinueBlock::Open));
    assert_eq!(s.step.as_deref(), Some("Run the tests"), "the tool's own description");

    assert!(client.settings_get().await.unwrap().follow_sessions);
    let off = client.sessions_set_following(false).await.unwrap();
    assert!(!off.follow_sessions);
    next(&mut events, "session.removed", |e| matches!(e, Event::SessionRemoved(_))).await;
    assert!(client.sessions_list(None).await.unwrap().is_empty());
    let (other, _e) = host.client().await;
    assert!(!other.settings_get().await.unwrap().follow_sessions);
    client.sessions_set_following(true).await.unwrap();
    assert_eq!(client.sessions_list(None).await.unwrap().len(), 1);
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn moving_a_session_ends_its_process_with_one_signal_then_continues_it() {
    use std::os::unix::process::ExitStatusExt;

    let host = TestHost::start().await;
    let (client, _events) = host.client().await;
    let (link, _link_events) =
        Client::connect(&host.socket, HelloParams::new(ClientKind::Link, &[])).await.unwrap();
    assert!(link.host().caps.iter().any(|c| c == cap::MOVE));
    let id = uuid(7);
    host.claude_session(&id, &host.work, "Tidy the build");
    let (mut child, start) = own_claude(host.dir.path());
    let pid = child.id();
    host.claude_process(pid, start, &id, &host.work, "idle");
    let close = |start_ticks| SessionCloseParams { session_id: sc(&id), pid, start_ticks };
    let refused = |reason| SessionCloseResult { result: CloseOutcome::Refused, reason: Some(reason) };

    // Not in an agent folder yet: it couldn't be continued, so it isn't closed.
    let SessionPlaceResult::Place(place) = link.session_place(sc(&id)).await.unwrap() else {
        panic!("refused")
    };
    assert_eq!(
        (place.pid, place.start_ticks, place.state, place.state_exact),
        (pid, start, SessionState::Idle, true)
    );
    assert_eq!(place.continue_block, Some(ContinueBlock::Folder));
    assert_eq!(link.session_close(close(start)).await.unwrap(), refused(CloseRefusal::Folder));
    host.folder(&client).await;
    let SessionPlaceResult::Place(place) = link.session_place(sc(&id)).await.unwrap() else {
        panic!("refused")
    };
    assert_eq!(place.continue_block, None);

    // Only G7 Link, only the process it names, only while idle.
    let denied = client.session_close(close(start)).await.unwrap_err();
    assert_eq!(code(&denied), Some(ErrorCode::Denied));
    assert_eq!(link.session_close(close(start + 1)).await.unwrap(), refused(CloseRefusal::NoProcess));
    host.claude_process(pid, start, &id, &host.work, "busy");
    assert_eq!(link.session_close(close(start)).await.unwrap(), refused(CloseRefusal::Busy));
    host.claude_process(pid, start, &id, &host.work, "waiting");
    assert_eq!(link.session_close(close(start)).await.unwrap(), refused(CloseRefusal::NeedsYou));
    assert!(child.try_wait().unwrap().is_none(), "nothing was sent");

    // Idle: one SIGTERM, and it exits.
    host.claude_process(pid, start, &id, &host.work, "idle");
    let closed = link.session_close(close(start)).await.unwrap();
    assert_eq!(closed, SessionCloseResult { result: CloseOutcome::Closed, reason: None });
    assert_eq!(child.wait().unwrap().signal(), Some(15));

    // The host's own check still decides: open while its `/proc` shows it.
    let busy = link.session_continue(continue_params(&sc(&id), Mode::Supervised, Origin::launcher())).await;
    assert_eq!(code(&busy.unwrap_err()), Some(ErrorCode::Busy));
    host.end_process(pid);
    let thread =
        link.session_continue(continue_params(&sc(&id), Mode::Supervised, Origin::launcher())).await.unwrap();
    assert!(thread.continued);
    assert_eq!(thread.native_id.as_deref(), Some(id.as_str()));
    host.stop().await;
}
