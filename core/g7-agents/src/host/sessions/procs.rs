// SPDX-License-Identifier: MPL-2.0
//! Which `claude` and `codex` processes run as this user, and Claude's own
//! status files (`~/.claude/sessions/<pid>.json`), which say which process
//! has which session and whether it waits for the user.
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

/// `(ppid, pgrp, start ticks)` from a `stat` line: fields after the last `)`,
/// so a `)` inside `comm` can't shift them.
pub fn parse_stat(stat: &str) -> Option<(u32, u32, u64)> {
    let after = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = after.split_whitespace().collect();
    // fields[0] is field 3 (state); field 22 (start time) is fields[19].
    Some((fields.get(1)?.parse().ok()?, fields.get(2)?.parse().ok()?, fields.get(19)?.parse().ok()?))
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
    // `/proc` lists a few dozen other names before the pids.
    let entries = tail::list_dir(proc_root, MAX_PROC_SCAN + 256)
        .into_iter()
        .filter(|(name, _)| !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()))
        .take(MAX_PROC_SCAN);
    for (name, dir) in entries {
        let Ok(pid) = name.parse::<u32>() else { continue };
        let Ok(meta) = std::fs::symlink_metadata(&dir) else { continue };
        if meta.uid() != euid {
            continue;
        }
        let Some(comm) = read_small(&dir.join("comm"), MAX_COMM) else { continue };
        let comm = match comm.trim_end_matches('\n') {
            "claude" => Comm::Claude,
            "codex" => Comm::Codex,
            _ => continue,
        };
        let Some((ppid, pgrp, start_ticks)) =
            read_small(&dir.join("stat"), MAX_STAT).as_deref().and_then(parse_stat)
        else {
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

/// Every usable status file, by session UUID: a live one wins, then the newest.
pub fn statuses(roots: &Roots, scan: &Scan) -> HashMap<String, Status> {
    let mut out: HashMap<String, Status> = HashMap::new();
    for (name, path) in tail::list_dir(&roots.claude_status_dir(), MAX_STATUS_FILES) {
        let Some(pid) = paths::status_file_pid(&name) else { continue };
        let Ok((file, meta)) = tail::open_regular(&path) else { continue };
        let mut bytes = Vec::new();
        if std::io::Read::read_to_end(&mut std::io::Read::take(file, MAX_STATUS_FILE), &mut bytes).is_err() {
            continue;
        }
        let mtime_ms = tail::Stamp::of(&meta).mtime_ms();
        let Some(status) = parse_status(pid, &bytes, mtime_ms, scan) else { continue };
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

    fn euid() -> u32 {
        crate::client::euid()
    }

    /// A host pid no fake process has as its parent.
    const NOT_PARENT: u32 = 999_999;

    #[test]
    fn the_start_time_survives_a_paren_in_comm() {
        let stat = "77 (a) b (c)) S 1 77 77 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 98765 0 0";
        assert_eq!(parse_stat(stat), Some((1, 77, 98765)));
        assert_eq!(parse_stat("77 (x) S"), None);
        assert_eq!(parse_stat("no paren"), None);
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
    }

    #[test]
    fn argv_forms() {
        let p = Proc {
            pid: 1,
            ppid: 0,
            pgrp: 1,
            comm: Comm::Claude,
            start_ticks: 0,
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
