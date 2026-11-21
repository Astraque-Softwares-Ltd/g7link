// SPDX-License-Identifier: MPL-2.0
//! `session.place` (reply on the PC, design §1.2 with the spike results of
//! §9.1): the one process a followed session runs in, the processes above it
//! with the terminals under each, and a few runs of its newest text, so that
//! G7 Link can find the session's window and prove it before it pastes.
//! Everything is read again on every call.
//!
//! - Claude: the process whose live status file names the session, else one
//!   whose command line resumes it without forking it.
//! - Codex: only a TUI whose command line resumes it (`codex resume <uuid>`).
//!   A TUI holds no session file open and the daemon that does is shared
//!   (spike S4), so any other open Codex session is refused `place`.
//! - Sessions in the Claude or Codex app are refused `place` (spike S2).
//! - Claude's working and waiting states come from its status file only,
//!   never from the transcript (spike S5); without one the state is a guess.
//!
//! Reads `stat`, `cmdline`, `comm` and the `exe` link of processes, Claude's
//! status files and the session's own file, as the session list does; never
//! `environ`. Blocking: called from `spawn_blocking`.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::paths::Roots;
use super::procs::{self, Comm, Proc, Scan, Status};
use super::{Ctx, MAX_LOCKS, Tail, claude_source, codex_open, is_wrapper, not_found, tail};
use crate::api::*;
use crate::host::Host;
use crate::host::store::now_ms;

/// Text items the marks are taken from, newest first.
const MARK_ITEMS: usize = 3;
/// A run tells texts apart only with this many different characters.
const MARK_VARIETY: usize = 8;

impl Host {
    /// `session.place`. `NOT_FOUND` for a session that isn't listed or
    /// whose file can't be read.
    pub async fn session_place(self: &Arc<Self>, session_id: &str) -> Result<SessionPlaceResult, RpcError> {
        let listed = self.listed(session_id).await.ok_or_else(not_found)?;
        let roots = self.sessions.roots.clone();
        let (euid, host_pid) = (crate::client::euid(), std::process::id());
        tokio::task::spawn_blocking(move || find(&roots, euid, host_pid, &listed.ctx, now_ms()))
            .await
            .map_err(|_| RpcError::new(ErrorCode::Internal, "The check failed."))?
    }
}

/// Where the session `ctx` runs now.
pub(super) fn find(
    roots: &Roots,
    euid: u32,
    host_pid: u32,
    ctx: &Ctx,
    now: u64,
) -> Result<SessionPlaceResult, RpcError> {
    find_with(roots, euid, host_pid, ctx, now, &procs::owner)
}

/// [`find`], with who owns a `/proc` entry decided by `owner`.
fn find_with(
    roots: &Roots,
    euid: u32,
    host_pid: u32,
    ctx: &Ctx,
    now: u64,
    owner: &dyn Fn(&Path) -> Option<u32>,
) -> Result<SessionPlaceResult, RpcError> {
    let refused =
        |reason, source| Ok(SessionPlaceResult::Refused(PlaceRefused { reason, agent: ctx.agent, source }));
    let scan = procs::scan(&roots.proc_root, euid, host_pid);
    let statuses = match ctx.agent {
        AgentId::Claude => procs::status_list(roots, &scan),
        AgentId::Codex => Vec::new(),
    };
    let agent = match holders(&scan, &statuses, ctx).as_slice() {
        [] => {
            let open = ctx.agent == AgentId::Codex && codex_still_open(roots, &scan, ctx, now);
            return refused(if open { PlaceRefusal::Place } else { PlaceRefusal::Ended }, ctx.source);
        }
        [one] => *one,
        _ => return refused(PlaceRefusal::Ambiguous, ctx.source),
    };
    let status = statuses.iter().find(|s| s.live && s.pid == agent.pid && s.uuid == ctx.uuid);
    let source = match ctx.agent {
        AgentId::Claude => {
            claude_source(status, agent.exe.as_deref(), agent.names_claude_session(&ctx.uuid), roots)
        }
        // Its command line resumes the session: a TUI, wherever the session began.
        AgentId::Codex => SessionSource::Terminal,
    };
    if source == SessionSource::DesktopApp {
        return refused(PlaceRefusal::Place, source);
    }
    let Newest { reader, items, mtime_ms, turn_unseen } = read_newest(ctx).map_err(|_| not_found())?;
    let (state, state_exact) = match status.and_then(|s| status_state(s.status.as_deref())) {
        Some(state) => (state, true),
        // A turn longer than the window: the reader saw neither its start nor its end.
        None if turn_unseen => (SessionState::Working, false),
        None => (reader.guess(now.saturating_sub(mtime_ms)), false),
    };
    let tree = procs::tree(&roots.proc_root);
    Ok(SessionPlaceResult::Place(SessionPlace {
        agent: ctx.agent,
        source,
        state,
        state_exact,
        pid: agent.pid,
        start_ticks: agent.start_ticks,
        pgrp: agent.pgrp,
        tty: agent.tty(),
        foreground: agent.foreground(),
        chain: chain(&roots.proc_root, euid, agent, &tree, owner),
        marks: marks(newest_texts(&items)),
    }))
}

/// What a status file's `status` says, when it is a word this version knows.
fn status_state(status: Option<&str>) -> Option<SessionState> {
    match status? {
        "busy" => Some(SessionState::Working),
        "waiting" => Some(SessionState::NeedsYou),
        "idle" => Some(SessionState::Idle),
        _ => None,
    }
}

/// The processes that have the session, by pid; of an agent started by
/// another that names the session too (a launcher script), the inner one.
fn holders<'a>(scan: &'a Scan, statuses: &[Status], ctx: &Ctx) -> Vec<&'a Proc> {
    let uuid = ctx.uuid.as_str();
    let mut pids: HashSet<u32> = HashSet::new();
    match ctx.agent {
        AgentId::Claude => {
            pids.extend(statuses.iter().filter(|s| s.live && s.uuid == uuid).map(|s| s.pid));
            // A command line says only what the process started on: one whose
            // live status file names another session has moved on (`/resume`).
            let moved: HashSet<u32> =
                statuses.iter().filter(|s| s.live && s.uuid != uuid).map(|s| s.pid).collect();
            pids.extend(
                scan.procs
                    .values()
                    .filter(|p| p.comm == Comm::Claude && p.names_claude_session(uuid))
                    .filter(|p| !p.forks_session() && !moved.contains(&p.pid))
                    .map(|p| p.pid),
            );
        }
        AgentId::Codex => {
            pids.extend(scan.procs.values().filter(|p| p.resumes_codex_session(uuid)).map(|p| p.pid));
        }
    }
    let wraps = |outer: u32| pids.iter().any(|&inner| inner != outer && is_above(scan, outer, inner));
    let mut out: Vec<&Proc> =
        pids.iter().filter(|&&pid| !wraps(pid)).filter_map(|p| scan.procs.get(p)).collect();
    out.sort_by_key(|p| p.pid);
    out
}

/// `outer` is an ancestor of `inner` through agent processes.
fn is_above(scan: &Scan, outer: u32, inner: u32) -> bool {
    let mut at = inner;
    for _ in 0..MAX_PLACE_CHAIN {
        let Some(p) = scan.procs.get(&at) else { return false };
        if p.ppid == outer {
            return true;
        }
        at = p.ppid;
    }
    false
}

/// A Codex session no TUI names is still open by the list's other signs (a
/// write lock, a recent write, a Codex in its folder): it runs somewhere G7
/// can't tell.
fn codex_still_open(roots: &Roots, scan: &Scan, ctx: &Ctx, now: u64) -> bool {
    let locks: Vec<String> =
        tail::list_dir(&roots.codex_locks(), MAX_LOCKS).into_iter().map(|(name, _)| name).collect();
    let mtime = tail::stamp(&ctx.path).map_or(0, |s| s.mtime_ms());
    codex_open(scan, &ctx.uuid, &locks, now, mtime, Some(&ctx.cwd), None)
}

/// What the session file's newest window holds.
struct Newest {
    reader: Tail,
    /// Its items by `seq`.
    items: BTreeMap<u64, Item>,
    mtime_ms: u64,
    /// A Codex rollout whose window starts after the file's start and holds
    /// no turn marker: a turn may be running that the reader can't see, and
    /// its guess (idle) is not to be trusted.
    turn_unseen: bool,
}

fn read_newest(ctx: &Ctx) -> std::io::Result<Newest> {
    let window = tail::read_window(&ctx.path, None, 0)?;
    let mtime_ms = window.stamp.mtime_ms();
    let mut reader = Tail::new(ctx.agent, Some(PathBuf::from(&ctx.cwd)));
    let mut items = BTreeMap::new();
    for line in &window.lines {
        for item in reader.feed(line, mtime_ms) {
            items.insert(item.seq, item);
        }
    }
    let turn_unseen = ctx.agent == AgentId::Codex
        && window.more_before()
        && !window.lines.iter().any(|line| is_turn_marker(&line.bytes));
    Ok(Newest { reader, items, mtime_ms, turn_unseen })
}

/// A rollout line that starts or ends a Codex turn.
fn is_turn_marker(line: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else { return false };
    v.get("type").and_then(serde_json::Value::as_str) == Some("event_msg")
        && matches!(
            v.get("payload").and_then(|p| p.get("type")).and_then(serde_json::Value::as_str),
            Some("task_started" | "task_complete" | "turn_aborted")
        )
}

/// The processes above `agent`, parent first: see [`SessionPlace::chain`].
fn chain(
    proc_root: &Path,
    euid: u32,
    agent: &Proc,
    tree: &procs::Tree,
    owner: &dyn Fn(&Path) -> Option<u32>,
) -> Vec<Ancestor> {
    let mut chain = Vec::new();
    let mut seen = HashSet::from([agent.pid]);
    let mut below_start = agent.start_ticks;
    let mut next = agent.ppid;
    while chain.len() < MAX_PLACE_CHAIN && next > 1 && seen.insert(next) {
        let dir = proc_root.join(next.to_string());
        if owner(&dir) != Some(euid) {
            break;
        }
        let Some(stat) = procs::read_stat(&dir) else { break };
        // A parent can't start after its child: the pid was reused.
        if stat.start_ticks > below_start {
            break;
        }
        chain.push(Ancestor {
            pid: next,
            start_ticks: stat.start_ticks,
            comm: clean_cut(&stat.comm, MAX_PLACE_NAME).0,
            exe_name: tail::read_link(&dir.join("exe")).as_deref().and_then(exe_name),
            ptys_under: tree.complete.then(|| tree.ttys_under(next)),
        });
        below_start = stat.start_ticks;
        next = stat.ppid;
    }
    chain
}

/// A program's file name, never its path, without the ` (deleted)` a
/// program replaced since it started has.
fn exe_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    let name = clean_cut(name.strip_suffix(" (deleted)").unwrap_or(&name), MAX_PLACE_NAME).0;
    (!name.is_empty()).then_some(name)
}

/// The session's newest user and assistant texts, newest first.
fn newest_texts(items: &BTreeMap<u64, Item>) -> impl Iterator<Item = &str> {
    items
        .values()
        .rev()
        .filter(|i| matches!(i.kind, ItemKind::User | ItemKind::Assistant))
        .filter_map(|i| i.text.as_deref())
        .filter(|t| !t.trim().is_empty() && !is_wrapper(t))
        .take(MARK_ITEMS)
}

/// Up to [`MAX_PLACE_MARKS`] runs of [`PLACE_MARK_CHARS`] characters from
/// `texts` (newest first), whitespace removed, each from the end of a piece
/// of text a terminal shows as written: see [`pieces`]. Runs of mostly
/// punctuation, or of few different characters, are skipped.
pub(crate) fn marks<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for text in texts {
        for piece in pieces(text).iter().rev() {
            let mut end = piece.len();
            while end >= PLACE_MARK_CHARS {
                let run = &piece[end - PLACE_MARK_CHARS..end];
                end -= PLACE_MARK_CHARS;
                if !strong(run) {
                    continue;
                }
                let run: String = run.iter().collect();
                if !out.contains(&run) {
                    out.push(run);
                }
                if out.len() == MAX_PLACE_MARKS {
                    return out;
                }
            }
        }
    }
    out
}

fn strong(run: &[char]) -> bool {
    let alphanumeric = run.iter().filter(|c| c.is_alphanumeric()).count();
    alphanumeric * 2 >= run.len() && run.iter().collect::<HashSet<_>>().len() >= MARK_VARIETY
}

/// A character a terminal may draw as something else (markdown's marks):
/// a piece never holds one.
fn breaks_piece(c: char) -> bool {
    c.is_control()
        || matches!(c, '*' | '_' | '`' | '#' | '<' | '>' | '|' | '~' | '[' | ']' | '(' | ')' | '\\')
}

/// A line that starts a block the terminal draws its own way (a heading, a
/// quote, a table row, a code fence, a list item): its text without the
/// list marker.
fn block_line(line: &str) -> Option<&str> {
    if line.starts_with(['#', '>', '|', '`', '~']) {
        return Some(line);
    }
    if let Some(rest) = ["- ", "* ", "+ "].iter().find_map(|m| line.strip_prefix(m)) {
        return Some(rest);
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        return line[digits..].strip_prefix(". ").or_else(|| line[digits..].strip_prefix(") "));
    }
    None
}

/// `text` without whitespace, in pieces a run may come from: lines of one
/// paragraph join (wrapping only moves whitespace), while a markdown mark, a
/// block's line, or a line without letters or digits (a rule, a fence)
/// ends a piece.
fn pieces(text: &str) -> Vec<Vec<char>> {
    fn flush(piece: &mut Vec<char>, out: &mut Vec<Vec<char>>) {
        if !piece.is_empty() {
            out.push(std::mem::take(piece));
        }
    }
    let mut out = Vec::new();
    let mut piece = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        if !line.chars().any(char::is_alphanumeric) {
            flush(&mut piece, &mut out);
            continue;
        }
        let block = block_line(line);
        if block.is_some() {
            flush(&mut piece, &mut out);
        }
        for c in block.unwrap_or(line).chars().filter(|c| !c.is_whitespace()) {
            if breaks_piece(c) {
                flush(&mut piece, &mut out);
            } else {
                piece.push(c);
            }
        }
        if block.is_some() {
            flush(&mut piece, &mut out);
        }
    }
    flush(&mut piece, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use serde_json::json;

    use super::*;
    use crate::host::sessions::paths;
    use crate::host::sessions::procs::tests::{fake_proc, fake_terminal};
    use crate::host::{Caps, Role, test_host};

    const U: &str = "11111111-2222-4333-8444-555555555555";
    const U2: &str = "22222222-3333-4444-8555-666666666666";
    /// A host pid no fake process has as its parent.
    const NOT_PARENT: u32 = 999_999;
    const PTS0: u32 = 34816;
    const LONG_ASK: &str = "Please find out why the integration tests time out on the second run only";
    const LONG_ANSWER: &str = "The second run reuses the socket the first one left behind, so every client waits for a host that already exited";

    fn euid() -> u32 {
        crate::client::euid()
    }

    struct Fake {
        dir: tempfile::TempDir,
        roots: Roots,
    }

    impl Fake {
        fn new() -> Fake {
            let dir = tempfile::tempdir().unwrap();
            let roots = Roots {
                claude_home: dir.path().join("claude"),
                codex_home: dir.path().join("codex"),
                proc_root: dir.path().join("proc"),
                claude_app_dir: Some(dir.path().join("config/Claude")),
            };
            std::fs::create_dir_all(&roots.proc_root).unwrap();
            std::fs::write(roots.proc_root.join("stat"), "cpu 1 2 3\nbtime 1795000000\n").unwrap();
            Fake { dir, roots }
        }

        fn proc(&self, pid: u32, comm: &str, ppid: u32, start: u64, argv: &[&str]) {
            fake_proc(&self.roots.proc_root, pid, comm, ppid, start, argv, Path::new("/w"));
        }

        fn term(&self, pid: u32, pgrp: u32, tty: u32, tpgid: i32) {
            fake_terminal(&self.roots.proc_root, pid, pgrp, tty, tpgid);
        }

        fn exe(&self, pid: u32, target: &str) {
            let link = self.roots.proc_root.join(pid.to_string()).join("exe");
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(target, link).unwrap();
        }

        /// systemd --user (50) above `emulator` (100), above a shell (101) on
        /// pts/0, running `agent` (102) as the terminal's foreground job.
        fn terminal(&self, emulator: &str, agent: &str, argv: &[&str]) {
            self.proc(50, "systemd", 1, 10, &["/usr/lib/systemd/systemd", "--user"]);
            self.proc(100, emulator, 50, 100, &[emulator]);
            self.proc(101, "bash", 100, 101, &["bash"]);
            self.proc(102, agent, 101, 102, argv);
            self.term(101, 101, PTS0, 102);
            self.term(102, 102, PTS0, 102);
        }

        fn status(&self, pid: u32, start: u64, uuid: &str, status: &str, entrypoint: &str) {
            let dir = self.roots.claude_status_dir();
            std::fs::create_dir_all(&dir).unwrap();
            let file = json!({"pid": pid, "sessionId": uuid, "cwd": "/w", "procStart": start.to_string(),
                              "entrypoint": entrypoint, "status": status, "updatedAt": 5});
            std::fs::write(dir.join(format!("{pid}.json")), file.to_string()).unwrap();
        }

        /// A Claude transcript of `(role, text)` turns; an assistant text ends its turn.
        fn transcript(&self, uuid: &str, turns: &[(&str, &str)]) -> PathBuf {
            let dir = self.roots.claude_projects().join("-w");
            std::fs::create_dir_all(&dir).unwrap();
            let lines: Vec<String> = turns
                .iter()
                .enumerate()
                .map(|(n, (role, text))| {
                    let common = |kind: &str| {
                        json!({"type": kind, "uuid": format!("{kind}-{n}"), "sessionId": uuid, "cwd": "/w",
                               "timestamp": "2026-11-21T09:00:00.000Z", "isSidechain": false})
                    };
                    let mut line = common(role);
                    line["message"] = if *role == "user" {
                        json!({"role": "user", "content": text})
                    } else {
                        json!({"id": format!("m{n}"), "role": "assistant", "content": [{"type": "text", "text": text}],
                               "stop_reason": "end_turn"})
                    };
                    line.to_string()
                })
                .collect();
            let path = dir.join(format!("{uuid}.jsonl"));
            std::fs::write(&path, lines.join("\n") + "\n").unwrap();
            path
        }

        /// A Codex rollout with one exchange, its turn done or not.
        fn rollout(&self, uuid: &str, originator: &str, done: bool) -> PathBuf {
            let mut lines = vec![
                json!({"timestamp": "2026-11-21T09:00:00Z", "type": "session_meta", "payload": {"id": uuid, "cwd": "/w", "originator": originator}}),
                json!({"timestamp": "2026-11-21T09:00:01Z", "type": "event_msg", "payload": {"type": "task_started", "turn_id": "t"}}),
                json!({"timestamp": "2026-11-21T09:00:01Z", "type": "event_msg", "payload": {"type": "item_completed", "item": {"type": "UserMessage", "id": "u1", "content": [{"type": "text", "text": LONG_ASK}]}}}),
            ];
            if done {
                lines.push(json!({"timestamp": "2026-11-21T09:00:02Z", "type": "event_msg", "payload": {"type": "item_completed", "item": {"type": "AgentMessage", "id": "a1", "phase": "final_answer", "content": [{"type": "Text", "text": LONG_ANSWER}]}}}));
                lines.push(json!({"timestamp": "2026-11-21T09:00:03Z", "type": "event_msg", "payload": {"type": "task_complete", "turn_id": "t", "last_agent_message": LONG_ANSWER}}));
            }
            let path = self.dir.path().join(format!("rollout-2026-11-21T09-00-00-{uuid}.jsonl"));
            let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
            std::fs::write(&path, body).unwrap();
            path
        }

        fn find(&self, ctx: &Ctx) -> SessionPlaceResult {
            find(&self.roots, euid(), NOT_PARENT, ctx, now_ms()).unwrap()
        }

        fn place(&self, ctx: &Ctx) -> SessionPlace {
            match self.find(ctx) {
                SessionPlaceResult::Place(place) => place,
                refused => panic!("refused: {refused:?}"),
            }
        }

        fn refused(&self, ctx: &Ctx) -> PlaceRefused {
            match self.find(ctx) {
                SessionPlaceResult::Refused(refused) => refused,
                place => panic!("placed: {place:?}"),
            }
        }
    }

    fn ctx(agent: AgentId, uuid: &str, path: PathBuf, source: SessionSource) -> Ctx {
        Ctx {
            agent,
            uuid: uuid.into(),
            path,
            open: true,
            live: None,
            title_hint: None,
            cwd: "/w".into(),
            source,
            folder_id: None,
            folder_label: "w".into(),
            blocked: None,
        }
    }

    /// A Claude session in a terminal, its status file saying `status`.
    fn claude_in(fake: &Fake, emulator: &str, status: &str) -> Ctx {
        fake.terminal(emulator, "claude", &["claude"]);
        fake.status(102, 102, U, status, "cli");
        let path = fake.transcript(U, &[("user", LONG_ASK), ("assistant", LONG_ANSWER)]);
        ctx(AgentId::Claude, U, path, SessionSource::Terminal)
    }

    fn comms(place: &SessionPlace) -> Vec<&str> {
        place.chain.iter().map(|a| a.comm.as_str()).collect()
    }

    fn ptys(place: &SessionPlace) -> Vec<Option<u32>> {
        place.chain.iter().map(|a| a.ptys_under).collect()
    }

    #[test]
    fn alacritty_one_window_one_terminal() {
        let fake = Fake::new();
        let c = claude_in(&fake, "alacritty", "idle");
        let place = fake.place(&c);
        assert_eq!((place.agent, place.source), (AgentId::Claude, SessionSource::Terminal));
        assert_eq!((place.pid, place.start_ticks, place.pgrp), (102, 102, 102));
        assert_eq!((place.state, place.state_exact), (SessionState::Idle, true));
        assert_eq!(place.tty, Some(PTS0));
        assert!(place.foreground);
        assert_eq!(comms(&place), ["bash", "alacritty", "systemd"], "parent first; pid 1 is not listed");
        assert_eq!(place.chain[1].exe_name.as_deref(), Some("alacritty"));
        assert_eq!(place.chain[1].pid, 100);
        assert_eq!(place.chain[1].start_ticks, 100);
        assert_eq!(ptys(&place), [Some(1), Some(1), Some(1)]);
        assert!(!place.marks.is_empty());
    }

    #[test]
    fn gnome_terminal_with_one_tab_and_with_three() {
        let fake = Fake::new();
        let c = claude_in(&fake, "gnome-terminal-", "idle");
        fake.exe(100, "/usr/libexec/gnome-terminal-server");
        let one = fake.place(&c);
        assert_eq!(one.chain[1].exe_name.as_deref(), Some("gnome-terminal-server"));
        assert_eq!(one.chain[1].ptys_under, Some(1));
        for (pid, tty) in [(103, PTS0 + 1), (104, PTS0 + 2)] {
            fake.proc(pid, "bash", 100, u64::from(pid), &["bash"]);
            fake.term(pid, pid, tty, i32::try_from(pid).unwrap());
        }
        let three = fake.place(&c);
        assert_eq!(three.chain[1].ptys_under, Some(3), "three tabs: no structural proof");
        assert_eq!(three.chain[0].ptys_under, Some(1), "the agent's shell has one");
    }

    #[test]
    fn ptyxis_through_its_agent_and_kitty_with_two_shells() {
        let fake = Fake::new();
        fake.proc(50, "systemd", 1, 10, &["systemd", "--user"]);
        fake.proc(100, "ptyxis", 50, 100, &["ptyxis"]);
        fake.proc(110, "ptyxis-agent", 100, 110, &["ptyxis-agent"]);
        fake.proc(111, "bash", 110, 111, &["bash"]);
        fake.proc(112, "codex", 111, 112, &["codex", "resume", U]);
        fake.term(111, 111, PTS0, 112);
        fake.term(112, 112, PTS0, 112);
        let path = fake.rollout(U, "codex-tui", true);
        let c = ctx(AgentId::Codex, U, path, SessionSource::Terminal);
        let place = fake.place(&c);
        assert_eq!(comms(&place), ["bash", "ptyxis-agent", "ptyxis", "systemd"]);
        assert_eq!(ptys(&place), [Some(1), Some(1), Some(1), Some(1)]);

        let fake = Fake::new();
        let c = claude_in(&fake, "kitty", "idle");
        fake.proc(103, "zsh", 100, 103, &["zsh"]);
        fake.term(103, 103, PTS0 + 1, 103);
        let place = fake.place(&c);
        assert_eq!(place.chain[1].comm, "kitty");
        assert_eq!(place.chain[1].ptys_under, Some(2));
    }

    #[test]
    fn multiplexers_ssh_and_vs_code_show_in_the_chain() {
        for (name, exe) in [
            ("tmux: server", "/usr/bin/tmux"),
            ("zellij", "/usr/bin/zellij"),
            ("screen", "/usr/bin/screen"),
            ("code", "/usr/share/code/code"),
            ("sshd", "/usr/sbin/sshd"),
        ] {
            let fake = Fake::new();
            let c = claude_in(&fake, name, "idle");
            fake.exe(100, exe);
            let place = fake.place(&c);
            assert_eq!(place.chain[1].comm, name, "the host refuses none of them; G7 Link does");
            assert_eq!(
                place.chain[1].exe_name.as_deref(),
                Some(Path::new(exe).file_name().unwrap().to_str().unwrap())
            );
        }
    }

    #[test]
    fn the_chain_stops_at_pid_1_another_user_and_a_reused_pid() {
        let fake = Fake::new();
        let c = claude_in(&fake, "alacritty", "idle");
        // The emulator's parent is pid 1.
        fake.proc(100, "alacritty", 1, 100, &["alacritty"]);
        assert_eq!(comms(&fake.place(&c)), ["bash", "alacritty"]);

        // Another user's process: neither it nor anything above is listed.
        let other = fake.roots.proc_root.join("100");
        let owner = |dir: &Path| if dir == other { Some(euid().wrapping_add(1)) } else { procs::owner(dir) };
        let SessionPlaceResult::Place(place) =
            find_with(&fake.roots, euid(), NOT_PARENT, &c, now_ms(), &owner).unwrap()
        else {
            panic!("refused")
        };
        assert_eq!(comms(&place), ["bash"]);

        // The emulator's pid now names a process that started after the shell.
        fake.proc(100, "alacritty", 50, 500, &["alacritty"]);
        assert_eq!(comms(&fake.place(&c)), ["bash"], "a parent can't start after its child");

        // A loop in the parents ends.
        fake.proc(100, "alacritty", 101, 100, &["alacritty"]);
        assert_eq!(comms(&fake.place(&c)), ["bash", "alacritty"]);
    }

    #[test]
    fn the_chain_is_capped_at_32() {
        let fake = Fake::new();
        fake.proc(200, "kitty", 1, 200, &["kitty"]);
        for pid in 201..=240u32 {
            fake.proc(pid, "bash", pid - 1, u64::from(pid), &["bash"]);
        }
        fake.proc(300, "claude", 240, 300, &["claude"]);
        fake.term(300, 300, PTS0, 300);
        fake.status(300, 300, U, "idle", "cli");
        let path = fake.transcript(U, &[("user", LONG_ASK)]);
        let place = fake.place(&ctx(AgentId::Claude, U, path, SessionSource::Terminal));
        assert_eq!(place.chain.len(), MAX_PLACE_CHAIN);
        assert_eq!(place.chain[0].pid, 240);
        assert_eq!(place.chain[31].pid, 209, "never reaches the window's process");
    }

    #[test]
    fn a_count_from_part_of_the_processes_is_left_out() {
        let fake = Fake::new();
        fake.terminal("alacritty", "claude", &["claude"]);
        let scan = procs::scan(&fake.roots.proc_root, euid(), NOT_PARENT);
        let agent = &scan.procs[&102];
        let full = procs::tree(&fake.roots.proc_root);
        let partial = procs::Tree::new([(101, 100, PTS0), (102, 101, PTS0)], false);
        let counts = |tree: &procs::Tree| -> Vec<Option<u32>> {
            chain(&fake.roots.proc_root, euid(), agent, tree, &procs::owner)
                .iter()
                .map(|a| a.ptys_under)
                .collect()
        };
        assert_eq!(counts(&full), [Some(1), Some(1), Some(1)]);
        assert_eq!(counts(&partial), [None, None, None]);
    }

    #[test]
    fn a_replaced_program_keeps_its_name() {
        assert_eq!(exe_name(Path::new("/usr/bin/ptyxis (deleted)")).as_deref(), Some("ptyxis"));
        assert_eq!(exe_name(Path::new("/opt/x/kitty")).as_deref(), Some("kitty"));
        assert_eq!(exe_name(Path::new("/")), None);
        let long = format!("/bin/{}", "k".repeat(100));
        assert_eq!(exe_name(Path::new(&long)).unwrap().len(), MAX_PLACE_NAME);
    }

    #[test]
    fn not_in_the_foreground_when_suspended_or_without_a_terminal() {
        let fake = Fake::new();
        let c = claude_in(&fake, "alacritty", "idle");
        // Suspended: the shell's group has the terminal.
        fake.term(101, 101, PTS0, 101);
        fake.term(102, 102, PTS0, 101);
        let place = fake.place(&c);
        assert_eq!(place.tty, Some(PTS0));
        assert!(!place.foreground, "what is typed would reach the shell");
        // No terminal at all.
        fake.term(102, 102, 0, -1);
        let place = fake.place(&c);
        assert_eq!(place.tty, None);
        assert!(!place.foreground);
    }

    #[test]
    fn claude_states_come_from_its_status_file_only() {
        for (status, state) in [
            ("busy", SessionState::Working),
            ("waiting", SessionState::NeedsYou),
            ("idle", SessionState::Idle),
        ] {
            let fake = Fake::new();
            let place = fake.place(&claude_in(&fake, "alacritty", status));
            assert_eq!((place.state, place.state_exact), (state, true), "{status}");
        }
        // A word this version doesn't know: the transcript's guess, not exact.
        let fake = Fake::new();
        let place = fake.place(&claude_in(&fake, "alacritty", "compacting"));
        assert_eq!((place.state, place.state_exact), (SessionState::Idle, false));

        // No status file: found by its command line, the state guessed.
        let fake = Fake::new();
        fake.terminal("alacritty", "claude", &["claude", "--resume", U]);
        let path = fake.transcript(U, &[("assistant", LONG_ANSWER), ("user", LONG_ASK)]);
        let place = fake.place(&ctx(AgentId::Claude, U, path, SessionSource::Terminal));
        assert_eq!(place.pid, 102);
        assert_eq!((place.state, place.state_exact), (SessionState::Working, false), "mid-turn by its file");
    }

    #[test]
    fn claude_no_process_two_processes_and_the_app() {
        // The status file's process start doesn't match: the pid was reused.
        let fake = Fake::new();
        fake.terminal("alacritty", "claude", &["claude"]);
        fake.status(102, 999, U, "idle", "cli");
        let path = fake.transcript(U, &[("user", LONG_ASK)]);
        let c = ctx(AgentId::Claude, U, path, SessionSource::Terminal);
        let refused = fake.refused(&c);
        assert_eq!((refused.reason, refused.agent), (PlaceRefusal::Ended, AgentId::Claude));

        // Resumed twice.
        fake.status(102, 102, U, "idle", "cli");
        fake.proc(202, "claude", 101, 202, &["claude", "-r", U]);
        assert_eq!(fake.refused(&c).reason, PlaceRefusal::Ambiguous);

        // In the Claude app (S2): refused, wherever its window is.
        let fake = Fake::new();
        fake.terminal("claude-desktop", "claude", &["claude"]);
        fake.status(102, 102, U, "idle", "claude-desktop");
        let path = fake.transcript(U, &[("user", LONG_ASK)]);
        let refused = fake.refused(&ctx(AgentId::Claude, U, path, SessionSource::DesktopApp));
        assert_eq!((refused.reason, refused.source), (PlaceRefusal::Place, SessionSource::DesktopApp));
    }

    #[test]
    fn a_claude_command_line_that_no_longer_runs_the_session() {
        let fake = Fake::new();
        // Forked: a new session, not the one it resumed.
        fake.terminal("alacritty", "claude", &["claude", "--resume", U, "--fork-session"]);
        let path = fake.transcript(U, &[("user", LONG_ASK)]);
        let c = ctx(AgentId::Claude, U, path, SessionSource::Terminal);
        assert_eq!(fake.refused(&c).reason, PlaceRefusal::Ended);
        // Moved on with `/resume`: its live status file names another session.
        fake.proc(102, "claude", 101, 102, &["claude", "--resume", U]);
        fake.term(102, 102, PTS0, 102);
        fake.status(102, 102, U2, "idle", "cli");
        assert_eq!(fake.refused(&c).reason, PlaceRefusal::Ended);
        let other = fake.transcript(U2, &[("user", LONG_ASK)]);
        assert_eq!(fake.place(&ctx(AgentId::Claude, U2, other, SessionSource::Terminal)).pid, 102);
    }

    #[test]
    fn codex_only_by_resume_on_its_command_line() {
        // `codex resume <uuid>`: placed, idle once its turn is done, a guess.
        let fake = Fake::new();
        fake.terminal("ptyxis", "codex", &["codex", "resume", U]);
        let path = fake.rollout(U, "codex-tui", true);
        let c = ctx(AgentId::Codex, U, path, SessionSource::Terminal);
        let place = fake.place(&c);
        assert_eq!((place.agent, place.source, place.pid), (AgentId::Codex, SessionSource::Terminal, 102));
        assert_eq!((place.state, place.state_exact), (SessionState::Idle, false));
        assert!(
            place
                .marks
                .iter()
                .any(|m| LONG_ANSWER.split_whitespace().collect::<String>().contains(m.as_str()))
        );
        // Mid-turn.
        fake.rollout(U, "codex-tui", false);
        assert_eq!(fake.place(&c).state, SessionState::Working);

        // A Codex in the session's folder that doesn't name it (S4): open,
        // but no proof which process has it.
        let fake = Fake::new();
        fake.terminal("ptyxis", "codex", &["codex"]);
        let path = fake.rollout(U, "codex-tui", true);
        let c = ctx(AgentId::Codex, U, path.clone(), SessionSource::Terminal);
        let refused = fake.refused(&c);
        assert_eq!((refused.reason, refused.agent), (PlaceRefusal::Place, AgentId::Codex));
        // Nothing has it and nothing wrote it lately: ended.
        std::fs::remove_dir_all(fake.roots.proc_root.join("102")).unwrap();
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_000_000);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(old).unwrap();
        assert_eq!(fake.refused(&c).reason, PlaceRefusal::Ended);

        // The headless `codex exec resume` and a fork are not it.
        for argv in [["codex", "exec", "resume", U], ["codex", "fork", U, "--yolo"]] {
            let fake = Fake::new();
            fake.terminal("ptyxis", "codex", &argv);
            let path = fake.rollout(U, "codex-tui", true);
            assert_eq!(
                fake.refused(&ctx(AgentId::Codex, U, path, SessionSource::Terminal)).reason,
                PlaceRefusal::Place
            );
        }
    }

    #[test]
    fn a_codex_turn_longer_than_the_window_is_not_idle() {
        let fake = Fake::new();
        fake.terminal("ptyxis", "codex", &["codex", "resume", U]);
        let path = fake.rollout(U, "codex-tui", false);
        let output = json!({"timestamp": "2026-11-21T09:00:02Z", "type": "response_item",
                            "payload": {"type": "function_call_output", "call_id": "c", "output": "x".repeat(4000)}});
        let mut body = std::fs::read_to_string(&path).unwrap();
        while body.len() < 2 * SESSION_TAIL as usize {
            body.push_str(&format!("{output}\n"));
        }
        std::fs::write(&path, &body).unwrap();
        let c = ctx(AgentId::Codex, U, path.clone(), SessionSource::Terminal);
        assert_eq!(fake.place(&c).state, SessionState::Working, "its start is out of the window");
        let done = json!({"timestamp": "2026-11-21T09:00:03Z", "type": "event_msg", "payload": {"type": "task_complete", "turn_id": "t"}});
        body.push_str(&format!("{done}\n"));
        std::fs::write(&path, &body).unwrap();
        assert_eq!(fake.place(&c).state, SessionState::Idle);
    }

    #[test]
    fn codex_app_sessions_resumed_twice_and_launcher_scripts() {
        // A Codex-app session no TUI has: refused (S2's rule for the apps).
        let fake = Fake::new();
        let path = fake.rollout(U, "Codex Desktop", true);
        let c = ctx(AgentId::Codex, U, path, SessionSource::DesktopApp);
        let refused = fake.refused(&c);
        assert_eq!((refused.reason, refused.source), (PlaceRefusal::Place, SessionSource::DesktopApp));
        // Resumed in a terminal: that TUI is where a reply goes.
        fake.terminal("ptyxis", "codex", &["codex", "resume", U]);
        assert_eq!(fake.place(&c).source, SessionSource::Terminal);
        // A launcher that runs the real program as its child, both naming it.
        fake.proc(103, "codex", 102, 103, &["/opt/codex/bin/codex", "resume", U]);
        fake.term(103, 102, PTS0, 102);
        let place = fake.place(&c);
        assert_eq!(place.pid, 103, "the inner one");
        assert_eq!(comms(&place)[..2], ["codex", "bash"]);
        // Resumed in a second terminal as well.
        fake.proc(203, "codex", 101, 203, &["codex", "resume", U]);
        assert_eq!(fake.refused(&c).reason, PlaceRefusal::Ambiguous);
    }

    #[test]
    fn no_marks_for_a_session_without_text_and_a_file_gone_is_not_found() {
        let fake = Fake::new();
        fake.terminal("alacritty", "claude", &["claude"]);
        fake.status(102, 102, U, "busy", "cli");
        let path = fake.transcript(U, &[]);
        let c = ctx(AgentId::Claude, U, path.clone(), SessionSource::Terminal);
        assert!(fake.place(&c).marks.is_empty());
        std::fs::remove_file(&path).unwrap();
        let err = find(&fake.roots, euid(), NOT_PARENT, &c, now_ms()).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    /// Roughly what a terminal shows of `text`, as G7 Link compares it: the
    /// marks drawn away, list markers as bullets, rewrapped at `width` with
    /// an indent, a glyph in front, whitespace removed.
    fn screen(text: &str, width: usize) -> String {
        let drawn: Vec<String> = text
            .lines()
            .map(|l| {
                let l = l.trim_start();
                let l = l.strip_prefix("- ").map_or_else(|| l.to_owned(), |rest| format!("• {rest}"));
                l.replace(['*', '`', '#', '_'], "")
            })
            .collect();
        let chars: Vec<char> = drawn.join("\n").chars().collect();
        let wrapped: Vec<String> = chars.chunks(width).map(|c| c.iter().collect()).collect();
        format!("⏺ {}", wrapped.join("\n  ")).chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn marks_survive_wrapping_and_markdown() {
        let wrapped = "The build failed because the linker\ncouldn't find libfoo in the place it usually looks,\nso try the newer package first.";
        let found = marks([wrapped]);
        assert_eq!(found.len(), 3);
        assert!(
            found.iter().all(|m| m.chars().count() == PLACE_MARK_CHARS && !m.contains(char::is_whitespace))
        );
        for width in [17, 40, 80] {
            let shown = screen(wrapped, width);
            assert!(found.iter().all(|m| shown.contains(m.as_str())), "{width}: {found:?}");
        }
        assert!(found[0].ends_with("first."), "newest first: from the end");

        let markdown = "## Why it fails\n\nRun `cargo test --locked` and then **check** the output of every single build step carefully.\n- the first item of a list that is long enough to hold a run\n- the second item of a list that is long enough to hold a run\n\n---\n| a | b |\n```rust\nfn the_function_that_holds_the_build_together() {}\n```";
        let found = marks([markdown]);
        assert!(!found.is_empty());
        let shown = screen(markdown, 30);
        for m in &found {
            assert!(shown.contains(m.as_str()), "{m}");
            assert!(!m.contains(['`', '*', '#', '|', '•']), "{m}");
            assert!(!m.contains("runthe"), "never across two list items: {m}");
            assert!(!m.starts_with("rustfn"), "never from a fence line into the code: {m}");
        }
    }

    #[test]
    fn marks_from_short_text_look_further_back_and_skip_punctuation() {
        assert!(marks(["Done."]).is_empty(), "short text has none");
        let found = marks(["Done.", LONG_ASK]);
        assert!(!found.is_empty(), "the text before it is on the screen too");
        let ask: String = LONG_ASK.split_whitespace().collect();
        assert!(found.iter().all(|m| ask.contains(m.as_str())));
        assert!(marks(["--------------------------------------------------------------------"]).is_empty());
        assert!(marks(["....,,,,....,,,,....,,,,....,,,,....,,,,....,,,,"]).is_empty());
        assert!(
            marks(["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]).is_empty(),
            "too few different characters"
        );
        assert!(marks(std::iter::empty::<&str>()).is_empty());
        // At most three, and no repeats.
        let many = [LONG_ANSWER; 4].join(" ");
        let m = marks([many.as_str(), LONG_ASK]);
        assert_eq!(m.len(), MAX_PLACE_MARKS);
        assert_eq!(m.iter().collect::<HashSet<_>>().len(), m.len());
        // A wide character counts as one.
        let wide = "Das Ergebnis enthält Größenangaben für jede einzelne Änderung";
        assert!(marks([wide]).iter().all(|m| m.chars().count() == PLACE_MARK_CHARS));
    }

    #[tokio::test]
    async fn the_host_answers_session_place() {
        let dir = tempfile::tempdir().unwrap();
        let host = test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let fake = Fake { roots: host.sessions.roots.clone(), dir: tempfile::tempdir().unwrap() };
        std::fs::create_dir_all(&fake.roots.proc_root).unwrap();
        std::fs::write(fake.roots.proc_root.join("stat"), "cpu 1\nbtime 1795000000\n").unwrap();
        claude_in(&fake, "alacritty", "busy");
        let role = Role::Client(ClientKind::Link, Caps::default());
        let id = paths::session_id(AgentId::Claude, U).unwrap();
        let answer = host.dispatch(&role, 1, method::SESSION_PLACE, json!({"session_id": id})).await.unwrap();
        let SessionPlaceResult::Place(place) = serde_json::from_value(answer).unwrap() else {
            panic!("refused")
        };
        assert_eq!((place.pid, place.state, place.state_exact), (102, SessionState::Working, true));
        let unknown = paths::session_id(AgentId::Claude, U2).unwrap();
        for id in [unknown.as_str(), "not-an-id"] {
            let err =
                host.dispatch(&role, 1, method::SESSION_PLACE, json!({"session_id": id})).await.unwrap_err();
            assert_eq!(err.code, ErrorCode::NotFound, "{id}");
        }
        let err = host.dispatch(&role, 1, method::SESSION_PLACE, json!({})).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
    }
}
