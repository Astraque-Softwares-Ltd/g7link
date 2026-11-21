// SPDX-License-Identifier: MPL-2.0
//! Marks on threads (§U3.5): pin, archive, unread, snooze and rename, kept
//! by the host so the PC, the launcher and the phone see the same; and the
//! history around the thread list: search and the store's size.
//!
//! The shelves are exclusive: a thread is on the main list, archived, or
//! snoozed. Nothing here sends a message or starts a turn: a snooze that
//! ends only puts the thread back on the list.

use std::sync::Arc;

use crate::api::*;
use crate::host::store::now_ms;
use crate::host::threads::{Entry, emit_state, err};
use crate::host::{Host, db};

/// What a thread's `unread` is made of, and what its last announced state
/// was (a snooze ends early on a change into needing the user).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marks {
    /// The last `seq` a client had on screen.
    pub read_seq: u64,
    /// "Mark unread"; the next view clears it.
    pub unread_mark: bool,
    seen_state: State,
    seen_asking: u32,
}

impl Marks {
    /// For a thread made or loaded with nothing kept about its reading: all
    /// read, unless it was stored unread.
    pub fn of(info: &ThreadInfo) -> Marks {
        Marks {
            read_seq: info.last_seq,
            unread_mark: info.unread,
            seen_state: info.state,
            seen_asking: info.asking,
        }
    }

    /// Marked unread, or something happened since anyone looked and the
    /// thread has finished or waits for the user.
    pub fn unread(&self, info: &ThreadInfo) -> bool {
        self.unread_mark
            || (info.last_seq > self.read_seq
                && matches!(info.state, State::Done | State::Failed | State::Waiting))
    }

    fn read(&mut self, info: &ThreadInfo) {
        self.read_seq = info.last_seq;
        self.unread_mark = false;
    }
}

/// Brings a thread's marks up to date before its state goes out: a thread
/// a PC shows is read; a snoozed thread that has just come to need the
/// user (a request, a question) or has just failed comes back, unread.
pub(crate) fn settle(host: &Host, entry: &mut Entry) {
    let info = &mut entry.info;
    let marks = &mut entry.marks;
    let needs_now = info.state != marks.seen_state && matches!(info.state, State::Waiting | State::Failed);
    let woke = needs_now || info.asking > marks.seen_asking;
    marks.seen_state = info.state;
    marks.seen_asking = info.asking;
    if woke && info.snoozed_until_ms.take().is_some() {
        marks.unread_mark = true;
    }
    if host.hub.viewed(&info.id) {
        marks.read(info);
    }
    info.unread = marks.unread(info);
}

/// A `thread.mark` that has been checked.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkChange {
    /// Without repeats, in the order asked.
    ids: Vec<String>,
    pinned: Option<bool>,
    archived: Option<bool>,
    unread: Option<bool>,
    /// `Some(None)`: the snooze ends.
    snooze: Option<Option<u64>>,
    now: u64,
}

impl MarkChange {
    fn check(p: &ThreadMarkParams, now: u64) -> Result<MarkChange, RpcError> {
        if p.thread_ids.is_empty() || p.thread_ids.len() > MAX_MARK_THREADS {
            return Err(err(ErrorCode::BadRequest, "Mark 1 to 50 threads at a time."));
        }
        if !p.by.is_valid() {
            return Err(err(ErrorCode::BadRequest, "Bad origin."));
        }
        let mut ids: Vec<String> = Vec::with_capacity(p.thread_ids.len());
        for id in &p.thread_ids {
            if !is_valid_thread_id(id) {
                return Err(err(ErrorCode::BadRequest, "Bad thread id."));
            }
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        let snooze = match p.snooze_until_ms {
            None => None,
            Some(0) => Some(None),
            Some(until) if until <= now => {
                return Err(err(ErrorCode::BadRequest, "Pick a time that hasn't passed."));
            }
            Some(until) if until - now > MAX_SNOOZE_MS => {
                return Err(err(ErrorCode::BadRequest, "Snooze for 30 days at most."));
            }
            Some(until) => Some(Some(until)),
        };
        if p.pinned.is_none() && p.archived.is_none() && p.unread.is_none() && snooze.is_none() {
            return Err(err(ErrorCode::BadRequest, "Nothing to change."));
        }
        Ok(MarkChange { ids, pinned: p.pinned, archived: p.archived, unread: p.unread, snooze, now })
    }

    /// It archives, and `entry` runs: it can't be archived.
    fn refused(&self, entry: &Entry) -> bool {
        self.archived == Some(true) && entry.info.state.is_running()
    }

    fn apply(&self, entry: &mut Entry) {
        let info = &mut entry.info;
        if let Some(pinned) = self.pinned {
            info.pinned = pinned;
        }
        if let Some(archived) = self.archived {
            if archived != info.archived {
                entry.kept.archived_ms = archived.then_some(self.now);
            }
            info.archived = archived;
            if archived {
                info.snoozed_until_ms = None;
            }
        }
        if let Some(snooze) = self.snooze {
            info.snoozed_until_ms = snooze;
            if snooze.is_some() {
                info.archived = false;
            }
        }
        match self.unread {
            Some(true) => entry.marks.unread_mark = true,
            Some(false) => entry.marks.read(info),
            None => {}
        }
    }
}

/// A title a person gave: one line, cleaned, at most [`MAX_TITLE`] bytes.
fn given_title(title: &str) -> Result<String, RpcError> {
    // Anything far past the limit is not a title.
    if title.len() > MAX_TITLE * 8 {
        return Err(err(ErrorCode::BadRequest, "The name is too long."));
    }
    let title = cut(&collapse_whitespace(&clean(title)), MAX_TITLE).to_owned();
    if title.is_empty() {
        return Err(err(ErrorCode::BadRequest, "Give the thread a name."));
    }
    Ok(title)
}

/// The order of the main list: needs you, working, pinned, the rest.
pub(crate) fn main_rank(info: &ThreadInfo) -> u8 {
    if info.state == State::Waiting || info.asking > 0 {
        0
    } else if info.state.is_running() {
        1
    } else if info.pinned {
        2
    } else {
        3
    }
}

impl Host {
    /// `thread.mark`: threads that don't exist (any more) are left out of
    /// the reply; a running thread isn't archived and is listed in `busy`.
    pub async fn thread_mark(self: &Arc<Self>, p: ThreadMarkParams) -> Result<ThreadMarkResult, RpcError> {
        let change = MarkChange::check(&p, now_ms())?;
        let mut reg = self.lock_reg();
        let mut result = ThreadMarkResult::default();
        for id in &change.ids {
            let Some(entry) = reg.threads.get_mut(id) else { continue };
            if change.refused(entry) {
                result.busy.push(id.clone());
                continue;
            }
            change.apply(entry);
            emit_state(self, entry);
            result.threads.push(entry.info.clone());
        }
        match (result.threads.is_empty(), result.busy.is_empty()) {
            (true, true) => Err(err(ErrorCode::NotFound, "No such thread.")),
            (true, false) => Err(err(ErrorCode::Busy, "Wait until it finishes.")),
            _ => Ok(result),
        }
    }

    /// `thread.rename`.
    pub async fn thread_rename(self: &Arc<Self>, p: ThreadRenameParams) -> Result<ThreadInfo, RpcError> {
        if p.by.as_ref().is_some_and(|by| !by.is_valid()) {
            return Err(err(ErrorCode::BadRequest, "Bad origin."));
        }
        let title = given_title(&p.title)?;
        let mut reg = self.lock_reg();
        let entry =
            reg.threads.get_mut(&p.thread_id).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
        if entry.info.title != title {
            entry.info.title = title;
            emit_state(self, entry);
        }
        Ok(entry.info.clone())
    }

    /// A launcher or G7 Link shows `thread_id` now (`ui.viewing`): it's read.
    pub(crate) fn thread_viewed(&self, thread_id: &str) {
        let mut reg = self.lock_reg();
        let Some(entry) = reg.threads.get_mut(thread_id) else { return };
        entry.marks.read(&entry.info);
        if entry.info.unread {
            emit_state(self, entry);
        }
    }

    /// A message was sent to `thread_id`: it goes back on the main list.
    pub(crate) fn sent_to(&self, thread_id: &str) {
        let mut reg = self.lock_reg();
        let Some(entry) = reg.threads.get_mut(thread_id) else { return };
        if entry.info.archived || entry.info.snoozed_until_ms.is_some() {
            entry.info.archived = false;
            entry.info.snoozed_until_ms = None;
            emit_state(self, entry);
        }
    }

    /// Snoozes that have ended put their threads back on the list, and
    /// send nothing.
    pub(crate) fn wake_snoozed(&self) {
        let now = now_ms();
        let mut reg = self.lock_reg();
        for entry in reg.threads.values_mut().filter(|e| e.info.snoozed_until_ms.is_some_and(|t| t <= now)) {
            entry.info.snoozed_until_ms = None;
            emit_state(self, entry);
        }
    }

    /// `threads.search`, on the read-only connection off the async runtime.
    /// A newer search from the same connection stops this one.
    pub async fn threads_search(
        &self,
        p: ThreadsSearchParams,
        client: u64,
    ) -> Result<ThreadsSearchResult, RpcError> {
        if p.query.len() > MAX_SEARCH_QUERY || p.cursor.as_ref().is_some_and(|c| c.len() > MAX_SEARCH_CURSOR)
        {
            return Err(err(ErrorCode::BadRequest, "The search is too long."));
        }
        if p.query.trim().is_empty() {
            return Ok(ThreadsSearchResult::default());
        }
        let reader = self
            .store
            .reader()
            .ok_or_else(|| err(ErrorCode::Unavailable, "G7 couldn't open its thread store."))?;
        let stop = db::Interrupt::new();
        if let Some(older) =
            self.searches.lock().unwrap_or_else(|e| e.into_inner()).insert(client, stop.clone())
        {
            older.stop();
        }
        let query = db::SearchQuery {
            query: p.query,
            limit: p.limit.unwrap_or(DEFAULT_SEARCH_LIMIT).clamp(1, MAX_SEARCH_LIMIT),
            cursor: p.cursor,
            archived: p.archived,
            visible: None,
        };
        let page = tokio::task::spawn_blocking(move || reader.search(&query, &stop))
            .await
            .map_err(|_| err(ErrorCode::Internal, "The search failed."))?;
        match page {
            Ok(page) => {
                Ok(ThreadsSearchResult { hits: page.hits, next: page.next, titles_only: page.titles_only })
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                Err(err(ErrorCode::Busy, "A newer search took its place."))
            }
            Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
                Err(err(ErrorCode::BadRequest, "Bad search."))
            }
            Err(_) => Err(err(ErrorCode::Internal, "The search failed.")),
        }
    }

    /// `store.info`: the database's counts and size, and the restore points' size.
    pub async fn store_info(&self) -> Result<StoreInfo, RpcError> {
        let reader = self
            .store
            .reader()
            .ok_or_else(|| err(ErrorCode::Unavailable, "G7 couldn't open its thread store."))?;
        let checkpoints = crate::host::git::checkpoints_dir(&self.config.data_dir);
        let info = tokio::task::spawn_blocking(move || {
            reader.info().map(|info| (info, crate::host::git::store_bytes(&checkpoints)))
        })
        .await
        .map_err(|_| err(ErrorCode::Internal, "Couldn't read the store."))?;
        let (info, checkpoint_bytes) =
            info.map_err(|_| err(ErrorCode::Internal, "Couldn't read the store."))?;
        Ok(StoreInfo {
            threads: info.threads,
            archived: info.archived,
            items: info.items,
            bytes: info.bytes,
            found: info.found,
            checkpoint_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::threads::test_info;

    fn mark(ids: &[&str]) -> ThreadMarkParams {
        ThreadMarkParams::new(ids.iter().map(|s| (*s).to_owned()).collect(), Origin::launcher())
    }

    #[test]
    fn a_mark_is_checked_before_anything_changes() {
        let a = "a".repeat(32);
        let now = 1_000;
        let code = |p: &ThreadMarkParams| MarkChange::check(p, now).unwrap_err().code;
        assert_eq!(code(&mark(&[])), ErrorCode::BadRequest);
        let many: Vec<String> = (0..=MAX_MARK_THREADS).map(|i| format!("{i:032x}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(code(&ThreadMarkParams { pinned: Some(true), ..mark(&many) }), ErrorCode::BadRequest);
        assert_eq!(code(&ThreadMarkParams { pinned: Some(true), ..mark(&["../x"]) }), ErrorCode::BadRequest);
        assert_eq!(code(&mark(&[&a])), ErrorCode::BadRequest, "nothing to change");
        let snooze = |until| ThreadMarkParams { snooze_until_ms: Some(until), ..mark(&[&a]) };
        assert_eq!(code(&snooze(now)), ErrorCode::BadRequest, "the past");
        assert_eq!(code(&snooze(now + MAX_SNOOZE_MS + 1)), ErrorCode::BadRequest, "past 30 days");
        assert_eq!(
            MarkChange::check(&snooze(now + MAX_SNOOZE_MS), now).unwrap().snooze,
            Some(Some(now + MAX_SNOOZE_MS))
        );
        assert_eq!(MarkChange::check(&snooze(0), now).unwrap().snooze, Some(None), "0 ends it");
        let twice =
            MarkChange::check(&ThreadMarkParams { pinned: Some(true), ..mark(&[&a, &a]) }, now).unwrap();
        assert_eq!(twice.ids, std::slice::from_ref(&a));
    }

    #[test]
    fn unread_is_a_mark_or_news_since_the_last_look_once_it_stops() {
        let mut info = ThreadInfo { last_seq: 5, state: State::Done, ..test_info(AgentId::Claude) };
        let marks = Marks::of(&info);
        assert!(!marks.unread(&info), "loaded: all read");
        info.last_seq = 9;
        assert!(marks.unread(&info));
        info.state = State::Working;
        assert!(!marks.unread(&info), "not while it works");
        info.state = State::Interrupted;
        assert!(!marks.unread(&info), "nor when stopped");
        info.state = State::Waiting;
        assert!(marks.unread(&info), "it needs you");
        let marked = Marks { unread_mark: true, ..Marks::of(&info) };
        assert!(marked.unread(&info));
        assert!(Marks::of(&ThreadInfo { unread: true, ..info.clone() }).unread(&info), "stored unread stays");
    }

    #[test]
    fn the_shelves_exclude_each_other() {
        let now = 1_000;
        let info = test_info(AgentId::Codex);
        let mut entry = crate::host::threads::test_entry(info);
        let snooze = MarkChange::check(
            &ThreadMarkParams { snooze_until_ms: Some(now + 60_000), ..mark(&[&"b".repeat(32)]) },
            now,
        )
        .unwrap();
        let archive =
            MarkChange::check(&ThreadMarkParams { archived: Some(true), ..mark(&[&"b".repeat(32)]) }, now)
                .unwrap();
        archive.apply(&mut entry);
        snooze.apply(&mut entry);
        assert!(
            !entry.info.archived && entry.info.snoozed_until_ms == Some(now + 60_000),
            "snoozing unarchives"
        );
        archive.apply(&mut entry);
        assert!(entry.info.archived && entry.info.snoozed_until_ms.is_none(), "archiving ends the snooze");
        let unread =
            MarkChange::check(&ThreadMarkParams { unread: Some(true), ..mark(&[&"b".repeat(32)]) }, now)
                .unwrap();
        unread.apply(&mut entry);
        assert!(entry.marks.unread_mark);
        let read = MarkChange { unread: Some(false), ..unread };
        read.apply(&mut entry);
        assert!(!entry.marks.unread_mark && entry.marks.read_seq == entry.info.last_seq);
    }

    #[tokio::test]
    async fn a_snooze_ends_when_the_thread_comes_to_need_you_or_its_time_comes() {
        let dir = tempfile::tempdir().unwrap();
        let host = crate::host::test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let later = now_ms() + 3_600_000;
        let info = ThreadInfo {
            state: State::Waiting,
            last_seq: 4,
            snoozed_until_ms: Some(later),
            ..test_info(AgentId::Claude)
        };
        let mut entry = crate::host::threads::test_entry(info);
        settle(&host, &mut entry);
        assert_eq!(entry.info.snoozed_until_ms, Some(later), "snoozed while it waited: it stays snoozed");
        entry.info.state = State::Working;
        settle(&host, &mut entry);
        entry.info.asking = 1;
        settle(&host, &mut entry);
        assert_eq!(entry.info.snoozed_until_ms, None, "a question woke it");
        assert!(entry.info.unread && entry.marks.unread_mark, "back as unread");

        let mut entry = crate::host::threads::test_entry(ThreadInfo {
            id: "c".repeat(32),
            snoozed_until_ms: Some(later),
            ..test_info(AgentId::Codex)
        });
        entry.info.state = State::Failed;
        settle(&host, &mut entry);
        assert_eq!(entry.info.snoozed_until_ms, None, "it failed");

        let due = ThreadInfo {
            id: "d".repeat(32),
            snoozed_until_ms: Some(now_ms() - 1),
            ..test_info(AgentId::Claude)
        };
        let kept =
            ThreadInfo { id: "e".repeat(32), snoozed_until_ms: Some(later), ..test_info(AgentId::Claude) };
        for info in [due, kept] {
            let id = info.id.clone();
            host.lock_reg().threads.insert(id, crate::host::threads::test_entry(info));
        }
        host.wake_snoozed();
        let reg = host.lock_reg();
        assert_eq!(reg.threads[&"d".repeat(32)].info.snoozed_until_ms, None);
        assert_eq!(reg.threads[&"e".repeat(32)].info.snoozed_until_ms, Some(later));
        assert!(!reg.threads[&"d".repeat(32)].info.unread, "its time came: nothing new, not unread");
    }

    #[test]
    fn titles_are_one_clean_line() {
        assert_eq!(given_title("  Fix\n the\tbuild \u{202e} ").unwrap(), "Fix the build");
        assert_eq!(given_title(&"é".repeat(60)).unwrap().len(), 80);
        assert_eq!(given_title(" \n\t ").unwrap_err().code, ErrorCode::BadRequest);
        assert_eq!(given_title(&"x".repeat(MAX_TITLE * 8 + 1)).unwrap_err().code, ErrorCode::BadRequest);
    }

    #[test]
    fn the_main_list_puts_what_needs_you_first() {
        let base = test_info(AgentId::Claude);
        let rank = |info: ThreadInfo| main_rank(&info);
        assert_eq!(rank(ThreadInfo { state: State::Waiting, ..base.clone() }), 0);
        assert_eq!(rank(ThreadInfo { state: State::Working, asking: 1, ..base.clone() }), 0);
        assert_eq!(rank(ThreadInfo { state: State::Working, pinned: true, ..base.clone() }), 1);
        assert_eq!(rank(ThreadInfo { state: State::Done, pinned: true, ..base.clone() }), 2);
        assert_eq!(rank(ThreadInfo { state: State::Failed, ..base }), 3);
    }
}
