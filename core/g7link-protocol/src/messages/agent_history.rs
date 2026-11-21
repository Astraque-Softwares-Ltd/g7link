// SPDX-License-Identifier: MPL-2.0
//! Bodies of the agents' history and undo messages (`docs/protocol/agents.md`
//! §6): a thread's turns and what each changed, search, marks and renames,
//! forks, rewinding the conversation, and bringing in sessions found on the
//! PC. Plain protocol v1: no capability, governed by the `agents` toggle
//! (and `agent_sessions` for what was found).
//!
//! A phone never receives a folder's path, an absolute path, a restore
//! plan's file list, or anything to restore files with: the PC's core makes
//! every answer fit before it leaves. A word this build doesn't know reads
//! as absent, never as a reason to drop the message.

use std::ops::Not;

use serde::{Deserialize, Serialize};

use super::{
    AgentItem, AgentThread, agent, clean_agent_text, clean_line, fits_word, folder_name, is_agent_word,
    is_bidi_control, is_lower_hex, is_relative_path, is_valid_agent_id, keep_newest_items, known_word,
};

/// Limits and wire words of §6.
pub mod history {
    /// Turns in an `agent.turns` answer.
    pub const MAX_TURNS: usize = 100;
    /// Turns an `agent.turns` request gets when it doesn't say.
    pub const DEFAULT_TURNS: u32 = 30;
    /// Files in an `agent.diff.files` answer.
    pub const MAX_TURN_FILES: usize = 500;
    /// The most of one file's patch a phone gets; the PC stops there and
    /// sets `truncated`.
    pub const MAX_PATCH_BYTES: usize = 1024 * 1024;
    /// What a patch is split into before its parts are made: about a
    /// quarter of a part, cut at a line end where one is near.
    pub const PATCH_PIECE_BYTES: usize = 64 * 1024;
    /// An `agent.search` query.
    pub const MAX_QUERY_BYTES: usize = 200;
    /// Hits in an `agent.search` answer, and the default a request gets.
    pub const MAX_HITS: usize = 50;
    pub const DEFAULT_HITS: u32 = 20;
    /// A paging cursor the PC hands out (`agent.search`'s `next`).
    pub const MAX_CURSOR_BYTES: usize = 256;
    /// A hit's snippet, in characters.
    pub const MAX_SNIPPET_CHARS: usize = 240;
    /// Marks in one snippet.
    pub const MAX_MARKS: usize = 8;
    /// Threads one `agent.mark` changes.
    pub const MAX_MARK_THREADS: usize = 50;
    /// How far ahead a snooze may go: 30 days.
    pub const MAX_SNOOZE_MS: u64 = 30 * 24 * 60 * 60 * 1000;
    /// An `agent.found` query.
    pub const MAX_FOUND_QUERY_BYTES: usize = 128;
    /// Sessions in an `agent.found` answer, and the default a request gets.
    pub const MAX_FOUND: usize = 100;
    pub const DEFAULT_FOUND: u32 = 50;
    /// Items in an `agent.found.preview` answer, and the default.
    pub const MAX_PREVIEW_ITEMS: usize = 50;
    /// Sessions one `agent.import` brings in.
    pub const MAX_IMPORT_IDS: usize = 50;
    /// An import job's id, and a message's `key`: lowercase hex digits.
    pub const JOB_ID_BYTES: usize = 16;
    pub const KEY_BYTES: usize = 16;

    /// `agent.threads`' `show`: which shelf.
    pub mod show {
        /// Not archived and not snoozed (the default).
        pub const MAIN: &str = "main";
        pub const ARCHIVED: &str = "archived";
        pub const SNOOZED: &str = "snoozed";
        pub const ALL: &[&str] = &[MAIN, ARCHIVED, SNOOZED];
    }

    /// `agent.found`'s `show`.
    pub mod found_show {
        pub const ALL_SESSIONS: &str = "all";
        /// Not brought in yet.
        pub const NEW: &str = "new";
        pub const ALL: &[&str] = &[ALL_SESSIONS, NEW];
    }

    /// A turn's `checkpoint`: its restore point.
    pub mod checkpoint {
        pub const PENDING: &str = "pending";
        pub const READY: &str = "ready";
        pub const MISSING: &str = "missing";
        /// The folder isn't in a git work tree.
        pub const NONE: &str = "none";
        pub const EXPIRED: &str = "expired";
        pub const ALL: &[&str] = &[PENDING, READY, MISSING, NONE, EXPIRED];
    }

    /// A turn's `why`: why its restore point is missing.
    pub mod why {
        pub const BUSY: &str = "busy";
        pub const SLOW: &str = "slow";
        pub const LARGE: &str = "large";
        pub const REFUSED: &str = "refused";
        pub const OFF: &str = "off";
        pub const BEFORE: &str = "before";
        pub const ALL: &[&str] = &[BUSY, SLOW, LARGE, REFUSED, OFF, BEFORE];
    }

    /// How a turn ended (`ended`).
    pub mod turn_end {
        pub const DONE: &str = "done";
        pub const FAILED: &str = "failed";
        pub const STOPPED: &str = "stopped";
        pub const ALL: &[&str] = &[DONE, FAILED, STOPPED];
    }

    /// What happened to a file in a turn (`status`).
    pub mod file_status {
        pub const ADDED: &str = "added";
        pub const MODIFIED: &str = "modified";
        pub const DELETED: &str = "deleted";
        pub const RENAMED: &str = "renamed";
        /// Its type changed (a file became a link).
        pub const TYPE: &str = "type";
        /// Too large to record: no diff, and a rewind never touches it.
        pub const UNRECORDED: &str = "unrecorded";
        pub const ALL: &[&str] = &[ADDED, MODIFIED, DELETED, RENAMED, TYPE, UNRECORDED];
    }

    /// Why part of a rewind can't be done (`blocked`).
    pub mod rewind_blocked {
        pub const SHARED: &str = "shared";
        pub const BUSY: &str = "busy";
        pub const NO_CHECKPOINT: &str = "no_checkpoint";
        pub const NOT_GIT: &str = "not_git";
        pub const EXPIRED: &str = "expired";
        pub const OFF: &str = "off";
        /// The agent can't continue from that turn.
        pub const AGENT: &str = "agent";
        pub const ALL: &[&str] = &[SHARED, BUSY, NO_CHECKPOINT, NOT_GIT, EXPIRED, OFF, AGENT];
    }

    /// Which notice a `notice` item is (`AgentItem.notice`), for those a
    /// client acts on. Receivers read a word they don't know as absent.
    pub mod notice {
        /// "Rewound to here": carries Undo while the thread's `undo` is set.
        pub const REWOUND: &str = "rewound";
        /// "Rewound to here" after Edit from here: the message went back to the composer.
        pub const EDIT_FROM_HERE: &str = "edit_from_here";
        /// Files only: "Restored the files to before this message. …"
        pub const FILES_RESTORED: &str = "files_restored";
        /// "Forked from here. …", a fork's first item.
        pub const FORKED: &str = "forked";
        pub const ALL: &[&str] = &[REWOUND, EDIT_FROM_HERE, FILES_RESTORED, FORKED];
    }

    /// Why a thread can be read but not continued (`readonly`).
    pub mod readonly {
        /// Brought in from a session outside every agent folder.
        pub const OUTSIDE_FOLDERS: &str = "outside_folders";
        /// Its agent folder is no longer listed.
        pub const FOLDER_MISSING: &str = "folder_missing";
        pub const ALL: &[&str] = &[OUTSIDE_FOLDERS, FOLDER_MISSING];
    }
}

/// Whether `key` is a message's idempotency key: 16 lowercase hex digits.
pub fn is_send_key(key: &str) -> bool {
    is_lower_hex(key, history::KEY_BYTES)
}

/// Whether `job` is an import job's id: 16 lowercase hex digits.
pub fn is_import_job(job: &str) -> bool {
    is_lower_hex(job, history::JOB_ID_BYTES)
}

/// Whether `text` has no control or bidi characters.
fn is_one_line(text: &str) -> bool {
    !text.chars().any(|c| c.is_control() || is_bidi_control(c))
}

/// `text` on one line: each control or bidi character becomes a space, so
/// what's typed keeps its words apart.
pub fn one_line(text: &str) -> String {
    text.chars().map(|c| if c.is_control() || is_bidi_control(c) { ' ' } else { c }).collect()
}

/// Ids, each valid and none twice.
fn are_unique_ids(ids: &[String]) -> bool {
    ids.iter().all(|id| is_valid_agent_id(id)) && ids.iter().enumerate().all(|(i, id)| !ids[..i].contains(id))
}

/// A turn, or a range of turns from `from` to `to`: exactly one of the two.
fn is_turn_range(turn: Option<u32>, from: Option<u32>, to: Option<u32>) -> bool {
    match (turn, from, to) {
        (Some(turn), None, None) => turn >= 1,
        (None, Some(from), Some(to)) => from >= 1 && from <= to,
        _ => false,
    }
}

// ---- What `AgentThread` gains ----

/// Where a fork came from (`AgentThread.forked_from`).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentForkedFrom {
    /// That thread's id; kept after it's deleted.
    pub thread: String,
    /// The turn it continues from; absent: the latest when it was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    /// That thread's title when it was made ("Fork of <title>").
    #[serde(default)]
    pub title: String,
}

/// Never prints the title.
impl std::fmt::Debug for AgentForkedFrom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentForkedFrom")
            .field("thread", &self.thread)
            .field("turn", &self.turn)
            .finish_non_exhaustive()
    }
}

impl AgentForkedFrom {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread) && self.turn != Some(0) && self.title.len() <= agent::MAX_TITLE_BYTES
    }

    /// Within its limits, or `None` when its thread isn't an id.
    pub fn sanitized(mut self) -> Option<Self> {
        if !is_valid_agent_id(&self.thread) {
            return None;
        }
        if self.turn == Some(0) {
            self.turn = None;
        }
        clean_line(&mut self.title, agent::MAX_TITLE_BYTES);
        Some(self)
    }
}

/// A rewind that can still be undone (`AgentThread.undo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentUndo {
    /// Unix milliseconds.
    pub until_ms: u64,
    /// It put files back: only G7 Link on the PC can undo it ("Undo it on <PC>").
    #[serde(default)]
    pub files: bool,
}

/// Body of the `agent.send` answer: `duplicate` when the PC had already
/// taken a message with that `key` (in the last 24 h) and sent nothing now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentSent {
    #[serde(default, skip_serializing_if = "Not::not")]
    pub duplicate: bool,
}

/// Whether the fields §6 added to `thread` are within their limits.
pub(super) fn is_valid_thread_history(thread: &AgentThread) -> bool {
    thread.forked_from.as_ref().is_none_or(AgentForkedFrom::is_valid) && fits_word(&thread.readonly)
}

/// The fields §6 added to `thread`, within their limits: a bad fork origin
/// and a `readonly` this build doesn't know read as absent.
pub(super) fn clean_thread_history(thread: &mut AgentThread) {
    thread.forked_from = thread.forked_from.take().and_then(AgentForkedFrom::sanitized);
    known_word(&mut thread.readonly, history::readonly::ALL);
}

// ---- Threads by shelf ----

/// Body of `agent.threads`: phone to PC. Without `show`, the main list.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentThreadsRequest {
    /// One of [`history::show`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<String>,
    /// Threads last changed before this, Unix milliseconds (the next page).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_ms: Option<u64>,
}

impl AgentThreadsRequest {
    pub fn is_valid(&self) -> bool {
        self.show.as_deref().is_none_or(|s| history::show::ALL.contains(&s))
    }

    /// Whether it asks for anything but the newest of the main list.
    pub fn is_paged(&self) -> bool {
        self.before_ms.is_some() || self.show.as_deref().is_some_and(|s| s != history::show::MAIN)
    }
}

// ---- Turns and diffs ----

/// Body of `agent.turns`: phone to PC.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentTurnsRequest {
    pub thread: String,
    /// Turns before this one; the newest without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<u32>,
    /// Default [`history::DEFAULT_TURNS`], at most [`history::MAX_TURNS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl AgentTurnsRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
            && self.before != Some(0)
            && self.limit.is_none_or(|l| (1..=history::MAX_TURNS as u32).contains(&l))
    }

    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(history::DEFAULT_TURNS).clamp(1, history::MAX_TURNS as u32)
    }
}

/// One turn of a thread (`agent.turns`, `agent.turn`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentTurn {
    /// 1-based per thread, never reused.
    pub turn: u32,
    /// The user item that started it.
    pub prompt_seq: u64,
    /// Its last item, once it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seq: Option<u64>,
    /// Unix milliseconds.
    pub started_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<u64>,
    /// One of [`history::turn_end`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<String>,
    /// One of [`history::checkpoint`]; absent when the word isn't known here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
    /// With `missing`: one of [`history::why`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// With `ready`: files changed, lines added and removed.
    #[serde(default)]
    pub files: u32,
    #[serde(default)]
    pub adds: u32,
    #[serde(default)]
    pub dels: u32,
    /// More files changed than `agent.diff.files` lists.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub more_files: bool,
    /// Large files left out of its restore point.
    #[serde(default)]
    pub skipped: u32,
    /// The PC's verdicts: a fork from here, a rewind to before it.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub can_fork: bool,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub can_rewind: bool,
}

impl AgentTurn {
    pub fn is_valid(&self) -> bool {
        self.turn >= 1 && fits_word(&self.ended) && fits_word(&self.checkpoint) && fits_word(&self.why)
    }

    /// Within its limits (unknown words absent), or `None` for turn 0.
    pub fn sanitized(mut self) -> Option<Self> {
        if self.turn == 0 {
            return None;
        }
        known_word(&mut self.ended, history::turn_end::ALL);
        known_word(&mut self.checkpoint, history::checkpoint::ALL);
        known_word(&mut self.why, history::why::ALL);
        Some(self)
    }
}

/// Body of the `agent.turns` answer (paged): newest first.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentTurns {
    #[serde(default)]
    pub turns: Vec<AgentTurn>,
}

impl AgentTurns {
    pub fn is_valid(&self) -> bool {
        self.turns.len() <= history::MAX_TURNS && self.turns.iter().all(AgentTurn::is_valid)
    }

    pub fn sanitized(mut self) -> Self {
        self.turns = self.turns.into_iter().filter_map(AgentTurn::sanitized).collect();
        self.turns.truncate(history::MAX_TURNS);
        self
    }
}

/// Body of `agent.diff.files`: the files a turn, or a range of turns,
/// changed. Exactly one of `turn` and `from` with `to`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentDiffRequest {
    pub thread: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<u32>,
}

impl AgentDiffRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread) && is_turn_range(self.turn, self.from, self.to)
    }
}

/// A file a turn changed: relative to the thread's folder, `/` between components.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentTurnFile {
    pub path: String,
    /// One of [`history::file_status`]; absent when the word isn't known here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Its earlier path (`renamed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default)]
    pub adds: u32,
    #[serde(default)]
    pub dels: u32,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub binary: bool,
}

/// Never prints a path.
impl std::fmt::Debug for AgentTurnFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTurnFile")
            .field("status", &self.status)
            .field("adds", &self.adds)
            .field("dels", &self.dels)
            .field("binary", &self.binary)
            .finish_non_exhaustive()
    }
}

impl AgentTurnFile {
    pub fn is_valid(&self) -> bool {
        is_relative_path(&self.path)
            && self.from.as_deref().is_none_or(is_relative_path)
            && fits_word(&self.status)
    }

    /// Within its limits, or `None` when its path isn't one that stays in
    /// the folder (never cut: a cut path would name another file). An
    /// earlier path that isn't is left out.
    pub fn sanitized(mut self) -> Option<Self> {
        if !is_relative_path(&self.path) {
            return None;
        }
        if !self.from.as_deref().is_none_or(is_relative_path) {
            self.from = None;
        }
        known_word(&mut self.status, history::file_status::ALL);
        Some(self)
    }
}

/// Body of the `agent.diff.files` answer (paged): `adds` and `dels` count
/// every file, also those past the list.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentTurnFiles {
    #[serde(default)]
    pub files: Vec<AgentTurnFile>,
    /// More files changed than are listed.
    #[serde(default)]
    pub more: bool,
    #[serde(default)]
    pub adds: u32,
    #[serde(default)]
    pub dels: u32,
}

impl AgentTurnFiles {
    pub fn is_valid(&self) -> bool {
        self.files.len() <= history::MAX_TURN_FILES && self.files.iter().all(AgentTurnFile::is_valid)
    }

    /// Files that stay in the folder, at most [`history::MAX_TURN_FILES`]
    /// (`more` set when some were left out for that).
    pub fn sanitized(mut self) -> Self {
        self.files = self.files.into_iter().filter_map(AgentTurnFile::sanitized).collect();
        if self.files.len() > history::MAX_TURN_FILES {
            self.files.truncate(history::MAX_TURN_FILES);
            self.more = true;
        }
        self
    }
}

/// Body of `agent.diff.patch`: one file's patch in a turn or a range.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPatchRequest {
    pub thread: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<u32>,
    /// As `agent.diff.files` listed it; the PC looks it up, never passes it on trust.
    pub path: String,
}

/// Never prints the path.
impl std::fmt::Debug for AgentPatchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPatchRequest")
            .field("thread", &self.thread)
            .field("turn", &self.turn)
            .field("from", &self.from)
            .field("to", &self.to)
            .finish_non_exhaustive()
    }
}

impl AgentPatchRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
            && is_turn_range(self.turn, self.from, self.to)
            && is_relative_path(&self.path)
    }
}

/// Body of the `agent.diff.patch` answer (paged): the file's whole patch,
/// at most [`history::MAX_PATCH_BYTES`]; its parts each carry a piece of
/// `patch`, in order, and the receiver joins them.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPatch {
    #[serde(default)]
    pub patch: String,
    /// The patch was cut.
    #[serde(default)]
    pub truncated: bool,
    /// A binary file: no text.
    #[serde(default)]
    pub binary: bool,
}

/// Never prints the patch.
impl std::fmt::Debug for AgentPatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPatch")
            .field("bytes", &self.patch.len())
            .field("truncated", &self.truncated)
            .field("binary", &self.binary)
            .finish()
    }
}

impl AgentPatch {
    pub fn is_valid(&self) -> bool {
        self.patch.len() <= history::MAX_PATCH_BYTES
            && !self
                .patch
                .chars()
                .any(|c| is_bidi_control(c) || (c.is_control() && !matches!(c, '\n' | '\t')))
    }

    /// Without control characters other than newline and tab and without
    /// bidi controls, cut to [`history::MAX_PATCH_BYTES`] (then `truncated`).
    pub fn sanitized(self) -> Self {
        if self.is_valid() {
            return self;
        }
        let (patch, cut) = clean_agent_text(&self.patch, true, history::MAX_PATCH_BYTES);
        Self { patch, truncated: self.truncated || cut, binary: self.binary }
    }

    /// `patch` in pieces of at most [`history::PATCH_PIECE_BYTES`], each
    /// ending at a line end when one is in its second half.
    fn pieces(patch: &str) -> Vec<String> {
        let mut pieces = Vec::with_capacity(patch.len() / history::PATCH_PIECE_BYTES + 1);
        let mut rest = patch;
        while !rest.is_empty() {
            let mut end = super::cut_text(rest, history::PATCH_PIECE_BYTES).len();
            if end < rest.len()
                && let Some(line) = rest[..end].rfind('\n').filter(|&at| at + 1 > end / 2)
            {
                end = line + 1;
            }
            if end == 0 {
                // A character longer than a piece can't be: take it whole.
                end = rest.chars().next().map_or(rest.len(), char::len_utf8);
            }
            pieces.push(rest[..end].to_owned());
            rest = &rest[end..];
        }
        pieces
    }
}

impl crate::lanes::Paged for AgentPatch {
    type Item = String;

    fn take_items(&mut self) -> Vec<String> {
        let patch = std::mem::take(&mut self.patch);
        Self::pieces(&patch)
    }

    fn set_items(&mut self, items: Vec<String>) {
        self.patch = items.concat();
    }

    fn continued(&self, items: Vec<String>) -> Self {
        Self { patch: items.concat(), truncated: self.truncated, binary: self.binary }
    }

    fn append(&mut self, more: Self) {
        self.patch.push_str(&more.patch);
        self.truncated |= more.truncated;
        self.binary |= more.binary;
    }
}

// ---- Search ----

/// Body of `agent.search`: phone to PC, the PC's threads by what was said in them.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentSearchRequest {
    /// 1 to [`history::MAX_QUERY_BYTES`], one line. Words of one or two
    /// letters match titles only.
    pub query: String,
    /// Default [`history::DEFAULT_HITS`], at most [`history::MAX_HITS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Archived threads too.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub archived: bool,
}

/// Never prints the query or the cursor.
impl std::fmt::Debug for AgentSearchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSearchRequest")
            .field("bytes", &self.query.len())
            .field("limit", &self.limit)
            .field("paged", &self.cursor.is_some())
            .field("archived", &self.archived)
            .finish()
    }
}

/// Whether `cursor` is a cursor the PC could have handed out.
fn is_cursor(cursor: &str) -> bool {
    (1..=history::MAX_CURSOR_BYTES).contains(&cursor.len()) && is_one_line(cursor)
}

impl AgentSearchRequest {
    pub fn is_valid(&self) -> bool {
        self.query.len() <= history::MAX_QUERY_BYTES
            && !self.query.trim().is_empty()
            && is_one_line(&self.query)
            && self.limit.is_none_or(|l| (1..=history::MAX_HITS as u32).contains(&l))
            && self.cursor.as_deref().is_none_or(is_cursor)
    }

    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(history::DEFAULT_HITS).clamp(1, history::MAX_HITS as u32)
    }
}

/// One search hit: a message, or a thread's title (`seq` 0).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentHit {
    pub thread: String,
    pub title: String,
    pub agent: String,
    /// The folder's label.
    pub folder: String,
    /// The thread's: one of [`agent::state`].
    pub state: String,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub archived: bool,
    /// The item that matched; 0: the title.
    pub seq: u64,
    /// The item's kind, when it's one known here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// When the item was written, Unix milliseconds.
    pub when: u64,
    /// At most [`history::MAX_SNIPPET_CHARS`] characters of the text around
    /// the first match, one line.
    pub snippet: String,
    /// The matches in `snippet` as (start, length) in UTF-16 units, at most
    /// [`history::MAX_MARKS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<(u16, u16)>,
    /// Hits in the same thread left off this page.
    #[serde(default)]
    pub more_in_thread: u32,
}

/// Never prints the title or the snippet.
impl std::fmt::Debug for AgentHit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHit")
            .field("thread", &self.thread)
            .field("seq", &self.seq)
            .field("kind", &self.kind)
            .field("marks", &self.marks.len())
            .field("more_in_thread", &self.more_in_thread)
            .finish_non_exhaustive()
    }
}

/// UTF-16 units in `text`.
fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// Whether every mark lies inside `snippet`, with a length.
fn are_marks_inside(marks: &[(u16, u16)], snippet: &str) -> bool {
    let units = utf16_len(snippet);
    marks.len() <= history::MAX_MARKS
        && marks.iter().all(|&(start, len)| len > 0 && usize::from(start) + usize::from(len) <= units)
}

impl AgentHit {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
            && is_valid_agent_id(&self.agent)
            && self.title.len() <= agent::MAX_TITLE_BYTES
            && self.folder.len() <= agent::MAX_LABEL_BYTES
            && agent::state::ALL.contains(&self.state.as_str())
            && fits_word(&self.kind)
            && self.snippet.chars().count() <= history::MAX_SNIPPET_CHARS
            && is_one_line(&self.snippet)
            && are_marks_inside(&self.marks, &self.snippet)
    }

    /// Within its limits, or `None` when an id or the state is bad. The
    /// snippet's control and bidi characters become spaces, one for one,
    /// so the marks keep their places; marks past its end go.
    pub fn sanitized(mut self) -> Option<Self> {
        if !is_valid_agent_id(&self.thread)
            || !is_valid_agent_id(&self.agent)
            || !agent::state::ALL.contains(&self.state.as_str())
        {
            return None;
        }
        clean_line(&mut self.title, agent::MAX_TITLE_BYTES);
        self.folder = folder_name(&self.folder).to_owned();
        clean_line(&mut self.folder, agent::MAX_LABEL_BYTES);
        known_word(&mut self.kind, agent::item::ALL);
        self.snippet = one_line(&self.snippet).chars().take(history::MAX_SNIPPET_CHARS).collect();
        let units = utf16_len(&self.snippet);
        self.marks.retain(|&(start, len)| len > 0 && usize::from(start) + usize::from(len) <= units);
        self.marks.truncate(history::MAX_MARKS);
        Some(self)
    }
}

/// Body of the `agent.search` answer (paged): newest first.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentHits {
    #[serde(default)]
    pub hits: Vec<AgentHit>,
    /// The cursor for the next page; absent: there's none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// Every word was shorter than three letters: titles were searched,
    /// not messages ("Type 3 letters or more to search messages.").
    #[serde(default, skip_serializing_if = "Not::not")]
    pub titles_only: bool,
}

impl AgentHits {
    pub fn is_valid(&self) -> bool {
        self.hits.len() <= history::MAX_HITS
            && self.hits.iter().all(AgentHit::is_valid)
            && self.next.as_deref().is_none_or(is_cursor)
    }

    pub fn sanitized(mut self) -> Self {
        self.hits = self.hits.into_iter().filter_map(AgentHit::sanitized).collect();
        self.hits.truncate(history::MAX_HITS);
        if !self.next.as_deref().is_none_or(is_cursor) {
            self.next = None;
        }
        self
    }
}

// ---- Marks and renames ----

/// Body of `agent.mark`: pin, archive, mark unread or snooze up to
/// [`history::MAX_MARK_THREADS`] threads; at least one change. Answered
/// with the threads that changed (`AgentThreads`, paged).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentMark {
    pub threads: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread: Option<bool>,
    /// Hidden until then, Unix milliseconds; 0 brings it back now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snooze: Option<u64>,
}

impl AgentMark {
    pub fn is_valid(&self) -> bool {
        (1..=history::MAX_MARK_THREADS).contains(&self.threads.len())
            && are_unique_ids(&self.threads)
            && (self.pinned.is_some()
                || self.archived.is_some()
                || self.unread.is_some()
                || self.snooze.is_some())
    }

    /// Whether its snooze, if any, is in the future and within
    /// [`history::MAX_SNOOZE_MS`] of `now_ms` (the receiver's clock).
    pub fn is_snooze_in_range(&self, now_ms: u64) -> bool {
        match self.snooze {
            None | Some(0) => true,
            Some(until) => until > now_ms && until - now_ms <= history::MAX_SNOOZE_MS,
        }
    }
}

/// Body of `agent.rename`.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentRename {
    pub thread: String,
    /// 1 to [`agent::MAX_TITLE_BYTES`], one line.
    pub title: String,
}

/// Never prints the title.
impl std::fmt::Debug for AgentRename {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRename").field("thread", &self.thread).finish_non_exhaustive()
    }
}

impl AgentRename {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
            && self.title.len() <= agent::MAX_TITLE_BYTES
            && !self.title.trim().is_empty()
            && is_one_line(&self.title)
    }
}

// ---- Fork and rewind ----

/// Body of `agent.fork`: a new thread continuing from a copy of this one,
/// up to `turn` (the latest without it). Files are never touched.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentForkRequest {
    pub thread: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
}

impl AgentForkRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread) && self.turn != Some(0)
    }
}

/// Body of `agent.rewind.preview`: the conversation back to before `turn`.
/// A phone rewinds the conversation only: one that asks for the files too
/// is refused (`DENIED`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentRewindPreview {
    pub thread: String,
    pub turn: u32,
    /// Never from a phone.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub files: bool,
}

impl AgentRewindPreview {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread) && self.turn >= 1
    }
}

/// Body of the `agent.rewind.preview` answer: what a rewind of the
/// conversation would leave out. Nothing has changed yet.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentRewindPlan {
    /// Used once, by the phone that asked, before `expires_ms`.
    pub token: String,
    pub expires_ms: u64,
    /// Turns and messages that leave the thread.
    #[serde(default)]
    pub turns: u32,
    #[serde(default)]
    pub messages: u32,
    /// One of [`history::rewind_blocked`]: why it can't be done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
}

/// Never prints the token.
impl std::fmt::Debug for AgentRewindPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRewindPlan")
            .field("expires_ms", &self.expires_ms)
            .field("turns", &self.turns)
            .field("messages", &self.messages)
            .field("blocked", &self.blocked)
            .finish_non_exhaustive()
    }
}

impl AgentRewindPlan {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.token) && fits_word(&self.blocked)
    }

    pub fn sanitized(mut self) -> Option<Self> {
        if !is_valid_agent_id(&self.token) {
            return None;
        }
        known_word(&mut self.blocked, history::rewind_blocked::ALL);
        Some(self)
    }
}

/// Body of `agent.rewind.apply`: do what the preview said. `refill` puts
/// the turn's message back as the thread's draft (edit from here).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentRewindApply {
    pub token: String,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub refill: bool,
    /// Never from a phone: files changed after the agent's turn.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub include_later: bool,
}

/// Never prints the token.
impl std::fmt::Debug for AgentRewindApply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRewindApply")
            .field("refill", &self.refill)
            .field("include_later", &self.include_later)
            .finish_non_exhaustive()
    }
}

impl AgentRewindApply {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.token)
    }
}

/// Body of `agent.rewind.undo`: bring back what the last rewind left out,
/// while the thread's `undo` is set.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentRewindUndo {
    pub thread: String,
}

impl AgentRewindUndo {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
    }
}

// ---- Sessions found on the PC, and bringing them in ----

/// Body of `agent.found`: phone to PC, every Claude and Codex session on it.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentFoundRequest {
    /// Matches title and folder; one line, at most [`history::MAX_FOUND_QUERY_BYTES`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// An agent's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// One of [`history::found_show`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<String>,
    /// Sessions last active before this, Unix milliseconds (the next page).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_ms: Option<u64>,
    /// Default [`history::DEFAULT_FOUND`], at most [`history::MAX_FOUND`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Never prints the query.
impl std::fmt::Debug for AgentFoundRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentFoundRequest")
            .field("query_bytes", &self.query.as_ref().map(String::len))
            .field("agent", &self.agent)
            .field("show", &self.show)
            .field("before_ms", &self.before_ms)
            .field("limit", &self.limit)
            .finish()
    }
}

impl AgentFoundRequest {
    pub fn is_valid(&self) -> bool {
        self.query.as_deref().is_none_or(|q| q.len() <= history::MAX_FOUND_QUERY_BYTES && is_one_line(q))
            && self.agent.as_deref().is_none_or(is_valid_agent_id)
            && self.show.as_deref().is_none_or(|s| history::found_show::ALL.contains(&s))
            && self.limit.is_none_or(|l| (1..=history::MAX_FOUND as u32).contains(&l))
    }

    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(history::DEFAULT_FOUND).clamp(1, history::MAX_FOUND as u32)
    }
}

/// A Claude or Codex session on the PC, as a phone sees it: its folder is a
/// label, never a path.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentFound {
    pub id: String,
    pub agent: String,
    pub title: String,
    /// Its agent folder's label, else its folder's name.
    pub folder: String,
    /// Set when it's inside an agent folder: only then can it continue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_ms: Option<u64>,
    /// Its newest file's last change, Unix milliseconds.
    pub updated_ms: u64,
    pub bytes: u64,
    /// Kept compressed.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub packed: bool,
    /// One of [`agent::session_source`].
    pub source: String,
    /// Something has it open now.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub open: bool,
    /// The thread brought in from it, while that thread exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// It couldn't be read: it can't be brought in.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub damaged: bool,
}

/// Never prints the title or the folder.
impl std::fmt::Debug for AgentFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentFound")
            .field("id", &self.id)
            .field("agent", &self.agent)
            .field("source", &self.source)
            .field("packed", &self.packed)
            .field("open", &self.open)
            .field("thread", &self.thread)
            .field("damaged", &self.damaged)
            .finish_non_exhaustive()
    }
}

impl AgentFound {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.id)
            && is_valid_agent_id(&self.agent)
            && self.title.len() <= agent::MAX_TITLE_BYTES
            && self.folder.len() <= agent::MAX_LABEL_BYTES
            && !self.folder.contains(['/', '\\'])
            && self.folder_id.as_deref().is_none_or(is_valid_agent_id)
            && is_agent_word(&self.source)
            && self.thread.as_deref().is_none_or(is_valid_agent_id)
    }

    /// Within its limits (a folder path cut to its last component, an
    /// unknown source read as `other`), or `None` when an id is bad.
    pub fn sanitized(mut self) -> Option<Self> {
        if !is_valid_agent_id(&self.id) || !is_valid_agent_id(&self.agent) {
            return None;
        }
        clean_line(&mut self.title, agent::MAX_TITLE_BYTES);
        self.folder = folder_name(&self.folder).to_owned();
        clean_line(&mut self.folder, agent::MAX_LABEL_BYTES);
        if !self.folder_id.as_deref().is_none_or(is_valid_agent_id) {
            self.folder_id = None;
        }
        if !agent::session_source::ALL.contains(&self.source.as_str()) {
            self.source = agent::session_source::OTHER.to_owned();
        }
        if !self.thread.as_deref().is_none_or(is_valid_agent_id) {
            self.thread = None;
        }
        Some(self)
    }
}

/// Body of the `agent.found` answer (paged).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentFoundList {
    #[serde(default)]
    pub found: Vec<AgentFound>,
    /// The `before_ms` of the next page; absent: there's none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_ms: Option<u64>,
    /// Sessions that match, in all.
    #[serde(default)]
    pub total: u32,
    /// The PC is still looking: ask again for more.
    #[serde(default)]
    pub scanning: bool,
    /// The PC stopped at one of its bounds: not every session is listed.
    #[serde(default)]
    pub partial: bool,
}

impl AgentFoundList {
    pub fn is_valid(&self) -> bool {
        self.found.len() <= history::MAX_FOUND && self.found.iter().all(AgentFound::is_valid)
    }

    pub fn sanitized(mut self) -> Self {
        self.found = self.found.into_iter().filter_map(AgentFound::sanitized).collect();
        self.found.truncate(history::MAX_FOUND);
        self
    }
}

/// Body of `agent.found.preview`: the newest messages of a found session,
/// read on demand and kept nowhere.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentFoundPreview {
    pub id: String,
    /// Default and most: [`history::MAX_PREVIEW_ITEMS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl AgentFoundPreview {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.id)
            && self.limit.is_none_or(|l| (1..=history::MAX_PREVIEW_ITEMS as u32).contains(&l))
    }

    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(history::MAX_PREVIEW_ITEMS as u32).clamp(1, history::MAX_PREVIEW_ITEMS as u32)
    }
}

/// Body of the `agent.found.preview` answer (paged). Items' `seq`s come
/// from the session's file: they grow, may be 0 and aren't consecutive.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentFoundItems {
    #[serde(default)]
    pub items: Vec<AgentItem>,
}

impl AgentFoundItems {
    pub fn is_valid(&self) -> bool {
        self.items.len() <= history::MAX_PREVIEW_ITEMS
            && self.items.iter().all(AgentItem::is_valid_in_session)
    }

    /// Bad items left out, then the oldest past [`history::MAX_PREVIEW_ITEMS`].
    pub fn sanitized(mut self) -> Self {
        self.items = self.items.into_iter().filter_map(AgentItem::sanitized_in_session).collect();
        keep_newest_items(&mut self.items);
        self.items.truncate(history::MAX_PREVIEW_ITEMS);
        self
    }
}

/// Body of `agent.import`: bring in found sessions as threads (`ids`), or
/// stop a job (`cancel`). Exactly one of the two.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentImportRequest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<String>,
}

impl AgentImportRequest {
    pub fn is_valid(&self) -> bool {
        match (&self.cancel, self.ids.is_empty()) {
            (Some(job), true) => is_import_job(job),
            (None, false) => self.ids.len() <= history::MAX_IMPORT_IDS && are_unique_ids(&self.ids),
            _ => false,
        }
    }
}

/// Body of the `agent.import` answer: the job, and how many it brings in.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentImportJob {
    pub job: String,
    pub total: u32,
}

impl AgentImportJob {
    pub fn is_valid(&self) -> bool {
        is_import_job(&self.job)
    }
}

// ---- Events, PC to phone on the agent lane ----

/// Body of `agent.turn`: a turn started or ended, or its restore point settled.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentTurnEvent {
    pub thread: String,
    pub turn: AgentTurn,
}

impl AgentTurnEvent {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread) && self.turn.is_valid()
    }

    pub fn sanitized(self) -> Option<Self> {
        if !is_valid_agent_id(&self.thread) {
            return None;
        }
        Some(Self { thread: self.thread, turn: self.turn.sanitized()? })
    }
}

/// Body of `agent.cut`: items `from_seq` to `to_seq` of a thread left it
/// (a rewind), or came back (`restored`, its undo). Drop or read again
/// that range.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentCut {
    pub thread: String,
    pub from_seq: u64,
    pub to_seq: u64,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub restored: bool,
}

impl AgentCut {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread) && self.from_seq >= 1 && self.from_seq <= self.to_seq
    }
}

/// Body of `agent.import.progress`: how an import job is going. The last
/// one has `done + failed == total`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentImportProgress {
    pub job: String,
    pub done: u32,
    pub failed: u32,
    pub total: u32,
    /// The thread the session just brought in became.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
}

impl AgentImportProgress {
    pub fn is_valid(&self) -> bool {
        is_import_job(&self.job)
            && u64::from(self.done) + u64::from(self.failed) <= u64::from(self.total)
            && self.thread.as_deref().is_none_or(is_valid_agent_id)
    }

    /// Whether the job ended.
    pub fn is_finished(&self) -> bool {
        u64::from(self.done) + u64::from(self.failed) == u64::from(self.total)
    }
}

#[cfg(test)]
mod tests {
    use serde::de::DeserializeOwned;

    use super::*;
    use crate::{
        Envelope,
        lanes::{Paged, RPC_PART_TARGET},
        messages::types,
    };

    const THREAD: &str = "0123456789abcdef0123456789abcdef";
    const TOKEN: &str = "ffff0000ffff0000ffff0000ffff0000";
    const JOB: &str = "0123456789abcdef";
    const FOUND: &str = "sc0123456789abcdef0123456789abcdef";

    fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(t: &str, body: &T) {
        let env = Envelope::new(t, body).unwrap();
        let back = Envelope::from_cbor(&env.to_cbor()).unwrap();
        assert_eq!(&back.body::<T>().unwrap(), body);
    }

    /// The keys of `body` as it goes on the wire.
    fn keys<T: Serialize>(body: &T) -> Vec<String> {
        let mut bytes = Vec::new();
        ciborium::into_writer(body, &mut bytes).unwrap();
        let value: ciborium::Value = ciborium::from_reader(bytes.as_slice()).unwrap();
        value.as_map().unwrap().iter().map(|(k, _)| k.as_text().unwrap().to_owned()).collect()
    }

    fn turn(n: u32) -> AgentTurn {
        AgentTurn {
            turn: n,
            prompt_seq: u64::from(n) * 10,
            last_seq: Some(u64::from(n) * 10 + 9),
            started_ms: 1_795_000_000_000,
            ended_ms: Some(1_795_000_060_000),
            ended: Some(history::turn_end::DONE.into()),
            checkpoint: Some(history::checkpoint::READY.into()),
            why: None,
            files: 3,
            adds: 40,
            dels: 2,
            more_files: false,
            skipped: 1,
            can_fork: true,
            can_rewind: true,
        }
    }

    fn hit() -> AgentHit {
        AgentHit {
            thread: THREAD.into(),
            title: "Fix the login test".into(),
            agent: "claude".into(),
            folder: "g7linux".into(),
            state: agent::state::DONE.into(),
            archived: false,
            seq: 12,
            kind: Some(agent::item::ASSISTANT.into()),
            when: 1_795_000_000_000,
            snippet: "…the login test fails because the token expired".into(),
            marks: vec![(5, 5), (11, 4)],
            more_in_thread: 2,
        }
    }

    fn found() -> AgentFound {
        AgentFound {
            id: FOUND.into(),
            agent: "claude".into(),
            title: "Fix the login test".into(),
            folder: "g7linux".into(),
            folder_id: Some("f1".into()),
            first_ms: Some(1_794_000_000_000),
            updated_ms: 1_795_000_000_000,
            bytes: 120_000,
            packed: true,
            source: agent::session_source::TERMINAL.into(),
            open: false,
            thread: None,
            damaged: false,
        }
    }

    #[test]
    fn every_body_round_trips() {
        round_trip(types::OK, &AgentSent { duplicate: true });
        round_trip(
            types::AGENT_THREADS,
            &AgentThreadsRequest { show: Some(history::show::ARCHIVED.into()), before_ms: Some(1) },
        );
        round_trip(
            types::AGENT_TURNS,
            &AgentTurnsRequest { thread: THREAD.into(), before: Some(9), limit: Some(5) },
        );
        round_trip(types::OK, &AgentTurns { turns: vec![turn(2), turn(1)] });
        round_trip(
            types::AGENT_DIFF_FILES,
            &AgentDiffRequest { thread: THREAD.into(), from: Some(1), to: Some(3), ..Default::default() },
        );
        let file = AgentTurnFile {
            path: "src/a b.rs".into(),
            status: Some(history::file_status::RENAMED.into()),
            from: Some("src/old.rs".into()),
            adds: 4,
            dels: 1,
            binary: false,
        };
        round_trip(types::OK, &AgentTurnFiles { files: vec![file], more: true, adds: 4, dels: 1 });
        round_trip(
            types::AGENT_DIFF_PATCH,
            &AgentPatchRequest {
                thread: THREAD.into(),
                turn: Some(2),
                path: "-leading.txt".into(),
                ..Default::default()
            },
        );
        round_trip(
            types::OK,
            &AgentPatch { patch: "@@ -1 +1 @@\n-a\n+b\n".into(), truncated: false, binary: false },
        );
        round_trip(
            types::AGENT_SEARCH,
            &AgentSearchRequest {
                query: "login token".into(),
                limit: Some(10),
                cursor: Some("c1".into()),
                archived: true,
            },
        );
        round_trip(types::OK, &AgentHits { hits: vec![hit()], next: Some("c2".into()), titles_only: true });
        round_trip(
            types::AGENT_MARK,
            &AgentMark {
                threads: vec![THREAD.into()],
                pinned: Some(true),
                snooze: Some(1_795_000_000_000),
                ..Default::default()
            },
        );
        round_trip(types::AGENT_RENAME, &AgentRename { thread: THREAD.into(), title: "Login".into() });
        round_trip(types::AGENT_FORK, &AgentForkRequest { thread: THREAD.into(), turn: Some(3) });
        round_trip(
            types::AGENT_REWIND_PREVIEW,
            &AgentRewindPreview { thread: THREAD.into(), turn: 3, files: false },
        );
        round_trip(
            types::OK,
            &AgentRewindPlan {
                token: TOKEN.into(),
                expires_ms: 1_795_000_120_000,
                turns: 2,
                messages: 9,
                blocked: Some(history::rewind_blocked::SHARED.into()),
            },
        );
        round_trip(
            types::AGENT_REWIND_APPLY,
            &AgentRewindApply { token: TOKEN.into(), refill: true, include_later: false },
        );
        round_trip(types::AGENT_REWIND_UNDO, &AgentRewindUndo { thread: THREAD.into() });
        round_trip(
            types::AGENT_FOUND,
            &AgentFoundRequest {
                query: Some("login".into()),
                agent: Some("codex".into()),
                show: Some(history::found_show::NEW.into()),
                before_ms: Some(5),
                limit: Some(20),
            },
        );
        round_trip(
            types::OK,
            &AgentFoundList {
                found: vec![found()],
                next_ms: Some(4),
                total: 330,
                scanning: true,
                partial: false,
            },
        );
        round_trip(types::AGENT_FOUND_PREVIEW, &AgentFoundPreview { id: FOUND.into(), limit: Some(10) });
        round_trip(types::AGENT_IMPORT, &AgentImportRequest { ids: vec![FOUND.into()], cancel: None });
        round_trip(types::AGENT_IMPORT, &AgentImportRequest { ids: Vec::new(), cancel: Some(JOB.into()) });
        round_trip(types::OK, &AgentImportJob { job: JOB.into(), total: 1 });
        round_trip(types::AGENT_TURN, &AgentTurnEvent { thread: THREAD.into(), turn: turn(1) });
        round_trip(
            types::AGENT_CUT,
            &AgentCut { thread: THREAD.into(), from_seq: 4, to_seq: 9, restored: true },
        );
        round_trip(
            types::AGENT_IMPORT_PROGRESS,
            &AgentImportProgress {
                job: JOB.into(),
                done: 1,
                failed: 0,
                total: 2,
                thread: Some(THREAD.into()),
            },
        );
    }

    #[test]
    fn a_turn_never_carries_a_restore_verdict() {
        // What a PC's host says about putting files back stays on the PC: a
        // turn has no `can_restore`, so one sent anyway is dropped on arrival.
        #[derive(Serialize)]
        struct HostTurn {
            turn: u32,
            prompt_seq: u64,
            started_ms: u64,
            can_fork: bool,
            can_restore: bool,
        }
        let host = HostTurn { turn: 1, prompt_seq: 1, started_ms: 1, can_fork: true, can_restore: true };
        let env = Envelope::new(types::AGENT_TURN, &host).unwrap();
        let turn: AgentTurn = Envelope::from_cbor(&env.to_cbor()).unwrap().body().unwrap();
        assert!(turn.can_fork);
        assert!(!keys(&turn).iter().any(|k| k == "can_restore"), "{:?}", keys(&turn));
        let event = AgentTurnEvent { thread: THREAD.into(), turn: turn.clone() };
        let wire = keys(&event.turn);
        assert!(!wire.iter().any(|k| k == "can_restore"));
    }

    #[test]
    fn a_found_session_never_carries_a_path() {
        let mut row = found();
        row.folder = "/home/me/src/g7linux".into();
        assert!(!row.is_valid());
        let row = row.sanitized().unwrap();
        assert_eq!(row.folder, "g7linux");
        assert!(row.is_valid());
        let wire = keys(&row);
        assert!(!wire.iter().any(|k| k == "cwd" || k == "path"), "{wire:?}");
        // An unknown source reads as `other`; a bad folder id or thread goes.
        let odd = AgentFound {
            source: "vscode".into(),
            folder_id: Some("../x".into()),
            thread: Some("NOT-AN-ID".into()),
            ..found()
        }
        .sanitized()
        .unwrap();
        assert_eq!(odd.source, agent::session_source::OTHER);
        assert_eq!((odd.folder_id, odd.thread), (None, None));
        assert!(AgentFound { id: "../etc".into(), ..found() }.sanitized().is_none());
    }

    #[test]
    fn unknown_words_read_as_absent() {
        let later = AgentTurn {
            ended: Some("vanished".into()),
            checkpoint: Some("cloud".into()),
            why: Some("gremlins".into()),
            ..turn(1)
        };
        assert!(later.is_valid(), "a later word is still a word");
        let clean = later.sanitized().unwrap();
        assert_eq!((clean.ended, clean.checkpoint, clean.why), (None, None, None));
        assert!(AgentTurn { turn: 0, ..turn(1) }.sanitized().is_none());

        let file =
            AgentTurnFile { path: "a.rs".into(), status: Some("exploded".into()), ..Default::default() };
        assert_eq!(file.sanitized().unwrap().status, None);
        let unrecorded = AgentTurnFile {
            path: "big.bin".into(),
            status: Some(history::file_status::UNRECORDED.into()),
            ..Default::default()
        };
        assert_eq!(unrecorded.sanitized().unwrap().status.as_deref(), Some("unrecorded"));

        let plan =
            AgentRewindPlan { token: TOKEN.into(), blocked: Some("eclipse".into()), ..Default::default() };
        assert_eq!(plan.sanitized().unwrap().blocked, None);

        let mut thread = AgentThread { readonly: Some("someday".into()), ..Default::default() };
        clean_thread_history(&mut thread);
        assert_eq!(thread.readonly, None);
        thread.readonly = Some(history::readonly::OUTSIDE_FOLDERS.into());
        clean_thread_history(&mut thread);
        assert_eq!(thread.readonly.as_deref(), Some("outside_folders"));
    }

    #[test]
    fn diff_paths_stay_in_the_folder() {
        for bad in ["/etc/passwd", "../up.rs", "a/../../b", "~/x", "C:/x", "a\nb", "a\u{202E}b", ""] {
            let file = AgentTurnFile { path: bad.into(), ..Default::default() };
            assert!(!file.is_valid(), "{bad:?}");
            assert!(file.sanitized().is_none(), "{bad:?}");
            let patch = AgentPatchRequest {
                thread: THREAD.into(),
                turn: Some(1),
                path: bad.into(),
                ..Default::default()
            };
            assert!(!patch.is_valid(), "{bad:?}");
        }
        let renamed =
            AgentTurnFile { path: "b.rs".into(), from: Some("/abs/a.rs".into()), ..Default::default() };
        assert_eq!(renamed.sanitized().unwrap().from, None, "an earlier path outside goes");
        let long = AgentPatchRequest {
            thread: THREAD.into(),
            turn: Some(1),
            path: "d/".repeat(256) + "x",
            ..Default::default()
        };
        assert!(!long.is_valid(), "over 512 bytes");
        let files = AgentTurnFiles {
            files: (0..501).map(|i| AgentTurnFile { path: format!("f{i}"), ..Default::default() }).collect(),
            ..Default::default()
        };
        assert!(!files.is_valid());
        let files = files.sanitized();
        assert_eq!(files.files.len(), history::MAX_TURN_FILES);
        assert!(files.more);
    }

    #[test]
    fn a_range_is_a_turn_or_from_to() {
        let at = |turn, from, to| AgentDiffRequest { thread: THREAD.into(), turn, from, to };
        assert!(at(Some(1), None, None).is_valid());
        assert!(at(None, Some(2), Some(2)).is_valid());
        assert!(at(None, Some(1), Some(4)).is_valid());
        assert!(!at(Some(0), None, None).is_valid());
        assert!(!at(None, Some(3), Some(2)).is_valid());
        assert!(!at(None, Some(1), None).is_valid());
        assert!(!at(Some(1), Some(1), Some(2)).is_valid());
        assert!(!at(None, None, None).is_valid());
    }

    #[test]
    fn a_patch_splits_at_line_ends_and_joins_back_byte_for_byte() {
        let line = "+ fn é() { /* ünïcödé */ }\n";
        let patch = line.repeat(history::MAX_PATCH_BYTES / line.len());
        let body = AgentPatch { patch: patch.clone(), truncated: true, binary: false };
        assert!(body.is_valid());
        let parts = body.clone().split(RPC_PART_TARGET);
        assert!(parts.len() >= 4, "{} parts", parts.len());
        for part in &parts {
            let len = Envelope::new(types::PART, part).unwrap().to_cbor().len();
            assert!(len <= RPC_PART_TARGET, "{len}");
            assert!(part.patch.ends_with('\n'), "a part ends at a line end");
            assert!(part.truncated);
        }
        let mut parts = parts.into_iter();
        let mut whole = parts.next().unwrap();
        parts.for_each(|p| whole.append(p));
        assert_eq!(whole, body);

        // One long line with no line end: cut on character boundaries.
        let wide = "ü".repeat(200_000);
        let pieces = AgentPatch::pieces(&wide);
        assert!(pieces.iter().all(|p| p.len() <= history::PATCH_PIECE_BYTES));
        assert_eq!(pieces.concat(), wide);
        assert_eq!(AgentPatch::default().split(RPC_PART_TARGET).len(), 1, "an empty patch is one ok");
    }

    /// Splits at `target` and appends the parts back together, checking each
    /// part repeats what it must.
    fn split_and_join<B: Paged + Clone + PartialEq + std::fmt::Debug>(body: B, target: usize) -> (Vec<B>, B) {
        let parts = body.clone().split(target);
        let mut rest = parts.clone().into_iter();
        let mut whole = rest.next().unwrap();
        rest.for_each(|p| whole.append(p));
        assert_eq!(whole, body);
        (parts, whole)
    }

    #[test]
    fn lists_keep_their_totals_and_flags_across_parts() {
        let files = AgentTurnFiles {
            files: (0..300)
                .map(|i| AgentTurnFile {
                    path: format!("src/{i}/{}.rs", "f".repeat(60)),
                    ..Default::default()
                })
                .collect(),
            more: true,
            adds: 900,
            dels: 12,
        };
        let (parts, _) = split_and_join(files, 4096);
        assert!(parts.len() > 2);
        assert!(parts.iter().all(|p| p.adds == 900 && p.dels == 12));
        assert!(parts.iter().any(|p| p.more));

        let hits = AgentHits {
            hits: (0..50).map(|_| AgentHit { snippet: "x".repeat(240), ..hit() }).collect(),
            next: Some("c9".into()),
            titles_only: true,
        };
        let (parts, _) = split_and_join(hits, 4096);
        assert!(parts.len() > 2);
        assert!(parts.iter().all(|p| p.next.as_deref() == Some("c9")));

        let list = AgentFoundList {
            found: (0..100).map(|_| found()).collect(),
            next_ms: Some(7),
            total: 330,
            scanning: true,
            partial: true,
        };
        let (parts, whole) = split_and_join(list, 4096);
        assert!(parts.len() > 2);
        assert!(parts.iter().all(|p| p.total == 330 && p.next_ms == Some(7)));
        assert!(whole.scanning && whole.partial);

        let turns = AgentTurns { turns: (1..=100).rev().map(turn).collect() };
        assert!(split_and_join(turns, 1024).0.len() > 2);
    }

    #[test]
    fn a_patch_is_cleaned_and_cut() {
        let dirty = AgentPatch { patch: "-a\r\n+b\u{1b}[31m\u{202E}\n".into(), ..Default::default() };
        assert!(!dirty.is_valid());
        let clean = dirty.sanitized();
        assert_eq!(clean.patch, "-a\n+b[31m\n");
        assert!(!clean.truncated);
        let big =
            AgentPatch { patch: "x".repeat(history::MAX_PATCH_BYTES + 1), ..Default::default() }.sanitized();
        assert_eq!(big.patch.len(), history::MAX_PATCH_BYTES);
        assert!(big.truncated);
    }

    #[test]
    fn a_snippet_keeps_its_marks_in_place() {
        assert!(hit().is_valid());
        // A newline and a bidi control become spaces one for one: the marks still point at "login".
        let dirty = AgentHit { snippet: "the\nlogin\u{202E}test".into(), marks: vec![(4, 5)], ..hit() };
        assert!(!dirty.is_valid());
        let clean = dirty.sanitized().unwrap();
        assert_eq!(clean.snippet, "the login test");
        assert_eq!(clean.marks, [(4, 5)]);
        let utf16: Vec<u16> = clean.snippet.encode_utf16().collect();
        assert_eq!(String::from_utf16(&utf16[4..9]).unwrap(), "login");

        // Astral characters count two units each.
        let astral = AgentHit { snippet: "😀 login".into(), marks: vec![(3, 5)], ..hit() };
        assert!(astral.is_valid());
        let past = AgentHit { snippet: "😀 login".into(), marks: vec![(3, 6), (0, 0)], ..hit() };
        assert!(!past.is_valid());
        assert!(past.sanitized().unwrap().marks.is_empty());

        // At most 240 characters, and marks past the cut go.
        let long = AgentHit { snippet: "é".repeat(300), marks: vec![(0, 2), (250, 2)], ..hit() };
        let long = long.sanitized().unwrap();
        assert_eq!(long.snippet.chars().count(), history::MAX_SNIPPET_CHARS);
        assert_eq!(long.marks, [(0, 2)]);
        let many = AgentHit { marks: vec![(0, 1); 9], ..hit() }.sanitized().unwrap();
        assert_eq!(many.marks.len(), history::MAX_MARKS);

        // A folder path is cut to its name, an unknown kind goes, a bad state drops the hit.
        let odd = AgentHit { folder: "/home/me/g7linux".into(), kind: Some("hologram".into()), ..hit() };
        let odd = odd.sanitized().unwrap();
        assert_eq!((odd.folder.as_str(), odd.kind), ("g7linux", None));
        assert!(AgentHit { state: "asleep".into(), ..hit() }.sanitized().is_none());
    }

    #[test]
    fn search_requests_are_one_line_and_bounded() {
        let query = |q: &str| AgentSearchRequest { query: q.into(), ..Default::default() };
        assert!(query("lo").is_valid(), "two letters search titles");
        assert!(query(&"x".repeat(200)).is_valid());
        assert!(!query(&"x".repeat(201)).is_valid());
        assert!(!query("   ").is_valid());
        assert!(!query("a\nb").is_valid());
        assert!(!query("\u{2066}x").is_valid());
        assert!(!AgentSearchRequest { limit: Some(51), ..query("x") }.is_valid());
        assert!(!AgentSearchRequest { limit: Some(0), ..query("x") }.is_valid());
        assert!(!AgentSearchRequest { cursor: Some("c".repeat(257)), ..query("x") }.is_valid());
        assert_eq!(query("x").limit(), history::DEFAULT_HITS);
        assert_eq!(one_line("a\nb\u{202E}c"), "a b c");
        let hits = AgentHits { next: Some("a\nb".into()), ..Default::default() };
        assert_eq!(hits.sanitized().next, None);
    }

    #[test]
    fn marks_take_up_to_fifty_threads_and_a_month() {
        let ids = |n: usize| (0..n).map(|i| format!("{i:032x}")).collect::<Vec<_>>();
        let mark = |n| AgentMark { threads: ids(n), pinned: Some(true), ..Default::default() };
        assert!(mark(1).is_valid());
        assert!(mark(50).is_valid());
        assert!(!mark(51).is_valid());
        assert!(!mark(0).is_valid());
        assert!(!AgentMark { threads: ids(1), ..Default::default() }.is_valid(), "nothing to change");
        let twice = AgentMark {
            threads: vec![THREAD.into(), THREAD.into()],
            unread: Some(true),
            ..Default::default()
        };
        assert!(!twice.is_valid());
        let now = 1_795_000_000_000;
        let snooze = |until| AgentMark { snooze: Some(until), ..mark(1) };
        assert!(snooze(0).is_snooze_in_range(now), "0 brings it back");
        assert!(snooze(now + 3_600_000).is_snooze_in_range(now));
        assert!(snooze(now + history::MAX_SNOOZE_MS).is_snooze_in_range(now));
        assert!(!snooze(now + history::MAX_SNOOZE_MS + 1).is_snooze_in_range(now));
        assert!(!snooze(now - 1).is_snooze_in_range(now), "in the past");
    }

    #[test]
    fn renames_forks_and_rewinds_check_their_fields() {
        assert!(!AgentRename { thread: THREAD.into(), title: " ".into() }.is_valid());
        assert!(!AgentRename { thread: THREAD.into(), title: "x".repeat(81) }.is_valid());
        assert!(!AgentRename { thread: THREAD.into(), title: "a\nb".into() }.is_valid());
        assert!(!AgentForkRequest { thread: THREAD.into(), turn: Some(0) }.is_valid());
        assert!(AgentForkRequest { thread: THREAD.into(), turn: None }.is_valid());
        assert!(!AgentRewindPreview { thread: THREAD.into(), turn: 0, files: false }.is_valid());
        // Asking for the files is well-formed; the PC refuses it (`DENIED`).
        assert!(AgentRewindPreview { thread: THREAD.into(), turn: 1, files: true }.is_valid());
        assert!(!AgentRewindApply { token: "NOPE".into(), ..Default::default() }.is_valid());
        assert!(!AgentRewindPlan { token: String::new(), ..Default::default() }.is_valid());
        let preview = AgentRewindPreview { thread: THREAD.into(), turn: 1, files: false };
        assert!(!keys(&preview).contains(&"files".to_owned()), "a phone's preview never names files");
    }

    #[test]
    fn found_and_import_requests_check_their_fields() {
        let ok = AgentFoundRequest::default();
        assert!(ok.is_valid());
        assert_eq!(ok.limit(), history::DEFAULT_FOUND);
        assert!(!AgentFoundRequest { query: Some("x".repeat(129)), ..ok.clone() }.is_valid());
        assert!(!AgentFoundRequest { query: Some("a\tb".into()), ..ok.clone() }.is_valid());
        assert!(!AgentFoundRequest { show: Some("old".into()), ..ok.clone() }.is_valid());
        assert!(!AgentFoundRequest { limit: Some(101), ..ok.clone() }.is_valid());
        assert!(!AgentFoundRequest { agent: Some("Claude".into()), ..ok }.is_valid());
        assert!(!AgentFoundPreview { id: FOUND.into(), limit: Some(51) }.is_valid());

        let ids = |n: usize| (0..n).map(|i| format!("sc{i:032x}")).collect::<Vec<_>>();
        assert!(AgentImportRequest { ids: ids(50), cancel: None }.is_valid());
        assert!(!AgentImportRequest { ids: ids(51), cancel: None }.is_valid());
        assert!(!AgentImportRequest { ids: Vec::new(), cancel: None }.is_valid());
        assert!(!AgentImportRequest { ids: ids(1), cancel: Some(JOB.into()) }.is_valid(), "one or the other");
        assert!(!AgentImportRequest { ids: Vec::new(), cancel: Some("xyz".into()) }.is_valid());
        assert!(!AgentImportRequest { ids: vec![FOUND.into(), FOUND.into()], cancel: None }.is_valid());
        assert!(!AgentImportJob { job: "ABC".into(), total: 1 }.is_valid());
    }

    #[test]
    fn events_check_their_fields() {
        assert!(AgentCut { thread: THREAD.into(), from_seq: 3, to_seq: 3, restored: false }.is_valid());
        assert!(!AgentCut { thread: THREAD.into(), from_seq: 4, to_seq: 3, restored: false }.is_valid());
        assert!(!AgentCut { thread: THREAD.into(), from_seq: 0, to_seq: 3, restored: false }.is_valid());
        let progress = AgentImportProgress { job: JOB.into(), done: 1, failed: 1, total: 2, thread: None };
        assert!(progress.is_valid() && progress.is_finished());
        assert!(!AgentImportProgress { done: 2, ..progress.clone() }.is_valid(), "more than total");
        assert!(
            !AgentImportProgress { done: u32::MAX, failed: u32::MAX, total: u32::MAX, ..progress }.is_valid()
        );
        let event = AgentTurnEvent { thread: THREAD.into(), turn: AgentTurn { turn: 0, ..turn(1) } };
        assert!(!event.is_valid());
        assert!(event.sanitized().is_none());
    }

    #[test]
    fn a_notice_says_which_one_it_is() {
        let notice = AgentItem {
            seq: 5,
            id: "n:5".into(),
            kind: agent::item::NOTICE.into(),
            notice: Some(history::notice::REWOUND.into()),
            ..Default::default()
        };
        assert!(notice.is_valid());
        round_trip(types::AGENT_EVENT, &notice);
        assert_eq!(notice.clone().sanitized().unwrap().notice.as_deref(), Some("rewound"));
        let later = AgentItem { notice: Some("time_warp".into()), ..notice.clone() };
        assert_eq!(later.sanitized().unwrap().notice, None, "a word not known here reads as absent");
        let answer = AgentItem { kind: agent::item::ASSISTANT.into(), ..notice };
        assert_eq!(answer.sanitized().unwrap().notice, None, "only a notice says which one");
    }

    #[test]
    fn a_fork_origin_is_cleaned_or_dropped() {
        let origin = AgentForkedFrom { thread: THREAD.into(), turn: Some(0), title: "a\u{1b}b".into() };
        assert!(!origin.is_valid());
        let origin = origin.sanitized().unwrap();
        assert_eq!((origin.turn, origin.title.as_str()), (None, "ab"));
    }

    #[test]
    fn keys_and_jobs_are_sixteen_hex_digits() {
        assert!(is_send_key("0123456789abcdef"));
        assert!(!is_send_key("0123456789ABCDEF"));
        assert!(!is_send_key("0123456789abcde"));
        assert!(is_import_job(JOB));
        assert!(!is_import_job("g123456789abcdef"));
    }

    #[test]
    fn nothing_prints_text_paths_or_tokens() {
        let shown = format!(
            "{:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
            hit(),
            found(),
            AgentTurnFile { path: "secret/dir.rs".into(), ..Default::default() },
            AgentPatchRequest { path: "secret/dir.rs".into(), ..Default::default() },
            AgentPatch { patch: "+secret".into(), ..Default::default() },
            AgentSearchRequest {
                query: "secret".into(),
                cursor: Some("secret".into()),
                ..Default::default()
            },
            AgentRename { thread: THREAD.into(), title: "secret".into() },
            AgentRewindPlan { token: TOKEN.into(), ..Default::default() },
            AgentForkedFrom { thread: THREAD.into(), turn: None, title: "secret".into() },
        );
        assert!(!shown.contains("secret") && !shown.contains(TOKEN), "{shown}");
        assert!(!shown.contains("login") && !shown.contains("Fix the"), "{shown}");
        let found_query =
            format!("{:?}", AgentFoundRequest { query: Some("secret".into()), ..Default::default() });
        assert!(!found_query.contains("secret"));
        let apply = format!("{:?}", AgentRewindApply { token: TOKEN.into(), ..Default::default() });
        assert!(!apply.contains(TOKEN));
    }

    #[test]
    fn a_thread_carries_its_marks_fork_and_undo() {
        let thread = AgentThread {
            id: THREAD.into(),
            agent: "claude".into(),
            folder: "g7linux".into(),
            title: "Fork of a fix".into(),
            state: agent::state::DONE.into(),
            mode: agent::mode::SUPERVISED.into(),
            updated: 1,
            pinned: true,
            archived: true,
            unread: true,
            snoozed_until_ms: Some(1_795_000_000_000),
            turns: 4,
            forked_from: Some(AgentForkedFrom {
                thread: THREAD.into(),
                turn: Some(2),
                title: "A fix".into(),
            }),
            found: true,
            readonly: Some(history::readonly::FOLDER_MISSING.into()),
            undo: Some(AgentUndo { until_ms: 9, files: true }),
            ..Default::default()
        };
        assert!(thread.is_valid());
        round_trip(types::AGENT_THREAD, &thread);
        assert_eq!(thread.clone().sanitized(), Some(thread.clone()));
        // A phone without `agent.control` still gets them: §6 needs no capability.
        let legacy = thread.clone().for_legacy();
        assert!(legacy.pinned && legacy.undo.is_some() && legacy.forked_from.is_some());
        // An older thread reads without them.
        let plain = AgentThread { pinned: false, archived: false, unread: false, ..thread.clone() };
        let wire = keys(&AgentThread {
            snoozed_until_ms: None,
            turns: 0,
            forked_from: None,
            found: false,
            readonly: None,
            undo: None,
            ..plain
        });
        for key in [
            "pinned",
            "archived",
            "unread",
            "snoozed_until_ms",
            "turns",
            "forked_from",
            "found",
            "readonly",
            "undo",
        ] {
            assert!(!wire.iter().any(|k| k == key), "{key} written while unset");
        }
        let bad = AgentThread {
            forked_from: Some(AgentForkedFrom { thread: "../x".into(), ..Default::default() }),
            ..thread
        };
        assert!(!bad.is_valid());
        assert_eq!(bad.sanitized().unwrap().forked_from, None, "a bad fork origin goes, the thread stays");
    }

    #[test]
    fn a_held_message_carries_its_key_and_a_folder_its_switch() {
        use crate::messages::{AgentCatalogFolder, AgentSend, AgentStart};
        let send = AgentSend {
            thread: THREAD.into(),
            text: "hi".into(),
            key: Some("0123456789abcdef".into()),
            ..Default::default()
        };
        assert!(send.is_valid());
        round_trip(types::AGENT_SEND, &send);
        assert!(!AgentSend { key: Some("not-a-key".into()), ..send.clone() }.is_valid());
        assert!(!AgentSend { key: Some("0123456789ABCDEF".into()), ..send.clone() }.is_valid());
        assert!(!keys(&AgentSend { key: None, ..send }).contains(&"key".to_owned()));
        let start = AgentStart {
            agent: "claude".into(),
            folder: "f1".into(),
            prompt: "hi".into(),
            mode: agent::mode::SUPERVISED.into(),
            key: Some("0123456789abcdef".into()),
            ..Default::default()
        };
        assert!(start.is_valid());
        assert!(!AgentStart { key: Some("x".into()), ..start }.is_valid());
        let folder = AgentCatalogFolder { id: "f1".into(), label: "g7linux".into(), checkpoints: true };
        round_trip(
            types::AGENT_CATALOG,
            &crate::messages::AgentCatalog { folders: vec![folder], ..Default::default() },
        );
        #[derive(Serialize)]
        struct OldFolder {
            id: String,
            label: String,
        }
        let old = OldFolder { id: "f1".into(), label: "x".into() };
        let old: AgentCatalogFolder = Envelope::new(types::AGENT_CATALOG, &old).unwrap().body().unwrap();
        assert!(!old.checkpoints, "a PC that doesn't say keeps none");
    }
}
