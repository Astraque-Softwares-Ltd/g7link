// SPDX-License-Identifier: MPL-2.0
//! Where sessions started outside G7 live, the exact file-name patterns the
//! reader accepts, and the mapping between the agents' UUIDs and our ids.
//!
//! Only names matching these patterns are ever opened. The credential files,
//! keyrings, databases, history, settings, memory folders and per-session
//! subfolders that sit beside them never match (see the tests).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::api::AgentId;
use crate::host::env;

/// The directories the session reader looks in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    /// `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
    pub claude_home: PathBuf,
    /// `$CODEX_HOME`, else `~/.codex`.
    pub codex_home: PathBuf,
    /// `/proc` (a fake tree in tests).
    pub proc_root: PathBuf,
    /// `$XDG_CONFIG_HOME/Claude`: the Claude desktop app's own copy of the CLI lives here.
    pub claude_app_dir: Option<PathBuf>,
}

impl Roots {
    /// From an environment list: `CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
    /// `XDG_CONFIG_HOME` and `HOME`. `None` without a usable home.
    pub fn from_env(vars: &[(OsString, OsString)]) -> Option<Roots> {
        let abs = |name: &str| env::lookup(vars, name).map(PathBuf::from).filter(|p| p.is_absolute());
        let home = abs("HOME");
        let claude_home = abs("CLAUDE_CONFIG_DIR").or_else(|| home.as_ref().map(|h| h.join(".claude")))?;
        let codex_home = abs("CODEX_HOME").or_else(|| home.as_ref().map(|h| h.join(".codex")))?;
        let config = abs("XDG_CONFIG_HOME").or_else(|| home.as_ref().map(|h| h.join(".config")));
        Some(Roots {
            claude_home,
            codex_home,
            proc_root: PathBuf::from("/proc"),
            claude_app_dir: config.map(|c| c.join("Claude")),
        })
    }

    /// `~/.claude/sessions`: one status file per running Claude.
    pub fn claude_status_dir(&self) -> PathBuf {
        self.claude_home.join("sessions")
    }

    /// `~/.claude/projects`: one folder per working directory.
    pub fn claude_projects(&self) -> PathBuf {
        self.claude_home.join("projects")
    }

    /// `~/.codex/sessions/YYYY/MM/DD`.
    pub fn codex_day(&self, (year, month, day): (i64, u32, u32)) -> PathBuf {
        self.codex_home
            .join("sessions")
            .join(format!("{year:04}"))
            .join(format!("{month:02}"))
            .join(format!("{day:02}"))
    }

    /// `~/.codex/sessions`: `YYYY/MM/DD` day folders.
    pub fn codex_sessions(&self) -> PathBuf {
        self.codex_home.join("sessions")
    }

    /// `~/.codex/archived_sessions`: rollouts directly in it, or under
    /// `YYYY/MM/DD` (reconcile §7). Only the import scan looks here.
    pub fn codex_archived(&self) -> PathBuf {
        self.codex_home.join("archived_sessions")
    }

    /// `~/.codex/session_index.jsonl`: thread names.
    pub fn codex_index(&self) -> PathBuf {
        self.codex_home.join("session_index.jsonl")
    }

    /// `~/.codex/thread-writer-locks`: only the names are read.
    pub fn codex_locks(&self) -> PathBuf {
        self.codex_home.join("thread-writer-locks")
    }
}

/// A lowercase UUID: `8-4-4-4-12` hex digits.
pub fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
}

/// A Claude status file: `^[0-9]{1,10}\.json$`. Gives the pid.
pub fn status_file_pid(name: &str) -> Option<u32> {
    let digits = name.strip_suffix(".json")?;
    if digits.is_empty() || digits.len() > 10 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// A Claude transcript: `^<uuid>\.jsonl$`. Gives the UUID.
pub fn transcript_uuid(name: &str) -> Option<&str> {
    name.strip_suffix(".jsonl").filter(|u| is_uuid(u))
}

/// A plain Codex rollout:
/// `^rollout-\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-<uuid>\.jsonl$`. Gives the
/// UUID. Compressed (`.zst`) and `<uuid>_<uuid>` names don't match: live
/// following reads plain one-part rollouts only.
pub fn rollout_uuid(name: &str) -> Option<&str> {
    match rollout_name(name)? {
        RolloutName { thread, part: None, packed: false, .. } => Some(thread),
        _ => None,
    }
}

/// A rollout's name as the import scan reads it (reconcile §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RolloutName<'a> {
    /// `YYYY-MM-DDTHH-MM-SS`: the order of a thread's parts.
    pub stamp: &'a str,
    /// The thread's UUID (`session_meta.payload.id`).
    pub thread: &'a str,
    /// A later part's own UUID: `rollout-<ts>-<thread>_<part>`.
    pub part: Option<&'a str>,
    /// `.jsonl.zst`.
    pub packed: bool,
}

impl RolloutName<'_> {
    /// The id other parts' `history_base.thread_id` names this file by: its
    /// part's UUID, or the thread's for the first part.
    pub fn segment(&self) -> &str {
        self.part.unwrap_or(self.thread)
    }
}

/// Any Codex rollout name:
/// `^rollout-\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-<uuid>(_<uuid>)?\.jsonl(\.zst)?$`.
pub fn rollout_name(name: &str) -> Option<RolloutName<'_>> {
    let rest = name.strip_prefix("rollout-")?;
    let (rest, packed) = match rest.strip_suffix(".jsonl.zst") {
        Some(rest) => (rest, true),
        None => (rest.strip_suffix(".jsonl")?, false),
    };
    let (stamp, uuids) = (rest.get(..19)?, rest.get(19..)?);
    let uuids = uuids.strip_prefix('-')?;
    let stamp_ok = stamp.bytes().enumerate().all(|(i, b)| match i {
        4 | 7 | 13 | 16 => b == b'-',
        10 => b == b'T',
        _ => b.is_ascii_digit(),
    });
    let (thread, part) = match uuids.split_once('_') {
        Some((thread, part)) => (thread, Some(part)),
        None => (uuids, None),
    };
    (stamp_ok && is_uuid(thread) && part.is_none_or(is_uuid)).then_some(RolloutName {
        stamp,
        thread,
        part,
        packed,
    })
}

/// A day-folder name in `sessions/`: `YYYY`, `MM` (01-12) or `DD` (01-31) by `depth` 0, 1, 2.
pub fn is_date_folder(name: &str, depth: usize) -> bool {
    let digits = |n: usize| name.len() == n && name.bytes().all(|b| b.is_ascii_digit());
    match depth {
        0 => digits(4),
        1 => digits(2) && (1..=12).contains(&name.parse::<u32>().unwrap_or(0)),
        2 => digits(2) && (1..=31).contains(&name.parse::<u32>().unwrap_or(0)),
        _ => false,
    }
}

/// Our id for an agent's session: `"sc"` or `"sx"` and the UUID without dashes.
pub fn session_id(agent: AgentId, uuid: &str) -> Option<String> {
    if !is_uuid(uuid) {
        return None;
    }
    let prefix = match agent {
        AgentId::Claude => "sc",
        AgentId::Codex => "sx",
    };
    Some(format!("{prefix}{}", uuid.replace('-', "")))
}

/// The agent and its UUID for one of our session ids.
pub fn parse_session_id(id: &str) -> Option<(AgentId, String)> {
    if !crate::api::is_valid_session_id(id) {
        return None;
    }
    let agent = if id.starts_with("sc") { AgentId::Claude } else { AgentId::Codex };
    let hex = &id[2..];
    Some((agent, format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])))
}

/// The civil date `(year, month, day)` of a day count since 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Days since 1970-01-01 of a civil date.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The Codex day folders to look in at `now_ms`: today and the 7 days
/// before it in UTC, with a day either side so every local date is covered
/// whatever the time zone.
pub fn codex_days(now_ms: u64) -> Vec<(i64, u32, u32)> {
    let today = i64::try_from(now_ms / 86_400_000).unwrap_or(0);
    ((today - 8)..=(today + 1)).rev().map(civil_from_days).collect()
}

/// RFC 3339 (`2026-11-19T09:35:02.123Z`, or with an offset) to Unix ms.
pub fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let part = s.get(from..from + len)?;
        part.bytes().all(|c| c.is_ascii_digit()).then(|| part.parse().ok())?
    };
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (year, month, day) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (hour, minute, second) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut i = 19;
    let mut millis = 0i64;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return None;
        }
        let frac = &s[start..i.min(start + 3)];
        millis = frac.parse::<i64>().ok()? * 10i64.pow(3 - u32::try_from(frac.len()).ok()?);
    }
    let offset_s = match b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        sign @ (b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
            let minutes = num(i + 1, 2)? * 60 + num(i + 4, 2)?;
            if *sign == b'+' { minutes * 60 } else { -minutes * 60 }
        }
        _ => return None,
    };
    let days = days_from_civil(year, u32::try_from(month).ok()?, u32::try_from(day).ok()?);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_s;
    u64::try_from(secs * 1000 + millis).ok()
}

/// Whether `path` is inside `root` component by component (or equal to it).
pub fn is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: &str = "11111111-2222-4333-8444-5555555abcde";

    #[test]
    fn the_forbidden_names_never_match() {
        let forbidden = [
            ".credentials.json",
            ".claude.json",
            "auth.json",
            "1234.key",
            "4242.json.key",
            "history.jsonl",
            "config.toml",
            "settings.json",
            "settings.local.json",
            "state_5.sqlite",
            "state_5.sqlite-wal",
            "logs_1.sqlite",
            "memory",
            "subagents",
            "tool-results",
            UUID,
            "session_index.json",
            "rollout-2026-11-19T09-35-02-11111111-2222-4333-8444-555555555555.jsonl.zst",
            "rollout-2026-11-19T09-35-02-11111111-2222-4333-8444-555555555555_11111111-2222-4333-8444-555555555555.jsonl",
            "",
            ".json",
            "12345678901.json",
            "-1.json",
            "4242.JSON",
            "../4242.json",
        ];
        for name in forbidden {
            assert_eq!(status_file_pid(name), None, "{name}");
            assert_eq!(transcript_uuid(name), None, "{name}");
            assert_eq!(rollout_uuid(name), None, "{name}");
        }
    }

    #[test]
    fn the_import_scan_takes_compressed_and_later_parts_and_nothing_beside_them() {
        const PART: &str = "019a0000-1111-7222-8333-444444444444";
        let stamp = "rollout-2026-11-19T09-35-02";
        let name = format!("{stamp}-{UUID}.jsonl.zst");
        let r = rollout_name(&name).unwrap();
        assert_eq!((r.stamp, r.thread, r.part, r.packed), ("2026-11-19T09-35-02", UUID, None, true));
        assert_eq!(r.segment(), UUID);
        let name = format!("{stamp}-{UUID}_{PART}.jsonl.zst");
        let r = rollout_name(&name).unwrap();
        assert_eq!((r.thread, r.part, r.packed), (UUID, Some(PART), true));
        assert_eq!(r.segment(), PART);
        let name = format!("{stamp}-{UUID}_{PART}.jsonl");
        assert_eq!(rollout_name(&name).unwrap().part, Some(PART));
        assert_eq!(rollout_uuid(&name), None, "live following keeps refusing a later part");
        assert_eq!(rollout_uuid(&format!("{stamp}-{UUID}.jsonl.zst")), None, "and a compressed file");
        assert_eq!(rollout_uuid(&format!("{stamp}-{UUID}.jsonl")), Some(UUID));
        let neighbours = [
            format!("{stamp}-{UUID}.jsonl.zst.tmp"),
            format!("{stamp}-{UUID}.jsonl.zstd"),
            format!("{stamp}-{UUID}.jsonl.gz"),
            format!("{stamp}-{UUID}.zst"),
            format!("{stamp}-{UUID}.json.zst"),
            format!("{stamp}-{UUID}_.jsonl"),
            format!("{stamp}-{UUID}__{PART}.jsonl"),
            format!("{stamp}-{UUID}_{PART}_{PART}.jsonl.zst"),
            format!("{stamp}-{UUID}_{}.jsonl", PART.to_uppercase()),
            format!("{stamp}-{UUID}.jsonl.zst/"),
            format!(".{stamp}-{UUID}.jsonl.zst"),
            format!("{stamp}-{UUID}.jsonl.zst.lock"),
            "auth.json".into(),
            "config.toml".into(),
            "history.jsonl".into(),
            "session_index.jsonl".into(),
            "state_5.sqlite".into(),
            "state_5.sqlite-wal".into(),
            "logs_1.sqlite".into(),
            "rollout-.jsonl.zst".into(),
        ];
        for name in &neighbours {
            assert_eq!(rollout_name(name), None, "{name}");
            assert_eq!(transcript_uuid(name), None, "{name}");
        }
        for (name, depth, ok) in [
            ("2026", 0, true),
            ("11", 1, true),
            ("19", 2, true),
            ("13", 1, false),
            ("00", 2, false),
            ("32", 2, false),
            ("202", 0, false),
            ("2026", 1, false),
            ("1", 2, false),
            ("..", 0, false),
            ("rollout-2026", 0, false),
        ] {
            assert_eq!(is_date_folder(name, depth), ok, "{name} at {depth}");
        }
        let roots = Roots::from_env(&[(OsString::from("HOME"), OsString::from("/h"))]).unwrap();
        assert_eq!(roots.codex_sessions(), Path::new("/h/.codex/sessions"));
        assert_eq!(roots.codex_archived(), Path::new("/h/.codex/archived_sessions"));
    }

    #[test]
    fn the_patterns_take_what_the_table_names() {
        assert_eq!(status_file_pid("4242.json"), Some(4242));
        assert_eq!(status_file_pid("4294967295.json"), Some(u32::MAX));
        assert_eq!(status_file_pid("4294967296.json"), None, "past a pid's range");
        assert_eq!(transcript_uuid(&format!("{UUID}.jsonl")), Some(UUID));
        assert_eq!(transcript_uuid(&format!("{}.jsonl", UUID.to_uppercase())), None);
        assert_eq!(transcript_uuid(&format!("x{UUID}.jsonl")), None);
        assert_eq!(rollout_uuid(&format!("rollout-2026-11-19T09-35-02-{UUID}.jsonl")), Some(UUID));
        assert_eq!(rollout_uuid(&format!("rollout-2026-11-19T09:35:02-{UUID}.jsonl")), None);
        assert_eq!(rollout_uuid(&format!("rollout-2026-11-19-09-35-02-{UUID}.jsonl")), None);
    }

    #[test]
    fn ids_map_and_round_trip() {
        let id = session_id(AgentId::Claude, UUID).unwrap();
        assert_eq!(id, "sc111111112222433384445555555abcde");
        assert_eq!(parse_session_id(&id), Some((AgentId::Claude, UUID.to_owned())));
        let id = session_id(AgentId::Codex, UUID).unwrap();
        assert!(id.starts_with("sx"));
        assert_eq!(parse_session_id(&id), Some((AgentId::Codex, UUID.to_owned())));
        assert_eq!(session_id(AgentId::Claude, "not-a-uuid"), None);
        assert_eq!(session_id(AgentId::Claude, &UUID.to_uppercase()), None);
        assert_eq!(parse_session_id("sc../../x"), None);
        assert_eq!(parse_session_id(&"a".repeat(34)), None);
    }

    #[test]
    fn roots_follow_the_environment() {
        let vars = |list: &[(&str, &str)]| -> Vec<(OsString, OsString)> {
            list.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect()
        };
        let roots = Roots::from_env(&vars(&[("HOME", "/h")])).unwrap();
        assert_eq!(roots.claude_home, Path::new("/h/.claude"));
        assert_eq!(roots.codex_home, Path::new("/h/.codex"));
        assert_eq!(roots.claude_app_dir.as_deref(), Some(Path::new("/h/.config/Claude")));
        let roots = Roots::from_env(&vars(&[
            ("HOME", "/h"),
            ("CLAUDE_CONFIG_DIR", "/c"),
            ("CODEX_HOME", "/x"),
            ("XDG_CONFIG_HOME", "/cfg"),
        ]))
        .unwrap();
        assert_eq!(
            (roots.claude_home.as_path(), roots.codex_home.as_path()),
            (Path::new("/c"), Path::new("/x"))
        );
        assert_eq!(roots.claude_app_dir.as_deref(), Some(Path::new("/cfg/Claude")));
        assert_eq!(roots.codex_day((2026, 1, 5)), Path::new("/x/sessions/2026/01/05"));
        assert!(Roots::from_env(&vars(&[("HOME", "relative")])).is_none());
    }

    #[test]
    fn dates_and_times() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2026, 11, 19), 20_776);
        for days in [-1000, 0, 59, 60, 11_016, 20_776, 40_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        let days = codex_days(20_776 * 86_400_000 + 1000);
        assert_eq!(days.len(), 10);
        assert_eq!(days[0], (2026, 11, 20));
        assert_eq!(days[1], (2026, 11, 19));
        assert_eq!(days[9], (2026, 11, 11));
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:01Z"), Some(1000));
        assert_eq!(parse_rfc3339_ms("2026-11-19T09:35:02.123Z"), Some(20_776 * 86_400_000 + 34_502_123));
        assert_eq!(parse_rfc3339_ms("2026-11-19T09:35:02.1Z"), Some(20_776 * 86_400_000 + 34_502_100));
        assert_eq!(
            parse_rfc3339_ms("2026-11-19T10:35:02.123456+01:00"),
            Some(20_776 * 86_400_000 + 34_502_123)
        );
        for bad in [
            "",
            "2026-11-19",
            "2026-13-19T09:35:02Z",
            "2026-11-19T09:35:02",
            "2026-11-19T09:35:02.Z",
            "x026-11-19T09:35:02Z",
        ] {
            assert_eq!(parse_rfc3339_ms(bad), None, "{bad}");
        }
    }
}
