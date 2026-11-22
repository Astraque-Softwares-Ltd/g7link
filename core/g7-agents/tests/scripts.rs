// SPDX-License-Identifier: MPL-2.0
//! Project scripts end to end (phase 6, §X3, §X12 H): an in-process host on a socket in a temp
//! dir, threads made by `g7-agents-fake-cli`, and short real `/bin/sh` lines (`printf`, `sleep`,
//! `exit`, `trap`) run in a temp folder. Each test waits for the exact event it needs. Nothing
//! reaches the network.
#![cfg(feature = "fake-cli")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use g7_agents::api::*;
use g7_agents::client::{Client, ClientError, Events};
use g7_agents::host::{self, Config, Served};
use tokio::sync::oneshot;

const WAIT: Duration = Duration::from_secs(20);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The fake `claude`: one finished turn.
fn claude(dir: &Path) -> PathBuf {
    let path = dir.join("claude-cli");
    let body = format!(
        "#!/bin/sh\nexport FAKE_CLI_SCRIPT={}\nexport FAKE_CLI_LOG={}\nexec {} claude \"$@\"\n",
        quote(&fixture("claude-01-list.jsonl")),
        quote(&dir.join("cli.log")),
        quote(Path::new(env!("CARGO_BIN_EXE_g7-agents-fake-cli")))
    );
    executable(&path, &body);
    path
}

#[derive(Default)]
struct Options {
    /// Added to the host's environment.
    env: Vec<(&'static str, String)>,
    idle_exit: Option<Duration>,
}

struct TestHost {
    dir: tempfile::TempDir,
    socket: PathBuf,
    work: PathBuf,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<std::io::Result<Served>>>,
}

impl TestHost {
    async fn start(opts: Options) -> TestHost {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let run = dir.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
        let home = dir.path().join("home");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        let mut env: Vec<(std::ffi::OsString, std::ffi::OsString)> =
            vec![("HOME".into(), home.clone().into_os_string()), ("PATH".into(), "/usr/bin:/bin".into())];
        env.extend(opts.env.iter().map(|(k, v)| ((*k).into(), v.into())));
        let config = Config {
            socket: run.join("host.sock"),
            runtime_dir: run,
            data_dir: dir.path().join("data/g7-agents"),
            claude_bin: Some(claude(dir.path())),
            codex_bin: None,
            bridge_exe: PathBuf::from(env!("CARGO_BIN_EXE_g7-agents")),
            notify: false,
            env,
            idle_exit: opts.idle_exit.unwrap_or(host::IDLE_EXIT),
            agent_idle: Duration::from_secs(600),
            approval_lifetime_ms: APPROVAL_LIFETIME_MS,
            claude_home: home.join(".claude"),
            codex_home: home.join(".codex"),
            proc_root: PathBuf::from("/proc"),
            sessions: false,
            keep_awake_bus: false,
        };
        let socket = config.socket.clone();
        let (stop, stopped) = oneshot::channel::<()>();
        let mut task = tokio::spawn(host::serve(config, async move {
            let _ = stopped.await;
        }));
        let deadline = tokio::time::Instant::now() + WAIT;
        while !socket.exists() {
            assert!(!task.is_finished(), "the host stopped before it listened: {:?}", (&mut task).await);
            assert!(tokio::time::Instant::now() < deadline, "the host didn't listen");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        TestHost { dir, socket, work, stop: Some(stop), task: Some(task) }
    }

    async fn client(&self, kind: ClientKind, caps: &[&str]) -> (Client, Events) {
        let (client, events) = Client::connect(&self.socket, HelloParams::new(kind, caps)).await.unwrap();
        client.subscribe(ThreadSelection::All, false).await.unwrap();
        (client, events)
    }

    /// G7 Link, saying `scripts`.
    async fn link(&self) -> (Client, Events) {
        self.client(ClientKind::Link, &[cap::SCRIPTS]).await
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

/// A folder at `path` with `scripts` (name, command).
async fn folder_at(link: &Client, path: &Path, scripts: &[(&str, &str)]) -> Folder {
    let folder = link.folders_add(path.to_string_lossy().into_owned(), None).await.unwrap();
    let wanted = scripts
        .iter()
        .map(|(name, command)| ScriptWanted { id: None, name: (*name).into(), command: (*command).into() })
        .collect();
    link.folders_set_with(FoldersSetParams { id: folder.id, scripts: Some(wanted), ..Default::default() })
        .await
        .unwrap()
}

/// A thread in `folder` whose one turn has finished.
async fn thread_in(link: &Client, events: &mut Events, folder: &Folder) -> ThreadInfo {
    let p = ThreadStartParams::new(
        AgentId::Claude,
        folder.id.clone(),
        "List the files here.".into(),
        Mode::Supervised,
        Origin::launcher(),
    );
    let thread = link.thread_start(p).await.unwrap();
    next(events, "the turn's end", |e| matches!(e, Event::ThreadDone(d) if d.thread_id == thread.id)).await;
    thread
}

fn script<'a>(folder: &'a Folder, name: &str) -> &'a Script {
    folder.scripts.iter().find(|s| s.name == name).unwrap()
}

async fn run(link: &Client, thread: &ThreadInfo, folder: &Folder, name: &str) -> ScriptRun {
    let p = ScriptRunParams {
        thread_id: thread.id.clone(),
        script_id: script(folder, name).id.clone(),
        by: None,
    };
    link.script_run(p).await.unwrap()
}

/// The run's last `script.progress`.
async fn ended(events: &mut Events, run: &str) -> ScriptRun {
    let ended = next(
        events,
        "the run's end",
        |e| matches!(e, Event::ScriptProgress(p) if p.run.id == run && p.run.phase != RunPhase::Running),
    )
    .await;
    match ended {
        Event::ScriptProgress(p) => p.run,
        _ => unreachable!(),
    }
}

/// Every line the run kept.
async fn output(link: &Client, run: &str) -> ScriptRun {
    link.script_output(ScriptOutputParams { run: run.into(), from: None }).await.unwrap()
}

/// The first line the run prints, as it arrives.
async fn first_line(events: &mut Events, run: &str) -> String {
    let event = next(
        events,
        "the run's first line",
        |e| matches!(e, Event::ScriptProgress(p) if p.run.id == run && !p.run.lines.is_empty()),
    )
    .await;
    match event {
        Event::ScriptProgress(p) => p.run.lines[0].clone(),
        _ => unreachable!(),
    }
}

fn reason(e: &ClientError) -> Option<String> {
    match e {
        ClientError::Remote(r) => r.data.as_ref()?.get("reason")?.as_str().map(str::to_owned),
        _ => None,
    }
}

/// Whether process `pid` is gone (or only waiting to be reaped by someone else), within a few seconds.
async fn gone(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok();
        let state =
            stat.as_deref().and_then(|s| s.rsplit_once(") ")).and_then(|(_, rest)| rest.chars().next());
        if state.is_none_or(|c| c == 'Z' || c == 'X') {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_line_reaches_the_users_shell_as_typed_in_the_folder_with_the_hosts_environment() {
    // A shell that writes down its arguments, then is `/bin/sh`.
    let scratch = tempfile::tempdir().unwrap();
    let argv = scratch.path().join("argv");
    let shell = scratch.path().join("myshell");
    executable(
        &shell,
        &format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\0' \"$a\"; done > {}\nexec /bin/sh \"$@\"\n",
            quote(&argv)
        ),
    );
    let host = TestHost::start(Options {
        env: vec![
            ("SHELL", shell.to_string_lossy().into_owned()),
            ("G7_AGENTS_SECRET_SOCKET", "x".into()),
            ("CLAUDE_CODE_ENTRYPOINT", "cli".into()),
            ("CLAUDECODE", "1".into()),
            ("GIT_DIR", "/nonexistent/.git".into()),
            ("KEEP_ME", "yes".into()),
        ],
        ..Default::default()
    })
    .await;
    let (link, mut events) = host.link().await;
    let quoted = r#"printf '%s|%s|%s\n' "it's" "$(echo sub)" '$KEEP_ME' && pwd && cat && echo "stdin ended""#;
    let dash = r#"-not an option "q" $(x) && y"#;
    let folder = folder_at(&link, &host.work, &[("quoted", quoted), ("dash", dash), ("env", "env")]).await;
    let thread = thread_in(&link, &mut events, &folder).await;

    let started = run(&link, &thread, &folder, "quoted").await;
    assert_eq!(
        (started.phase, started.command.as_str(), started.by.kind),
        (RunPhase::Running, quoted, OriginKind::Launcher)
    );
    let done = ended(&mut events, &started.id).await;
    assert_eq!((done.phase, done.exit), (RunPhase::Done, Some(0)), "{done:?}");
    let args = std::fs::read(&argv).unwrap();
    assert_eq!(args, [b"-c\0".as_slice(), quoted.as_bytes(), b"\0"].concat(), "exactly -c and the line");
    let lines = output(&link, &started.id).await.lines;
    assert_eq!(
        lines,
        ["it's|sub|$KEEP_ME", folder.path.as_str(), "stdin ended"],
        "the folder, and stdin at its end"
    );

    let dashed = run(&link, &thread, &folder, "dash").await;
    ended(&mut events, &dashed.id).await;
    assert_eq!(std::fs::read(&argv).unwrap(), [b"-c\0".as_slice(), dash.as_bytes(), b"\0"].concat());

    let env = run(&link, &thread, &folder, "env").await;
    ended(&mut events, &env.id).await;
    let vars = output(&link, &env.id).await.lines;
    assert!(vars.iter().any(|v| v == "KEEP_ME=yes"), "{vars:?}");
    for v in &vars {
        assert!(!v.starts_with("G7_AGENTS_") && !v.starts_with("CLAUDE_CODE_"), "{v}");
        assert!(!v.starts_with("CLAUDECODE=") && !v.starts_with("GIT_DIR="), "{v}");
    }
    assert_eq!(link.script_runs(thread.id.clone()).await.unwrap().len(), 3);
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn how_a_run_ends_is_its_exit_a_signal_or_a_shell_that_couldnt_start() {
    let host = TestHost::start(Options::default()).await;
    let (link, mut events) = host.link().await;
    let folder = folder_at(
        &link,
        &host.work,
        &[("ok", "exit 0"), ("three", "echo no; exit 3"), ("killed", "kill -KILL $$")],
    )
    .await;
    let thread = thread_in(&link, &mut events, &folder).await;
    let ok = run(&link, &thread, &folder, "ok").await;
    assert_eq!((ended(&mut events, &ok.id).await.phase), RunPhase::Done);
    let three = run(&link, &thread, &folder, "three").await;
    let three = ended(&mut events, &three.id).await;
    assert_eq!((three.phase, three.exit, three.error), (RunPhase::Failed, Some(3), None));
    let killed = run(&link, &thread, &folder, "killed").await;
    let killed = ended(&mut events, &killed.id).await;
    assert_eq!((killed.phase, killed.exit), (RunPhase::Failed, None));
    assert_eq!(killed.error.map(|e| e.reason).as_deref(), Some("signal"));
    // The thread lists them, newest first.
    let listed: Vec<String> =
        link.script_runs(thread.id.clone()).await.unwrap().into_iter().map(|r| r.id).collect();
    assert_eq!(listed.len(), 3);
    let state = link.threads_list(None, None).await.unwrap().into_iter().find(|t| t.id == thread.id).unwrap();
    let phases: Vec<RunPhase> = state.runs.iter().map(|r| r.phase).collect();
    assert_eq!(phases, [RunPhase::Failed, RunPhase::Failed, RunPhase::Done], "newest first");
    host.stop().await;

    // A shell that can't be started: the run is still kept, failed.
    let scratch = tempfile::tempdir().unwrap();
    let broken = scratch.path().join("broken-shell");
    executable(&broken, "#!/nonexistent/interpreter\n");
    let host = TestHost::start(Options {
        env: vec![("SHELL", broken.to_string_lossy().into_owned())],
        ..Default::default()
    })
    .await;
    let (link, mut events) = host.link().await;
    let folder = folder_at(&link, &host.work, &[("ok", "exit 0")]).await;
    let thread = thread_in(&link, &mut events, &folder).await;
    let failed = run(&link, &thread, &folder, "ok").await;
    assert_eq!(failed.phase, RunPhase::Failed);
    assert_eq!(failed.error.as_ref().map(|e| e.reason.as_str()), Some("spawn"));
    let kept = output(&link, &failed.id).await;
    assert_eq!(kept.lines.len(), 1);
    assert!(kept.lines[0].starts_with("The shell couldn't be started."), "{:?}", kept.lines);
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_ends_a_run_a_trap_after_the_grace_and_a_child_that_holds_the_pipes() {
    let host = TestHost::start(Options::default()).await;
    let (link, mut events) = host.link().await;
    let folder = folder_at(
        &link,
        &host.work,
        &[
            ("sleep", "echo up; sleep 600"),
            ("trap", "trap '' TERM; echo up; sleep 600"),
            ("child", "sleep 600 & echo $!"),
        ],
    )
    .await;
    let thread = thread_in(&link, &mut events, &folder).await;
    let stop = |run: &ScriptRun| ScriptStopParams { run: run.id.clone(), by: None };

    let sleeping = run(&link, &thread, &folder, "sleep").await;
    first_line(&mut events, &sleeping.id).await;
    let asked = Instant::now();
    assert_eq!(
        link.script_stop(stop(&sleeping)).await.unwrap().phase,
        RunPhase::Running,
        "as it is when asked"
    );
    let end = ended(&mut events, &sleeping.id).await;
    assert_eq!((end.phase, end.exit), (RunPhase::Stopped, None));
    assert!(asked.elapsed() < Duration::from_millis(RUN_STOP_GRACE_MS), "TERM ended it");

    let trapped = run(&link, &thread, &folder, "trap").await;
    first_line(&mut events, &trapped.id).await;
    let asked = Instant::now();
    link.script_stop(stop(&trapped)).await.unwrap();
    assert_eq!(ended(&mut events, &trapped.id).await.phase, RunPhase::Stopped);
    let took = asked.elapsed();
    assert!(took >= Duration::from_millis(RUN_STOP_GRACE_MS - 500), "KILL after the grace: {took:?}");
    assert!(took < Duration::from_millis(RUN_STOP_GRACE_MS + 4000), "{took:?}");

    // The shell exits at once; its child holds the pipes, so the run runs on.
    let child = run(&link, &thread, &folder, "child").await;
    let pid: u32 = first_line(&mut events, &child.id).await.parse().unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let listed = link.script_runs(thread.id.clone()).await.unwrap();
    assert_eq!(listed.iter().find(|r| r.id == child.id).map(|r| r.phase), Some(RunPhase::Running));
    link.script_stop(stop(&child)).await.unwrap();
    assert_eq!(ended(&mut events, &child.id).await.phase, RunPhase::Stopped);
    assert!(gone(pid).await, "the child was stopped with its group");
    // Stopping an ended run answers it as it is.
    assert_eq!(link.script_stop(stop(&child)).await.unwrap().phase, RunPhase::Stopped);
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_run_of_a_script_per_place_four_a_thread_and_eight_in_the_host() {
    let host = TestHost::start(Options::default()).await;
    let (link, mut events) = host.link().await;
    let names: Vec<String> = (1..=9).map(|n| format!("s{n}")).collect();
    let scripts: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "sleep 600")).collect();
    let folder = folder_at(&link, &host.work, &scripts).await;
    let a = thread_in(&link, &mut events, &folder).await;
    let b = thread_in(&link, &mut events, &folder).await;
    let c = thread_in(&link, &mut events, &folder).await;

    let first = run(&link, &a, &folder, "s1").await;
    for thread in [&a, &b] {
        let p = ScriptRunParams {
            thread_id: thread.id.clone(),
            script_id: script(&folder, "s1").id.clone(),
            by: None,
        };
        let busy = link.script_run(p).await.unwrap_err();
        assert_eq!((busy.code(), reason(&busy).as_deref()), (Some(ErrorCode::Busy), Some("script_running")));
        let ClientError::Remote(e) = &busy else { panic!() };
        assert_eq!(e.data.as_ref().unwrap()["run"], first.id.as_str(), "the one running");
        assert_eq!(e.message, "\u{201c}s1\u{201d} is already running here.");
    }
    for name in ["s2", "s3", "s4"] {
        run(&link, &a, &folder, name).await;
    }
    let p =
        ScriptRunParams { thread_id: a.id.clone(), script_id: script(&folder, "s5").id.clone(), by: None };
    let fifth = link.script_run(p).await.unwrap_err();
    assert_eq!((fifth.code(), reason(&fifth).as_deref()), (Some(ErrorCode::Limit), Some("runs")));
    for name in ["s5", "s6", "s7", "s8"] {
        run(&link, &b, &folder, name).await;
    }
    let p =
        ScriptRunParams { thread_id: c.id.clone(), script_id: script(&folder, "s9").id.clone(), by: None };
    let ninth = link.script_run(p).await.unwrap_err();
    assert_eq!((ninth.code(), reason(&ninth).as_deref()), (Some(ErrorCode::Limit), Some("runs")));
    let state = link.threads_list(None, None).await.unwrap().into_iter().find(|t| t.id == a.id).unwrap();
    assert_eq!(state.runs.len(), 4);
    assert!(state.runs.iter().all(|r| r.phase == RunPhase::Running));
    // A script gone from the list can't be run; a run of it goes on.
    link.folders_set_with(FoldersSetParams {
        id: folder.id.clone(),
        scripts: Some(Vec::new()),
        ..Default::default()
    })
    .await
    .unwrap();
    let p =
        ScriptRunParams { thread_id: c.id.clone(), script_id: script(&folder, "s9").id.clone(), by: None };
    let missing = link.script_run(p).await.unwrap_err();
    assert_eq!((missing.code(), reason(&missing).as_deref()), (Some(ErrorCode::NotFound), Some("no_script")));
    assert_eq!(
        link.script_runs(a.id.clone()).await.unwrap().iter().filter(|r| r.phase == RunPhase::Running).count(),
        4
    );
    assert_eq!(host.stop().await, Served::Stopped);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flood_keeps_the_newest_lines_and_events_stay_within_their_bounds() {
    let host = TestHost::start(Options::default()).await;
    let (link, mut events) = host.link().await;
    let flood = "i=0; while [ $i -lt 10000 ]; do printf '%098d\\n' $i; i=$((i+1)); done";
    let dirty = "printf '\\033[31mred\\033[0m https://user:secret@example.test/x \\342\\200\\256bidi %s/x\\n' \"$HOME\"";
    let folder = folder_at(&link, &host.work, &[("flood", flood), ("dirty", dirty)]).await;
    let thread = thread_in(&link, &mut events, &folder).await;

    let started = run(&link, &thread, &folder, "flood").await;
    let mut seen: Vec<ScriptRun> = Vec::new();
    loop {
        let event = next(
            &mut events,
            "the flood's progress",
            |e| matches!(e, Event::ScriptProgress(p) if p.run.id == started.id),
        )
        .await;
        let Event::ScriptProgress(p) = event else { unreachable!() };
        let done = p.run.phase != RunPhase::Running;
        seen.push(p.run);
        if done {
            break;
        }
    }
    let mut after = 0u64;
    for event in &seen {
        assert!(event.lines.len() <= MAX_RUN_EVENT_LINES, "{}", event.lines.len());
        assert!(event.lines.iter().map(String::len).sum::<usize>() <= MAX_RUN_EVENT_BYTES);
        assert!(event.from >= after, "numbered on, gaps allowed: {} after {after}", event.from);
        if !event.lines.is_empty() {
            after = event.from + event.lines.len() as u64;
        }
    }
    let last = seen.last().unwrap();
    assert_eq!((last.phase, last.exit), (RunPhase::Done, Some(0)));
    let kept = output(&link, &started.id).await;
    assert_eq!(kept.lines.len(), MAX_RUN_LINES);
    assert_eq!(kept.dropped, 10_000 - MAX_RUN_LINES as u32, "every line not kept is counted");
    assert_eq!(kept.from + kept.lines.len() as u64, after, "the output ends where the events did");
    assert!(kept.lines.iter().all(|l| l.len() == 98 && l.bytes().all(|b| b.is_ascii_digit())));
    // Asked from a line on: only those.
    let tail = link
        .script_output(ScriptOutputParams { run: started.id.clone(), from: Some(after - 3) })
        .await
        .unwrap();
    assert_eq!((tail.from, tail.lines.len()), (after - 3, 3));

    let dirty = run(&link, &thread, &folder, "dirty").await;
    ended(&mut events, &dirty.id).await;
    let line = output(&link, &dirty.id).await.lines.remove(0);
    assert!(line.starts_with("red https://"), "{line}");
    assert!(!line.contains('\u{1b}') && !line.contains("secret") && !line.contains('\u{202e}'), "{line}");
    assert!(line.ends_with(" ~/x"), "the home folder written ~: {line}");
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_the_thread_or_removing_the_folder_stops_its_runs() {
    let host = TestHost::start(Options::default()).await;
    let (link, mut events) = host.link().await;
    let folder = folder_at(&link, &host.work, &[("sleep", "echo up; sleep 600")]).await;
    let thread = thread_in(&link, &mut events, &folder).await;
    let doomed = run(&link, &thread, &folder, "sleep").await;
    first_line(&mut events, &doomed.id).await;
    link.thread_delete(thread.id.clone()).await.unwrap();
    assert_eq!(ended(&mut events, &doomed.id).await.phase, RunPhase::Stopped);

    let other = host.dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let second = folder_at(&link, &other, &[("sleep", "echo up; sleep 600")]).await;
    let thread = thread_in(&link, &mut events, &second).await;
    let running = run(&link, &thread, &second, "sleep").await;
    first_line(&mut events, &running.id).await;
    link.folders_remove(second.id.clone()).await.unwrap();
    assert_eq!(ended(&mut events, &running.id).await.phase, RunPhase::Stopped);
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_keeps_the_host_up_and_shutdown_leaves_no_child() {
    let host =
        TestHost::start(Options { idle_exit: Some(Duration::from_millis(500)), ..Default::default() }).await;
    let (link, mut events) = host.link().await;
    let folder = folder_at(
        &link,
        &host.work,
        &[("sleep", "echo $$; exec sleep 600"), ("deaf", "trap '' TERM; echo $$; exec sleep 600")],
    )
    .await;
    let thread = thread_in(&link, &mut events, &folder).await;
    let sleeping = run(&link, &thread, &folder, "sleep").await;
    let first: u32 = first_line(&mut events, &sleeping.id).await.parse().unwrap();
    let deaf = run(&link, &thread, &folder, "deaf").await;
    let second: u32 = first_line(&mut events, &deaf.id).await.parse().unwrap();
    drop(link);
    drop(events);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!host.task.as_ref().unwrap().is_finished(), "no client, but runs: the host stays");
    let asked = Instant::now();
    assert_eq!(host.stop().await, Served::Stopped);
    assert!(asked.elapsed() < Duration::from_secs(15), "{:?}", asked.elapsed());
    assert!(gone(first).await && gone(second).await, "no run outlives its host");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_nothing_running_the_host_still_idles_out() {
    let mut host =
        TestHost::start(Options { idle_exit: Some(Duration::from_millis(500)), ..Default::default() }).await;
    let (link, mut events) = host.link().await;
    let folder = folder_at(&link, &host.work, &[("quick", "exit 0")]).await;
    let thread = thread_in(&link, &mut events, &folder).await;
    let quick = run(&link, &thread, &folder, "quick").await;
    ended(&mut events, &quick.id).await;
    drop(link);
    drop(events);
    let task = host.task.take().unwrap();
    let served = tokio::time::timeout(WAIT, task).await.expect("idled out").unwrap().unwrap();
    assert_eq!(served, Served::Idle);
}
