// SPDX-License-Identifier: GPL-3.0-or-later
//! Phase 3 of the Agents page, without I/O: a thread's turns for the
//! timeline, search, bringing in sessions found on this PC, the diff, and the
//! rewind dialog; and what phones get of them (never a path outside a folder,
//! a `cwd`, or a file restore).
//!
//! Everything here is data for the page; its words are the page's own
//! (`qsTr`), except the few the host writes (the reasons a file was left).

use std::collections::{HashMap, HashSet};

use g7_agents::api::{
    self, AgentId, Checkpoint, CheckpointWhy, FileStatus, Folder, Found, ItemKind, RewindAction, RewindBlock,
    RewindPlan, SearchHit, SessionsScanResult, ThreadInfo, ThreadsSearchResult, TurnFile, TurnFilesResult,
    TurnInfo,
};
use g7_agents_render as render;
use g7link_core::{
    AgentForkedFrom, AgentFound, AgentFoundList, AgentHit, AgentHits, AgentRewindPlan, AgentThread,
    AgentTurn, AgentTurnFile, AgentTurnFiles, AgentUndo, TurnRange as WireRange,
};
use serde_json::{Value, json};

use super::{rows, shown_path, source::QuoteRow, word};
use crate::i18n::tr;

// ---- Turns, for the timeline ----

/// The notices a client acts on; source control's are only their text.
pub(crate) fn render_notice(notice: api::NoticeKind) -> Option<render::NoticeKind> {
    match notice {
        api::NoticeKind::Rewound => Some(render::NoticeKind::Rewound),
        api::NoticeKind::EditFromHere => Some(render::NoticeKind::EditFromHere),
        api::NoticeKind::FilesRestored => Some(render::NoticeKind::FilesRestored),
        api::NoticeKind::Forked => Some(render::NoticeKind::Forked),
        api::NoticeKind::Committed
        | api::NoticeKind::Pushed
        | api::NoticeKind::Switched
        | api::NoticeKind::PrOpened
        | api::NoticeKind::PrMerged => None,
    }
}

fn render_checkpoint(checkpoint: Checkpoint) -> render::Checkpoint {
    match checkpoint {
        Checkpoint::Pending => render::Checkpoint::Pending,
        Checkpoint::Ready => render::Checkpoint::Ready,
        Checkpoint::Missing => render::Checkpoint::Missing,
        Checkpoint::NotGit => render::Checkpoint::NotGit,
        Checkpoint::Expired => render::Checkpoint::Expired,
    }
}

fn render_why(why: CheckpointWhy) -> render::CheckpointWhy {
    match why {
        CheckpointWhy::Busy => render::CheckpointWhy::Busy,
        CheckpointWhy::Slow => render::CheckpointWhy::Slow,
        CheckpointWhy::Large => render::CheckpointWhy::Large,
        CheckpointWhy::Refused => render::CheckpointWhy::Refused,
        CheckpointWhy::Off => render::CheckpointWhy::Off,
        CheckpointWhy::Before => render::CheckpointWhy::Before,
    }
}

fn render_status(status: FileStatus) -> render::TurnFileStatus {
    match status {
        FileStatus::Added => render::TurnFileStatus::Added,
        FileStatus::Modified => render::TurnFileStatus::Modified,
        FileStatus::Deleted => render::TurnFileStatus::Deleted,
        FileStatus::Renamed => render::TurnFileStatus::Renamed,
        FileStatus::Type => render::TurnFileStatus::Type,
        FileStatus::Unrecorded => render::TurnFileStatus::Unrecorded,
    }
}

/// A file of `turn.files` as the timeline reads it.
pub(crate) fn file_in(file: &TurnFile) -> render::TurnFileIn<'_> {
    render::TurnFileIn {
        path: &file.path,
        status: render_status(file.status),
        adds: file.adds,
        dels: file.dels,
    }
}

/// A turn as the timeline reads it, with the first of its files once read.
pub(crate) fn turn_in<'a>(turn: &TurnInfo, paths: &'a [render::TurnFileIn<'a>]) -> render::TurnIn<'a> {
    render::TurnIn {
        turn: turn.turn,
        checkpoint: render_checkpoint(turn.checkpoint),
        why: turn.why.map(render_why),
        ended_ms: turn.ended_ms,
        files: turn.files,
        adds: turn.adds,
        dels: turn.dels,
        skipped: turn.skipped,
        paths,
        can_fork: turn.can_fork,
        can_rewind: turn.can_rewind,
        can_restore: turn.can_restore,
    }
}

/// The files of `turn.files` a turn's Changes row names (the timeline shows
/// at most this many).
pub(crate) const PATHS_KEPT: usize = render::bounds::MAX_CHANGED_FILES;

/// Puts `turn` among `turns` (ordered by number), replacing the one with its number.
pub(crate) fn upsert_turn(turns: &mut Vec<TurnInfo>, turn: TurnInfo) {
    match turns.binary_search_by_key(&turn.turn, |t| t.turn) {
        Ok(at) => turns[at] = turn,
        Err(at) => turns.insert(at, turn),
    }
}

/// Recorded turns whose files the Changes row needs and nobody asked for yet.
pub(crate) fn paths_wanted(
    turns: &[TurnInfo],
    known: &HashMap<u32, Vec<TurnFile>>,
    asked: &HashSet<u32>,
) -> Vec<u32> {
    turns
        .iter()
        .filter(|t| t.checkpoint == Checkpoint::Ready && t.files > 0)
        .map(|t| t.turn)
        .filter(|n| !known.contains_key(n) && !asked.contains(n))
        .collect()
}

/// When a turn still recording its changes must be drawn again: a second
/// after it ended, the row says "Recording changes…".
pub(crate) fn recording_due(turns: &[TurnInfo], now_ms: u64) -> Option<u64> {
    turns
        .iter()
        .filter(|t| t.checkpoint == Checkpoint::Pending)
        .filter_map(|t| t.ended_ms)
        .map(|ended| ended.saturating_add(render::bounds::RECORDING_AFTER_MS))
        .filter(|due| *due > now_ms)
        .min()
}

/// The newest turns that are recorded one after another up to the newest
/// recorded one: what "Whole thread" covers (`None` when nothing is).
pub(crate) fn whole_span(turns: &[TurnInfo]) -> Option<(u32, u32)> {
    let mut ready = turns.iter().rev().skip_while(|t| t.checkpoint != Checkpoint::Ready);
    let last = ready.next()?.turn;
    let mut first = last;
    for t in ready {
        if t.checkpoint != Checkpoint::Ready || t.turn + 1 != first {
            break;
        }
        first = t.turn;
    }
    Some((first, last))
}

// ---- Lists ----

/// Snoozed now: out of the main list until then.
pub(crate) fn is_snoozed(info: &ThreadInfo, now_ms: u64) -> bool {
    info.snoozed_until_ms.is_some_and(|until| until > now_ms)
}

/// A snooze the host takes: in the future and at most 30 days ahead (0 ends one).
pub(crate) fn snooze_ok(until_ms: u64, now_ms: u64) -> bool {
    until_ms == 0 || (until_ms > now_ms && until_ms - now_ms <= api::MAX_SNOOZE_MS)
}

/// The fields phase 3 adds to a thread's row on the page.
pub(crate) fn thread_extras(t: &ThreadInfo, agent: Option<&api::AgentInfo>, now_ms: u64) -> Value {
    json!({
        "pinned": t.pinned,
        "archived": t.archived,
        "unread": t.unread,
        "snoozedUntil": t.snoozed_until_ms.filter(|_| is_snoozed(t, now_ms)).unwrap_or(0),
        "turns": t.turns,
        "forkOf": t.forked_from.as_ref().map(|f| json!({ "id": f.thread_id, "title": f.title, "turn": f.turn }))
            .unwrap_or_else(|| json!({})),
        "found": t.found,
        "readonly": t.readonly.as_ref().map(word).unwrap_or_default(),
        "undo": t.undo.filter(|u| u.until_ms > now_ms).map(|u| json!({ "until": u.until_ms, "files": u.files }))
            .unwrap_or_else(|| json!({})),
        "limitResets": agent.and_then(|a| limit_resets(a, now_ms)).unwrap_or(0),
    })
}

/// When the agent's limit that holds it back resets: the reached window's,
/// else the one near its end; `None` while it reports none ahead.
pub(crate) fn limit_resets(agent: &api::AgentInfo, now_ms: u64) -> Option<u64> {
    let ahead = |state| {
        agent
            .limits
            .iter()
            .filter(|l| l.state == state)
            .filter_map(|l| l.resets_ms)
            .filter(|r| *r > now_ms)
            .min()
    };
    ahead(api::LimitState::Reached).or_else(|| ahead(api::LimitState::Warn))
}

// ---- Search ----

/// The list's search: what was typed, and the newest answer.
#[derive(Debug, Default)]
pub(crate) struct Search {
    pub query: String,
    pub archived: bool,
    /// Counts searches: only the newest one's answer is shown.
    pub seq: u64,
    pub loading: bool,
    pub hits: Vec<SearchHit>,
    pub titles_only: bool,
    pub failed: bool,
}

impl Search {
    /// The user typed (or turned archived threads on or off): the number of
    /// the search to run after the pause, or `None` when the field is empty
    /// or nothing changed.
    pub fn typed(&mut self, query: &str, archived: bool) -> Option<u64> {
        let query = api::cut(query.trim(), api::MAX_SEARCH_QUERY).to_owned();
        if query == self.query && archived == self.archived {
            return None;
        }
        self.query = query;
        self.archived = archived;
        self.seq += 1;
        self.failed = false;
        if self.query.is_empty() {
            self.loading = false;
            self.hits.clear();
            self.titles_only = false;
            return None;
        }
        self.loading = true;
        Some(self.seq)
    }

    /// Search `seq` answered; false when a newer one replaced it.
    pub fn answered(&mut self, seq: u64, result: Option<ThreadsSearchResult>) -> bool {
        if seq != self.seq {
            return false;
        }
        self.loading = false;
        match result {
            Some(result) => {
                self.hits = result.hits;
                self.titles_only = result.titles_only;
                self.failed = false;
            }
            None => {
                self.hits.clear();
                self.failed = true;
            }
        }
        true
    }

    pub fn json(&self, threads: &HashMap<String, ThreadInfo>) -> Value {
        let mut order: Vec<&str> = Vec::new();
        let mut by_thread: HashMap<&str, Vec<&SearchHit>> = HashMap::new();
        for hit in &self.hits {
            let list = by_thread.entry(hit.thread_id.as_str()).or_insert_with(|| {
                order.push(hit.thread_id.as_str());
                Vec::new()
            });
            list.push(hit);
        }
        let groups: Vec<Value> = order
            .iter()
            .map(|id| {
                let hits = &by_thread[id];
                let first = hits[0];
                let info = threads.get(*id);
                let shown: Vec<Value> = hits
                    .iter()
                    .filter(|h| h.seq > 0)
                    .take(api::MAX_HITS_PER_THREAD)
                    .map(|h| hit_json(h, first.agent))
                    .collect();
                json!({
                    "thread": id,
                    "title": info.map_or(first.title.as_str(), |t| t.title.as_str()),
                    "agent": first.agent.as_str(),
                    "agentName": first.agent.display_name(),
                    "folder": first.folder_label,
                    "state": word(&info.map_or(first.state, |t| t.state)),
                    "archived": first.archived,
                    "updatedMs": info.map_or_else(|| hits.iter().map(|h| h.ts_ms).max().unwrap_or(0), |t| t.updated_ms),
                    "more": hits.iter().map(|h| h.more_in_thread).max().unwrap_or(0),
                    "hits": shown,
                })
            })
            .collect();
        json!({
            "query": self.query,
            "archived": self.archived,
            "loading": self.loading,
            "titlesOnly": self.titles_only,
            "failed": self.failed,
            "groups": groups,
        })
    }
}

/// A hit as the list draws it: the snippet as markup with the matches bold
/// (every character escaped), who wrote it, and when.
fn hit_json(hit: &SearchHit, agent: AgentId) -> Value {
    let marks: Vec<(u32, u32)> = hit.marks.iter().map(|&(s, l)| (u32::from(s), u32::from(l))).collect();
    let parts = render::snippet(&hit.snippet, &marks);
    let mut markup = String::new();
    let mut plain = String::new();
    for part in &parts {
        let text = rows::escaped(&part.text, false);
        if part.mark {
            markup.push_str("<b>");
            markup.push_str(&text);
            markup.push_str("</b>");
        } else {
            markup.push_str(&text);
        }
        plain.push_str(&part.text);
    }
    let who = match hit.kind {
        Some(ItemKind::User) => "you",
        Some(ItemKind::Assistant | ItemKind::Reasoning | ItemKind::Plan | ItemKind::Tool) => "agent",
        _ => "",
    };
    json!({
        "seq": hit.seq.to_string(),
        "who": who,
        "agentName": agent.display_name(),
        "when": hit.ts_ms,
        "markup": markup,
        "text": plain,
    })
}

// ---- Bringing in sessions ----

/// An import that runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Job {
    pub id: String,
    pub ids: Vec<String>,
    pub done: u32,
    pub failed: u32,
    pub total: u32,
}

/// The "Bring in earlier sessions" screen.
#[derive(Debug, Default)]
pub(crate) struct Import {
    pub query: String,
    pub agent: Option<AgentId>,
    /// Counts scans asked for: only the newest answer is kept.
    pub seq: u64,
    pub loading: bool,
    pub found: Vec<Found>,
    pub total: u32,
    pub scanning: bool,
    pub partial: bool,
    pub next_ms: Option<u64>,
    pub failed: bool,
    /// Ids picked to bring in, in the order picked.
    pub selected: Vec<String>,
    pub job: Option<Job>,
    /// The last job ended: how many came in, and the sessions that couldn't be read.
    pub done: Option<(u32, Vec<Found>)>,
}

/// Where a found session is grouped: its agent folder, else its own folder.
fn group_key(found: &Found) -> String {
    match &found.folder_id {
        Some(id) => format!("f:{id}"),
        None => format!("d:{}", found.cwd),
    }
}

impl Import {
    /// A session can be picked: not brought in already, and readable.
    pub fn selectable(found: &Found) -> bool {
        !found.damaged && found.thread_id.is_none()
    }

    /// A new scan is asked for (the query or the agent changed): its number.
    pub fn rescan(&mut self) -> u64 {
        self.seq += 1;
        self.loading = true;
        self.seq
    }

    /// Scan `seq` answered; a page after the first adds to the list.
    pub fn scanned(&mut self, seq: u64, more: bool, result: Option<SessionsScanResult>) -> bool {
        if seq != self.seq {
            return false;
        }
        self.loading = false;
        match result {
            Some(r) => {
                if more {
                    let have: HashSet<String> = self.found.iter().map(|f| f.id.clone()).collect();
                    self.found.extend(r.found.into_iter().filter(|f| !have.contains(&f.id)));
                } else {
                    self.found = r.found;
                }
                self.total = r.total;
                self.scanning = r.scanning;
                self.partial = r.partial;
                self.next_ms = r.next_ms;
                self.failed = false;
                // A session brought in meanwhile can't be picked any more.
                let found = &self.found;
                self.selected.retain(|id| found.iter().find(|f| &f.id == id).is_none_or(Import::selectable));
            }
            None => self.failed = true,
        }
        true
    }

    /// Picks a session, or leaves it; at most 50 at a time.
    pub fn toggle(&mut self, id: &str) {
        if let Some(at) = self.selected.iter().position(|s| s == id) {
            self.selected.remove(at);
            return;
        }
        let ok = self.found.iter().find(|f| f.id == id).is_some_and(Import::selectable);
        if ok && self.selected.len() < api::MAX_IMPORT_IDS {
            self.selected.push(id.to_owned());
        }
    }

    /// Picks every session of a group that can be, or leaves them all when
    /// every one is picked already.
    pub fn toggle_group(&mut self, key: &str) {
        let ids: Vec<String> = self
            .found
            .iter()
            .filter(|f| group_key(f) == key && Import::selectable(f))
            .map(|f| f.id.clone())
            .collect();
        if ids.iter().all(|id| self.selected.contains(id)) {
            self.selected.retain(|id| !ids.contains(id));
            return;
        }
        for id in ids {
            if self.selected.len() >= api::MAX_IMPORT_IDS {
                break;
            }
            if !self.selected.contains(&id) {
                self.selected.push(id);
            }
        }
    }

    /// The job `job` moved on; true when it ended.
    pub fn progress(&mut self, job: &str, done: u32, failed: u32, total: u32) -> bool {
        let Some(j) = self.job.as_mut().filter(|j| j.id == job) else { return false };
        j.done = done;
        j.failed = failed;
        j.total = total;
        u64::from(done) + u64::from(failed) >= u64::from(total)
    }

    /// The job ended and the list was read again: what came in, and which
    /// of the sessions it named still aren't threads.
    pub fn finish(&mut self) {
        let Some(job) = self.job.take() else { return };
        let failed: Vec<Found> = job
            .ids
            .iter()
            .filter_map(|id| self.found.iter().find(|f| &f.id == id))
            .filter(|f| f.thread_id.is_none())
            .cloned()
            .collect();
        let brought = job.done;
        self.selected.retain(|id| !job.ids.contains(id));
        self.done = Some((brought, failed));
    }

    pub fn json(&self, folders: &[Folder], home: Option<&std::path::Path>) -> Value {
        let mut order: Vec<String> = Vec::new();
        let mut groups: HashMap<String, Vec<&Found>> = HashMap::new();
        for f in &self.found {
            let key = group_key(f);
            groups
                .entry(key.clone())
                .or_insert_with(|| {
                    order.push(key);
                    Vec::new()
                })
                .push(f);
        }
        let groups: Vec<Value> = order
            .iter()
            .map(|key| {
                let rows = &groups[key];
                let first = rows[0];
                let path = match &first.folder_id {
                    Some(id) => folders.iter().find(|f| &f.id == id).map(|f| shown_path(&f.path, home)),
                    None => None,
                }
                .unwrap_or_else(|| shown_path(&first.cwd, home));
                let pickable: Vec<&&Found> = rows.iter().filter(|f| Import::selectable(f)).collect();
                json!({
                    "key": key,
                    "path": path,
                    "label": first.folder_label,
                    "count": rows.len(),
                    "selectable": !pickable.is_empty(),
                    "selected": !pickable.is_empty() && pickable.iter().all(|f| self.selected.contains(&f.id)),
                    "rows": rows.iter().map(|f| self.found_json(f, &path)).collect::<Vec<_>>(),
                })
            })
            .collect();
        let job = self.job.as_ref().map_or_else(
            || json!({}),
            |j| json!({ "done": j.done.saturating_add(j.failed), "total": j.total.max(j.ids.len() as u32) }),
        );
        let done = self.done.as_ref().map_or_else(
            || json!({}),
            |(brought, failed)| {
                json!({
                    "brought": brought,
                    "failed": failed.iter().map(|f| self.found_json(f, &shown_path(&f.cwd, home))).collect::<Vec<_>>(),
                })
            },
        );
        json!({
            "query": self.query,
            "agent": self.agent.map(|a| a.as_str()).unwrap_or_default(),
            "loading": self.loading,
            "failed": self.failed,
            "total": self.total,
            "scanning": self.scanning,
            "partial": self.partial,
            "more": self.next_ms.is_some(),
            "selected": self.selected.len(),
            "selectedShown": self.selected.iter().filter(|id| self.found.iter().any(|f| &f.id == *id)).count(),
            "groups": groups,
            "job": job,
            "done": done,
        })
    }

    fn found_json(&self, f: &Found, path: &str) -> Value {
        let badge = if f.damaged {
            "damaged"
        } else if f.thread_id.is_some() {
            "in"
        } else if f.open {
            "open"
        } else if f.folder_id.is_none() {
            "outside"
        } else {
            ""
        };
        json!({
            "id": f.id,
            "title": f.title,
            "agent": f.agent.as_str(),
            "agentName": f.agent.display_name(),
            "path": path,
            "updatedMs": f.updated_ms,
            "source": word(&f.source),
            "badge": badge,
            "selectable": Import::selectable(f),
            "selected": self.selected.contains(&f.id),
            "thread": f.thread_id.clone().unwrap_or_default(),
        })
    }
}

// ---- The diff ----

/// The patch of the file the diff shows.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Patch {
    pub path: String,
    pub loading: bool,
    pub failed: bool,
    pub binary: bool,
    pub truncated: bool,
    /// The file's rows, as the page's diff view reads them (JSON).
    pub doc: String,
    /// The same rows as plain text, for a quote of the lines picked.
    pub rows: Vec<QuoteRow>,
}

/// The diff of a turn, or of the whole thread.
#[derive(Debug, Default)]
pub(crate) struct Diff {
    pub thread_id: String,
    /// The turn the Changes row belongs to (0: none, for a diff of
    /// what isn't committed in a thread without turns).
    pub turn: u32,
    /// What isn't committed yet, from the folder's status (§SC3.3).
    pub uncommitted: bool,
    /// The file to show first once the list is read ("" for the first).
    pub want: String,
    /// What "Whole thread" covers, when anything is recorded.
    pub whole: Option<(u32, u32)>,
    pub scope_whole: bool,
    /// Counts reads of the file list: only the newest is shown.
    pub seq: u64,
    pub loading: bool,
    /// The host can't give it (a turn in the range isn't recorded).
    pub unavailable: bool,
    pub files: Vec<TurnFile>,
    /// Files changed in all (the list may hold fewer).
    pub total: u32,
    pub more: bool,
    pub adds: u32,
    pub dels: u32,
    pub selected: usize,
    /// Counts reads of a patch: only the newest is shown.
    pub patch_seq: u64,
    pub patch: Patch,
}

impl Diff {
    pub fn range(&self) -> api::TurnRange {
        match (self.scope_whole, self.whole) {
            (true, Some((from, to))) if from < to => api::TurnRange::Span { from, to },
            (true, Some((_, to))) => api::TurnRange::One(to),
            _ => api::TurnRange::One(self.turn),
        }
    }

    /// The list for `seq` arrived; false when a newer read replaced it.
    pub fn listed(&mut self, seq: u64, result: Option<TurnFilesResult>, total: u32) -> bool {
        if seq != self.seq {
            return false;
        }
        self.loading = false;
        match result {
            Some(r) => {
                self.total = total.max(r.files.len() as u32);
                self.more = r.more;
                self.adds = r.adds;
                self.dels = r.dels;
                self.files = r.files;
                self.unavailable = false;
            }
            None => {
                self.files.clear();
                self.unavailable = true;
            }
        }
        let want = std::mem::take(&mut self.want);
        self.selected = self.files.iter().position(|f| f.path == want).unwrap_or(0);
        self.patch = Patch::default();
        true
    }

    /// The file to read next: the selected one, when it has text to show.
    pub fn wants_patch(&self) -> Option<&TurnFile> {
        self.files.get(self.selected).filter(|f| f.status != FileStatus::Unrecorded && !f.binary)
    }

    /// Moves to file `index` (wrapping with `by` when stepping); true when it moved.
    pub fn select(&mut self, index: usize) -> bool {
        if index >= self.files.len() || (index == self.selected && !self.patch.path.is_empty()) {
            return false;
        }
        self.selected = index;
        self.patch = Patch::default();
        true
    }

    pub fn step(&mut self, by: i32) -> bool {
        let len = self.files.len();
        if len == 0 {
            return false;
        }
        let len_i = i64::try_from(len).unwrap_or(i64::MAX);
        let at = (i64::try_from(self.selected).unwrap_or(0) + i64::from(by)).rem_euclid(len_i);
        self.select(usize::try_from(at).unwrap_or(0))
    }

    pub fn json(&self) -> Value {
        let files: Vec<Value> = self
            .files
            .iter()
            .map(|f| {
                json!({
                    "path": f.path,
                    "from": f.from.clone().unwrap_or_default(),
                    "status": word(&f.status),
                    "adds": f.adds,
                    "dels": f.dels,
                    "binary": f.binary,
                })
            })
            .collect();
        let selected = self.files.get(self.selected);
        json!({
            "turn": self.turn,
            "scope": if self.uncommitted { "uncommitted" } else if self.scope_whole { "thread" } else { "turn" },
            "canWhole": self.whole.is_some(),
            "canTurn": self.turn > 0,
            "loading": self.loading,
            "unavailable": self.unavailable,
            "files": files,
            "more": (self.total as usize).saturating_sub(self.files.len()),
            "adds": self.adds,
            "dels": self.dels,
            "selected": self.selected,
            "patch": {
                "path": self.patch.path,
                "loading": self.patch.loading,
                "failed": self.patch.failed,
                "binary": self.patch.binary || selected.is_some_and(|f| f.binary),
                "truncated": self.patch.truncated,
                "unrecorded": selected.is_some_and(|f| f.status == FileStatus::Unrecorded),
                "doc": self.patch.doc,
            },
        })
    }
}

/// A file's patch pages put together into the page's diff rows, on a worker.
pub(crate) fn patch_doc(text: &str, host_cut: bool, binary: bool, palette: &rows::CodePalette) -> Patch {
    let doc = render::diff(text);
    let truncated =
        host_cut || doc.more || doc.files.iter().any(|f| f.note == Some(render::DiffNote::Truncated));
    let binary = binary || doc.files.iter().any(|f| f.note == Some(render::DiffNote::Binary));
    let quote_rows = doc
        .files
        .first()
        .map(|f| {
            f.rows
                .iter()
                .take(render::bounds::MAX_DIFF_ROWS)
                .map(|r| QuoteRow {
                    old: r.old_no,
                    new: r.new_no,
                    hunk: r.kind == render::DiffKind::Hunk,
                    text: r.text.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    Patch {
        path: String::new(),
        loading: false,
        failed: false,
        binary,
        truncated,
        doc: rows::diff_json(&doc, render::bounds::MAX_DIFF_ROWS, palette).to_string(),
        rows: quote_rows,
    }
}

// ---- The rewind dialog ----

/// What a rewind puts back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Choice {
    #[default]
    Both,
    Conversation,
    Files,
}

impl Choice {
    pub fn parse(word: &str) -> Option<Choice> {
        match word {
            "both" => Some(Choice::Both),
            "conversation" => Some(Choice::Conversation),
            "files" => Some(Choice::Files),
            _ => None,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Choice::Both => "both",
            Choice::Conversation => "conversation",
            Choice::Files => "files",
        }
    }

    /// The parts `rewind.preview` is asked for: (conversation, files).
    pub fn parts(self) -> (bool, bool) {
        match self {
            Choice::Both => (true, true),
            Choice::Conversation => (true, false),
            Choice::Files => (false, true),
        }
    }
}

/// How the user asked to rewind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Confirm {
    Click,
    /// Enter or Space.
    Key,
}

/// The dialog that asks before a rewind.
#[derive(Debug, Default)]
pub(crate) struct Rewind {
    pub thread_id: String,
    pub turn: u32,
    /// The message it goes back to before.
    pub quote: String,
    pub choice: Choice,
    /// What can be done at all, from the first preview: the conversation,
    /// the files, and why not the other.
    pub can_conversation: bool,
    pub can_files: bool,
    pub blocked: Option<RewindBlock>,
    /// Why the files weren't asked for (the message's own action word).
    pub files_why: String,
    /// Counts previews: only the newest one's plan is shown.
    pub seq: u64,
    pub loading: bool,
    /// The plan of the choice shown; its token is what Rewind uses.
    pub plan: Option<RewindPlan>,
    pub include_later: bool,
    /// The files changed between the preview and Rewind: the lists were read again.
    pub conflict: bool,
    pub applying: bool,
    /// The Rewind button has the keyboard (reached with Tab): Enter may confirm.
    pub button_focused: bool,
    /// Neither part can be done (the host's own reason, in the app's words).
    pub error: String,
}

impl Rewind {
    pub fn new(thread_id: String, turn: u32, quote: String, files: bool, files_why: String) -> Self {
        Self {
            thread_id,
            turn,
            quote,
            choice: if files { Choice::Both } else { Choice::Conversation },
            can_conversation: true,
            can_files: files,
            files_why,
            loading: true,
            ..Default::default()
        }
    }

    pub fn allowed(&self, choice: Choice) -> bool {
        match choice {
            Choice::Both => self.can_conversation && self.can_files,
            Choice::Conversation => self.can_conversation,
            Choice::Files => self.can_files,
        }
    }

    /// The preview `seq` answered; the first also says what can be done.
    pub fn previewed(&mut self, seq: u64, plan: Result<RewindPlan, String>, first: bool) -> bool {
        if seq != self.seq {
            return false;
        }
        self.loading = false;
        match plan {
            Ok(plan) => {
                if first {
                    self.can_conversation = plan.cut.is_some();
                    self.can_files = plan.files.is_some();
                    self.blocked = plan.blocked;
                    if !self.allowed(self.choice) {
                        self.choice = [Choice::Both, Choice::Conversation, Choice::Files]
                            .into_iter()
                            .find(|c| self.allowed(*c))
                            .unwrap_or(Choice::Conversation);
                    }
                }
                self.error.clear();
                self.plan = Some(plan);
            }
            Err(why) => {
                self.plan = None;
                self.error = why;
            }
        }
        true
    }

    /// The user picked `choice`: the number of the preview to ask for, when
    /// it differs from the one shown and can be done.
    pub fn choose(&mut self, choice: Choice) -> Option<u64> {
        if choice == self.choice || !self.allowed(choice) || self.applying {
            return None;
        }
        self.choice = choice;
        self.include_later = false;
        self.conflict = false;
        Some(self.ask())
    }

    /// A new preview of the choice shown (the lists read again).
    pub fn ask(&mut self) -> u64 {
        self.seq += 1;
        self.loading = true;
        self.plan = None;
        self.seq
    }

    /// The token to rewind with, when `how` may confirm now: a click, or
    /// Enter on the Rewind button reached with Tab. Never a bare Enter.
    pub fn confirm(&mut self, how: Confirm) -> Option<(String, bool)> {
        if self.applying || self.loading || (how == Confirm::Key && !self.button_focused) {
            return None;
        }
        let plan = self.plan.as_ref()?;
        let files = plan.files.as_ref();
        let include_later = self.include_later && files.is_some_and(|f| f.later > 0);
        self.applying = true;
        Some((plan.token.clone(), include_later))
    }

    pub fn json(&self, agent_name: &str, folder: &str) -> Value {
        let plan = self.plan.as_ref();
        let files = plan.and_then(|p| p.files.as_ref());
        let list = |action: RewindAction, later: bool| -> Vec<&str> {
            files.map_or_else(Vec::new, |f| {
                f.change
                    .iter()
                    .filter(|c| c.action == action && c.later == later)
                    .map(|c| c.path.as_str())
                    .collect()
            })
        };
        let later: Vec<&str> = files.map_or_else(Vec::new, |f| {
            f.change.iter().filter(|c| c.later).map(|c| c.path.as_str()).collect()
        });
        json!({
            "turn": self.turn,
            "quote": self.quote,
            "agentName": agent_name,
            "folder": folder,
            "choice": self.choice.word(),
            "can": {
                "both": self.allowed(Choice::Both),
                "conversation": self.allowed(Choice::Conversation),
                "files": self.allowed(Choice::Files),
            },
            "blocked": self.blocked.as_ref().map(word).unwrap_or_default(),
            "filesWhy": self.files_why,
            "loading": self.loading,
            "applying": self.applying,
            "conflict": self.conflict,
            "error": self.error,
            "messages": plan.and_then(|p| p.cut).map_or(0, |c| c.messages),
            "turns": plan.and_then(|p| p.cut).map_or(0, |c| c.turns),
            "files": files.is_some(),
            "restore": list(RewindAction::Restore, false),
            "delete": list(RewindAction::Delete, false),
            "bringBack": list(RewindAction::BringBack, false),
            "more": files.map_or(0, |f| f.more),
            "later": later,
            "laterCount": files.map_or(0, |f| f.later.max(later.len() as u32)),
            "includeLater": self.include_later,
            "skipped": files.map_or(0, |f| f.skipped),
            "headMoved": files.is_some_and(|f| f.head_moved),
            "ready": plan.is_some() && !self.loading,
        })
    }
}

/// A file a rewind left, with its reason in the app's words (the host
/// writes English).
pub(crate) fn left_reason(why: &str) -> String {
    match why {
        "changed after the agent finished" => tr!("Agents", "changed after the agent finished"),
        "changed since you looked" => tr!("Agents", "changed since you looked"),
        "changed into a folder" => tr!("Agents", "changed into a folder"),
        "not a file" => tr!("Agents", "not a file"),
        "couldn't be changed back" => tr!("Agents", "couldn't be changed back"),
        other => other.to_owned(),
    }
}

// ---- What phones get ----

/// A turn as a phone sees it: never whether its files can be put back.
pub(crate) fn wire_turn(t: &TurnInfo) -> AgentTurn {
    AgentTurn {
        turn: t.turn,
        prompt_seq: t.prompt_seq,
        last_seq: t.last_seq,
        started_ms: t.started_ms,
        ended_ms: t.ended_ms,
        ended: t.ended.as_ref().map(word),
        checkpoint: Some(word(&t.checkpoint)),
        why: t.why.as_ref().map(word),
        files: t.files,
        adds: t.adds,
        dels: t.dels,
        more_files: t.more_files,
        skipped: t.skipped,
        can_fork: t.can_fork,
        can_rewind: t.can_rewind,
    }
}

pub(crate) fn api_range(range: WireRange) -> api::TurnRange {
    match range {
        WireRange::Turn(turn) => api::TurnRange::One(turn),
        WireRange::Span { from, to } => api::TurnRange::Span { from, to },
    }
}

/// A turn's files as a phone sees them: relative to the folder, as the host gives them.
pub(crate) fn wire_files(r: TurnFilesResult) -> AgentTurnFiles {
    AgentTurnFiles {
        files: r
            .files
            .into_iter()
            .map(|f| AgentTurnFile {
                status: Some(word(&f.status)),
                path: f.path,
                from: f.from,
                adds: f.adds,
                dels: f.dels,
                binary: f.binary,
            })
            .collect(),
        more: r.more,
        adds: r.adds,
        dels: r.dels,
    }
}

pub(crate) fn wire_hits(r: ThreadsSearchResult) -> AgentHits {
    AgentHits {
        hits: r
            .hits
            .into_iter()
            .map(|h| AgentHit {
                thread: h.thread_id,
                title: h.title,
                agent: h.agent.as_str().to_owned(),
                folder: h.folder_label,
                state: word(&h.state),
                archived: h.archived,
                seq: h.seq,
                kind: h.kind.as_ref().map(word),
                when: h.ts_ms,
                snippet: h.snippet,
                marks: h.marks,
                more_in_thread: h.more_in_thread,
            })
            .collect(),
        next: r.next,
        titles_only: r.titles_only,
    }
}

/// Sessions found here as a phone sees them: a folder's name, never a `cwd`.
pub(crate) fn wire_found(r: SessionsScanResult) -> AgentFoundList {
    AgentFoundList {
        found: r
            .found
            .into_iter()
            .map(|f| AgentFound {
                id: f.id,
                agent: f.agent.as_str().to_owned(),
                title: f.title,
                folder: f.folder_label,
                folder_id: f.folder_id,
                first_ms: f.first_ms,
                updated_ms: f.updated_ms,
                bytes: f.bytes,
                packed: f.packed,
                source: word(&f.source),
                open: f.open,
                thread: f.thread_id,
                damaged: f.damaged,
            })
            .collect(),
        next_ms: r.next_ms,
        total: r.total,
        scanning: r.scanning,
        partial: r.partial,
    }
}

/// A conversation rewind's plan as a phone sees it: what leaves the
/// thread, never a file. `None` when the conversation can't go back.
pub(crate) fn wire_plan(plan: &RewindPlan) -> Option<AgentRewindPlan> {
    let cut = plan.cut?;
    Some(AgentRewindPlan {
        token: plan.token.clone(),
        expires_ms: plan.expires_ms,
        turns: cut.turns,
        messages: cut.messages,
        blocked: plan.blocked.filter(|b| !b.blocks_files()).as_ref().map(word),
    })
}

/// What phase 3 adds to a thread as a phone sees it.
pub(crate) fn wire_thread_extras(info: &ThreadInfo, thread: &mut AgentThread) {
    thread.pinned = info.pinned;
    thread.archived = info.archived;
    thread.unread = info.unread;
    thread.snoozed_until_ms = info.snoozed_until_ms;
    thread.turns = info.turns;
    thread.forked_from = info.forked_from.as_ref().map(|f| AgentForkedFrom {
        thread: f.thread_id.clone(),
        turn: f.turn,
        title: f.title.clone(),
    });
    thread.found = info.found;
    thread.readonly = info.readonly.as_ref().map(word);
    thread.undo = info.undo.map(|u| AgentUndo { until_ms: u.until_ms, files: u.files });
}

#[cfg(test)]
mod tests {
    use g7_agents::api::{RewindCut, RewindFile, RewindFiles, SessionSource, State};

    use super::*;

    fn turn(n: u32, checkpoint: Checkpoint) -> TurnInfo {
        TurnInfo {
            turn: n,
            prompt_seq: u64::from(n) * 10,
            last_seq: None,
            started_ms: 1,
            ended_ms: None,
            ended: None,
            checkpoint,
            why: None,
            files: 2,
            adds: 3,
            dels: 1,
            more_files: false,
            skipped: 0,
            can_fork: true,
            can_rewind: true,
            can_restore: true,
        }
    }

    fn found(id: &str, folder: Option<&str>, thread: Option<&str>, damaged: bool) -> Found {
        Found {
            id: id.into(),
            agent: AgentId::Codex,
            title: format!("Session {id}"),
            folder_label: "shamba-api".into(),
            folder_id: folder.map(Into::into),
            cwd: "/home/me/kazi/shamba-api".into(),
            first_ms: None,
            updated_ms: 5,
            bytes: 10,
            packed: true,
            source: SessionSource::Terminal,
            open: false,
            thread_id: thread.map(Into::into),
            damaged,
        }
    }

    #[test]
    fn turns_stay_ordered_and_recorded_ones_are_read_once() {
        let mut turns = Vec::new();
        upsert_turn(&mut turns, turn(3, Checkpoint::Pending));
        upsert_turn(&mut turns, turn(1, Checkpoint::Ready));
        upsert_turn(&mut turns, turn(2, Checkpoint::Missing));
        upsert_turn(&mut turns, turn(3, Checkpoint::Ready));
        assert_eq!(turns.iter().map(|t| t.turn).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(turns[2].checkpoint, Checkpoint::Ready, "a newer word replaces the turn");
        let known = HashMap::from([(1, Vec::new())]);
        assert_eq!(paths_wanted(&turns, &known, &HashSet::new()), [3]);
        assert!(paths_wanted(&turns, &known, &HashSet::from([3])).is_empty(), "asked already");
        assert_eq!(whole_span(&turns), Some((3, 3)), "turn 2 breaks the run");
        turns[1].checkpoint = Checkpoint::Ready;
        assert_eq!(whole_span(&turns), Some((1, 3)));
        assert_eq!(whole_span(&[turn(1, Checkpoint::NotGit)]), None);
    }

    #[test]
    fn a_turn_still_recording_is_drawn_again_a_second_after_it_ended() {
        let mut pending = turn(1, Checkpoint::Pending);
        assert_eq!(recording_due(&[pending.clone()], 5_000), None, "still running");
        pending.ended_ms = Some(4_500);
        assert_eq!(recording_due(&[pending.clone()], 5_000), Some(5_500));
        assert_eq!(recording_due(&[pending.clone()], 5_500), None, "the second has passed");
        let ready = TurnInfo { ended_ms: Some(4_900), ..turn(2, Checkpoint::Ready) };
        assert_eq!(recording_due(&[ready], 5_000), None);
    }

    #[test]
    fn a_snooze_is_in_the_future_and_within_thirty_days() {
        assert!(snooze_ok(0, 100), "0 ends it");
        assert!(!snooze_ok(100, 100));
        assert!(snooze_ok(101, 100));
        assert!(snooze_ok(100 + api::MAX_SNOOZE_MS, 100));
        assert!(!snooze_ok(101 + api::MAX_SNOOZE_MS, 100));
    }

    fn hit(thread: &str, seq: u64, more: u32) -> SearchHit {
        SearchHit {
            thread_id: thread.into(),
            title: format!("Thread {thread}"),
            agent: AgentId::Claude,
            folder_label: "duka-pos".into(),
            state: State::Done,
            archived: false,
            seq,
            kind: Some(if seq.is_multiple_of(2) { ItemKind::User } else { ItemKind::Assistant }),
            ts_ms: 100 + seq,
            snippet: "the retry <waits>".into(),
            marks: vec![(4, 5)],
            more_in_thread: more,
        }
    }

    #[test]
    fn a_newer_search_replaces_an_older_one() {
        let mut search = Search::default();
        let first = search.typed("re", false).unwrap();
        let second = search.typed("retry", false).unwrap();
        assert_eq!(search.typed(" retry ", false), None, "the same query asks nothing");
        let result = ThreadsSearchResult { hits: vec![hit("a", 2, 0)], next: None, titles_only: false };
        assert!(
            !search.answered(first, Some(ThreadsSearchResult { titles_only: true, ..Default::default() }))
        );
        assert!(search.loading, "the newer one still runs");
        assert!(search.answered(second, Some(result)));
        assert!(!search.loading && !search.titles_only);
        assert_eq!(search.hits.len(), 1);
        assert!(search.typed("retry", true).is_some(), "archived on asks again");
        assert_eq!(search.typed("", true), None);
        assert!(search.hits.is_empty() && !search.loading, "an empty field shows the list");
    }

    #[test]
    fn hits_are_grouped_by_thread_with_their_matches_marked_and_escaped() {
        let mut search = Search::default();
        let seq = search.typed("retry", false).unwrap();
        let hits = vec![hit("a", 2, 0), hit("b", 3, 0), hit("a", 5, 2), hit("a", 0, 0)];
        search.answered(seq, Some(ThreadsSearchResult { hits, next: None, titles_only: false }));
        let json = search.json(&HashMap::new());
        let groups = json["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0]["thread"], "a");
        assert_eq!(groups[0]["more"], 2);
        assert_eq!(groups[0]["hits"].as_array().unwrap().len(), 2, "the title match is the group itself");
        let first = &groups[0]["hits"][0];
        assert_eq!(first["who"], "you");
        assert_eq!(first["markup"], "the <b>retry</b> &lt;waits&gt;");
        assert_eq!(first["text"], "the retry <waits>");
        assert_eq!(groups[1]["hits"][0]["who"], "agent");
    }

    #[test]
    fn sessions_are_picked_by_row_or_by_folder_and_never_twice() {
        let mut import = Import::default();
        let seq = import.rescan();
        let result = SessionsScanResult {
            found: vec![
                found("sx1", Some("f1"), None, false),
                found("sx2", Some("f1"), Some("t1"), false),
                found("sx3", Some("f1"), None, true),
                found("sx4", None, None, false),
            ],
            next_ms: Some(4),
            total: 9,
            scanning: true,
            partial: false,
        };
        assert!(import.scanned(seq, false, Some(result)));
        import.toggle("sx2");
        import.toggle("sx3");
        assert!(import.selected.is_empty(), "brought in or damaged: not picked");
        import.toggle_group("f:f1");
        assert_eq!(import.selected, ["sx1"]);
        import.toggle_group("f:f1");
        assert!(import.selected.is_empty(), "all picked: the group is left");
        import.toggle("sx4");
        let json = import.json(&[], Some(std::path::Path::new("/home/me")));
        assert_eq!(json["groups"].as_array().unwrap().len(), 2);
        assert_eq!(json["groups"][1]["path"], "~/kazi/shamba-api");
        let badges: Vec<&str> = json["groups"][0]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["badge"].as_str().unwrap())
            .collect();
        assert_eq!(badges, ["", "in", "damaged"]);
        assert_eq!(json["groups"][1]["rows"][0]["badge"], "outside");
        assert!(json.to_string().contains("shamba-api"));
        // Another scan answering late changes nothing.
        let newer = import.rescan();
        assert!(!import.scanned(seq, false, None));
        assert!(import.scanned(newer, false, None) && import.failed);
    }

    #[test]
    fn at_most_fifty_are_brought_in_at_once() {
        let mut import = Import::default();
        let seq = import.rescan();
        let found: Vec<Found> = (0..60).map(|i| found(&format!("sx{i}"), Some("f1"), None, false)).collect();
        import.scanned(seq, false, Some(SessionsScanResult { found, ..Default::default() }));
        import.toggle_group("f:f1");
        assert_eq!(import.selected.len(), api::MAX_IMPORT_IDS);
        import.toggle("sx59");
        assert_eq!(import.selected.len(), api::MAX_IMPORT_IDS);
    }

    #[test]
    fn a_finished_import_lists_what_couldnt_be_read() {
        let job = Job { id: "j".into(), ids: vec!["sx1".into(), "sx2".into()], done: 0, failed: 0, total: 2 };
        let mut import = Import { job: Some(job), ..Import::default() };
        assert!(!import.progress("other", 1, 0, 2));
        assert!(!import.progress("j", 1, 0, 2));
        assert!(import.progress("j", 1, 1, 2));
        let seq = import.rescan();
        let result = SessionsScanResult {
            found: vec![found("sx1", Some("f1"), Some("t1"), false), found("sx2", Some("f1"), None, false)],
            ..Default::default()
        };
        import.scanned(seq, false, Some(result));
        import.finish();
        let (brought, failed) = import.done.clone().unwrap();
        assert_eq!(brought, 1);
        assert_eq!(failed.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["sx2"]);
        assert!(import.job.is_none());
    }

    #[test]
    fn the_diff_covers_a_turn_or_the_whole_thread_and_steps_through_files() {
        let mut diff = Diff { turn: 3, whole: Some((1, 3)), ..Default::default() };
        assert_eq!(diff.range(), api::TurnRange::One(3));
        diff.scope_whole = true;
        assert_eq!(diff.range(), api::TurnRange::Span { from: 1, to: 3 });
        diff.whole = Some((3, 3));
        assert_eq!(diff.range(), api::TurnRange::One(3), "one recorded turn is a span of one");
        diff.seq = 1;
        let file = |path: &str, status, binary| TurnFile {
            path: path.into(),
            status,
            from: None,
            adds: 1,
            dels: 0,
            binary,
        };
        let result = TurnFilesResult {
            files: vec![
                file("a.rs", FileStatus::Modified, false),
                file("logo.png", FileStatus::Added, true),
                file("big.json", FileStatus::Unrecorded, false),
            ],
            more: true,
            adds: 3,
            dels: 0,
        };
        assert!(!diff.listed(0, None, 0), "an older read");
        assert!(diff.listed(1, Some(result), 600));
        let json = diff.json();
        assert_eq!(json["more"], 597);
        assert!(diff.wants_patch().is_some());
        assert!(diff.step(1));
        assert!(diff.wants_patch().is_none(), "binary: no text to read");
        assert_eq!(diff.json()["patch"]["binary"], true);
        assert!(diff.step(1));
        assert_eq!(diff.json()["patch"]["unrecorded"], true);
        assert!(diff.step(1));
        assert_eq!(diff.selected, 0, "it wraps");
    }

    #[test]
    fn a_cut_patch_says_so_and_a_binary_one_has_no_rows() {
        let patch = patch_doc(
            "diff --git a/a b/a\n@@ -1 +1 @@\n-a\n+b\n",
            false,
            false,
            &rows::CodePalette::default(),
        );
        assert!(!patch.truncated && !patch.binary);
        assert!(patch.doc.contains("\"add\""));
        assert!(
            patch_doc("@@ -1,9 +1,9 @@\n-a\n", false, false, &rows::CodePalette::default()).truncated,
            "it stops inside a hunk"
        );
        assert!(patch_doc("", true, false, &rows::CodePalette::default()).truncated, "the host cut it");
        assert!(
            patch_doc(
                "diff --git a/x b/x\nBinary files a/x and b/x differ\n",
                false,
                false,
                &rows::CodePalette::default()
            )
            .binary
        );
    }

    fn plan(files: bool, cut: bool) -> RewindPlan {
        RewindPlan {
            token: "ab".repeat(16),
            expires_ms: 9,
            thread_id: "t".into(),
            turn: 2,
            cut: cut.then_some(RewindCut { turns: 2, messages: 5 }),
            files: files.then(|| RewindFiles {
                change: vec![
                    RewindFile { path: "a.rs".into(), action: RewindAction::Restore, later: false },
                    RewindFile { path: "new.rs".into(), action: RewindAction::Delete, later: false },
                    RewindFile { path: "old.rs".into(), action: RewindAction::BringBack, later: false },
                    RewindFile { path: "Cargo.lock".into(), action: RewindAction::Restore, later: true },
                ],
                more: 0,
                later: 1,
                skipped: 2,
                head_moved: true,
                tree: "0".repeat(40),
            }),
            blocked: (!files).then_some(RewindBlock::Shared),
        }
    }

    #[test]
    fn the_rewind_dialog_offers_what_can_be_done_and_says_why_not() {
        let mut rewind = Rewind::new("t".into(), 2, "Ongeza backoff".into(), true, String::new());
        rewind.seq = 1;
        assert!(rewind.previewed(1, Ok(plan(false, true)), true));
        assert_eq!(rewind.choice, Choice::Conversation, "files blocked: conversation only");
        assert!(!rewind.allowed(Choice::Both) && !rewind.allowed(Choice::Files));
        let json = rewind.json("Claude", "duka-pos");
        assert_eq!(json["blocked"], "shared");
        assert_eq!(json["messages"], 5);
        assert_eq!(rewind.choose(Choice::Files), None, "a blocked choice can't be picked");

        let mut rewind = Rewind::new("t".into(), 2, String::new(), true, String::new());
        rewind.seq = 1;
        rewind.previewed(1, Ok(plan(true, true)), true);
        assert_eq!(rewind.choice, Choice::Both);
        let json = rewind.json("Claude", "duka-pos");
        assert_eq!(json["restore"], json!(["a.rs"]));
        assert_eq!(json["delete"], json!(["new.rs"]));
        assert_eq!(json["bringBack"], json!(["old.rs"]));
        assert_eq!(json["later"], json!(["Cargo.lock"]), "apart, and unticked");
        assert_eq!(json["includeLater"], false);
        assert_eq!((json["skipped"].as_u64(), json["headMoved"].as_bool()), (Some(2), Some(true)));
        let seq = rewind.choose(Choice::Files).unwrap();
        assert!(rewind.plan.is_none() && rewind.loading, "a new preview for the new choice");
        assert!(!rewind.previewed(seq - 1, Ok(plan(true, true)), false));
        assert!(rewind.previewed(seq, Ok(plan(true, false)), false));
    }

    #[test]
    fn enter_never_confirms_a_rewind_unless_the_button_has_the_keyboard() {
        let mut rewind = Rewind::new("t".into(), 2, String::new(), true, String::new());
        rewind.seq = 1;
        assert_eq!(rewind.confirm(Confirm::Click), None, "nothing to rewind before the preview");
        rewind.previewed(1, Ok(plan(true, true)), true);
        assert_eq!(rewind.confirm(Confirm::Key), None, "a bare Enter does nothing");
        rewind.button_focused = true;
        rewind.include_later = true;
        assert_eq!(rewind.confirm(Confirm::Key), Some(("ab".repeat(16), true)));
        assert_eq!(rewind.confirm(Confirm::Click), None, "once");
        rewind.applying = false;
        rewind.button_focused = false;
        assert!(rewind.confirm(Confirm::Click).is_some(), "a click confirms");
    }

    #[test]
    fn phones_never_get_files_or_a_folder_path() {
        let p = plan(true, true);
        let wire = wire_plan(&p).unwrap();
        assert_eq!((wire.turns, wire.messages, wire.blocked), (2, 5, None));
        assert!(wire_plan(&plan(true, false)).is_none(), "no conversation part: nothing for a phone");
        let mut blocked = plan(false, true);
        blocked.blocked = Some(RewindBlock::Agent);
        assert_eq!(wire_plan(&blocked).unwrap().blocked.as_deref(), Some("agent"));
        let mut t = turn(1, Checkpoint::Ready);
        t.can_restore = true;
        let wire = serde_json::to_value(wire_turn(&t)).unwrap();
        assert!(wire.get("can_restore").is_none());
        assert_eq!(wire["checkpoint"], "ready");
        let list = wire_found(SessionsScanResult {
            found: vec![found("sx1", None, None, false)],
            ..Default::default()
        });
        let json = serde_json::to_string(&list).unwrap();
        assert!(!json.contains("/home/me"), "{json}");
        assert_eq!(list.found[0].folder, "shamba-api");
    }
}
