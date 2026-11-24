// SPDX-License-Identifier: MPL-2.0
//! `session.close` (Move to G7 by a signal, `plans/move-and-focus.md` §4.7,
//! the owner's choice in §2): G7 Link asks the host to end the one process a
//! followed session runs in, then continues the session as a thread.
//!
//! The session is placed again first. It must still run in the process G7
//! Link names (pid and start time), started in a terminal (a tab, tmux, ssh,
//! an editor's panel: any chain), be idle (Claude: by its status file), and
//! nothing may stop `session.continue` afterwards
//! (`SessionPlace.continue_block`). Then a pidfd pins the process, the
//! kernel's `/proc` is read again for its user, start time, name and state,
//! and SIGTERM goes through the pidfd, so it can't reach a process that
//! reused the pid. Never SIGKILL, and never a second signal to a process: one
//! still there after [`CLOSE_WAIT_MS`] is left alone (`open`).
//!
//! `session.continue`'s own check of what has the session open still
//! decides whether it can be continued; this never weakens it.

use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{procs, tail};
use crate::api::*;
use crate::host::Host;

/// The kernel's `/proc`: a pidfd is a real process's, whatever `/proc` the
/// session reader looks at.
const PROC: &str = "/proc";
const MAX_STAT: u64 = 4096;
/// How often the exit is looked for.
const EXIT_POLL: Duration = Duration::from_millis(100);

/// Processes sent SIGTERM, oldest first, at most [`MAX_CLOSED_PROCS`].
#[derive(Debug, Default)]
pub(crate) struct Signalled(Mutex<VecDeque<(u32, u64)>>);

impl Signalled {
    /// SIGTERM to the process through `ender`, unless it was sent one
    /// already. The list is held meanwhile, so two requests can't both send.
    fn term(&self, ender: &dyn Ender, pid: u32, start_ticks: u64) -> io::Result<Signal> {
        let mut sent = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if sent.contains(&(pid, start_ticks)) {
            return Ok(Signal::Before);
        }
        let outcome = ender.term(pid, start_ticks)?;
        if outcome == Signal::Sent {
            if sent.len() >= MAX_CLOSED_PROCS {
                sent.pop_front();
            }
            sent.push_back((pid, start_ticks));
        }
        Ok(outcome)
    }
}

/// What became of a SIGTERM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signal {
    /// Sent now.
    Sent,
    /// Sent by an earlier request; nothing now.
    Before,
    /// The process is gone.
    Gone,
    /// The pid names another process now, or one of another user, or not an agent.
    Other,
    /// The process is stopped (job control): a signal would act only once it runs again.
    Stopped,
}

/// Ends a process named by its pid and start time. Blocking.
pub(crate) trait Ender: Send + Sync {
    /// Sends it SIGTERM if it is still that agent process, and running.
    fn term(&self, pid: u32, start_ticks: u64) -> io::Result<Signal>;
    /// It has exited (a zombie too), or the pid names another process.
    fn gone(&self, pid: u32, start_ticks: u64) -> bool;
}

/// A pidfd and the kernel's `/proc`.
struct Pidfd;

impl Ender for Pidfd {
    fn term(&self, pid: u32, start_ticks: u64) -> io::Result<Signal> {
        let Some(raw) = i32::try_from(pid).ok().and_then(rustix::process::Pid::from_raw) else {
            return Ok(Signal::Other);
        };
        let pidfd = match rustix::process::pidfd_open(raw, rustix::process::PidfdFlags::empty()) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::SRCH) => return Ok(Signal::Gone),
            Err(e) => return Err(e.into()),
        };
        // The pidfd holds whatever had the pid when it was opened: that is
        // the process asked for only if it still has the pid now.
        match agent_now(Path::new(PROC), crate::client::euid(), pid, start_ticks) {
            Now::Runs => {}
            Now::Stopped => return Ok(Signal::Stopped),
            Now::Gone => return Ok(Signal::Gone),
            Now::Other => return Ok(Signal::Other),
        }
        match rustix::process::pidfd_send_signal(&pidfd, rustix::process::Signal::TERM) {
            Ok(()) => Ok(Signal::Sent),
            Err(rustix::io::Errno::SRCH) => Ok(Signal::Gone),
            Err(e) => Err(e.into()),
        }
    }

    fn gone(&self, pid: u32, start_ticks: u64) -> bool {
        matches!(agent_now(Path::new(PROC), crate::client::euid(), pid, start_ticks), Now::Gone | Now::Other)
    }
}

/// What a pid is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Now {
    /// The agent process asked for, running.
    Runs,
    /// That process, stopped.
    Stopped,
    /// Exited, or a zombie.
    Gone,
    /// Another process has the pid, or it is another user's, or not an agent.
    Other,
}

/// What `pid` under `proc_root` is: the `claude` or `codex` process of user
/// `euid` that started at `start_ticks`, or not.
fn agent_now(proc_root: &Path, euid: u32, pid: u32, start_ticks: u64) -> Now {
    let dir = proc_root.join(pid.to_string());
    let Ok(bytes) = tail::read_head(&dir.join("stat"), MAX_STAT) else { return Now::Gone };
    let text = String::from_utf8_lossy(&bytes);
    let Some(stat) = procs::parse_stat(&text) else { return Now::Gone };
    // Field 3, the state, comes first after the last `)`.
    let state = text.rfind(')').and_then(|at| text[at + 1..].split_whitespace().next());
    if matches!(state, Some("Z" | "X" | "x")) {
        return Now::Gone;
    }
    if stat.start_ticks != start_ticks
        || procs::owner(&dir) != Some(euid)
        || !matches!(stat.comm.as_str(), "claude" | "codex")
    {
        return Now::Other;
    }
    if matches!(state, Some("T" | "t")) { Now::Stopped } else { Now::Runs }
}

fn refused(reason: CloseRefusal) -> SessionCloseResult {
    SessionCloseResult { result: CloseOutcome::Refused, reason: Some(reason) }
}

/// Whether the place `session.place` answered may be closed: the process
/// asked for, started in a terminal, idle, and continuable afterwards.
fn closable(found: &SessionPlaceResult, pid: u32, start_ticks: u64) -> Result<(), CloseRefusal> {
    let place = match found {
        SessionPlaceResult::Place(place) => place,
        SessionPlaceResult::Refused(refused) => {
            return Err(match refused.reason {
                PlaceRefusal::Ended => CloseRefusal::Ended,
                PlaceRefusal::Ambiguous => CloseRefusal::Ambiguous,
                PlaceRefusal::Place | PlaceRefusal::Unknown => CloseRefusal::Place,
            });
        }
    };
    if (place.pid, place.start_ticks) != (pid, start_ticks) {
        return Err(CloseRefusal::NoProcess);
    }
    if place.source != SessionSource::Terminal {
        return Err(CloseRefusal::Place);
    }
    match place.state {
        SessionState::Idle => {}
        SessionState::Working => return Err(CloseRefusal::Busy),
        SessionState::NeedsYou | SessionState::NeedsYouGuess => return Err(CloseRefusal::NeedsYou),
        SessionState::Ended => return Err(CloseRefusal::Ended),
    }
    // Claude says when it is idle; a guess could end a running turn.
    if place.agent == AgentId::Claude && !place.state_exact {
        return Err(CloseRefusal::Place);
    }
    match place.continue_block {
        None => Ok(()),
        Some(ContinueBlock::Folder) => Err(CloseRefusal::Folder),
        Some(ContinueBlock::Agent) => Err(CloseRefusal::Agent),
        Some(ContinueBlock::Open) => Err(CloseRefusal::Place),
    }
}

impl Host {
    /// `session.close`, from G7 Link only.
    pub async fn session_close(
        self: &Arc<Self>,
        kind: ClientKind,
        p: SessionCloseParams,
    ) -> Result<SessionCloseResult, RpcError> {
        self.close_with(kind, p, Arc::new(Pidfd), Duration::from_millis(CLOSE_WAIT_MS)).await
    }

    /// [`Host::session_close`] through `ender`, waiting `wait` for the exit.
    async fn close_with(
        self: &Arc<Self>,
        kind: ClientKind,
        p: SessionCloseParams,
        ender: Arc<dyn Ender>,
        wait: Duration,
    ) -> Result<SessionCloseResult, RpcError> {
        if kind != ClientKind::Link {
            return Err(RpcError::new(ErrorCode::Denied, "Only G7 Link closes a session."));
        }
        let found = self.session_place(&p.session_id).await?;
        if let Err(reason) = closable(&found, p.pid, p.start_ticks) {
            return Ok(refused(reason));
        }
        let (pid, start_ticks) = (p.pid, p.start_ticks);
        let failed = || RpcError::new(ErrorCode::Internal, "Couldn't close it.");
        let sent = {
            let (host, ender) = (Arc::clone(self), Arc::clone(&ender));
            tokio::task::spawn_blocking(move || host.sessions.signalled.term(&*ender, pid, start_ticks))
                .await
                .map_err(|_| failed())?
                .map_err(|_| failed())?
        };
        match sent {
            Signal::Sent | Signal::Before => {}
            Signal::Gone => return Ok(refused(CloseRefusal::Ended)),
            Signal::Other => return Ok(refused(CloseRefusal::NoProcess)),
            Signal::Stopped => return Ok(refused(CloseRefusal::Place)),
        }
        let started = Instant::now();
        let result = loop {
            let ender = Arc::clone(&ender);
            let gone =
                tokio::task::spawn_blocking(move || ender.gone(pid, start_ticks)).await.unwrap_or(false);
            if gone {
                break CloseOutcome::Closed;
            }
            if started.elapsed() >= wait {
                break CloseOutcome::Open;
            }
            tokio::time::sleep(EXIT_POLL).await;
        };
        tracing::info!(
            pid,
            now = sent == Signal::Sent,
            closed = result == CloseOutcome::Closed,
            exit_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "session.close"
        );
        Ok(SessionCloseResult { result, reason: None })
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Child, Command, Stdio};

    use super::*;
    use crate::host::sessions::paths;
    use crate::host::sessions::place::tests::{Fake, U, agents_ready, claude_in};

    /// Long enough for a fake exit, short enough for a test.
    const SHORT: Duration = Duration::from_millis(300);

    /// Records what it is asked; answers `sent`, and `exits` once asked.
    struct Recorder {
        asked: Mutex<Vec<(u32, u64)>>,
        sent: Signal,
        exits: bool,
    }

    impl Recorder {
        fn new(sent: Signal, exits: bool) -> Arc<Recorder> {
            Arc::new(Recorder { asked: Mutex::new(Vec::new()), sent, exits })
        }

        fn asked(&self) -> Vec<(u32, u64)> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl Ender for Recorder {
        fn term(&self, pid: u32, start_ticks: u64) -> io::Result<Signal> {
            self.asked.lock().unwrap().push((pid, start_ticks));
            Ok(self.sent)
        }

        fn gone(&self, _pid: u32, _start_ticks: u64) -> bool {
            self.exits
        }
    }

    fn place() -> SessionPlace {
        SessionPlace {
            agent: AgentId::Claude,
            source: SessionSource::Terminal,
            state: SessionState::Idle,
            state_exact: true,
            pid: 102,
            start_ticks: 102,
            pgrp: 102,
            tty: None,
            foreground: false,
            chain: Vec::new(),
            marks: Vec::new(),
            continue_block: None,
        }
    }

    fn check(place: SessionPlace) -> Result<(), CloseRefusal> {
        closable(&SessionPlaceResult::Place(place), 102, 102)
    }

    #[test]
    fn only_an_idle_process_in_a_terminal_that_could_be_continued_may_be_closed() {
        assert_eq!(check(place()), Ok(()));
        // Codex says no state: its guess of idle is what there is.
        assert_eq!(check(SessionPlace { agent: AgentId::Codex, state_exact: false, ..place() }), Ok(()));
        // Neither a tty nor the foreground matters to a signal.
        assert_eq!(check(SessionPlace { tty: Some(34816), foreground: true, ..place() }), Ok(()));
        for (changed, refusal) in [
            (SessionPlace { pid: 103, ..place() }, CloseRefusal::NoProcess),
            (SessionPlace { start_ticks: 103, ..place() }, CloseRefusal::NoProcess),
            (SessionPlace { source: SessionSource::DesktopApp, ..place() }, CloseRefusal::Place),
            (SessionPlace { source: SessionSource::Other, ..place() }, CloseRefusal::Place),
            (SessionPlace { state: SessionState::Working, ..place() }, CloseRefusal::Busy),
            (
                SessionPlace {
                    agent: AgentId::Codex,
                    state: SessionState::Working,
                    state_exact: false,
                    ..place()
                },
                CloseRefusal::Busy,
            ),
            (SessionPlace { state: SessionState::NeedsYou, ..place() }, CloseRefusal::NeedsYou),
            (SessionPlace { state: SessionState::NeedsYouGuess, ..place() }, CloseRefusal::NeedsYou),
            (SessionPlace { state: SessionState::Ended, ..place() }, CloseRefusal::Ended),
            // Claude's idle from its transcript only: it may be mid-turn.
            (SessionPlace { state_exact: false, ..place() }, CloseRefusal::Place),
            (SessionPlace { continue_block: Some(ContinueBlock::Folder), ..place() }, CloseRefusal::Folder),
            (SessionPlace { continue_block: Some(ContinueBlock::Agent), ..place() }, CloseRefusal::Agent),
            (SessionPlace { continue_block: Some(ContinueBlock::Open), ..place() }, CloseRefusal::Place),
        ] {
            assert_eq!(check(changed.clone()), Err(refusal), "{changed:?}");
        }
        for (reason, refusal) in [
            (PlaceRefusal::Ended, CloseRefusal::Ended),
            (PlaceRefusal::Ambiguous, CloseRefusal::Ambiguous),
            (PlaceRefusal::Place, CloseRefusal::Place),
            (PlaceRefusal::Unknown, CloseRefusal::Place),
        ] {
            let found = SessionPlaceResult::Refused(PlaceRefused {
                reason,
                agent: AgentId::Claude,
                source: SessionSource::Terminal,
            });
            assert_eq!(closable(&found, 102, 102), Err(refusal));
        }
    }

    /// Rewrites a fake process's `stat` with state `state`.
    fn fake_state(fake: &Fake, pid: u32, comm: &str, state: &str, start: u64) {
        let stat = format!(
            "{pid} ({comm}) {state} 1 {pid} {pid} 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 {start} 1000 100 0"
        );
        std::fs::write(fake.roots.proc_root.join(pid.to_string()).join("stat"), stat).unwrap();
    }

    #[test]
    fn a_pid_is_the_agent_only_with_its_start_user_and_name_while_it_runs() {
        let fake = Fake::new();
        let root = &fake.roots.proc_root;
        let me = crate::client::euid();
        fake.proc(102, "claude", 1, 102, &["claude"]);
        assert_eq!(agent_now(root, me, 102, 102), Now::Runs);
        assert_eq!(agent_now(root, me, 102, 103), Now::Other, "another start: the pid was reused");
        assert_eq!(agent_now(root, me.wrapping_add(1), 102, 102), Now::Other, "another user's");
        assert_eq!(agent_now(root, me, 103, 102), Now::Gone);
        for (state, now) in
            [("R", Now::Runs), ("T", Now::Stopped), ("t", Now::Stopped), ("Z", Now::Gone), ("X", Now::Gone)]
        {
            fake_state(&fake, 102, "claude", state, 102);
            assert_eq!(agent_now(root, me, 102, 102), now, "{state}");
        }
        fake_state(&fake, 102, "codex", "S", 102);
        assert_eq!(agent_now(root, me, 102, 102), Now::Runs);
        fake_state(&fake, 102, "bash", "S", 102);
        assert_eq!(agent_now(root, me, 102, 102), Now::Other, "not an agent");
        fake_state(&fake, 102, "claude) S 1 (x", "S", 102);
        assert_eq!(agent_now(root, me, 102, 102), Now::Other, "a `)` in a name can't shift the fields");
        std::fs::write(root.join("102/stat"), "garbage").unwrap();
        assert_eq!(agent_now(root, me, 102, 102), Now::Gone);
    }

    /// A shell of the test's own, named `claude`, ready (its trap set when
    /// `ignore_term`) and waiting on its input; and its start time.
    fn own_claude(dir: &Path, ignore_term: bool) -> (Child, u64) {
        let exe = dir.join("claude");
        std::os::unix::fs::symlink("/bin/sh", &exe).unwrap();
        let script =
            if ignore_term { "trap '' TERM; echo ready; read line" } else { "echo ready; read line" };
        let mut child = Command::new(&exe)
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut ready = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(child.stdout.take().unwrap()), &mut ready)
            .unwrap();
        assert_eq!(ready, "ready\n");
        let start = procs::read_stat(&Path::new(PROC).join(child.id().to_string())).unwrap().start_ticks;
        (child, start)
    }

    fn gone_within(pid: u32, start: u64, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if Pidfd.gone(pid, start) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_process_of_the_tests_own_gets_sigterm_only_while_it_is_the_one_named() {
        let dir = tempfile::tempdir().unwrap();
        let (mut child, start) = own_claude(dir.path(), false);
        let pid = child.id();
        assert_eq!(Pidfd.term(pid, start + 1).unwrap(), Signal::Other);
        assert!(!gone_within(pid, start, SHORT), "another start time: nothing was sent");
        assert!(child.try_wait().unwrap().is_none());
        assert_eq!(Pidfd.term(pid, start).unwrap(), Signal::Sent);
        assert!(gone_within(pid, start, Duration::from_secs(5)), "a zombie is gone");
        assert_eq!(child.wait().unwrap().signal(), Some(15));
        assert!(matches!(Pidfd.term(pid, start).unwrap(), Signal::Gone | Signal::Other));

        // One that ignores SIGTERM is sent it once and stays.
        let dir = tempfile::tempdir().unwrap();
        let (mut child, start) = own_claude(dir.path(), true);
        let pid = child.id();
        assert_eq!(Pidfd.term(pid, start).unwrap(), Signal::Sent);
        assert!(!gone_within(pid, start, SHORT));
        drop(child.stdin.take());
        assert!(child.wait().unwrap().signal().is_none(), "it ended when its input did");
    }

    fn params(pid: u32, start_ticks: u64) -> SessionCloseParams {
        SessionCloseParams { session_id: paths::session_id(AgentId::Claude, U).unwrap(), pid, start_ticks }
    }

    async fn close(
        host: &Arc<Host>,
        kind: ClientKind,
        ender: &Arc<Recorder>,
    ) -> Result<SessionCloseResult, RpcError> {
        host.close_with(kind, params(102, 102), Arc::clone(ender) as Arc<dyn Ender>, SHORT).await
    }

    fn outcome(result: CloseOutcome) -> SessionCloseResult {
        SessionCloseResult { result, reason: None }
    }

    /// A host with an idle Claude in a terminal in an agent folder, ready to continue.
    async fn idle_claude() -> (tempfile::TempDir, Arc<Host>, Fake) {
        let (dir, host, fake) = Fake::hosted();
        host.test_folder(Path::new("/w"));
        agents_ready(&host, true).await;
        claude_in(&fake, "ptyxis", "idle");
        (dir, host, fake)
    }

    #[tokio::test]
    async fn g7_link_closes_an_idle_session_with_one_sigterm() {
        let (_dir, host, _fake) = idle_claude().await;
        let exits = Recorder::new(Signal::Sent, true);
        for kind in [ClientKind::Launcher, ClientKind::Cli, ClientKind::Test, ClientKind::Bridge] {
            let err = close(&host, kind, &exits).await.unwrap_err();
            assert_eq!(err.code, ErrorCode::Denied, "{kind:?}");
        }
        assert!(exits.asked().is_empty());
        assert_eq!(close(&host, ClientKind::Link, &exits).await.unwrap(), outcome(CloseOutcome::Closed));
        assert_eq!(exits.asked(), [(102, 102)]);
        // The method itself: refused to anyone but G7 Link before anything is read.
        let role = crate::host::Role::Client(ClientKind::Test, crate::host::Caps::default());
        let value = serde_json::to_value(params(102, 102)).unwrap();
        let err = host.dispatch(&role, 1, method::SESSION_CLOSE, value).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Denied);
    }

    #[tokio::test]
    async fn a_process_that_stays_is_never_sent_a_second_signal() {
        let (_dir, host, _fake) = idle_claude().await;
        let stays = Recorder::new(Signal::Sent, false);
        assert_eq!(close(&host, ClientKind::Link, &stays).await.unwrap(), outcome(CloseOutcome::Open));
        assert_eq!(close(&host, ClientKind::Link, &stays).await.unwrap(), outcome(CloseOutcome::Open));
        let (a, b) =
            tokio::join!(close(&host, ClientKind::Link, &stays), close(&host, ClientKind::Link, &stays));
        assert_eq!((a.unwrap(), b.unwrap()), (outcome(CloseOutcome::Open), outcome(CloseOutcome::Open)));
        assert_eq!(stays.asked(), [(102, 102)], "one signal, ever");
        // Once it has gone, a later ask says so.
        let gone = Recorder::new(Signal::Sent, true);
        assert_eq!(close(&host, ClientKind::Link, &gone).await.unwrap(), outcome(CloseOutcome::Closed));
        assert!(gone.asked().is_empty());
    }

    #[tokio::test]
    async fn two_g7_links_asking_at_once_send_one_signal() {
        let (_dir, host, _fake) = idle_claude().await;
        let exits = Recorder::new(Signal::Sent, true);
        let (a, b) =
            tokio::join!(close(&host, ClientKind::Link, &exits), close(&host, ClientKind::Link, &exits));
        assert_eq!((a.unwrap(), b.unwrap()), (outcome(CloseOutcome::Closed), outcome(CloseOutcome::Closed)));
        assert_eq!(exits.asked(), [(102, 102)]);
    }

    #[tokio::test]
    async fn a_process_gone_changed_or_stopped_at_the_signal_gets_none() {
        let (_dir, host, _fake) = idle_claude().await;
        for (sent, refusal) in [
            (Signal::Gone, CloseRefusal::Ended),
            (Signal::Other, CloseRefusal::NoProcess),
            (Signal::Stopped, CloseRefusal::Place),
        ] {
            let ender = Recorder::new(sent, true);
            let answer = close(&host, ClientKind::Link, &ender).await.unwrap();
            assert_eq!(answer, refused(refusal), "{sent:?}");
            assert_eq!(ender.asked().len(), 1);
        }
        // None of them counts as sent: a later close may still send one.
        let exits = Recorder::new(Signal::Sent, true);
        assert_eq!(close(&host, ClientKind::Link, &exits).await.unwrap(), outcome(CloseOutcome::Closed));
        assert_eq!(exits.asked().len(), 1);
    }

    #[tokio::test]
    async fn nothing_is_sent_to_a_session_that_isnt_idle_or_couldnt_be_continued() {
        let (_dir, host, fake) = idle_claude().await;
        let ender = Recorder::new(Signal::Sent, true);
        let asked = |host: Arc<Host>, pid: u32, start: u64, ender: Arc<Recorder>| async move {
            host.close_with(ClientKind::Link, params(pid, start), ender as Arc<dyn Ender>, SHORT)
                .await
                .unwrap()
        };
        assert_eq!(asked(host.clone(), 103, 102, ender.clone()).await, refused(CloseRefusal::NoProcess));
        assert_eq!(asked(host.clone(), 102, 101, ender.clone()).await, refused(CloseRefusal::NoProcess));
        for (status, refusal) in [("busy", CloseRefusal::Busy), ("waiting", CloseRefusal::NeedsYou)] {
            fake.status(102, 102, U, status, "cli");
            assert_eq!(close(&host, ClientKind::Link, &ender).await.unwrap(), refused(refusal), "{status}");
        }
        // Claude's state from its transcript alone.
        fake.status(102, 102, U, "compacting", "cli");
        assert_eq!(close(&host, ClientKind::Link, &ender).await.unwrap(), refused(CloseRefusal::Place));
        fake.status(102, 102, U, "idle", "cli");
        // It couldn't be continued afterwards.
        agents_ready(&host, false).await;
        assert_eq!(close(&host, ClientKind::Link, &ender).await.unwrap(), refused(CloseRefusal::Agent));
        let (_dir2, other, fake2) = Fake::hosted();
        agents_ready(&other, true).await;
        claude_in(&fake2, "ptyxis", "idle");
        assert_eq!(close(&other, ClientKind::Link, &ender).await.unwrap(), refused(CloseRefusal::Folder));
        // Resumed twice; then no process at all.
        agents_ready(&host, true).await;
        fake.proc(202, "claude", 101, 202, &["claude", "-r", U]);
        assert_eq!(close(&host, ClientKind::Link, &ender).await.unwrap(), refused(CloseRefusal::Ambiguous));
        std::fs::remove_dir_all(fake.roots.proc_root.join("202")).unwrap();
        std::fs::remove_dir_all(fake.roots.proc_root.join("102")).unwrap();
        assert_eq!(close(&host, ClientKind::Link, &ender).await.unwrap(), refused(CloseRefusal::Ended));
        assert!(ender.asked().is_empty(), "nothing was sent");
    }

    #[tokio::test]
    async fn the_claude_app_and_a_codex_app_session_are_never_closed() {
        let (_dir, host, fake) = Fake::hosted();
        host.test_folder(Path::new("/w"));
        agents_ready(&host, true).await;
        fake.terminal("claude-desktop", "claude", &["claude"]);
        fake.status(102, 102, U, "idle", "claude-desktop");
        fake.transcript(U, &[("user", "Hello there"), ("assistant", "Hi")]);
        let ender = Recorder::new(Signal::Sent, true);
        assert_eq!(close(&host, ClientKind::Link, &ender).await.unwrap(), refused(CloseRefusal::Place));

        let (_dir, host, fake) = Fake::hosted();
        host.test_folder(Path::new("/w"));
        agents_ready(&host, true).await;
        fake.terminal("ptyxis", "codex", &["codex", "resume", U]);
        fake.listed_rollout(U, "Codex Desktop", true);
        let id = paths::session_id(AgentId::Codex, U).unwrap();
        let p = SessionCloseParams { session_id: id, pid: 102, start_ticks: 102 };
        let answer = host.close_with(ClientKind::Link, p, Arc::clone(&ender) as Arc<dyn Ender>, SHORT).await;
        assert_eq!(answer.unwrap(), refused(CloseRefusal::Place));
        assert!(ender.asked().is_empty());
    }
}
