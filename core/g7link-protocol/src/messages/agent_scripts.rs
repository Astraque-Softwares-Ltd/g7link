// SPDX-License-Identifier: MPL-2.0
//! Bodies of project scripts (`docs/protocol/agents.md` §12): the commands a
//! PC's user typed for a folder in G7 Link on the PC, run in a thread's
//! folder from a thread, and their output. Plain protocol v1: no capability,
//! governed by the `agents` toggle.
//!
//! A phone sees every run of a thread it can read, with the last lines. It
//! sees a folder's scripts, their commands included, only while the folder's
//! "Phones can run these" switch is on (`phone_scripts`); the PC's agent
//! host checks that switch again on every request. A phone never receives a
//! run's command, a folder's path or where a run runs: no message here has a
//! field for any of them. Output lines are what the script printed, cleaned
//! on the PC.

use std::ops::Not;

use serde::{Deserialize, Serialize};

use super::{
    AgentCatalogFolder, AgentThread, clean_agent_text, clean_line, fits_word, is_agent_word, is_bidi_control,
    is_lower_hex, is_valid_agent_id, known_word,
};

/// Limits and wire words of §12.
pub mod scripts {
    /// Scripts in a folder.
    pub const MAX_SCRIPTS: usize = 12;
    /// A script's name, in bytes.
    pub const MAX_NAME_BYTES: usize = 40;
    /// A script's command, in bytes: one line.
    pub const MAX_COMMAND_BYTES: usize = 1024;
    /// A script's id: `s` and this many lowercase hex digits.
    pub const SCRIPT_ID_HEX: usize = 8;
    /// A run's id: lowercase hex digits.
    pub const RUN_ID_BYTES: usize = 16;
    /// `AgentThread.runs`.
    pub const MAX_THREAD_RUNS: usize = 8;
    /// Runs in an `agent.script.runs` answer.
    pub const MAX_RUNS: usize = 12;
    /// The newest lines of an `agent.script` event, at most this many and
    /// [`MAX_LIVE_BYTES`].
    pub const MAX_LIVE_LINES: usize = 40;
    pub const MAX_LIVE_BYTES: usize = 8 * 1024;
    /// The newest lines of an `agent.script.output` answer, at most this
    /// many and [`MAX_OUTPUT_BYTES`].
    pub const MAX_OUTPUT_LINES: usize = 200;
    pub const MAX_OUTPUT_BYTES: usize = 32 * 1024;
    /// One line of output, in characters.
    pub const MAX_LINE_CHARS: usize = 512;
    /// `by`: the phone that started a run, by its name.
    pub const MAX_BY_BYTES: usize = 64;

    /// Where a run stands (`phase`).
    pub mod phase {
        pub const RUNNING: &str = "running";
        pub const DONE: &str = "done";
        pub const FAILED: &str = "failed";
        pub const STOPPED: &str = "stopped";
        pub const ALL: &[&str] = &[RUNNING, DONE, FAILED, STOPPED];
    }

    /// Why a failed run failed (`reason`), when it isn't its exit status.
    pub mod reason {
        /// The shell couldn't be started: "Couldn't start it."
        pub const SPAWN: &str = "spawn";
        /// A signal G7 didn't send ended it.
        pub const SIGNAL: &str = "signal";
        pub const ALL: &[&str] = &[SPAWN, SIGNAL];
    }

    /// The words of a refusal a phone words itself (the PC's error's message).
    pub mod refused {
        /// `BUSY`: that script already runs there.
        pub const SCRIPT_RUNNING: &str = "script_running";
        /// `BUSY`: too many runs in the thread or on the PC.
        pub const RUNS: &str = "runs";
        /// `DENIED`: the folder's "Phones can run these" is off.
        pub const PHONE_SCRIPTS_OFF: &str = "phone_scripts_off";
        /// `NOT_FOUND`: the script isn't in the folder's list any more.
        pub const NO_SCRIPT: &str = "no_script";
        /// The folder is missing on the PC.
        pub const FOLDER_MISSING: &str = "folder_missing";
    }
}

/// Whether `text` has no control or bidi characters.
fn is_one_line(text: &str) -> bool {
    !text.chars().any(|c| c.is_control() || is_bidi_control(c))
}

/// Whether `id` is a script's id: `s` and 8 lowercase hex digits.
pub fn is_script_id(id: &str) -> bool {
    id.strip_prefix('s').is_some_and(|hex| is_lower_hex(hex, scripts::SCRIPT_ID_HEX))
}

/// Whether `id` is a run's id: 16 lowercase hex digits.
pub fn is_run_id(id: &str) -> bool {
    is_lower_hex(id, scripts::RUN_ID_BYTES)
}

/// Whether `name` can be a script's name: 1 to 40 bytes, one line, not blank.
fn is_script_name(name: &str) -> bool {
    name.len() <= scripts::MAX_NAME_BYTES && !name.trim().is_empty() && is_one_line(name)
}

/// Whether `command` can be shown as the line a script runs: 1 to 1,024
/// bytes, one line, not blank, no control character but tab and no bidi
/// control (what a phone shows is what runs).
fn is_command(command: &str) -> bool {
    command.len() <= scripts::MAX_COMMAND_BYTES
        && !command.trim().is_empty()
        && !command.chars().any(|c| (c.is_control() && c != '\t') || is_bidi_control(c))
}

// ---- A folder's scripts ----

/// A script of a folder, as the PC's user typed it in G7 Link on the PC.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentScript {
    /// `s` and 8 lowercase hex digits.
    pub id: String,
    pub name: String,
    /// The line the PC's shell runs, exactly.
    pub command: String,
}

/// Never prints the name or the command.
impl std::fmt::Debug for AgentScript {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentScript").field("id", &self.id).finish_non_exhaustive()
    }
}

impl AgentScript {
    pub fn is_valid(&self) -> bool {
        is_script_id(&self.id) && is_script_name(&self.name) && is_command(&self.command)
    }
}

/// Whether the fields §12 added to a catalog's `folder` are within their
/// limits: no script listed unless phones may run them.
pub(super) fn is_valid_folder_scripts(folder: &AgentCatalogFolder) -> bool {
    folder.scripts.len() <= scripts::MAX_SCRIPTS
        && usize::from(folder.script_count) <= scripts::MAX_SCRIPTS
        && (folder.phone_scripts || folder.scripts.is_empty())
        && folder.scripts.iter().all(AgentScript::is_valid)
        && folder.scripts.iter().enumerate().all(|(i, s)| !folder.scripts[..i].iter().any(|t| t.id == s.id))
}

/// The fields §12 added to a catalog's `folder`, within their limits: with
/// `phone_scripts` off no script is listed (its command never leaves); a
/// script out of its limits, or whose command isn't one line that can be
/// shown as it runs, is left out; at most [`scripts::MAX_SCRIPTS`], none twice.
pub(super) fn clean_folder_scripts(folder: &mut AgentCatalogFolder) {
    if !folder.phone_scripts {
        folder.scripts = Vec::new();
    }
    let mut kept: Vec<AgentScript> = Vec::with_capacity(folder.scripts.len().min(scripts::MAX_SCRIPTS));
    for script in std::mem::take(&mut folder.scripts) {
        if kept.len() == scripts::MAX_SCRIPTS {
            break;
        }
        if script.is_valid() && !kept.iter().any(|k| k.id == script.id) {
            kept.push(script);
        }
    }
    folder.scripts = kept;
    let max = u8::try_from(scripts::MAX_SCRIPTS).unwrap_or(u8::MAX);
    let listed = u8::try_from(folder.scripts.len()).unwrap_or(max);
    folder.script_count = folder.script_count.min(max).max(listed);
}

// ---- What a thread gains ----

/// A run of a thread, in brief (`AgentThread.runs`).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentRunBrief {
    /// 16 lowercase hex digits.
    pub id: String,
    /// The script's name when it started.
    pub name: String,
    /// One of [`scripts::phase`].
    pub phase: String,
    /// Its exit status, once it exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
}

/// Never prints the name.
impl std::fmt::Debug for AgentRunBrief {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRunBrief")
            .field("id", &self.id)
            .field("phase", &self.phase)
            .field("exit", &self.exit)
            .finish_non_exhaustive()
    }
}

impl AgentRunBrief {
    /// Its id, a name within its limits and a word, perhaps a phase this
    /// build doesn't know.
    pub fn is_valid(&self) -> bool {
        is_run_id(&self.id)
            && self.name.len() <= scripts::MAX_NAME_BYTES
            && is_one_line(&self.name)
            && is_agent_word(&self.phase)
    }

    /// Within its limits, or `None` for a bad id or a phase this build
    /// doesn't know.
    pub fn sanitized(mut self) -> Option<Self> {
        if !is_run_id(&self.id) || !scripts::phase::ALL.contains(&self.phase.as_str()) {
            return None;
        }
        clean_line(&mut self.name, scripts::MAX_NAME_BYTES);
        Some(self)
    }
}

/// Whether the field §12 added to `thread` is within its limits.
pub(super) fn is_valid_thread_runs(thread: &AgentThread) -> bool {
    thread.runs.len() <= scripts::MAX_THREAD_RUNS && thread.runs.iter().all(AgentRunBrief::is_valid)
}

/// The field §12 added to `thread`, within its limits: a run out of them, or
/// with a phase this build doesn't know, left out; the first
/// [`scripts::MAX_THREAD_RUNS`] kept.
pub(super) fn clean_thread_runs(thread: &mut AgentThread) {
    let mut runs: Vec<AgentRunBrief> =
        std::mem::take(&mut thread.runs).into_iter().filter_map(AgentRunBrief::sanitized).collect();
    runs.truncate(scripts::MAX_THREAD_RUNS);
    thread.runs = runs;
}

// ---- Runs ----

/// A run of a folder's script in a thread, as a phone sees it: never its
/// command, its folder's path or where it runs. An answer carries its newest
/// lines; `agent.script` only those new since the last.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentScriptRun {
    /// 16 lowercase hex digits.
    pub id: String,
    pub thread: String,
    /// The script's id.
    pub script: String,
    /// The script's name when it started.
    pub name: String,
    /// One of [`scripts::phase`].
    pub phase: String,
    /// Its exit status, once it exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    /// Unix milliseconds.
    #[serde(default)]
    pub started_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<u64>,
    /// The phone that started it, by its name; absent: the PC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// The phone this is sent to started it.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub mine: bool,
    /// What it printed, oldest first, cleaned on the PC.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    /// The number of `lines[0]`, counting every line the run printed from 0.
    #[serde(default)]
    pub from: u64,
    /// Lines not kept: "%n earlier lines not kept".
    #[serde(default)]
    pub dropped: u32,
    /// One of [`scripts::reason`]: why a failed run failed, when it isn't
    /// its exit status. Absent when the word isn't known here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Never prints the name, the phone's name or a line.
impl std::fmt::Debug for AgentScriptRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentScriptRun")
            .field("id", &self.id)
            .field("thread", &self.thread)
            .field("script", &self.script)
            .field("phase", &self.phase)
            .field("exit", &self.exit)
            .field("mine", &self.mine)
            .field("lines", &self.lines.len())
            .field("from", &self.from)
            .field("dropped", &self.dropped)
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

fn is_output_line(line: &str) -> bool {
    line.chars().count() <= scripts::MAX_LINE_CHARS
        && !line.chars().any(|c| (c.is_control() && c != '\t') || is_bidi_control(c))
}

/// One line of output: control characters (but tab) and bidi controls taken
/// out, at most [`scripts::MAX_LINE_CHARS`] characters.
fn output_line(line: &str) -> String {
    line.chars()
        .filter(|&c| !is_bidi_control(c) && (!c.is_control() || c == '\t'))
        .take(scripts::MAX_LINE_CHARS)
        .collect()
}

impl AgentScriptRun {
    /// Within its limits, its lines at most `max_lines` and `max_bytes`.
    pub fn is_valid_within(&self, max_lines: usize, max_bytes: usize) -> bool {
        is_run_id(&self.id)
            && is_valid_agent_id(&self.thread)
            && is_script_id(&self.script)
            && self.name.len() <= scripts::MAX_NAME_BYTES
            && is_one_line(&self.name)
            && is_agent_word(&self.phase)
            && self.by.as_deref().is_none_or(|b| b.len() <= scripts::MAX_BY_BYTES && is_one_line(b))
            && self.lines.len() <= max_lines
            && self.lines.iter().map(String::len).sum::<usize>() <= max_bytes
            && self.lines.iter().all(|l| is_output_line(l))
            && fits_word(&self.reason)
    }

    /// Within an answer's limits ([`scripts::MAX_OUTPUT_LINES`]).
    pub fn is_valid(&self) -> bool {
        self.is_valid_within(scripts::MAX_OUTPUT_LINES, scripts::MAX_OUTPUT_BYTES)
    }

    /// The run as a phone may have it: each line one line of at most
    /// [`scripts::MAX_LINE_CHARS`] characters, the newest `max_lines` within
    /// `max_bytes` (those left out counted in `from` and `dropped`); `None`
    /// for a bad id or a phase this build doesn't know.
    pub fn sanitized(mut self, max_lines: usize, max_bytes: usize) -> Option<Self> {
        if !is_run_id(&self.id)
            || !is_valid_agent_id(&self.thread)
            || !is_script_id(&self.script)
            || !scripts::phase::ALL.contains(&self.phase.as_str())
        {
            return None;
        }
        clean_line(&mut self.name, scripts::MAX_NAME_BYTES);
        self.by = self
            .by
            .map(|b| clean_agent_text(&b, false, scripts::MAX_BY_BYTES).0)
            .filter(|b| !b.trim().is_empty());
        known_word(&mut self.reason, scripts::reason::ALL);
        let mut lines: Vec<String> =
            std::mem::take(&mut self.lines).into_iter().map(|l| output_line(&l)).collect();
        let mut keep = 0;
        let mut bytes = 0usize;
        for line in lines.iter().rev() {
            if keep == max_lines || bytes + line.len() > max_bytes {
                break;
            }
            keep += 1;
            bytes += line.len();
        }
        let cut = lines.len() - keep;
        lines.drain(..cut);
        self.lines = lines;
        self.from = self.from.saturating_add(cut as u64);
        self.dropped = self.dropped.saturating_add(u32::try_from(cut).unwrap_or(u32::MAX));
        Some(self)
    }

    /// As an `agent.script` event carries it: its newest
    /// [`scripts::MAX_LIVE_LINES`] lines and [`scripts::MAX_LIVE_BYTES`].
    pub fn for_live(self) -> Option<Self> {
        self.sanitized(scripts::MAX_LIVE_LINES, scripts::MAX_LIVE_BYTES)
    }

    /// As an answer carries it: its newest [`scripts::MAX_OUTPUT_LINES`]
    /// lines and [`scripts::MAX_OUTPUT_BYTES`].
    pub fn for_output(self) -> Option<Self> {
        self.sanitized(scripts::MAX_OUTPUT_LINES, scripts::MAX_OUTPUT_BYTES)
    }

    /// As `agent.script.runs` lists it: without lines (`from` where the
    /// next would start).
    pub fn without_lines(self) -> Option<Self> {
        self.sanitized(0, 0)
    }

    /// Whether it has ended.
    pub fn ended(&self) -> bool {
        self.phase != scripts::phase::RUNNING
    }
}

impl crate::lanes::Paged for AgentScriptRun {
    type Item = String;

    fn take_items(&mut self) -> Vec<String> {
        std::mem::take(&mut self.lines)
    }

    fn set_items(&mut self, items: Vec<String>) {
        self.lines = items;
    }

    /// Everything but the lines repeats.
    fn continued(&self, items: Vec<String>) -> Self {
        Self { lines: items, ..self.clone_without_lines() }
    }

    fn append(&mut self, more: Self) {
        self.lines.extend(more.lines);
    }
}

impl AgentScriptRun {
    fn clone_without_lines(&self) -> Self {
        Self {
            id: self.id.clone(),
            thread: self.thread.clone(),
            script: self.script.clone(),
            name: self.name.clone(),
            phase: self.phase.clone(),
            exit: self.exit,
            started_ms: self.started_ms,
            ended_ms: self.ended_ms,
            by: self.by.clone(),
            mine: self.mine,
            lines: Vec::new(),
            from: self.from,
            dropped: self.dropped,
            reason: self.reason.clone(),
        }
    }
}

// ---- Requests ----

/// Body of `agent.script.run`: runs one of the thread's folder's scripts in
/// the thread's folder. The PC's host refuses it while the folder's
/// "Phones can run these" is off.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentScriptRunRequest {
    pub thread: String,
    /// The script's id, from the catalog.
    pub script: String,
}

impl AgentScriptRunRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread) && is_script_id(&self.script)
    }
}

/// Body of `agent.script.stop` and `agent.script.output`: one run.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentRunRef {
    pub run: String,
}

impl AgentRunRef {
    pub fn is_valid(&self) -> bool {
        is_run_id(&self.run)
    }
}

/// Body of `agent.script.runs`: a thread's runs.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentScriptRunsRequest {
    pub thread: String,
}

impl AgentScriptRunsRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
    }
}

/// Body of the `agent.script.runs` answer: newest first, at most
/// [`scripts::MAX_RUNS`], without lines.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentScriptRuns {
    #[serde(default)]
    pub runs: Vec<AgentScriptRun>,
}

impl AgentScriptRuns {
    pub fn is_valid(&self) -> bool {
        self.runs.len() <= scripts::MAX_RUNS && self.runs.iter().all(|r| r.is_valid_within(0, 0))
    }

    /// The runs a phone may have: a run out of its limits left out, lines
    /// taken off, at most [`scripts::MAX_RUNS`].
    pub fn sanitized(mut self) -> Self {
        self.runs =
            std::mem::take(&mut self.runs).into_iter().filter_map(AgentScriptRun::without_lines).collect();
        self.runs.truncate(scripts::MAX_RUNS);
        self
    }
}

// ---- The event ----

/// Body of `agent.script` (PC → phone, `agent` lane): a run's new lines and
/// where it stands; the last has its final `phase`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentScriptProgress {
    pub run: AgentScriptRun,
}

impl AgentScriptProgress {
    pub fn is_valid(&self) -> bool {
        self.run.is_valid_within(scripts::MAX_LIVE_LINES, scripts::MAX_LIVE_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Envelope, ble,
        lanes::{Lane, Paged, RPC_PART_TARGET, lane_of, rpc_of, svc},
        messages::{AgentCatalog, agent, types},
    };

    const THREAD: &str = "0123456789abcdef0123456789abcdef";
    const RUN: &str = "0123456789abcdef";
    const SCRIPT: &str = "s0a1b2c3d";

    fn script(n: u8) -> AgentScript {
        AgentScript { id: format!("s{n:08x}"), name: format!("dev {n}"), command: "npm run dev".into() }
    }

    fn run(lines: usize) -> AgentScriptRun {
        AgentScriptRun {
            id: RUN.into(),
            thread: THREAD.into(),
            script: SCRIPT.into(),
            name: "dev".into(),
            phase: scripts::phase::RUNNING.into(),
            started_ms: 1_795_000_000_000,
            lines: (0..lines).map(|i| format!("line {i}")).collect(),
            ..Default::default()
        }
    }

    fn folder(phone_scripts: bool, scripts: Vec<AgentScript>) -> AgentCatalogFolder {
        AgentCatalogFolder {
            id: "f1".into(),
            label: "app".into(),
            phone_scripts,
            script_count: u8::try_from(scripts.len()).unwrap(),
            scripts,
            ..Default::default()
        }
    }

    #[test]
    fn ids_have_their_shape() {
        assert!(is_script_id(SCRIPT) && is_script_id("s00000000"));
        for bad in ["", "s", "0a1b2c3d", "sa1b2c3d", "s0A1B2C3D", "s0a1b2c3d4", "t0a1b2c3d", "s0a1b2c3g"] {
            assert!(!is_script_id(bad), "{bad:?}");
        }
        assert!(is_run_id(RUN));
        for bad in ["", "0123456789abcde", "0123456789ABCDEF", "0123456789abcdef0", "../../etc/passwd"] {
            assert!(!is_run_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_folder_with_its_switch_off_lists_no_script() {
        let mut off = folder(false, vec![script(1), script(2)]);
        assert!(!is_valid_folder_scripts(&off), "a command of a switched-off folder");
        clean_folder_scripts(&mut off);
        assert!(off.scripts.is_empty() && is_valid_folder_scripts(&off));
        assert_eq!(off.script_count, 2, "the count still says how many");
        let catalog = AgentCatalog { folders: vec![folder(false, vec![script(1)])], ..Default::default() };
        let encoded = Envelope::new(types::AGENT_CATALOG, &catalog.sanitized()).unwrap().to_cbor();
        assert!(!encoded.windows(11).any(|w| w == b"npm run dev"), "the command never leaves");

        let mut on = folder(true, vec![script(1), script(2)]);
        assert!(is_valid_folder_scripts(&on));
        clean_folder_scripts(&mut on);
        assert_eq!(on.scripts.len(), 2);
        let back: AgentCatalogFolder = Envelope::new(types::AGENT_CATALOG, &on).unwrap().body().unwrap();
        assert_eq!(back, on);
        // An older PC's folder reads as no scripts and the switch off.
        let old: AgentCatalogFolder = Envelope::new(
            types::AGENT_CATALOG,
            &AgentCatalogFolder { id: "f1".into(), label: "app".into(), ..Default::default() },
        )
        .unwrap()
        .body()
        .unwrap();
        assert!(!old.phone_scripts && old.script_count == 0 && old.scripts.is_empty());
    }

    #[test]
    fn a_folders_scripts_keep_their_limits() {
        let mut many = folder(true, (0..15).map(script).collect());
        many.script_count = 15;
        assert!(!is_valid_folder_scripts(&many));
        clean_folder_scripts(&mut many);
        assert_eq!((many.scripts.len(), many.script_count), (scripts::MAX_SCRIPTS, 12));

        let bad = [
            AgentScript { id: "x1".into(), ..script(1) },
            AgentScript { name: String::new(), ..script(2) },
            AgentScript { name: "n".repeat(41), ..script(3) },
            AgentScript { name: "two\nlines".into(), ..script(4) },
            AgentScript { command: "  ".into(), ..script(5) },
            AgentScript { command: "x".repeat(1025), ..script(6) },
            AgentScript { command: "make\nrm -rf ~".into(), ..script(7) },
            AgentScript { command: "echo \u{202E}fr- mr".into(), ..script(8) },
        ];
        for s in &bad {
            assert!(!s.is_valid(), "{s:?}");
        }
        let mut mixed = folder(true, bad.to_vec());
        mixed.scripts.push(script(9));
        mixed.scripts.push(script(9));
        mixed.scripts.push(AgentScript { command: "cargo\ttest".into(), ..script(10) });
        clean_folder_scripts(&mut mixed);
        let ids: Vec<&str> = mixed.scripts.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["s00000009", "s0000000a"], "bad ones and a repeat left out; a tab is fine");
        assert!(AgentScript { command: "x".repeat(1024), name: "n".repeat(40), ..script(1) }.is_valid());
    }

    #[test]
    fn a_thread_carries_its_runs_in_brief() {
        let brief = |id: &str, phase: &str| AgentRunBrief {
            id: id.into(),
            name: "dev".into(),
            phase: phase.into(),
            exit: None,
        };
        let mut thread = AgentThread {
            id: THREAD.into(),
            agent: "claude".into(),
            state: agent::state::DONE.into(),
            mode: agent::mode::ASK.into(),
            runs: vec![
                brief(RUN, "running"),
                AgentRunBrief { exit: Some(3), ..brief("fedcba9876543210", "failed") },
            ],
            ..Default::default()
        };
        assert!(thread.is_valid());
        let back: AgentThread = Envelope::new(types::AGENT_THREAD, &thread).unwrap().body().unwrap();
        assert_eq!(back, thread);
        thread.runs.push(brief("abc", "running"));
        thread.runs.push(brief("1111111111111111", "paused"));
        thread.runs.extend((0..10).map(|i| brief(&format!("{i:016x}"), "done")));
        assert!(!thread.is_valid());
        let clean = thread.sanitized().unwrap();
        assert_eq!(clean.runs.len(), scripts::MAX_THREAD_RUNS);
        assert_eq!(clean.runs[1].exit, Some(3));
        assert!(clean.runs.iter().all(|r| r.id != "abc" && r.phase != "paused"));
        let none = AgentThread { runs: Vec::new(), ..clean };
        let back: AgentThread = Envelope::new(types::AGENT_THREAD, &none).unwrap().body().unwrap();
        assert!(back.runs.is_empty(), "an older PC's thread reads as no runs");
    }

    #[test]
    fn a_run_keeps_its_newest_lines_for_a_phone() {
        let mut long = run(300);
        long.lines[299] = format!("last\u{1b}[31m red\u{202E}{}", "é".repeat(600));
        long.from = 10;
        long.dropped = 10;
        let live = long.clone().for_live().unwrap();
        assert!(AgentScriptProgress { run: live.clone() }.is_valid(), "{live:?}");
        assert_eq!(live.lines.len(), scripts::MAX_LIVE_LINES);
        assert_eq!((live.from, live.dropped), (270, 270));
        assert!(live.lines[39].starts_with("last[31m red"));
        assert_eq!(live.lines[39].chars().count(), scripts::MAX_LINE_CHARS);
        let output = long.clone().for_output().unwrap();
        assert!(output.is_valid());
        assert_eq!(output.lines.len(), scripts::MAX_OUTPUT_LINES);
        assert_eq!(output.from, 110);
        let bulky = AgentScriptRun { lines: vec!["x".repeat(500); 100], ..run(0) }.for_live().unwrap();
        assert!(bulky.lines.iter().map(String::len).sum::<usize>() <= scripts::MAX_LIVE_BYTES);
        assert_eq!(bulky.lines.len(), 16);
        let bare = long.clone().without_lines().unwrap();
        assert!(bare.lines.is_empty() && bare.from == 310);

        for bad in [
            AgentScriptRun { id: "nope".into(), ..run(1) },
            AgentScriptRun { thread: "/home/me".into(), ..run(1) },
            AgentScriptRun { script: "dev".into(), ..run(1) },
            AgentScriptRun { phase: "paused".into(), ..run(1) },
        ] {
            assert!(bad.for_live().is_none());
        }
        let odd = AgentScriptRun {
            reason: Some("oom".into()),
            by: Some(format!("Pixel\n{}", "p".repeat(80))),
            phase: "failed".into(),
            ..run(1)
        }
        .for_output()
        .unwrap();
        assert_eq!(odd.reason, None, "a reason this build doesn't know reads as absent");
        assert_eq!(odd.by.as_deref().map(str::len), Some(scripts::MAX_BY_BYTES));
        assert!(odd.is_valid());
    }

    #[test]
    fn a_run_never_carries_a_command_or_a_place() {
        let encoded =
            Envelope::new(types::AGENT_SCRIPT, &AgentScriptProgress { run: run(2) }).unwrap().to_cbor();
        for never in [&b"command"[..], b"cwd", b"path", b"folder"] {
            assert!(!encoded.windows(never.len()).any(|w| w == never), "{}", String::from_utf8_lossy(never));
        }
        let printed =
            format!("{:?}", AgentScriptRun { name: "secret".into(), lines: vec!["secret".into()], ..run(0) });
        assert!(!printed.contains("secret"), "{printed}");
        let printed =
            format!("{:?}", AgentScript { command: "secret".into(), name: "secret".into(), ..script(1) });
        assert!(!printed.contains("secret"), "{printed}");
    }

    #[test]
    fn requests_hold_their_limits() {
        assert!(AgentScriptRunRequest { thread: THREAD.into(), script: SCRIPT.into() }.is_valid());
        assert!(!AgentScriptRunRequest { thread: THREAD.into(), script: "npm run dev".into() }.is_valid());
        assert!(!AgentScriptRunRequest { thread: "../x".into(), script: SCRIPT.into() }.is_valid());
        assert!(AgentRunRef { run: RUN.into() }.is_valid());
        assert!(!AgentRunRef { run: "1".into() }.is_valid());
        assert!(AgentScriptRunsRequest { thread: THREAD.into() }.is_valid());
        assert!(!AgentScriptRunsRequest { thread: String::new() }.is_valid());
        let runs = AgentScriptRuns { runs: (0..20).map(|_| run(5)).collect() };
        assert!(!runs.is_valid());
        let clean = runs.sanitized();
        assert!(clean.is_valid());
        assert_eq!(clean.runs.len(), scripts::MAX_RUNS);
        assert!(clean.runs.iter().all(|r| r.lines.is_empty()));
    }

    #[test]
    fn an_output_answer_goes_in_parts_and_joins() {
        let full =
            AgentScriptRun { lines: vec!["y".repeat(150); 200], from: 7, ..run(0) }.for_output().unwrap();
        assert_eq!((full.lines.len(), full.from), (200, 7));
        let parts = full.clone().split(RPC_PART_TARGET / 32);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| p.from == 7 && p.id == RUN));
        let mut joined = parts[0].clone();
        for part in parts.into_iter().skip(1) {
            joined.append(part);
        }
        assert_eq!(joined, full);
    }

    #[test]
    fn every_message_has_its_lane_and_none_travels_on_bluetooth() {
        for (t, op, paged) in [
            (types::AGENT_SCRIPT_RUN, "script.run", false),
            (types::AGENT_SCRIPT_STOP, "script.stop", false),
            (types::AGENT_SCRIPT_RUNS, "script.runs", false),
            (types::AGENT_SCRIPT_OUTPUT, "script.output", true),
        ] {
            let rpc = rpc_of(t).unwrap_or_else(|| panic!("{t} is an RPC"));
            assert_eq!((rpc.svc, rpc.op, rpc.paged), (svc::AGENT, op, paged), "{t}");
            assert!(!ble::carries(t), "{t}");
        }
        assert_eq!(lane_of(types::AGENT_SCRIPT), Some(Lane::Service(svc::AGENT)));
        assert!(!ble::carries(types::AGENT_SCRIPT));
        for never in ["agent.scripts.set", "agent.script.edit", "agent.folders.set", "agent.script.restart"] {
            assert_eq!(lane_of(never), None, "{never} has no message");
        }
    }
}
