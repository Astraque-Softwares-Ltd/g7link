// SPDX-License-Identifier: MPL-2.0
//! Which `claude` and `codex` processes run as this user, and Claude's own
//! status files (`~/.claude/sessions/<pid>.json`), which say which process
//! has which session and whether it waits for the user. For `session.place`
//! also every process's parent and terminal ([`tree`]).
//!
//! Only `comm`, `stat`, `cmdline` and the `cwd` and `exe` links are read,
//! never `environ` or any other file of a process.
//!
//! Blocking; read through the bounded helpers in `tail.rs`.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::api::{MAX_PROC_SCAN, MAX_STATUS_FILE};
use crate::host::sessions::paths::{self, Roots};
use crate::host::sessions::tail;

const MAX_COMM: u64 = 64;
const MAX_STAT: u64 = 4096;
const MAX_CMDLINE: u64 = 65_536;
/// Status files looked at.
const MAX_STATUS_FILES: usize = 512;
/// `/proc/<pid>/stat` start times are in these (USER_HZ, 100 on Linux).
const TICKS_PER_SECOND: u64 = 100;

/// Which agent a process is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comm {
    Claude,
    Codex,
}

/// One agent process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub pgrp: u32,
    pub comm: Comm,
    /// Field 22 of `stat`: clock ticks after boot.
    pub start_ticks: u64,
    /// Field 7 of `stat`: its terminal's device number, 0 for none.
    pub tty_nr: u32,
    /// Field 8 of `stat`: its terminal's foreground process group, -1 for none.
    pub tpgid: i32,
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub exe: Option<PathBuf>,
}

impl Proc {
    /// When it started, in Unix ms, given the boot time.
    pub fn started_ms(&self, boot_ms: u64) -> u64 {
        boot_ms + self.start_ticks * (1000 / TICKS_PER_SECOND)
    }

    /// `id` is an argument of its own, or right after `--resume` /
    /// `--session-id` (also as `--resume=<id>`).
    pub fn names_claude_session(&self, uuid: &str) -> bool {
        self.argv
            .windows(2)
            .any(|w| matches!(w[0].as_str(), "--resume" | "-r" | "--session-id") && w[1] == uuid)
            || self.argv.iter().any(|a| {
                a.strip_prefix("--resume=").or_else(|| a.strip_prefix("--session-id=")) == Some(uuid)
            })
    }

    /// Some argument holds `uuid`.
    pub fn mentions(&self, uuid: &str) -> bool {
        self.argv.iter().skip(1).any(|a| a.contains(uuid))
    }

    pub fn is_codex_app_server(&self) -> bool {
        self.comm == Comm::Codex && self.argv.iter().skip(1).any(|a| a == "app-server")
    }

    /// Started with `--fork-session`: it runs a new session, not the one
    /// its `--resume` names.
    pub fn forks_session(&self) -> bool {
        self.argv.iter().skip(1).any(|a| a == "--fork-session")
    }

    /// A Codex TUI started on this session: `resume`, then `uuid` as an
    /// argument of its own, and no `exec` (headless) before them. `fork`
    /// and `resume --last` don't name it.
    pub fn resumes_codex_session(&self, uuid: &str) -> bool {
        if self.comm != Comm::Codex || self.is_codex_app_server() {
            return false;
        }
        let args = self.argv.get(1..).unwrap_or_default();
        let Some(at) = args.iter().position(|a| a == "resume") else { return false };
        !args[..at].iter().any(|a| matches!(a.as_str(), "exec" | "e"))
            && args[at + 1..].iter().any(|a| a == uuid)
    }

    /// Its terminal, when it has one.
    pub fn tty(&self) -> Option<u32> {
        (self.tty_nr != 0).then_some(self.tty_nr)
    }

    /// It is its terminal's foreground job: what is typed there reaches it,
    /// not a shell.
    pub fn foreground(&self) -> bool {
        self.tty_nr != 0 && u32::try_from(self.tpgid).is_ok_and(|tpgid| tpgid == self.pgrp)
    }
}

/// Everything one process scan found.
#[derive(Debug, Clone, Default)]
pub struct Scan {
    pub procs: HashMap<u32, Proc>,
    /// Boot time in Unix ms, from `/proc/stat`.
    pub boot_ms: Option<u64>,
}

fn read_small(path: &Path, limit: u64) -> Option<String> {
    let bytes = tail::read_head(path, limit).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// What is kept of a `/proc/<pid>/stat` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    /// Field 2 without its parentheses (at most 15 bytes from the kernel).
    pub comm: String,
    pub ppid: u32,
    pub pgrp: u32,
    /// Field 7: the controlling terminal's device number, 0 for none.
    pub tty_nr: u32,
    /// Field 8: the terminal's foreground process group, -1 for none.
    pub tpgid: i32,
    /// Field 22: clock ticks after boot.
    pub start_ticks: u64,
}

/// A `stat` line's fields: `comm` between the first `(` and the last `)`,
/// the rest after that `)`, so a `)` inside `comm` can't shift them.
pub fn parse_stat(stat: &str) -> Option<Stat> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    if close < open {
        return None;
    }
    let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    // fields[0] is field 3 (state); field 22 (start time) is fields[19].
    let tty_nr: i32 = fields.get(4)?.parse().ok()?;
    Some(Stat {
        comm: stat[open + 1..close].to_owned(),
        ppid: fields.get(1)?.parse().ok()?,
        pgrp: fields.get(2)?.parse().ok()?,
        // The kernel prints the device number as a signed int: keep its bits.
        tty_nr: tty_nr as u32,
        tpgid: fields.get(5)?.parse().ok()?,
        start_ticks: fields.get(19)?.parse().ok()?,
    })
}

/// `/proc/<pid>/stat` under `dir`.
pub fn read_stat(dir: &Path) -> Option<Stat> {
    read_small(&dir.join("stat"), MAX_STAT).as_deref().and_then(parse_stat)
}

/// The owner of a `/proc/<pid>` folder: the process's user.
pub fn owner(dir: &Path) -> Option<u32> {
    std::fs::symlink_metadata(dir).ok().map(|m| m.uid())
}

/// The `/proc` entries that are processes, at most [`MAX_PROC_SCAN`], and
/// whether that was all of them.
fn pid_dirs(proc_root: &Path) -> (Vec<(u32, PathBuf)>, bool) {
    // `/proc` lists a few dozen other names before the pids.
    let listed = tail::list_dir(proc_root, MAX_PROC_SCAN + 256);
    let mut complete = listed.len() < MAX_PROC_SCAN + 256;
    let mut out = Vec::new();
    for (name, dir) in listed {
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if out.len() == MAX_PROC_SCAN {
            complete = false;
            break;
        }
        if let Ok(pid) = name.parse::<u32>() {
            out.push((pid, dir));
        }
    }
    (out, complete)
}

fn boot_ms(proc_root: &Path) -> Option<u64> {
    let stat = read_small(&proc_root.join("stat"), 64 * 1024)?;
    let secs: u64 = stat.lines().find_map(|l| l.strip_prefix("btime "))?.trim().parse().ok()?;
    Some(secs * 1000)
}

/// The `claude` and `codex` processes of user `euid`, except this host's
/// own children and their process groups (it knows them).
pub fn scan(proc_root: &Path, euid: u32, host_pid: u32) -> Scan {
    let mut found = Vec::new();
    for (pid, dir) in pid_dirs(proc_root).0 {
        if owner(&dir) != Some(euid) {
            continue;
        }
        let Some(comm) = read_small(&dir.join("comm"), MAX_COMM) else { continue };
        let comm = match comm.trim_end_matches('\n') {
            "claude" => Comm::Claude,
            "codex" => Comm::Codex,
            _ => continue,
        };
        let Some(Stat { ppid, pgrp, tty_nr, tpgid, start_ticks, .. }) = read_stat(&dir) else {
            continue;
        };
        let argv = tail::read_head(&dir.join("cmdline"), MAX_CMDLINE)
            .map(|b| {
                b.split(|&c| c == 0)
                    .filter(|a| !a.is_empty())
                    .map(|a| String::from_utf8_lossy(a).into_owned())
                    .collect()
            })
            .unwrap_or_default();
        found.push(Proc {
            pid,
            ppid,
            pgrp,
            comm,
            start_ticks,
            tty_nr,
            tpgid,
            argv,
            cwd: tail::read_link(&dir.join("cwd")),
            exe: tail::read_link(&dir.join("exe")),
        });
    }
    let ours: HashSet<u32> = found.iter().filter(|p| p.ppid == host_pid).map(|p| p.pid).collect();
    let procs = found
        .into_iter()
        .filter(|p| p.ppid != host_pid && !ours.contains(&p.pgrp))
        .map(|p| (p.pid, p))
        .collect();
    Scan { procs, boot_ms: boot_ms(proc_root) }
}

/// Every process's parent and terminal, of any user (`stat` is readable by
/// all): what `session.place` counts terminals under an ancestor from.
#[derive(Debug, Clone, Default)]
pub struct Tree {
    /// Children by parent pid.
    children: HashMap<u32, Vec<u32>>,
    /// Terminals (`tty_nr`, not 0) by pid.
    ttys: HashMap<u32, u32>,
    /// Every process was looked at: the scan didn't stop at [`MAX_PROC_SCAN`].
    pub complete: bool,
}

impl Tree {
    /// From `(pid, ppid, tty_nr)` of each process.
    pub fn new(procs: impl IntoIterator<Item = (u32, u32, u32)>, complete: bool) -> Tree {
        let mut tree = Tree { complete, ..Tree::default() };
        for (pid, ppid, tty_nr) in procs {
            tree.children.entry(ppid).or_default().push(pid);
            if tty_nr != 0 {
                tree.ttys.insert(pid, tty_nr);
            }
        }
        tree
    }

    /// How many distinct terminals the descendants of `pid` have (not
    /// `pid` itself).
    pub fn ttys_under(&self, pid: u32) -> u32 {
        let mut ttys = HashSet::new();
        let mut seen = HashSet::from([pid]);
        let mut next: Vec<u32> = self.children.get(&pid).cloned().unwrap_or_default();
        while let Some(child) = next.pop() {
            if !seen.insert(child) {
                continue;
            }
            if let Some(&tty) = self.ttys.get(&child) {
                ttys.insert(tty);
            }
            next.extend(self.children.get(&child).into_iter().flatten());
        }
        u32::try_from(ttys.len()).unwrap_or(u32::MAX)
    }
}

/// Reads [`Tree`]: one `stat` per process, at most [`MAX_PROC_SCAN`].
pub fn tree(proc_root: &Path) -> Tree {
    let (dirs, complete) = pid_dirs(proc_root);
    let procs = dirs.into_iter().filter_map(|(pid, dir)| read_stat(&dir).map(|s| (pid, s.ppid, s.tty_nr)));
    Tree::new(procs.collect::<Vec<_>>(), complete)
}

/// What a Claude status file says, reduced to what is kept. Its socket path,
/// host session id and pid domain are never kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub pid: u32,
    pub uuid: String,
    pub cwd: Option<String>,
    pub entrypoint: Option<String>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub waiting_for: Option<String>,
    /// A live process with that pid, named `claude`, started when the file says.
    pub live: bool,
    pub updated_ms: u64,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Reads one status file's text leniently. `None` when it isn't usable.
pub fn parse_status(file_pid: u32, bytes: &[u8], file_mtime_ms: u64, scan: &Scan) -> Option<Status> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let uuid = text(&v, "sessionId").filter(|u| paths::is_uuid(u))?;
    let pid = v.get("pid").and_then(Value::as_u64);
    let proc_start = match v.get("procStart") {
        Some(Value::String(s)) => s.trim().parse::<u64>().ok(),
        Some(Value::Number(n)) => n.as_u64(),
        _ => None,
    };
    let live = pid == Some(u64::from(file_pid))
        && scan
            .procs
            .get(&file_pid)
            .is_some_and(|p| p.comm == Comm::Claude && Some(p.start_ticks) == proc_start);
    let updated_ms = ["statusUpdatedAt", "updatedAt", "startedAt"]
        .iter()
        .find_map(|k| v.get(*k).and_then(Value::as_u64))
        .unwrap_or(file_mtime_ms);
    Some(Status {
        pid: file_pid,
        uuid,
        cwd: text(&v, "cwd"),
        entrypoint: text(&v, "entrypoint"),
        name: text(&v, "name"),
        status: text(&v, "status"),
        waiting_for: text(&v, "waitingFor"),
        live,
        updated_ms,
    })
}

/// Every usable status file, one per process, in no order.
pub fn status_list(roots: &Roots, scan: &Scan) -> Vec<Status> {
    let mut out = Vec::new();
    for (name, path) in tail::list_dir(&roots.claude_status_dir(), MAX_STATUS_FILES) {
        let Some(pid) = paths::status_file_pid(&name) else { continue };
        let Ok((file, meta)) = tail::open_regular(&path) else { continue };
        let mut bytes = Vec::new();
        if std::io::Read::read_to_end(&mut std::io::Read::take(file, MAX_STATUS_FILE), &mut bytes).is_err() {
            continue;
        }
        let mtime_ms = tail::Stamp::of(&meta).mtime_ms();
        if let Some(status) = parse_status(pid, &bytes, mtime_ms, scan) {
            out.push(status);
        }
    }
    out
}

/// Every usable status file, by session UUID: a live one wins, then the newest.
pub fn statuses(roots: &Roots, scan: &Scan) -> HashMap<String, Status> {
    let mut out: HashMap<String, Status> = HashMap::new();
    for status in status_list(roots, scan) {
        let better = match out.get(&status.uuid) {
            None => true,
            Some(old) => (status.live, status.updated_ms) > (old.live, old.updated_ms),
        };
        if better {
            out.insert(status.uuid.clone(), status);
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fake `/proc` entry.
    pub(crate) fn fake_proc(
        root: &Path,
        pid: u32,
        comm: &str,
        ppid: u32,
        start: u64,
        argv: &[&str],
        cwd: &Path,
    ) {
        let dir = root.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
        let stat = format!(
            "{pid} ({comm}) S {ppid} {pid} {pid} 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 {start} 1000 100 0"
        );
        std::fs::write(dir.join("stat"), stat).unwrap();
        let mut cmdline = Vec::new();
        for a in argv {
            cmdline.extend_from_slice(a.as_bytes());
            cmdline.push(0);
        }
        std::fs::write(dir.join("cmdline"), cmdline).unwrap();
        let _ = std::os::unix::fs::symlink(cwd, dir.join("cwd"));
        let _ = std::os::unix::fs::symlink(format!("/usr/bin/{comm}"), dir.join("exe"));
    }

    /// Rewrites a fake process's `stat` with its process group and terminal.
    pub(crate) fn fake_terminal(root: &Path, pid: u32, pgrp: u32, tty_nr: u32, tpgid: i32) {
        let path = root.join(pid.to_string()).join("stat");
        let Stat { comm, ppid, start_ticks, .. } = read_stat(path.parent().unwrap()).unwrap();
        let stat = format!(
            "{pid} ({comm}) S {ppid} {pgrp} {pgrp} {tty_nr} {tpgid} 4194560 1 0 0 0 0 0 0 0 20 0 1 0 {start_ticks} 1000 100 0"
        );
        std::fs::write(path, stat).unwrap();
    }

    fn euid() -> u32 {
        crate::client::euid()
    }

    /// A host pid no fake process has as its parent.
    const NOT_PARENT: u32 = 999_999;

    #[test]
    fn the_start_time_survives_a_paren_in_comm() {
        let stat = "77 (a) b (c)) S 1 77 77 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 98765 0 0";
        let s = parse_stat(stat).unwrap();
        assert_eq!((s.comm.as_str(), s.ppid, s.pgrp, s.start_ticks), ("a) b (c)", 1, 77, 98765));
        assert_eq!((s.tty_nr, s.tpgid), (0, -1), "no terminal");
        assert_eq!(parse_stat("77 (x) S"), None);
        assert_eq!(parse_stat("no paren"), None);
        assert_eq!(parse_stat(") 77 (x"), None, "the parentheses the wrong way round");
    }

    #[test]
    fn the_terminal_fields() {
        // pts/3 (major 136) in the foreground group 4400.
        let s = parse_stat("4400 (tmux: server) S 1 4400 4400 34819 4400 0 0 0 0 0 0 0 0 0 20 0 1 0 9 0 0")
            .unwrap();
        assert_eq!((s.comm.as_str(), s.tty_nr, s.tpgid), ("tmux: server", 34819, 4400));
        // A device number the kernel prints negative keeps its bits.
        let s = parse_stat("5 (x) S 1 5 5 -2147450877 5 0 0 0 0 0 0 0 0 0 20 0 1 0 9 0 0").unwrap();
        assert_eq!(s.tty_nr, 0x8000_8003);
        assert_eq!(parse_stat("5 (x) S 1 5 5 pts -1 0 0 0 0 0 0 0 0 0 20 0 1 0 9 0 0"), None);
    }

    #[test]
    fn in_the_foreground_only_when_its_group_has_the_terminal() {
        let p = Proc {
            pid: 50,
            ppid: 40,
            pgrp: 50,
            comm: Comm::Claude,
            start_ticks: 9,
            tty_nr: 34816,
            tpgid: 50,
            argv: vec!["claude".into()],
            cwd: None,
            exe: None,
        };
        assert_eq!(p.tty(), Some(34816));
        assert!(p.foreground());
        let suspended = Proc { tpgid: 40, ..p.clone() };
        assert!(!suspended.foreground(), "a shell has the terminal: what is typed goes to it");
        let no_terminal = Proc { tty_nr: 0, tpgid: -1, ..p.clone() };
        assert_eq!(no_terminal.tty(), None);
        assert!(!no_terminal.foreground());
        assert!(!Proc { tty_nr: 0, ..p }.foreground(), "no terminal, whatever tpgid says");
    }

    #[test]
    fn a_codex_tui_names_its_session_only_with_resume() {
        let u = "11111111-2222-4333-8444-555555555555";
        let codex = |argv: &[&str]| Proc {
            pid: 1,
            ppid: 0,
            pgrp: 1,
            comm: Comm::Codex,
            start_ticks: 0,
            tty_nr: 0,
            tpgid: -1,
            argv: argv.iter().map(|a| (*a).to_owned()).collect(),
            cwd: None,
            exe: None,
        };
        assert!(codex(&["codex", "resume", u]).resumes_codex_session(u));
        assert!(codex(&["codex", "-m", "gpt", "resume", u, "go on"]).resumes_codex_session(u));
        assert!(codex(&["/opt/codex", "resume", "--yolo", u]).resumes_codex_session(u));
        assert!(!codex(&["codex", "exec", "resume", u]).resumes_codex_session(u), "headless");
        assert!(!codex(&["codex", "e", "resume", u]).resumes_codex_session(u));
        assert!(!codex(&["codex", "fork", u]).resumes_codex_session(u), "a fork is a new session");
        assert!(!codex(&["codex", "resume", "--last"]).resumes_codex_session(u));
        assert!(!codex(&["codex", u, "resume"]).resumes_codex_session(u));
        assert!(!codex(&["codex", "resume", &format!("{u}x")]).resumes_codex_session(u));
        assert!(!codex(&["codex", "app-server", "resume", u]).resumes_codex_session(u));
        assert!(!codex(&["resume", u]).resumes_codex_session(u), "argv[0] is the program");
        let claude = Proc { comm: Comm::Claude, ..codex(&["claude", "resume", u]) };
        assert!(!claude.resumes_codex_session(u));
        assert!(Proc { comm: Comm::Claude, ..codex(&["claude", "-r", u, "--fork-session"]) }.forks_session());
    }

    #[test]
    fn only_our_agents_and_not_our_children() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("stat"), "cpu 1 2 3\nbtime 1795000000\n").unwrap();
        let cwd = Path::new("/work");
        fake_proc(root, 100, "claude", 1, 500, &["claude", "--resume", "abc"], cwd);
        fake_proc(root, 101, "codex", 1, 600, &["codex", "resume", "x"], cwd);
        fake_proc(root, 102, "bash", 1, 600, &["bash"], cwd);
        fake_proc(root, 103, "claude", 4242, 700, &["claude", "-p"], cwd);
        fake_proc(root, 104, "claude", 103, 710, &["claude"], cwd);
        // A grandchild in the child's process group.
        std::fs::write(
            root.join("104/stat"),
            "104 (claude) S 103 103 103 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 710 0 0",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("self")).unwrap();
        let scan = scan(root, euid(), 4242);
        let mut pids: Vec<u32> = scan.procs.keys().copied().collect();
        pids.sort();
        assert_eq!(pids, [100, 101], "not bash, not the host's child or its group");
        assert_eq!(scan.boot_ms, Some(1_795_000_000_000));
        let p = &scan.procs[&100];
        assert!(p.names_claude_session("abc"));
        assert!(!p.names_claude_session("ab"));
        assert_eq!(p.cwd.as_deref(), Some(cwd));
        assert_eq!(p.started_ms(1000), 1000 + 5000);
        assert!(scan.procs[&101].mentions("x"));
    }

    #[test]
    fn other_users_pids_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        fake_proc(dir.path(), 100, "claude", 1, 500, &["claude"], Path::new("/w"));
        assert!(scan(dir.path(), euid().wrapping_add(1), NOT_PARENT).procs.is_empty());
        assert_eq!(scan(dir.path(), euid(), NOT_PARENT).procs.len(), 1);
    }

    #[test]
    fn terminals_are_counted_under_a_process() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let w = Path::new("/w");
        // kitty (10) with two shells on two terminals, one running claude;
        // a third terminal elsewhere.
        fake_proc(root, 10, "kitty", 1, 100, &["kitty"], w);
        fake_proc(root, 11, "zsh", 10, 110, &["zsh"], w);
        fake_proc(root, 12, "claude", 11, 120, &["claude"], w);
        fake_proc(root, 13, "zsh", 10, 130, &["zsh"], w);
        fake_proc(root, 20, "bash", 1, 200, &["bash"], w);
        fake_terminal(root, 11, 11, 34816, 12);
        fake_terminal(root, 12, 12, 34816, 12);
        fake_terminal(root, 13, 13, 34817, 13);
        fake_terminal(root, 20, 20, 34818, 20);
        let t = tree(root);
        assert!(t.complete);
        assert_eq!(t.ttys_under(10), 2, "two shells, two terminals");
        assert_eq!(t.ttys_under(11), 1, "claude's, under its shell");
        assert_eq!(t.ttys_under(12), 0, "not counting itself");
        assert_eq!(t.ttys_under(1), 3);
        assert_eq!(t.ttys_under(4242), 0, "no such process");
        // A loop in the parents (a pid reused while reading) ends.
        let looped = Tree::new([(5, 6, 1), (6, 5, 2)], true);
        assert_eq!(looped.ttys_under(5), 1);
    }

    #[test]
    fn at_most_4096_entries_are_looked_at() {
        let dir = tempfile::tempdir().unwrap();
        for pid in 1..=4100u32 {
            let entry = dir.path().join(pid.to_string());
            std::fs::create_dir(&entry).unwrap();
            std::fs::write(entry.join("comm"), "codex\n").unwrap();
            std::fs::write(
                entry.join("stat"),
                format!("{pid} (codex) S 1 {pid} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 9 0"),
            )
            .unwrap();
        }
        assert_eq!(scan(dir.path(), euid(), NOT_PARENT).procs.len(), MAX_PROC_SCAN);
        let t = tree(dir.path());
        assert!(!t.complete, "a count from part of the processes can't prove anything");
    }

    #[test]
    fn argv_forms() {
        let p = Proc {
            pid: 1,
            ppid: 0,
            pgrp: 1,
            comm: Comm::Claude,
            start_ticks: 0,
            tty_nr: 0,
            tpgid: -1,
            argv: vec!["claude".into(), "--session-id=u1".into(), "--resume".into()],
            cwd: None,
            exe: None,
        };
        assert!(p.names_claude_session("u1"));
        assert!(!p.names_claude_session("claude"));
        let app = Proc { comm: Comm::Codex, argv: vec!["codex".into(), "app-server".into()], ..p };
        assert!(app.is_codex_app_server());
    }

    #[test]
    fn a_status_file_is_live_only_for_its_own_process() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proc");
        let cwd = Path::new("/w");
        fake_proc(&root, 4242, "claude", 1, 1234567, &["claude"], cwd);
        fake_proc(&root, 4343, "codex", 1, 55, &["codex"], cwd);
        let scan = scan(&root, euid(), NOT_PARENT);
        let uuid = "11111111-2222-4333-8444-555555555555";
        let file = |pid: u32, start: &str| {
            format!(
                r#"{{"pid":{pid},"sessionId":"{uuid}","cwd":"/w","procStart":"{start}","status":"waiting","waitingFor":"permission prompt","messagingSocketPath":"/x","updatedAt":5}}"#
            )
        };
        let live = parse_status(4242, file(4242, "1234567").as_bytes(), 1, &scan).unwrap();
        assert!(live.live);
        assert_eq!(live.waiting_for.as_deref(), Some("permission prompt"));
        assert_eq!(live.updated_ms, 5);
        let reused = parse_status(4242, file(4242, "1234000").as_bytes(), 1, &scan).unwrap();
        assert!(!reused.live, "the pid was reused by a later process");
        let renamed = parse_status(4343, file(4242, "1234567").as_bytes(), 1, &scan).unwrap();
        assert!(!renamed.live, "the file name's pid must match");
        let codex = parse_status(4343, file(4343, "55").as_bytes(), 1, &scan).unwrap();
        assert!(!codex.live, "only a claude process");
        assert!(parse_status(1, br#"{"sessionId":"not-a-uuid"}"#, 1, &scan).is_none());
        assert!(parse_status(1, b"{not json", 1, &scan).is_none());
    }
}
