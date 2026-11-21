// SPDX-License-Identifier: GPL-3.0-or-later
//! Phase 3 on the Agents page and for phones, with the host: a thread's
//! turns and what each changed, marks and shelves, search, bringing in
//! sessions found on this PC, the diff, rewind with its undo, fork, and the
//! restore points' switch. The state these read and write is in
//! [`history`](super::history); this is the I/O around it.

use g7_agents::api::{
    FoldersSetParams, FoundShow, ImportProgressEvent, RewindApplyParams, RewindPreviewParams,
    SessionsScanParams, SessionsScannedEvent, ThreadCutEvent, ThreadForkParams, ThreadMarkParams,
    ThreadRenameParams, ThreadShow, ThreadTurnsParams, ThreadsListParams, ThreadsSearchParams,
    TurnFilesParams, TurnPatchParams,
};
use g7_agents::api::{GitChange, GitPatchParams, TurnFile, TurnFilesResult};

use super::*;

/// How long search waits after the last key.
const SEARCH_QUIET: Duration = Duration::from_millis(150);
/// Turns read for the open thread.
const TURNS_READ: u32 = api::MAX_TURNS_LIMIT;
/// Pages of one patch read at most (the host cuts a file at 1 MiB).
const PATCH_PAGES: usize = 64;

fn origin_of(from: &DeviceId) -> Origin {
    Origin::phone(from.to_string(), device_name(from))
}

// ---- Turns ----

/// The recorded turns of the open view whose files its Changes rows need,
/// among the items it shows; marked as asked.
fn want_paths(open: &mut OpenView) -> Vec<u32> {
    let shown: HashSet<u32> = open.entries.iter().filter_map(|e| e.item.turn).collect();
    let mut wanted = history::paths_wanted(&open.turns, &open.turn_paths, &open.paths_asked);
    wanted.retain(|n| shown.contains(n));
    open.paths_asked.extend(wanted.iter().copied());
    wanted
}

/// Reads the open thread's turns, then the files of its recorded ones.
pub(super) async fn read_turns(client: Client, generation: u64, thread_id: String) {
    let params =
        ThreadTurnsParams { thread_id: thread_id.clone(), before_turn: None, limit: Some(TURNS_READ) };
    let turns = match client.thread_turns(params).await {
        Ok(turns) => turns,
        Err(e) => {
            tracing::debug!(error = %e, "the thread's turns weren't read");
            return;
        }
    };
    let wanted = {
        let mut s = lock();
        let Some(open) = s.open.as_mut().filter(|o| o.generation == generation) else { return };
        let mut turns = turns;
        turns.sort_by_key(|t| t.turn);
        turns.dedup_by_key(|t| t.turn);
        open.turns = Arc::new(turns);
        want_paths(open)
    };
    rerender();
    read_paths(client, generation, thread_id, wanted).await;
}

/// Reads the first files of each of `turns`, for their Changes rows.
async fn read_paths(client: Client, generation: u64, thread_id: String, turns: Vec<u32>) {
    for turn in turns {
        let params = TurnFilesParams::new(thread_id.clone(), api::TurnRange::One(turn));
        let files = match client.turn_files(params).await {
            Ok(r) => r.files.into_iter().take(history::PATHS_KEPT).collect(),
            Err(e) => {
                tracing::debug!(error = %e, "a turn's files weren't read");
                Vec::new()
            }
        };
        {
            let mut s = lock();
            let Some(open) = s.open.as_mut().filter(|o| o.generation == generation) else { return };
            Arc::make_mut(&mut open.turn_paths).insert(turn, files);
        }
        rerender();
    }
}

/// A turn of a thread started, ended, or its restore point settled.
pub(super) fn turn_event(bridge: &Bridge, client: &Client, thread_id: &str, turn: &api::TurnInfo) {
    let read = {
        let mut s = bridge.lock();
        let Some(open) = s.open.as_mut().filter(|o| o.is(OpenKind::Thread, thread_id)) else { return };
        let new = open.turns.binary_search_by_key(&turn.turn, |t| t.turn).is_err();
        history::upsert_turn(Arc::make_mut(&mut open.turns), turn.clone());
        if new {
            // The next turn ends what the last rewind left to say.
            open.left.clear();
        }
        // A restore point recorded again names its files again.
        if turn.checkpoint == api::Checkpoint::Ready && !open.turn_paths.contains_key(&turn.turn) {
            open.paths_asked.remove(&turn.turn);
        }
        let wanted = want_paths(open);
        (!wanted.is_empty()).then_some((open.generation, wanted))
    };
    rerender();
    if let Some((generation, wanted)) = read
        && core_host::try_host().is_some()
    {
        core_host::spawn(read_paths(client.clone(), generation, thread_id.to_owned(), wanted));
    }
}

/// Items left a thread (a rewind) or came back (its undo): the open view
/// drops them, or reads them again; its turns are read again either way.
pub(super) fn cut_event(bridge: &Bridge, client: &Client, e: &ThreadCutEvent) {
    let generation = {
        let mut s = bridge.lock();
        let Some(open) = s.open.as_mut().filter(|o| o.is(OpenKind::Thread, &e.thread_id)) else { return };
        apply_cut(open, e.from_seq, e.to_seq, e.restored);
        open.generation
    };
    rerender();
    changed();
    if core_host::try_host().is_none() {
        return;
    }
    let (client, thread_id, restored) = (client.clone(), e.thread_id.clone(), e.restored);
    core_host::spawn(async move {
        if restored {
            let params = ThreadReadParams {
                thread_id: thread_id.clone(),
                limit: Some(READ_PAGE),
                ..Default::default()
            };
            if let Ok(page) = client.thread_read(params).await {
                let mut s = lock();
                if let Some(open) = s.open.as_mut().filter(|o| o.generation == generation) {
                    for item in &page.items {
                        open.apply(Entry::from_item(item));
                    }
                }
            }
        }
        {
            let mut s = lock();
            if let Some(open) = s.open.as_mut().filter(|o| o.generation == generation) {
                open.turn_paths = Arc::default();
                open.paths_asked.clear();
            }
        }
        read_turns(client, generation, thread_id).await;
        changed();
    });
}

/// A rewind took items `from..=to` out of the open view; its undo brings
/// them back (read again), and the files the rewind left aren't news any more.
pub(super) fn apply_cut(open: &mut OpenView, from: u64, to: u64, restored: bool) {
    if restored {
        open.left.clear();
    } else {
        open.entries.retain(|x| x.item.seq < from || x.item.seq > to);
    }
}

/// The message a search hit opened the thread at, when it isn't among the
/// items shown yet.
pub(super) fn focus_missing(open: &OpenView) -> Option<u64> {
    open.focus.filter(|seq| !open.entries.iter().any(|e| e.item.seq == *seq))
}

/// Reads earlier items until message `seq` is among them, within what the
/// view keeps; past that, the page that ends with it takes the view's place
/// (so no gap is ever shown between two pages).
pub(super) async fn read_to(client: Client, generation: u64, thread_id: String, seq: u64) {
    loop {
        let before = {
            let s = lock();
            let Some(open) = s.open.as_ref().filter(|o| o.generation == generation) else { return };
            if focus_missing(open).is_none() {
                break;
            }
            match open.entries.first().map(|e| e.item.seq) {
                Some(oldest) if oldest > seq && open.entries.len() + READ_PAGE as usize <= OPEN_ITEMS => {
                    oldest
                }
                _ => 0,
            }
        };
        let jump = before == 0;
        let params = ThreadReadParams {
            thread_id: thread_id.clone(),
            before_seq: Some(if jump { seq.saturating_add(1) } else { before }),
            limit: Some(READ_PAGE),
            ..Default::default()
        };
        let Ok(page) = client.thread_read(params).await else { break };
        {
            let mut s = lock();
            let Some(open) = s.open.as_mut().filter(|o| o.generation == generation) else { return };
            if jump {
                open.entries.clear();
            }
            open.more = page.more_before;
            for item in &page.items {
                open.apply(Entry::from_item(item));
            }
            if jump || page.items.is_empty() {
                break;
            }
        }
    }
    let wanted = {
        let mut s = lock();
        s.open.as_mut().filter(|o| o.generation == generation).map(want_paths).unwrap_or_default()
    };
    rerender();
    changed();
    read_paths(client, generation, thread_id, wanted).await;
}

// ---- Shelves, marks, renames ----

/// A thread's state changed: the shelf shown follows it.
pub(super) fn shelf_update(bridge: &Bridge, info: &ThreadInfo) {
    let mut s = bridge.lock();
    let Some((show, list)) = s.shelf.as_mut() else { return };
    let belongs = match show {
        ThreadShow::Archived => info.archived,
        ThreadShow::Snoozed => !info.archived && history::is_snoozed(info, now_ms()),
        ThreadShow::Main => false,
    };
    match list.iter().position(|t| t.id == info.id) {
        Some(at) if belongs => list[at] = info.clone(),
        Some(at) => {
            list.remove(at);
        }
        None if belongs => {
            list.push(info.clone());
            list.sort_by_key(|t| std::cmp::Reverse(t.updated_ms));
        }
        None => {}
    }
}

/// The list shows "main", "archived" or "snoozed".
pub fn show_shelf(word: &str) {
    let show = match word {
        "archived" => ThreadShow::Archived,
        "snoozed" => ThreadShow::Snoozed,
        _ => {
            if lock().shelf.take().is_some() {
                changed();
            }
            return;
        }
    };
    {
        let mut s = lock();
        // Shown at once (empty, or as last read) while it's read again.
        let keep =
            s.shelf.as_ref().filter(|(was, _)| *was == show).map(|(_, l)| l.clone()).unwrap_or_default();
        s.shelf = Some((show, keep));
    }
    changed();
    page_request(
        Request::Read,
        move |client| async move {
            let params =
                ThreadsListParams { limit: Some(LISTED_THREADS as u32), before_ms: None, show: Some(show) };
            client.threads_list_with(params).await
        },
        move |threads: Vec<ThreadInfo>| {
            let mut s = lock();
            if s.shelf.as_ref().is_some_and(|(was, _)| *was == show) {
                s.shelf = Some((show, threads));
            }
            drop(s);
            changed();
        },
    );
}

/// The marks the page sets: `what` is "pin", "unpin", "archive",
/// "unarchive", "unread", "read", "snooze" (until `until_ms`) or "unsnooze",
/// for up to 50 threads.
pub fn mark(ids: Vec<String>, what: &str, until_ms: u64) {
    let Some(params) = mark_params(ids, what, until_ms, now_ms()) else { return };
    page_request(
        Request::Mark,
        move |client| async move { client.thread_mark(params).await },
        |result: api::ThreadMarkResult| {
            for t in &result.threads {
                bridge().remember(t);
                shelf_update(bridge(), t);
            }
            lists_changed();
            if !result.busy.is_empty() {
                crate::bridge::app::show_message(tr!("Agents", "You can change it when the agent stops."));
            }
        },
    );
}

/// What `thread.mark` is asked: `None` for no thread, an unknown word, or
/// a snooze that isn't ahead and within 30 days.
pub(super) fn mark_params(
    mut ids: Vec<String>,
    what: &str,
    until_ms: u64,
    now_ms: u64,
) -> Option<ThreadMarkParams> {
    let mut seen = HashSet::new();
    ids.retain(|id| seen.insert(id.clone()));
    ids.truncate(api::MAX_MARK_THREADS);
    if ids.is_empty() {
        return None;
    }
    let mut params = ThreadMarkParams::new(ids, Origin::launcher());
    match what {
        "pin" | "unpin" => params.pinned = Some(what == "pin"),
        "archive" | "unarchive" => params.archived = Some(what == "archive"),
        "unread" | "read" => params.unread = Some(what == "unread"),
        "snooze" if until_ms > 0 && history::snooze_ok(until_ms, now_ms) => {
            params.snooze_until_ms = Some(until_ms)
        }
        "unsnooze" => params.snooze_until_ms = Some(0),
        _ => return None,
    }
    Some(params)
}

/// Renames a thread (empty is refused).
pub fn rename(id: String, title: String) {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return;
    }
    let title = api::cut(&title, api::MAX_TITLE).to_owned();
    page_request(
        Request::Mark,
        move |client| async move {
            client.thread_rename(ThreadRenameParams { thread_id: id, title, by: None }).await
        },
        |info: ThreadInfo| {
            bridge().remember(&info);
            shelf_update(bridge(), &info);
            changed();
            lists_changed();
        },
    );
}

// ---- Search ----

/// The list's search field changed: threads are searched 150 ms after the
/// last key, and only the newest search's hits are shown.
pub fn search(query: String, archived: bool) {
    let seq = lock().search.typed(&query, archived);
    changed();
    let Some(seq) = seq else { return };
    if core_host::try_host().is_none() {
        return;
    }
    core_host::spawn(async move {
        tokio::time::sleep(SEARCH_QUIET).await;
        let params = {
            let s = lock();
            if s.search.seq != seq {
                return;
            }
            ThreadsSearchParams {
                query: s.search.query.clone(),
                limit: Some(api::MAX_SEARCH_LIMIT),
                cursor: None,
                archived: s.search.archived,
            }
        };
        let Some(client) = connected_client().await else {
            lock().search.answered(seq, None);
            changed();
            return;
        };
        let result = client.threads_search(params).await;
        if let Err(e) = &result {
            tracing::debug!(error = %e, "the search failed");
        }
        if lock().search.answered(seq, result.ok()) {
            changed();
        }
    });
}

/// Opens the thread of a hit at its message (0: the title, the newest).
pub fn open_hit(thread_id: String, seq: u64) {
    open_at(OpenKind::Thread, thread_id, (seq > 0).then_some(seq));
}

// ---- Bringing in sessions ----

/// Asks the host for the sessions it found (the first page, or the next),
/// after `wait` (the user may still be typing).
fn scan(more: bool, wait: Duration) {
    let seq = {
        let mut s = lock();
        let Some(import) = s.import.as_mut() else { return };
        if more && import.next_ms.is_none() {
            return;
        }
        import.rescan()
    };
    changed();
    if core_host::try_host().is_none() {
        return;
    }
    core_host::spawn(async move {
        tokio::time::sleep(wait).await;
        let params = {
            let s = lock();
            let Some(import) = s.import.as_ref().filter(|i| i.seq == seq) else { return };
            SessionsScanParams {
                query: Some(import.query.clone()).filter(|q| !q.is_empty()),
                agent: import.agent,
                show: Some(FoundShow::All),
                before_ms: if more { import.next_ms } else { None },
                limit: Some(api::MAX_SCAN_LIMIT),
            }
        };
        let result = match connected_client().await {
            Some(client) => client.sessions_scan(params).await.ok(),
            None => None,
        };
        let finished = {
            let mut s = lock();
            let Some(import) = s.import.as_mut() else { return };
            if !import.scanned(seq, more, result) {
                return;
            }
            // A job that ended is told once the list says which came in.
            let ended = import.job.as_ref().is_some_and(|j| j.done.saturating_add(j.failed) >= j.total);
            if ended {
                import.finish();
            }
            ended
        };
        if finished {
            refresh();
        }
        changed();
    });
}

/// Opens "Bring in earlier sessions": the host starts looking at once.
pub fn import_open() {
    {
        let mut s = lock();
        if s.import.is_some() {
            return;
        }
        s.import = Some(history::Import::default());
    }
    scan(false, Duration::ZERO);
}

/// Leaves it; an import that runs goes on.
pub fn import_close() {
    let closed = {
        let mut s = lock();
        let preview = s.open.as_ref().is_some_and(|o| o.kind == OpenKind::Preview);
        (s.import.take().is_some(), preview)
    };
    if closed.1 {
        close();
    }
    if closed.0 {
        changed();
    }
}

pub fn import_query(query: String) {
    let changed_query = {
        let mut s = lock();
        let Some(import) = s.import.as_mut() else { return };
        let query = api::cut(query.trim(), api::MAX_SCAN_QUERY).to_owned();
        let differs = import.query != query;
        import.query = query;
        differs
    };
    if changed_query {
        scan(false, SEARCH_QUIET);
    }
}

/// "all", "claude" or "codex".
pub fn import_agent(word: &str) {
    let agent = AgentId::parse(word);
    {
        let mut s = lock();
        let Some(import) = s.import.as_mut() else { return };
        if import.agent == agent {
            return;
        }
        import.agent = agent;
    }
    scan(false, Duration::ZERO);
}

/// The next page of sessions.
pub fn import_more() {
    scan(true, Duration::ZERO);
}

pub fn import_pick(id: String) {
    if let Some(import) = lock().import.as_mut() {
        import.toggle(&id);
    }
    changed();
}

pub fn import_pick_group(key: String) {
    if let Some(import) = lock().import.as_mut() {
        import.toggle_group(&key);
    }
    changed();
}

/// Leaves every session picked.
pub fn import_clear() {
    if let Some(import) = lock().import.as_mut() {
        import.selected.clear();
    }
    changed();
}

/// Brings in the sessions picked, or only `only` (a preview's "Bring in 1").
pub fn import_start(only: String) {
    let ids = {
        let mut s = lock();
        let Some(import) = s.import.as_mut().filter(|i| i.job.is_none()) else { return };
        let ids = if only.is_empty() { import.selected.clone() } else { vec![only] };
        if ids.is_empty() {
            return;
        }
        import.done = None;
        ids
    };
    let asked = ids.clone();
    page_request(
        Request::Import,
        move |client| async move { client.sessions_import(ids, Origin::launcher()).await },
        move |job: api::SessionsImportResult| {
            if let Some(import) = lock().import.as_mut() {
                import.job =
                    Some(history::Job { id: job.job, ids: asked, done: 0, failed: 0, total: job.total });
            }
            changed();
        },
    );
}

/// Stops the import that runs; what came in stays.
pub fn import_cancel() {
    let Some(job) = lock().import.as_ref().and_then(|i| i.job.as_ref()).map(|j| j.id.clone()) else { return };
    page_request(Request::Import, move |client| async move { client.import_cancel(job).await }, |()| {});
}

/// How an import goes; when it ends the list is read again to say which
/// sessions came in.
pub(super) fn import_progress(bridge: &Bridge, _client: &Client, e: &ImportProgressEvent) {
    let ended = {
        let mut s = bridge.lock();
        let Some(import) = s.import.as_mut() else { return };
        import.progress(&e.job, e.done, e.failed, e.total)
    };
    changed();
    if ended {
        scan(false, Duration::ZERO);
    }
}

/// The host found more sessions: the screen reads its list again (at most
/// once a second, as the host tells).
pub(super) fn sessions_scanned(bridge: &Bridge, _client: &Client, e: &SessionsScannedEvent) {
    let again = {
        let mut s = bridge.lock();
        let Some(import) = s.import.as_mut() else { return };
        // The event counts every session; a filtered list keeps its own count.
        if import.query.is_empty() && import.agent.is_none() {
            import.total = e.total;
        }
        import.scanning = e.scanning;
        import.partial = e.partial;
        !import.loading && import.next_ms.is_none_or(|_| import.found.len() < api::MAX_SCAN_LIMIT as usize)
    };
    changed();
    if again {
        scan(false, Duration::ZERO);
    }
}

/// Looks at a found session before bringing it in.
pub fn preview_open(id: String) {
    open_at(OpenKind::Preview, id, None);
}

pub fn preview_close() {
    if lock().open.as_ref().is_some_and(|o| o.kind == OpenKind::Preview) {
        close();
    }
}

/// The session previewed, as the page's header and details show it.
pub(super) fn preview_row(f: &api::Found, folders: &[Folder], home: Option<&std::path::Path>) -> Value {
    let path = match &f.folder_id {
        Some(id) => folders.iter().find(|x| &x.id == id).map(|x| shown_path(&x.path, home)),
        None => None,
    }
    .unwrap_or_else(|| shown_path(&f.cwd, home));
    json!({
        "id": f.id,
        "title": f.title,
        "agent": f.agent.as_str(),
        "agent_name": f.agent.display_name(),
        "folder": f.folder_label,
        "shown_cwd": path,
        "updated_ms": f.updated_ms,
        "state": "done",
        "source": word(&f.source),
        "selectable": history::Import::selectable(f),
        "brought_in": f.thread_id.is_some(),
    })
}

// ---- The diff ----

/// Opens what turn `turn` of the open thread changed, at file `path` ("" for the first).
pub fn diff_open(turn: u32, path: String) {
    let Some((seq, thread_id, range, total)) = ({
        let mut s = lock();
        let Some(open) = s.open.as_ref().filter(|o| o.kind == OpenKind::Thread) else { return };
        let thread_id = open.id.clone();
        let whole = history::whole_span(&open.turns);
        let total = open.turns.iter().find(|t| t.turn == turn).map_or(0, |t| t.files);
        let diff = history::Diff {
            thread_id: thread_id.clone(),
            turn,
            want: path,
            whole,
            seq: 1,
            loading: true,
            ..Default::default()
        };
        let range = diff.range();
        s.diff = Some(diff);
        Some((1, thread_id, range, total))
    }) else {
        return;
    };
    changed();
    list_files(seq, thread_id, range, total);
}

/// Reads the diff's file list, then the first file's patch.
fn list_files(seq: u64, thread_id: String, range: api::TurnRange, total: u32) {
    if core_host::try_host().is_none() {
        return;
    }
    core_host::spawn(async move {
        let result = match connected_client().await {
            Some(client) => client.turn_files(TurnFilesParams::new(thread_id, range)).await.ok(),
            None => None,
        };
        let total = match range {
            api::TurnRange::One(_) => total,
            api::TurnRange::Span { .. } => 0,
        };
        let read = {
            let mut s = lock();
            let Some(diff) = s.diff.as_mut() else { return };
            if !diff.listed(seq, result, total) {
                return;
            }
            patch_wanted(diff)
        };
        changed();
        if let Some(read) = read {
            read_patch(read);
        }
    });
}

/// A file's patch to read: its number, thread, range (`None`: what isn't
/// committed) and path.
type PatchRead = (u64, String, Option<api::TurnRange>, String);

/// What to read for the file the diff shows, if it has text.
fn patch_wanted(diff: &mut history::Diff) -> Option<PatchRead> {
    let path = diff.files.get(diff.selected)?.path.clone();
    diff.patch_seq += 1;
    diff.patch = history::Patch { path: path.clone(), ..Default::default() };
    diff.wants_patch()?;
    // An untracked folder has no patch; the page names it.
    if diff.uncommitted && path.ends_with('/') {
        return None;
    }
    diff.patch.loading = true;
    let range = (!diff.uncommitted).then(|| diff.range());
    Some((diff.patch_seq, diff.thread_id.clone(), range, path))
}

/// A page of a file's patch: of a turn's range, or of what isn't committed.
async fn patch_page(
    client: &Client,
    thread_id: &str,
    range: Option<api::TurnRange>,
    path: &str,
    offset: Option<u64>,
) -> Result<api::TurnPatchResult, ClientError> {
    match range {
        Some(range) => {
            client
                .turn_patch(TurnPatchParams::new(thread_id.to_owned(), range, path.to_owned(), offset))
                .await
        }
        None => {
            let params = GitPatchParams { thread_id: thread_id.to_owned(), path: path.to_owned(), offset };
            client.git_patch(params).await.map(|p| api::TurnPatchResult {
                patch: p.patch,
                next: p.next,
                truncated: p.truncated,
                binary: p.binary,
            })
        }
    }
}

/// Reads a file's patch a page at a time (at most 1 MiB), and parses it on a worker.
fn read_patch((seq, thread_id, range, path): PatchRead) {
    if core_host::try_host().is_none() {
        return;
    }
    core_host::spawn(async move {
        let mut text = String::new();
        let (mut cut, mut binary, mut failed) = (false, false, false);
        let mut offset = None;
        if let Some(client) = connected_client().await {
            let mut pages = 0;
            loop {
                pages += 1;
                match patch_page(&client, &thread_id, range, &path, offset).await {
                    Ok(page) => {
                        binary |= page.binary;
                        cut |= page.truncated;
                        let room = api::MAX_PATCH_FILE.saturating_sub(text.len());
                        let kept = api::cut(&page.patch, room);
                        cut |= kept.len() < page.patch.len();
                        text.push_str(kept);
                        match page.next {
                            None => break,
                            // A page that doesn't move on, or too many: what came is shown, cut.
                            Some(next)
                                if offset.is_some_and(|at| next <= at) || pages >= PATCH_PAGES || cut =>
                            {
                                cut = true;
                                break;
                            }
                            Some(next) => offset = Some(next),
                        }
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "a patch wasn't read");
                        failed = text.is_empty();
                        cut |= !failed;
                        break;
                    }
                }
            }
        } else {
            failed = true;
        }
        // The diff's word grounds are the page's; taken before the lock below.
        let palette = bridge().render().palette.clone();
        let parsed =
            tokio::task::spawn_blocking(move || history::patch_doc(&text, cut, binary, &palette)).await;
        {
            let mut s = lock();
            let Some(diff) = s.diff.as_mut().filter(|d| d.patch_seq == seq) else { return };
            match parsed {
                Ok(mut patch) if !failed => {
                    patch.path = path;
                    diff.patch = patch;
                }
                _ => diff.patch = history::Patch { path, failed: true, ..Default::default() },
            }
        }
        changed();
    });
}

/// "uncommitted", "turn" or "thread".
pub fn diff_scope(word: &str) {
    if word == "uncommitted" {
        let want = lock().diff.as_ref().and_then(|d| d.files.get(d.selected)).map(|f| f.path.clone());
        diff_uncommitted(want.unwrap_or_default());
        return;
    }
    let read = {
        let mut s = lock();
        let turn = s.diff.as_ref().map_or(0, |d| d.turn);
        let turn_files =
            s.open.as_ref().and_then(|o| o.turns.iter().find(|t| t.turn == turn)).map_or(0, |t| t.files);
        let Some(diff) = s.diff.as_mut() else { return };
        let whole = word == "thread" && diff.whole.is_some();
        if diff.uncommitted {
            if diff.turn == 0 {
                return;
            }
            // From what isn't committed back to the turn's own files.
            diff.uncommitted = false;
            diff.total = turn_files;
            diff.scope_whole = !whole;
        }
        if whole == diff.scope_whole {
            return;
        }
        diff.scope_whole = whole;
        diff.seq += 1;
        diff.loading = true;
        diff.patch_seq += 1;
        diff.patch = history::Patch::default();
        let total = if whole { 0 } else { diff.total };
        (diff.seq, diff.thread_id.clone(), diff.range(), total)
    };
    changed();
    list_files(read.0, read.1, read.2, read.3);
}

/// Shows file `index` of the diff.
pub fn diff_select(index: usize) {
    diff_moved(|diff| diff.select(index));
}

/// The next (`by` 1) or previous (-1) file, wrapping.
pub fn diff_step(by: i32) {
    diff_moved(|diff| diff.step(by.clamp(-1, 1)));
}

fn diff_moved(move_to: impl FnOnce(&mut history::Diff) -> bool) {
    let read = {
        let mut s = lock();
        let Some(diff) = s.diff.as_mut() else { return };
        if !move_to(diff) {
            return;
        }
        patch_wanted(diff)
    };
    changed();
    if let Some(read) = read {
        read_patch(read);
    }
}

/// The status's files as the diff lists them.
fn uncommitted_files(st: &api::GitStatus) -> TurnFilesResult {
    let files = st
        .files
        .iter()
        .map(|f| TurnFile {
            path: f.path.clone(),
            status: match f.status {
                GitChange::Added | GitChange::Untracked => api::FileStatus::Added,
                GitChange::Deleted => api::FileStatus::Deleted,
                GitChange::Renamed => api::FileStatus::Renamed,
                GitChange::Type => api::FileStatus::Type,
                GitChange::Modified | GitChange::Conflict => api::FileStatus::Modified,
            },
            from: f.from.clone(),
            adds: f.adds.unwrap_or(0),
            dels: f.dels.unwrap_or(0),
            binary: f.binary,
        })
        .collect();
    TurnFilesResult { files, more: st.more, adds: st.adds, dels: st.dels }
}

/// Opens what the open thread's folder hasn't committed, at `path` ("" for
/// the first file): the diff's "Uncommitted" scope, from the panel's status.
pub fn diff_uncommitted(path: String) {
    let read = {
        let mut s = lock();
        let Some(open) = s.open.as_ref().filter(|o| o.kind == OpenKind::Thread) else { return };
        let thread_id = open.id.clone();
        let whole = history::whole_span(&open.turns);
        let Some(st) = s.source.panel.status.as_ref().filter(|_| s.source.panel.thread_id == thread_id)
        else {
            return;
        };
        let listed = uncommitted_files(st);
        let total = st.total;
        let (turn, seq) = s.diff.as_ref().map_or((whole.map_or(0, |w| w.1), 1), |d| (d.turn, d.seq + 1));
        let mut diff = history::Diff {
            thread_id,
            turn,
            uncommitted: true,
            want: path,
            whole,
            seq,
            ..Default::default()
        };
        diff.listed(seq, Some(listed), total);
        let read = patch_wanted(&mut diff);
        s.diff = Some(diff);
        read
    };
    changed();
    if let Some(read) = read {
        read_patch(read);
    }
}

pub fn diff_close() {
    if lock().diff.take().is_some() {
        changed();
    }
}

// ---- Rewind, edit from here, undo, fork ----

/// The text of the message that started turn `turn` of the open view.
fn prompt_of(open: &OpenView, turn: u32) -> String {
    open.entries
        .iter()
        .find(|e| {
            e.item.turn == Some(turn)
                && e.item.kind == api::ItemKind::User
                && e.item.via.is_none()
                && e.item.deny_of.is_none()
        })
        .map(|e| e.text().to_owned())
        .unwrap_or_default()
}

/// Opens the dialog that asks before rewinding to before turn `turn`.
pub fn rewind_open(turn: u32) {
    let seq = {
        let mut s = lock();
        let Some(open) = s.open.as_ref().filter(|o| o.kind == OpenKind::Thread) else { return };
        let info = open.turns.iter().find(|t| t.turn == turn);
        let files = info.is_some_and(|t| t.can_restore);
        let why = match info.map(|t| t.checkpoint) {
            _ if files => "",
            Some(api::Checkpoint::NotGit) => "not_git",
            _ => "no_restore_point",
        };
        let mut rewind =
            history::Rewind::new(open.id.clone(), turn, prompt_of(open, turn), files, why.to_owned());
        let seq = rewind.ask();
        s.rewind = Some(rewind);
        seq
    };
    changed();
    preview(seq, true);
}

/// Asks the host what the dialog's choice would do.
fn preview(seq: u64, first: bool) {
    if core_host::try_host().is_none() {
        return;
    }
    core_host::spawn(async move {
        let params = {
            let s = lock();
            let Some(rewind) = s.rewind.as_ref().filter(|r| r.seq == seq) else { return };
            let (conversation, files) = rewind.choice.parts();
            RewindPreviewParams {
                thread_id: rewind.thread_id.clone(),
                turn: rewind.turn,
                conversation,
                files,
                by: None,
            }
        };
        let plan = match connected_client().await {
            Some(client) => {
                client.rewind_preview(params).await.map_err(|e| request_error(&e, Request::Rewind))
            }
            None => Err(tr!("Agents", "Couldn't reach the agent host")),
        };
        if let Some(rewind) = lock().rewind.as_mut() {
            rewind.previewed(seq, plan, first);
        }
        changed();
    });
}

/// "both", "conversation" or "files".
pub fn rewind_choose(word: &str) {
    let Some(choice) = history::Choice::parse(word) else { return };
    let seq = lock().rewind.as_mut().and_then(|r| r.choose(choice));
    changed();
    if let Some(seq) = seq {
        preview(seq, false);
    }
}

/// "Also undo N changes made after <agent> finished".
pub fn rewind_later(on: bool) {
    if let Some(rewind) = lock().rewind.as_mut() {
        rewind.include_later = on;
    }
    changed();
}

/// The Rewind button gained or lost the keyboard.
pub fn rewind_focus(focused: bool) {
    if let Some(rewind) = lock().rewind.as_mut() {
        rewind.button_focused = focused;
    }
}

pub fn rewind_close() {
    let closed = lock().rewind.take().is_some();
    if closed {
        changed();
    }
}

/// Rewind, as the dialog shows it: `how` is "click", or "key" (Enter or
/// Space), which counts only on the Rewind button reached with Tab.
pub fn rewind_apply(how: &str) {
    let how = if how == "click" { history::Confirm::Click } else { history::Confirm::Key };
    let Some((token, include_later)) = lock().rewind.as_mut().and_then(|r| r.confirm(how)) else { return };
    changed();
    if core_host::try_host().is_none() {
        return;
    }
    core_host::spawn(async move {
        let Some(client) = connected_client().await else {
            if let Some(rewind) = lock().rewind.as_mut() {
                rewind.applying = false;
            }
            crate::bridge::app::show_message(tr!("Agents", "Couldn't reach the agent host"));
            changed();
            return;
        };
        let params = RewindApplyParams { token, include_later, refill: false, by: Origin::launcher() };
        match client.rewind_apply(params).await {
            Ok(result) => {
                bridge().remember(&result.thread);
                let left: Vec<(String, String)> =
                    result.left.iter().map(|l| (l.path.clone(), history::left_reason(&l.why))).collect();
                {
                    let mut s = lock();
                    s.rewind = None;
                    if let Some(open) = s.open.as_mut().filter(|o| o.is(OpenKind::Thread, &result.thread.id))
                    {
                        open.left = left;
                    }
                }
                rerender();
                lists_changed();
                changed();
            }
            Err(e) => {
                let again = matches!(e.code(), Some(ErrorCode::Conflict | ErrorCode::NotFound));
                let seq = {
                    let mut s = lock();
                    let Some(rewind) = s.rewind.as_mut() else { return };
                    rewind.applying = false;
                    if again {
                        // The files changed, or the plan lapsed: the lists are read again.
                        rewind.conflict = e.code() == Some(ErrorCode::Conflict);
                        Some(rewind.ask())
                    } else {
                        None
                    }
                };
                changed();
                match seq {
                    Some(seq) => preview(seq, false),
                    None => crate::bridge::app::show_message(request_error(&e, Request::Rewind)),
                }
            }
        }
    });
}

/// Edit from here: the conversation goes back to before turn `turn`, and
/// its message returns to the composer. Undo puts it all back.
pub fn edit_from_here(turn: u32) {
    let Some(thread_id) = ({
        let mut s = lock();
        s.open.as_mut().filter(|o| o.kind == OpenKind::Thread).map(|open| {
            open.composer.refill = true;
            open.left.clear();
            open.id.clone()
        })
    }) else {
        return;
    };
    page_request(
        Request::Rewind,
        move |client| async move {
            let params = RewindPreviewParams { thread_id, turn, conversation: true, files: false, by: None };
            let plan = client.rewind_preview(params).await?;
            let params = RewindApplyParams {
                token: plan.token,
                include_later: false,
                refill: true,
                by: Origin::launcher(),
            };
            client.rewind_apply(params).await
        },
        |result: api::RewindApplyResult| {
            bridge().remember(&result.thread);
            rerender();
            changed();
            lists_changed();
        },
    );
    // A refill that never comes doesn't keep taking the composer.
    if core_host::try_host().is_some() {
        core_host::spawn(async {
            tokio::time::sleep(Duration::from_secs(15)).await;
            if let Some(open) = lock().open.as_mut() {
                open.composer.refill = false;
            }
        });
    }
}

/// Undoes the open thread's last rewind; after Edit from here the composer
/// gives its message back too.
pub fn undo_rewind() {
    let Some((thread_id, editing)) = ({
        let s = lock();
        s.open
            .as_ref()
            .filter(|o| o.kind == OpenKind::Thread)
            .and_then(|o| Some((o.id.clone(), super::editing(o, s.threads.get(&o.id)?))))
    }) else {
        return;
    };
    page_request(
        Request::Rewind,
        move |client| async move { client.rewind_undo(thread_id, Origin::launcher()).await },
        move |info: ThreadInfo| {
            bridge().remember(&info);
            {
                let mut s = lock();
                if let Some(open) = s.open.as_mut().filter(|o| o.is(OpenKind::Thread, &info.id)) {
                    open.left.clear();
                    open.composer.refill = false;
                    if editing {
                        open.composer.put(String::new());
                    }
                }
            }
            rerender();
            changed();
            lists_changed();
        },
    );
}

/// A new thread from the open one, through turn `turn`; it opens.
pub fn fork_turn(turn: u32) {
    let Some(thread_id) = open_id(OpenKind::Thread) else { return };
    page_request(
        Request::Fork,
        move |client| async move {
            client.thread_fork(ThreadForkParams { thread_id, turn: Some(turn), by: Origin::launcher() }).await
        },
        |info: ThreadInfo| {
            bridge().remember(&info);
            lists_changed();
            open_thread(info.id);
        },
    );
}

// ---- Setup ----

/// A folder's "Keep a restore point for each turn".
pub fn set_folder_checkpoints(id: String, on: bool) {
    page_request(
        Request::Setting,
        move |client| async move {
            client
                .folders_set_with(FoldersSetParams { id, checkpoints: Some(on), ..Default::default() })
                .await
        },
        |folder: Folder| {
            let mut s = lock();
            if let Some(f) = s.folders.iter_mut().find(|f| f.id == folder.id) {
                *f = folder;
            }
            drop(s);
            changed();
        },
    );
}

/// Reads what the host keeps, for Setup's History line.
pub fn store_info() {
    let Some(client) = ({
        let s = lock();
        s.client.clone().filter(|_| s.history)
    }) else {
        return;
    };
    if core_host::try_host().is_none() {
        return;
    }
    core_host::spawn(async move {
        if let Ok(info) = client.store_info().await {
            lock().store = Some(info);
            changed();
        }
    });
}

// ---- Phones' requests ----

/// Answers a phone's history and undo request (`docs/protocol/agents.md`
/// §6): never a file restore, a path outside a folder or a `cwd`.
pub(super) async fn phone(
    from: DeviceId,
    client: &Client,
    request: AgentRequest,
) -> Result<AgentReply, AgentFailure> {
    let fail = |e: ClientError| failure(&e);
    let origin = || origin_of(&from);
    Ok(match request {
        AgentRequest::ThreadsPage { show, before_ms } => {
            let show = match show.as_deref() {
                None | Some(wire_history::show::MAIN) => None,
                Some(wire_history::show::ARCHIVED) => Some(ThreadShow::Archived),
                Some(wire_history::show::SNOOZED) => Some(ThreadShow::Snoozed),
                Some(_) => return Err(AgentFailure::BadMessage),
            };
            let params = ThreadsListParams { limit: Some(agent::MAX_THREADS as u32), before_ms, show };
            let threads = client.threads_list_with(params).await.map_err(fail)?;
            AgentReply::Threads(threads.iter().map(|t| thread(t, Some(&from))).collect())
        }
        AgentRequest::Turns { thread: thread_id, before, limit } => {
            let params = ThreadTurnsParams { thread_id, before_turn: before, limit: Some(limit) };
            let turns = client.thread_turns(params).await.map_err(fail)?;
            AgentReply::Turns(turns.iter().map(history::wire_turn).collect())
        }
        AgentRequest::DiffFiles { thread: thread_id, range } => {
            let params = TurnFilesParams::new(thread_id, history::api_range(range));
            AgentReply::TurnFiles(history::wire_files(client.turn_files(params).await.map_err(fail)?))
        }
        AgentRequest::DiffPatch { thread: thread_id, range, path, offset } => {
            let params = TurnPatchParams::new(thread_id, history::api_range(range), path, offset);
            let page = client.turn_patch(params).await.map_err(fail)?;
            AgentReply::Patch(AgentPatchPage {
                patch: page.patch,
                next: page.next,
                truncated: page.truncated,
                binary: page.binary,
            })
        }
        AgentRequest::Search { query, limit, cursor, archived } => {
            let params = ThreadsSearchParams { query, limit: Some(limit), cursor, archived };
            AgentReply::Hits(history::wire_hits(client.threads_search(params).await.map_err(fail)?))
        }
        AgentRequest::Mark { threads, pinned, archived, unread, snooze } => {
            let mut params = ThreadMarkParams::new(threads, origin());
            params.pinned = pinned;
            params.archived = archived;
            params.unread = unread;
            params.snooze_until_ms = snooze;
            let result = client.thread_mark(params).await.map_err(fail)?;
            for t in &result.threads {
                bridge().remember(t);
                shelf_update(bridge(), t);
            }
            lists_changed();
            AgentReply::Threads(result.threads.iter().map(|t| thread(t, Some(&from))).collect())
        }
        AgentRequest::Rename { thread: thread_id, title } => {
            let params = ThreadRenameParams { thread_id, title, by: Some(origin()) };
            let info = client.thread_rename(params).await.map_err(fail)?;
            bridge().remember(&info);
            lists_changed();
            AgentReply::Thread(thread(&info, Some(&from)))
        }
        AgentRequest::Fork { thread: thread_id, turn } => {
            // The host makes a phone's fork supervised or plan.
            let info =
                client.thread_fork(ThreadForkParams { thread_id, turn, by: origin() }).await.map_err(fail)?;
            bridge().remember(&info);
            lists_changed();
            AgentReply::Thread(thread(&info, Some(&from)))
        }
        AgentRequest::RewindPreview { thread: thread_id, turn } => {
            // A phone rewinds the conversation only: its files are never asked about.
            let params =
                RewindPreviewParams { thread_id, turn, conversation: true, files: false, by: Some(origin()) };
            let plan = client.rewind_preview(params).await.map_err(fail)?;
            AgentReply::RewindPlan(history::wire_plan(&plan).ok_or(AgentFailure::Unsupported)?)
        }
        AgentRequest::RewindApply { token, refill } => {
            let params = RewindApplyParams { token, include_later: false, refill, by: origin() };
            let result = client.rewind_apply(params).await.map_err(fail)?;
            bridge().remember(&result.thread);
            lists_changed();
            AgentReply::Thread(thread(&result.thread, Some(&from)))
        }
        AgentRequest::RewindUndo { thread: thread_id } => {
            // A rewind that put files back is undone on this PC only.
            if bridge().known(&thread_id).and_then(|t| t.undo).is_some_and(|u| u.files) {
                return Err(AgentFailure::Denied);
            }
            let info = client.rewind_undo(thread_id, origin()).await.map_err(fail)?;
            bridge().remember(&info);
            lists_changed();
            AgentReply::Thread(thread(&info, Some(&from)))
        }
        AgentRequest::Found { query, agent: agent_id, show, before_ms, limit } => {
            let agent_id = match agent_id.as_deref() {
                None => None,
                Some(word) => Some(AgentId::parse(word).ok_or(AgentFailure::BadMessage)?),
            };
            let show = match show.as_deref() {
                None | Some(wire_history::found_show::ALL_SESSIONS) => FoundShow::All,
                Some(wire_history::found_show::NEW) => FoundShow::New,
                Some(_) => return Err(AgentFailure::BadMessage),
            };
            let params = SessionsScanParams {
                query,
                agent: agent_id,
                show: Some(show),
                before_ms,
                limit: Some(limit),
            };
            AgentReply::Found(history::wire_found(client.sessions_scan(params).await.map_err(fail)?))
        }
        AgentRequest::FoundPreview { id, limit } => {
            let items = client.session_preview(id, Some(limit)).await.map_err(fail)?;
            AgentReply::Items(AgentItems {
                thread: AgentThread::default(),
                items: items.iter().map(item).collect(),
                more_before: false,
                asks: Vec::new(),
                questions: Vec::new(),
            })
        }
        AgentRequest::Import { ids } => {
            let job = client.sessions_import(ids, origin()).await.map_err(fail)?;
            AgentReply::Job(AgentImportJob { job: job.job, total: job.total })
        }
        AgentRequest::ImportCancel { job } => {
            client.import_cancel(job).await.map_err(fail)?;
            AgentReply::Ok
        }
        _ => return Err(AgentFailure::Unsupported),
    })
}
