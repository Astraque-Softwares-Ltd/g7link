// SPDX-License-Identifier: MPL-2.0
//! A PC's agent threads' history and undo (`docs/protocol/agents.md` §6):
//! turns and what each changed, search, marks and renames, forks, rewinding
//! the conversation, sessions found on the PC, and on this phone the saved
//! threads and the messages held until the PC is back.
//!
//! Plain protocol v1: no capability. Everything follows the PC's `agents`
//! toggle (`Denied` when off); what was found on the PC also its
//! `agent_sessions` toggle. A phone never changes files on the PC: a rewind
//! from here is the conversation's only. No `Debug` here prints text,
//! paths or titles (protocol v1 §11).

use g7link_core as core;

use crate::{
    AgentItem, AgentItemKind, AgentSessionSource, AgentThread, AgentThreadState, G7LinkError, G7LinkNode,
    Result, agent_rows::AgentCheckpointWhy, ms, parse_id,
};

/// `Debug` for a record that prints only the fields named.
macro_rules! quiet_debug {
    ($ty:ident { $($field:ident),* }) => {
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($ty))$(.field(stringify!($field), &self.$field))*.finish_non_exhaustive()
            }
        }
    };
}

fn unix_ms(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

// ---- What a thread gains ----

/// Where a fork came from: "Fork of <title>".
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentForkedFrom {
    pub thread: String,
    /// The turn it continues from; null: the latest when it was made.
    pub turn: Option<u32>,
    pub title: String,
}

quiet_debug!(AgentForkedFrom { thread, turn });

impl From<core::AgentForkedFrom> for AgentForkedFrom {
    fn from(f: core::AgentForkedFrom) -> Self {
        Self { thread: f.thread, turn: f.turn, title: f.title }
    }
}

/// A rewind that can still be undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AgentUndo {
    pub until_ms: i64,
    /// It put files back: "Undo it on <PC>." (Undo disabled here).
    pub files: bool,
}

impl From<core::AgentUndo> for AgentUndo {
    fn from(u: core::AgentUndo) -> Self {
        Self { until_ms: ms(u.until_ms), files: u.files }
    }
}

/// Why a thread can be read but not continued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentReadonly {
    /// "Add <folder> to your agent folders to continue it."
    OutsideFolders,
    /// Its agent folder is no longer listed on the PC.
    FolderMissing,
}

impl AgentReadonly {
    pub(crate) fn from_wire(word: &str) -> Option<Self> {
        use core::history::readonly;
        Some(match word {
            readonly::OUTSIDE_FOLDERS => Self::OutsideFolders,
            readonly::FOLDER_MISSING => Self::FolderMissing,
            _ => return None,
        })
    }
}

/// Which notice a `Notice` item is, for those the app acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentNotice {
    /// "Rewound to here": carries Undo while the thread's `undo` is set.
    Rewound,
    /// "Rewound to here" after Edit from here.
    EditFromHere,
    /// "Restored the files to before this message. …"
    FilesRestored,
    /// "Forked from here. …", a fork's first item.
    Forked,
}

impl AgentNotice {
    pub(crate) fn from_wire(word: &str) -> Option<Self> {
        use core::history::notice;
        Some(match word {
            notice::REWOUND => Self::Rewound,
            notice::EDIT_FROM_HERE => Self::EditFromHere,
            notice::FILES_RESTORED => Self::FilesRestored,
            notice::FORKED => Self::Forked,
            _ => return None,
        })
    }
}

/// Which threads `agent_threads` lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentThreadShow {
    /// Neither archived nor snoozed.
    Main,
    Archived,
    Snoozed,
}

impl AgentThreadShow {
    pub(crate) fn wire(self) -> &'static str {
        use core::history::show;
        match self {
            Self::Main => show::MAIN,
            Self::Archived => show::ARCHIVED,
            Self::Snoozed => show::SNOOZED,
        }
    }
}

// ---- Turns and diffs ----

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentTurnEnd {
    Done,
    Failed,
    Stopped,
}

/// A turn's restore point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentCheckpoint {
    /// Being recorded ("Recording changes…").
    Pending,
    Ready,
    /// Not recorded; `why` says why.
    Missing,
    /// The folder isn't a git repository.
    NotGit,
    /// "This restore point is no longer kept."
    Expired,
}

/// One turn of a thread on a PC.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentTurn {
    pub turn: u32,
    /// The message that started it.
    pub prompt_seq: u64,
    pub last_seq: Option<u64>,
    pub started_ms: i64,
    pub ended_ms: Option<i64>,
    pub ended: Option<AgentTurnEnd>,
    /// Null when the PC said a word this build doesn't know.
    pub checkpoint: Option<AgentCheckpoint>,
    pub why: Option<AgentCheckpointWhy>,
    /// Files changed, lines added and removed (with `Ready`).
    pub files: u32,
    pub adds: u32,
    pub dels: u32,
    pub more_files: bool,
    /// Large files left out of its restore point.
    pub skipped: u32,
    /// The PC's verdicts: "Fork from here", "Rewind to here" / "Edit from here".
    pub can_fork: bool,
    pub can_rewind: bool,
}

impl From<core::AgentTurn> for AgentTurn {
    fn from(t: core::AgentTurn) -> Self {
        use core::history::{checkpoint, turn_end, why};
        Self {
            turn: t.turn,
            prompt_seq: t.prompt_seq,
            last_seq: t.last_seq,
            started_ms: ms(t.started_ms),
            ended_ms: t.ended_ms.map(ms),
            ended: t.ended.as_deref().and_then(|w| match w {
                turn_end::DONE => Some(AgentTurnEnd::Done),
                turn_end::FAILED => Some(AgentTurnEnd::Failed),
                turn_end::STOPPED => Some(AgentTurnEnd::Stopped),
                _ => None,
            }),
            checkpoint: t.checkpoint.as_deref().and_then(|w| match w {
                checkpoint::PENDING => Some(AgentCheckpoint::Pending),
                checkpoint::READY => Some(AgentCheckpoint::Ready),
                checkpoint::MISSING => Some(AgentCheckpoint::Missing),
                checkpoint::NONE => Some(AgentCheckpoint::NotGit),
                checkpoint::EXPIRED => Some(AgentCheckpoint::Expired),
                _ => None,
            }),
            why: t.why.as_deref().and_then(|w| match w {
                why::BUSY => Some(AgentCheckpointWhy::Busy),
                why::SLOW => Some(AgentCheckpointWhy::Slow),
                why::LARGE => Some(AgentCheckpointWhy::Large),
                why::REFUSED => Some(AgentCheckpointWhy::Refused),
                why::OFF => Some(AgentCheckpointWhy::Off),
                why::BEFORE => Some(AgentCheckpointWhy::Before),
                _ => None,
            }),
            files: t.files,
            adds: t.adds,
            dels: t.dels,
            more_files: t.more_files,
            skipped: t.skipped,
            can_fork: t.can_fork,
            can_rewind: t.can_rewind,
        }
    }
}

/// What happened to a file in a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    /// Its type changed (a file became a link).
    TypeChanged,
    /// Too large to record: no diff ("%n files weren't recorded (too large).").
    Unrecorded,
}

impl AgentFileStatus {
    fn from_wire(word: &str) -> Option<Self> {
        use core::history::file_status;
        Some(match word {
            file_status::ADDED => Self::Added,
            file_status::MODIFIED => Self::Modified,
            file_status::DELETED => Self::Deleted,
            file_status::RENAMED => Self::Renamed,
            file_status::TYPE => Self::TypeChanged,
            file_status::UNRECORDED => Self::Unrecorded,
            _ => return None,
        })
    }
}

/// A file a turn changed, relative to the thread's folder.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentTurnFile {
    pub path: String,
    /// Null when the PC said a word this build doesn't know: show it as changed.
    pub status: Option<AgentFileStatus>,
    /// Its earlier path (`Renamed`).
    pub from: Option<String>,
    pub adds: u32,
    pub dels: u32,
    pub binary: bool,
}

quiet_debug!(AgentTurnFile { status, adds, dels, binary });

impl From<core::AgentTurnFile> for AgentTurnFile {
    fn from(f: core::AgentTurnFile) -> Self {
        Self {
            status: f.status.as_deref().and_then(AgentFileStatus::from_wire),
            path: f.path,
            from: f.from,
            adds: f.adds,
            dels: f.dels,
            binary: f.binary,
        }
    }
}

/// The files a turn or a range changed: at most 500 (`more` past that);
/// `adds` and `dels` count them all.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentTurnFiles {
    pub files: Vec<AgentTurnFile>,
    pub more: bool,
    pub adds: u32,
    pub dels: u32,
}

impl From<core::AgentTurnFiles> for AgentTurnFiles {
    fn from(f: core::AgentTurnFiles) -> Self {
        Self {
            files: f.files.into_iter().map(Into::into).collect(),
            more: f.more,
            adds: f.adds,
            dels: f.dels,
        }
    }
}

/// The turns a diff covers: one, or from `from` to `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentTurnRange {
    /// "This turn".
    Turn { turn: u32 },
    /// "Whole thread": from the first turn to the last.
    Span { from: u32, to: u32 },
}

impl From<AgentTurnRange> for core::TurnRange {
    fn from(r: AgentTurnRange) -> Self {
        match r {
            AgentTurnRange::Turn { turn } => Self::Turn(turn),
            AgentTurnRange::Span { from, to } => Self::Span { from, to },
        }
    }
}

/// One file's patch, at most 1 MiB: read it with `agent_diff` on a worker.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPatch {
    pub patch: String,
    /// It was cut: "Showing the first 1 MB".
    pub truncated: bool,
    /// "Binary file": no text.
    pub binary: bool,
}

quiet_debug!(AgentPatch { truncated, binary });

/// A turn's first files (from `agent_diff_files`), for its `Changes` row
/// (`AgentTimeline.rows`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentTurnPaths {
    pub turn: u32,
    pub files: Vec<AgentTurnFile>,
}

// ---- Search ----

/// A match in a hit's snippet, in UTF-16 units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AgentSnippetMark {
    pub start: u32,
    pub len: u32,
}

/// A search hit: a message, or a thread's title (`seq` 0).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentHit {
    pub thread: String,
    pub title: String,
    pub agent: String,
    pub folder: String,
    pub state: AgentThreadState,
    pub archived: bool,
    /// The message that matched; 0: the title. Open the thread there.
    pub seq: u64,
    pub kind: Option<AgentItemKind>,
    pub when_ms: i64,
    /// One line around the first match; draw `marks` over it.
    pub snippet: String,
    pub marks: Vec<AgentSnippetMark>,
    /// "%n more in this thread".
    pub more_in_thread: u32,
}

quiet_debug!(AgentHit { thread, seq, kind, more_in_thread });

impl AgentHit {
    fn from_core(h: core::AgentHit) -> Option<Self> {
        Some(Self {
            state: AgentThreadState::from_wire(&h.state)?,
            kind: h.kind.as_deref().and_then(AgentItemKind::from_wire),
            thread: h.thread,
            title: h.title,
            agent: h.agent,
            folder: h.folder,
            archived: h.archived,
            seq: h.seq,
            when_ms: ms(h.when),
            snippet: h.snippet,
            marks: h
                .marks
                .into_iter()
                .map(|(start, len)| AgentSnippetMark { start: start.into(), len: len.into() })
                .collect(),
            more_in_thread: h.more_in_thread,
        })
    }
}

/// A page of search hits, newest first.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentHits {
    pub hits: Vec<AgentHit>,
    /// Pass as `cursor` for the next page; null: there's none.
    pub next: Option<String>,
    /// Only titles were searched: "Type 3 letters or more to search messages."
    pub titles_only: bool,
}

impl From<core::AgentHits> for AgentHits {
    fn from(h: core::AgentHits) -> Self {
        Self {
            hits: h.hits.into_iter().filter_map(AgentHit::from_core).collect(),
            next: h.next,
            titles_only: h.titles_only,
        }
    }
}

// ---- Rewind ----

/// Why a rewind can't be done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentRewindBlocked {
    /// "Another thread worked in this folder since then."
    Shared,
    /// "Git is in the middle of something in this folder."
    Busy,
    /// "No restore point for this turn."
    NoCheckpoint,
    /// "This isn't a git repository, so G7 can't undo file changes here."
    NotGit,
    /// "This restore point is no longer kept."
    Expired,
    /// Restore points are off for the folder.
    Off,
    /// "%1 can only be forked from its latest message."
    Agent,
}

/// What a rewind of the conversation would leave out. Nothing has changed.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentRewindPlan {
    /// For `agent_rewind_apply`, once, before `expires_ms`.
    pub token: String,
    pub expires_ms: i64,
    /// "%n later messages leave the thread."
    pub turns: u32,
    pub messages: u32,
    pub blocked: Option<AgentRewindBlocked>,
}

quiet_debug!(AgentRewindPlan { expires_ms, turns, messages, blocked });

impl From<core::AgentRewindPlan> for AgentRewindPlan {
    fn from(p: core::AgentRewindPlan) -> Self {
        use core::history::rewind_blocked as b;
        Self {
            blocked: p.blocked.as_deref().and_then(|w| match w {
                b::SHARED => Some(AgentRewindBlocked::Shared),
                b::BUSY => Some(AgentRewindBlocked::Busy),
                b::NO_CHECKPOINT => Some(AgentRewindBlocked::NoCheckpoint),
                b::NOT_GIT => Some(AgentRewindBlocked::NotGit),
                b::EXPIRED => Some(AgentRewindBlocked::Expired),
                b::OFF => Some(AgentRewindBlocked::Off),
                b::AGENT => Some(AgentRewindBlocked::Agent),
                _ => None,
            }),
            token: p.token,
            expires_ms: ms(p.expires_ms),
            turns: p.turns,
            messages: p.messages,
        }
    }
}

// ---- Sessions found on the PC ----

/// Which found sessions `agent_found` lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentFoundShow {
    All,
    /// Not brought in yet.
    New,
}

/// A Claude or Codex session on a PC. Its folder is a label, never a path.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentFound {
    pub id: String,
    pub agent: String,
    pub title: String,
    pub folder: String,
    /// Set when it's in an agent folder: only then can it continue.
    pub folder_id: Option<String>,
    pub first_ms: Option<i64>,
    pub updated_ms: i64,
    pub bytes: u64,
    pub packed: bool,
    pub source: AgentSessionSource,
    /// "Open in a terminal now".
    pub open: bool,
    /// "Brought in": the thread it became.
    pub thread: Option<String>,
    /// "Damaged": it can't be brought in.
    pub damaged: bool,
}

quiet_debug!(AgentFound { id, agent, source, open, thread, damaged });

impl From<core::AgentFound> for AgentFound {
    fn from(f: core::AgentFound) -> Self {
        Self {
            source: AgentSessionSource::from_wire(&f.source),
            id: f.id,
            agent: f.agent,
            title: f.title,
            folder: f.folder,
            folder_id: f.folder_id,
            first_ms: f.first_ms.map(ms),
            updated_ms: ms(f.updated_ms),
            bytes: f.bytes,
            packed: f.packed,
            open: f.open,
            thread: f.thread,
            damaged: f.damaged,
        }
    }
}

/// A page of found sessions.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentFoundList {
    pub found: Vec<AgentFound>,
    /// Pass as `before_ms` for the next page; null: there's none.
    pub next_ms: Option<i64>,
    pub total: u32,
    /// The PC is still looking ("Looking for sessions… %1 so far"): ask again.
    pub scanning: bool,
    /// The PC stopped at one of its bounds.
    pub partial: bool,
}

impl From<core::AgentFoundList> for AgentFoundList {
    fn from(f: core::AgentFoundList) -> Self {
        Self {
            found: f.found.into_iter().map(Into::into).collect(),
            next_ms: f.next_ms.map(ms),
            total: f.total,
            scanning: f.scanning,
            partial: f.partial,
        }
    }
}

/// An import job: sessions brought in as threads.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentImportJob {
    pub job: String,
    pub total: u32,
}

/// How an import job goes; the last has `done + failed == total`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentImportProgress {
    pub job: String,
    pub done: u32,
    pub failed: u32,
    pub total: u32,
    /// The thread the session just brought in became.
    pub thread: Option<String>,
}

impl From<core::AgentImportProgress> for AgentImportProgress {
    fn from(p: core::AgentImportProgress) -> Self {
        Self { job: p.job, done: p.done, failed: p.failed, total: p.total, thread: p.thread }
    }
}

// ---- Saved threads and held messages ----

/// Where a held message stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentHeldState {
    /// "Sends when <PC> is back" / "Waiting to send".
    Waiting,
    /// "Sending".
    Sending,
    /// The PC is back but can't take it yet: `why`.
    Blocked,
    /// "Not sent": offer Send and Discard; `why` says why.
    NotSent,
}

/// Why a held message isn't on its way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentHeldWhy {
    /// "Answer the request first".
    AnswerFirst,
    /// "Waiting: the queue on <PC> is full".
    QueueFull,
    /// The thread is gone from the PC.
    Gone,
    /// The thread can't be continued.
    ReadOnly,
    /// It waited more than a day: never sent by itself.
    TooOld,
    /// The PC refused it.
    Refused,
}

/// A message written while its PC couldn't take it. It goes once, in order,
/// when the PC is back.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentHeld {
    pub key: String,
    pub thread: String,
    pub text: String,
    pub created_ms: i64,
    pub state: AgentHeldState,
    pub why: Option<AgentHeldWhy>,
}

quiet_debug!(AgentHeld { thread, state, why });

impl From<core::AgentHeld> for AgentHeld {
    fn from(h: core::AgentHeld) -> Self {
        use core::{HeldState as S, HeldWhy as W};
        Self {
            state: match h.state {
                S::Waiting => AgentHeldState::Waiting,
                S::Sending => AgentHeldState::Sending,
                S::Blocked => AgentHeldState::Blocked,
                S::NotSent => AgentHeldState::NotSent,
            },
            why: h.why.map(|w| match w {
                W::AnswerFirst => AgentHeldWhy::AnswerFirst,
                W::QueueFull => AgentHeldWhy::QueueFull,
                W::Gone => AgentHeldWhy::Gone,
                W::ReadOnly => AgentHeldWhy::ReadOnly,
                W::TooOld => AgentHeldWhy::TooOld,
                W::Refused => AgentHeldWhy::Refused,
            }),
            key: h.key,
            thread: h.thread,
            text: h.text,
            created_ms: ms(h.created_ms),
        }
    }
}

/// A PC's threads as saved on this phone: "Saved %2 ago" from `saved_ms`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentCachedThreads {
    pub threads: Vec<AgentThread>,
    /// Null when nothing was saved.
    pub saved_ms: Option<i64>,
}

/// A thread's items as saved on this phone, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentCachedItems {
    pub thread: Option<AgentThread>,
    pub items: Vec<AgentItem>,
    pub saved_ms: Option<i64>,
}

/// One unwrapped id of a PC.
fn pc(id: &str) -> Result<core::DeviceId> {
    parse_id(id)
}

fn thread_or_internal(thread: core::AgentThread) -> Result<AgentThread> {
    AgentThread::from_core(thread)
        .ok_or_else(|| G7LinkError::Internal { reason: "unknown thread state".into() })
}

// Errors as the other agent methods: `Denied` when the PC's `agents` toggle
// is off for this phone (and for `agent_found*`/`agent_import`, its
// `agent_sessions` toggle); `NotFound` an unknown thread, turn, token or
// session; `BUSY` (`Internal`) while a thread works or waits.
#[uniffi::export]
impl G7LinkNode {
    /// A thread's turns, newest first: before turn `before`, at most `limit`
    /// (1 to 100; 30 when null).
    #[uniffi::method(default(before = None, limit = None))]
    pub async fn agent_turns(
        &self,
        id: String,
        thread: String,
        before: Option<u32>,
        limit: Option<u32>,
    ) -> Result<Vec<AgentTurn>> {
        let id = pc(&id)?;
        let node = self.node.clone();
        let turns = self.run(async move { node.agent_turns(id, thread, before, limit).await }).await?;
        Ok(turns.into_iter().map(Into::into).collect())
    }

    /// The files a turn or a range of turns changed. `Unsupported` while
    /// they weren't recorded (the turn's `checkpoint` isn't `Ready`).
    pub async fn agent_diff_files(
        &self,
        id: String,
        thread: String,
        range: AgentTurnRange,
    ) -> Result<AgentTurnFiles> {
        let id = pc(&id)?;
        let node = self.node.clone();
        let files = self.run(async move { node.agent_diff_files(id, thread, range.into()).await }).await?;
        Ok(files.into())
    }

    /// One file's patch, as `agent_diff_files` listed its path.
    pub async fn agent_diff_patch(
        &self,
        id: String,
        thread: String,
        range: AgentTurnRange,
        path: String,
    ) -> Result<AgentPatch> {
        let id = pc(&id)?;
        let node = self.node.clone();
        let patch =
            self.run(async move { node.agent_diff_patch(id, thread, range.into(), path).await }).await?;
        Ok(AgentPatch { patch: patch.patch, truncated: patch.truncated, binary: patch.binary })
    }

    /// Searches what a PC holds: messages and titles, newest first. Words of
    /// one or two letters search titles only (`titles_only`).
    #[uniffi::method(default(limit = None, cursor = None, archived = false))]
    pub async fn agent_search(
        &self,
        id: String,
        query: String,
        limit: Option<u32>,
        cursor: Option<String>,
        archived: bool,
    ) -> Result<AgentHits> {
        let id = pc(&id)?;
        let node = self.node.clone();
        let request = core::AgentSearchRequest { query, limit, cursor, archived };
        let hits = self.run(async move { node.agent_search(id, request).await }).await?;
        Ok(hits.into())
    }

    /// Pins, archives, marks unread or snoozes up to 50 threads; null leaves
    /// a mark as it is. `snooze_until_ms`: hidden until then (at most 30 days
    /// ahead), 0 brings it back. Archiving a working thread is skipped
    /// (`BUSY` when it's the only one). The threads that changed.
    #[uniffi::method(default(pinned = None, archived = None, unread = None, snooze_until_ms = None))]
    pub async fn agent_mark(
        &self,
        id: String,
        threads: Vec<String>,
        pinned: Option<bool>,
        archived: Option<bool>,
        unread: Option<bool>,
        snooze_until_ms: Option<i64>,
    ) -> Result<Vec<AgentThread>> {
        let id = pc(&id)?;
        let node = self.node.clone();
        let mark =
            core::AgentMark { threads, pinned, archived, unread, snooze: snooze_until_ms.map(unix_ms) };
        let threads = self.run(async move { node.agent_mark(id, mark).await }).await?;
        Ok(threads.into_iter().filter_map(AgentThread::from_core).collect())
    }

    /// Renames a thread (one line, 1 to 80 bytes).
    pub async fn agent_rename(&self, id: String, thread: String, title: String) -> Result<AgentThread> {
        let id = pc(&id)?;
        let node = self.node.clone();
        thread_or_internal(self.run(async move { node.agent_rename(id, thread, title).await }).await?)
    }

    /// A new thread from a copy of this one up to `turn` (the latest when
    /// null): "Forked from here. The files in %1 are as they are now."
    #[uniffi::method(default(turn = None))]
    pub async fn agent_fork(&self, id: String, thread: String, turn: Option<u32>) -> Result<AgentThread> {
        let id = pc(&id)?;
        let node = self.node.clone();
        thread_or_internal(self.run(async move { node.agent_fork(id, thread, turn).await }).await?)
    }

    /// What rewinding the conversation to before `turn` would leave out.
    /// Files aren't changed from the phone ("Do that on %1.").
    pub async fn agent_rewind_preview(
        &self,
        id: String,
        thread: String,
        turn: u32,
    ) -> Result<AgentRewindPlan> {
        let id = pc(&id)?;
        let node = self.node.clone();
        Ok(self.run(async move { node.agent_rewind_preview(id, thread, turn).await }).await?.into())
    }

    /// Rewinds as the preview said; `refill` puts the message back in the
    /// composer (edit from here, through the thread's draft).
    #[uniffi::method(default(refill = false))]
    pub async fn agent_rewind_apply(&self, id: String, token: String, refill: bool) -> Result<AgentThread> {
        let id = pc(&id)?;
        let node = self.node.clone();
        thread_or_internal(self.run(async move { node.agent_rewind_apply(id, token, refill).await }).await?)
    }

    /// Undoes the last rewind while the thread's `undo` is set and its
    /// `files` isn't (that one is undone on the PC).
    pub async fn agent_rewind_undo(&self, id: String, thread: String) -> Result<AgentThread> {
        let id = pc(&id)?;
        let node = self.node.clone();
        thread_or_internal(self.run(async move { node.agent_rewind_undo(id, thread).await }).await?)
    }

    /// The Claude and Codex sessions on a PC, newest first; `query` matches
    /// title and folder.
    #[uniffi::method(default(query = None, agent = None, show = None, before_ms = None, limit = None))]
    pub async fn agent_found(
        &self,
        id: String,
        query: Option<String>,
        agent: Option<String>,
        show: Option<AgentFoundShow>,
        before_ms: Option<i64>,
        limit: Option<u32>,
    ) -> Result<AgentFoundList> {
        let id = pc(&id)?;
        let node = self.node.clone();
        let show = show.map(|s| {
            match s {
                AgentFoundShow::All => core::history::found_show::ALL_SESSIONS,
                AgentFoundShow::New => core::history::found_show::NEW,
            }
            .to_owned()
        });
        let request =
            core::AgentFoundRequest { query, agent, show, before_ms: before_ms.map(unix_ms), limit };
        Ok(self.run(async move { node.agent_found(id, request).await }).await?.into())
    }

    /// A found session's newest messages, read now and kept nowhere.
    #[uniffi::method(default(limit = None))]
    pub async fn agent_found_preview(
        &self,
        id: String,
        session: String,
        limit: Option<u32>,
    ) -> Result<Vec<AgentItem>> {
        let id = pc(&id)?;
        let node = self.node.clone();
        let items = self.run(async move { node.agent_found_preview(id, session, limit).await }).await?;
        Ok(items.into_iter().filter_map(AgentItem::from_core).collect())
    }

    /// Brings found sessions in as threads (1 to 50); progress comes as
    /// `Event.AgentImport`. One job at a time (`BUSY`).
    pub async fn agent_import(&self, id: String, sessions: Vec<String>) -> Result<AgentImportJob> {
        let id = pc(&id)?;
        let node = self.node.clone();
        let job = self.run(async move { node.agent_import(id, sessions).await }).await?;
        Ok(AgentImportJob { job: job.job, total: job.total })
    }

    /// Stops an import job.
    pub async fn agent_import_cancel(&self, id: String, job: String) -> Result<()> {
        let id = pc(&id)?;
        let node = self.node.clone();
        Ok(self.run(async move { node.agent_import_cancel(id, job).await }).await?)
    }

    /// A PC's threads as saved on this phone, at once and offline.
    pub async fn agent_cached_threads(&self, id: String) -> Result<AgentCachedThreads> {
        let id = pc(&id)?;
        let node = self.node.clone();
        let cached = self.run(async move { node.agent_cached_threads(id).await }).await?;
        Ok(AgentCachedThreads {
            threads: cached.value.into_iter().filter_map(AgentThread::from_core).collect(),
            saved_ms: cached.saved_ms.map(ms),
        })
    }

    /// A thread's items as saved on this phone, oldest first.
    pub async fn agent_cached_items(&self, id: String, thread: String) -> Result<AgentCachedItems> {
        let id = pc(&id)?;
        let node = self.node.clone();
        let cached = self.run(async move { node.agent_cached_items(id, thread).await }).await?;
        Ok(AgentCachedItems {
            thread: cached.value.thread.and_then(AgentThread::from_core),
            items: cached.value.items.into_iter().filter_map(AgentItem::from_core).collect(),
            saved_ms: cached.saved_ms.map(ms),
        })
    }

    /// A PC's held messages, in the order written; changes come as
    /// `Event.AgentOutbox`.
    pub async fn agent_outbox(&self, id: String) -> Result<Vec<AgentHeld>> {
        let id = pc(&id)?;
        let node = self.node.clone();
        Ok(self.run(async move { node.agent_outbox(id).await }).await?.into_iter().map(Into::into).collect())
    }

    /// Sends a held message that wasn't sent ("Send"): it goes when its PC
    /// and thread can take it. `NotFound` when it's no longer held.
    pub async fn agent_outbox_send(&self, id: String, key: String) -> Result<()> {
        let id = pc(&id)?;
        let node = self.node.clone();
        Ok(self.run(async move { node.agent_outbox_send(id, key).await }).await?)
    }

    /// Forgets a held message ("Discard").
    pub async fn agent_outbox_discard(&self, id: String, key: String) -> Result<()> {
        let id = pc(&id)?;
        let node = self.node.clone();
        Ok(self.run(async move { node.agent_outbox_discard(id, key).await }).await?)
    }

    /// Forgets every saved thread ("Clear saved threads"); held messages stay.
    pub async fn agent_cache_clear(&self) -> Result<()> {
        let node = self.node.clone();
        Ok(self.run(async move { node.agent_cache_clear().await }).await?)
    }

    /// What the saved threads take on disk ("Clear saved threads (%1)").
    pub async fn agent_cache_bytes(&self) -> Result<u64> {
        let node = self.node.clone();
        Ok(self.run(async move { node.agent_cache_bytes().await }).await?)
    }

    /// "Keep threads on this phone": off forgets what was saved and saves
    /// nothing more; held messages still wait and go. Remembered.
    pub async fn agent_cache_keep(&self, keep: bool) -> Result<()> {
        let node = self.node.clone();
        Ok(self.run(async move { node.agent_cache_keep(keep).await }).await?)
    }

    /// Whether threads are kept on this phone.
    pub async fn agent_cache_keeps(&self) -> Result<bool> {
        let node = self.node.clone();
        Ok(self.run(async move { node.agent_cache_keeps().await }).await?)
    }
}
