// SPDX-License-Identifier: GPL-3.0-or-later
//! A pull request, read-only, on the Agents page (phase 6, §X6.6): its
//! card, author and description, its changed files (each opens the diff's
//! "Pull request" scope), checks and comments; and "Ask <agent> to review
//! it", one fixed message the host writes itself.
//!
//! The host reads it with the user's own `gh` and posts nothing; nothing in
//! this view writes to GitHub. The description and comments are drawn by
//! the render crate as Markdown: raw HTML stays text, no image loads, and a
//! link opens only after the page showed where it goes.

use g7_agents::api::{
    FileStatus, GhUse, PrDetail, PrDetailParams, PrDetailResult, PrFile, Purpose, ThreadSendParams, TurnFile,
    TurnFilesResult,
};
use g7_agents_render::BlockKind;

use super::*;

/// Blocks drawn of one text: the render crate's own bound is far above
/// what a 32 KiB description holds.
const MAX_BLOCKS: usize = 400;
/// A repository's `host/owner/name`, in bytes (the core's bound).
const MAX_REPO: usize = 140;

// ---- The rules (pure) ----

/// What the view's button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AskButton {
    /// "Ask <agent> to review it" in the view's thread; `busy`: it waits
    /// until the agent finishes.
    Thread { busy: bool },
    /// "Ask in a new thread…".
    NewThread,
    /// No button: nothing was read, or the GitHub CLI can't read for it.
    Hidden,
}

/// The button, from the view's thread (the one it was opened from, or the
/// host's thread on that branch), whether a detail was read, and the GitHub
/// CLI's state.
pub(crate) fn ask_button(thread: Option<&ThreadInfo>, has_detail: bool, gh: Option<GhUse>) -> AskButton {
    if !has_detail || gh.is_some_and(|g| g != GhUse::Ok) {
        return AskButton::Hidden;
    }
    match thread.filter(|t| t.readonly.is_none()) {
        Some(t) => AskButton::Thread { busy: t.state.is_running() || !t.queued.is_empty() },
        None => AskButton::NewThread,
    }
}

/// A changed file's status, as the diff names it.
fn file_status(change: &str) -> FileStatus {
    match change {
        "added" => FileStatus::Added,
        "deleted" => FileStatus::Deleted,
        "renamed" => FileStatus::Renamed,
        _ => FileStatus::Modified,
    }
}

/// The detail's files as the diff's "Pull request" scope lists them.
pub(crate) fn diff_files(detail: &PrDetail) -> TurnFilesResult {
    TurnFilesResult {
        files: detail
            .files
            .iter()
            .map(|f| TurnFile {
                path: f.path.clone(),
                status: file_status(&f.change),
                from: None,
                adds: f.adds,
                dels: f.dels,
                binary: false,
            })
            .collect(),
        more: detail.files_more,
        adds: detail.adds,
        dels: detail.dels,
    }
}

/// The letter the file list shows for a change.
fn change_letter(change: &str) -> &'static str {
    match change {
        "added" => "A",
        "deleted" => "D",
        "renamed" => "R",
        "copied" => "C",
        _ => "M",
    }
}

fn file_json(f: &PrFile) -> Value {
    json!({ "path": f.path, "change": f.change, "letter": change_letter(&f.change), "adds": f.adds, "dels": f.dels })
}

/// `text` as the page draws it: each Markdown block as markup the app built
/// (everything escaped), its links kept under `key:<n>` for the page's
/// "Open this link?".
pub(crate) fn blocks(
    text: &str,
    key: &str,
    palette: &CodePalette,
    links: &mut HashMap<String, Vec<LinkRef>>,
) -> Vec<Value> {
    let doc = render::markdown(text);
    doc.blocks
        .iter()
        .take(MAX_BLOCKS)
        .enumerate()
        .map(|(i, b)| {
            let key = format!("{key}:{i}");
            let (kind, level) = rows::block_word(b.kind);
            let (marker, number) = rows::marker_word(b.marker);
            let mut found = Vec::new();
            let markup = match b.kind {
                BlockKind::Code => rows::code_markup(b, palette),
                BlockKind::Table => b.table.as_ref().map_or_else(String::new, |t| {
                    let columns = usize::from(t.columns.max(1));
                    let lines: Vec<String> = t
                        .cells
                        .chunks(columns)
                        .map(|row| row.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" | "))
                        .collect();
                    rows::escaped(&lines.join("\n"), true)
                }),
                _ => rows::styled(&b.text, &b.spans, &mut found, palette),
            };
            if !found.is_empty() {
                links.insert(key.clone(), found);
            }
            json!({
                "key": key,
                "kind": kind,
                "level": level,
                "depth": b.depth,
                "quote": b.quote,
                "marker": marker,
                "number": number,
                "markup": markup,
            })
        })
        .collect()
}

/// What the page draws of a detail's texts: the description and each
/// comment's body, as blocks.
fn draw(detail: &PrDetail, seq: u64, palette: &CodePalette) -> (Value, HashMap<String, Vec<LinkRef>>) {
    let mut links = HashMap::new();
    let body = blocks(&detail.body, &format!("pr:{seq}:b"), palette, &mut links);
    let comments: Vec<Value> = detail
        .comments
        .iter()
        .enumerate()
        .map(|(i, c)| Value::Array(blocks(&c.body, &format!("pr:{seq}:c{i}"), palette, &mut links)))
        .collect();
    (json!({ "body": body, "comments": comments }), links)
}

// ---- What the page keeps ----

#[derive(Debug, Default)]
pub(crate) struct Review {
    /// The host has `cap::PR_DETAIL`.
    pub cap: bool,
    pub open: bool,
    /// The thread the view was opened from (its card), if any.
    pub thread_id: Option<String>,
    /// A list row's pull request; empty and 0 for the thread's own.
    pub repo: String,
    pub number: u32,
    pub seq: u64,
    pub loading: bool,
    pub failed: bool,
    pub result: Option<PrDetailResult>,
    /// The description's and comments' blocks, drawn off the GUI thread.
    pub drawn: Value,
    /// Their links, by block key.
    pub links: HashMap<String, Vec<LinkRef>>,
    /// "Ask <agent> to review it" was pressed and hasn't answered.
    pub asking: bool,
}

impl Review {
    fn detail(&self) -> Option<&PrDetail> {
        self.result.as_ref().and_then(|r| r.detail.as_ref())
    }
}

/// The thread the view asks in: the one it was opened from, else the
/// host's thread on that repository and branch, when this PC knows it.
fn view_thread(s: &BridgeState) -> Option<&ThreadInfo> {
    let r = &s.review;
    let id = r.thread_id.clone().or_else(|| r.detail().and_then(|d| d.thread_id.clone()))?;
    s.threads.get(&id)
}

// ---- The page's JSON ----

/// The view (`{}` while it's closed).
pub(super) fn json(s: &BridgeState) -> Value {
    let r = &s.review;
    if !r.open {
        return json!({});
    }
    let detail = r.detail();
    let gh = r.result.as_ref().map(|x| &x.gh).or(s.source.gh.as_ref());
    let hosts = gh.map(|g| g.hosts.as_slice()).unwrap_or_default();
    let thread = view_thread(s);
    let button = ask_button(thread, detail.is_some(), gh.map(|g| g.gh));
    let agent_name = thread
        .map(|t| t.agent.display_name().to_owned())
        .or_else(|| s.agents.iter().find(|a| a.installed && a.signed_in).map(|a| a.name.clone()))
        .unwrap_or_default();
    let number = detail.map_or(r.number, |d| d.pr.number);
    let comments: Vec<Value> = detail.map_or_else(Vec::new, |d| {
        d.comments
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let url = c.url.clone().filter(|u| source::safe_url(u, hosts)).unwrap_or_default();
                json!({
                    "author": c.author,
                    "atMs": c.at_ms,
                    "blocks": r.drawn.get("comments").and_then(|x| x.get(i)).cloned().unwrap_or_else(|| json!([])),
                    "cut": c.cut,
                    "review": c.review.clone().unwrap_or_default(),
                    "url": url,
                })
            })
            .collect()
    });
    json!({
        "open": true,
        "number": number,
        "repo": detail.map_or_else(|| r.repo.clone(), |d| d.repo.clone()),
        "loading": r.loading && detail.is_none(),
        "refreshing": r.loading,
        "failed": r.failed && detail.is_none(),
        "why": r.result.as_ref().and_then(|x| x.why.as_ref()).map(word).unwrap_or_default(),
        "gh": gh.map_or_else(|| json!({}), |g| json!({ "gh": word(&g.gh), "hosts": g.hosts })),
        "pr": detail.map_or_else(|| json!({}), |d| {
            let mut card = source::pr_json(&d.pr);
            card["canOpen"] = json!(source::safe_url(&d.pr.url, hosts));
            card
        }),
        "author": detail.map(|d| d.author.clone()).unwrap_or_default(),
        "body": r.drawn.get("body").cloned().unwrap_or_else(|| json!([])),
        "bodyCut": detail.is_some_and(|d| d.body_cut),
        "adds": detail.map_or(0, |d| d.adds),
        "dels": detail.map_or(0, |d| d.dels),
        "changed": detail.map_or(0, |d| d.changed),
        "files": detail.map_or_else(Vec::new, |d| d.files.iter().map(file_json).collect()),
        "filesMore": detail.map_or(0, |d| (d.changed as usize).saturating_sub(d.files.len())),
        "comments": comments,
        "commentsOlder": detail.map_or(0, |d| d.comments_older),
        "readMs": detail.map_or(0, |d| d.read_ms),
        "ask": {
            "mode": match button {
                AskButton::Thread { .. } => "thread",
                AskButton::NewThread => "new",
                AskButton::Hidden => "",
            },
            "busy": matches!(button, AskButton::Thread { busy: true }) || r.asking,
            "agentName": agent_name,
            "threadId": thread.map(|t| t.id.clone()).unwrap_or_default(),
            "folderId": detail.and_then(|d| d.folder_id.clone()).unwrap_or_default(),
        },
    })
}

/// A link in the view's markup (`key` as the page got it).
pub(crate) fn link(key: &str, index: usize) -> Option<LinkRef> {
    lock().review.links.get(key)?.get(index).cloned()
}

// ---- Talking to the host ----

/// The host connected: whether it reads pull requests in detail.
pub(crate) fn connected(cap: bool) {
    let reread = {
        let mut s = lock();
        s.review.cap = cap;
        cap && s.review.open
    };
    if reread {
        read(false);
    }
}

/// Opens the view: `repo` and `number` of a list row, or "" and 0 for the
/// open thread's own pull request.
pub fn open(repo: String, number: u32) {
    {
        let mut s = lock();
        if !s.review.cap {
            return;
        }
        let thread_id = if repo.is_empty() || number == 0 {
            let Some(id) = s.open.as_ref().filter(|o| o.kind == OpenKind::Thread).map(|o| o.id.clone())
            else {
                return;
            };
            Some(id)
        } else {
            None
        };
        let (repo, number) = if thread_id.is_some() { (String::new(), 0) } else { (repo, number) };
        let seq = s.review.seq;
        s.review = Review { cap: true, open: true, thread_id, repo, number, seq, ..Default::default() };
        s.diff = None;
    }
    read(false);
}

pub fn close() {
    let mut s = lock();
    if s.review.open {
        let (cap, seq) = (s.review.cap, s.review.seq);
        s.review = Review { cap, seq, ..Default::default() };
        if s.diff.as_ref().is_some_and(|d| d.pr.is_some()) {
            s.diff = None;
        }
        drop(s);
        changed();
    }
}

/// "Refresh": past the host's cache.
pub fn refresh() {
    read(true);
}

fn read(refresh: bool) {
    let Some((seq, params)) = ({
        let mut s = lock();
        let r = &mut s.review;
        if !r.open {
            None
        } else {
            r.seq += 1;
            r.loading = true;
            let params = match r.thread_id.clone() {
                Some(thread_id) => {
                    PrDetailParams { thread_id: Some(thread_id), repo: None, number: None, refresh }
                }
                None => PrDetailParams {
                    thread_id: None,
                    repo: Some(r.repo.clone()),
                    number: Some(r.number),
                    refresh,
                },
            };
            Some((r.seq, params))
        }
    }) else {
        return;
    };
    changed();
    source::with_client(move |client| async move {
        let read = client.pr_detail(params).await;
        // Drawn on a worker, with the page's palette taken before the lock.
        let palette = bridge().render().palette.clone();
        let drawn = match read.as_ref().ok().and_then(|r| r.detail.clone()) {
            Some(detail) => tokio::task::spawn_blocking(move || draw(&detail, seq, &palette)).await.ok(),
            None => None,
        };
        {
            let mut guard = lock();
            let s = &mut *guard;
            let r = &mut s.review;
            if !r.open || r.seq != seq {
                return;
            }
            r.loading = false;
            match read {
                Ok(result) => {
                    s.source.gh = Some(result.gh.clone());
                    r.failed = false;
                    // GitHub out of reach: the last detail read stays, marked.
                    let kept = r.result.as_ref().and_then(|x| x.detail.clone());
                    let detail = result.detail.clone().or(kept);
                    if let Some((shown, links)) = drawn {
                        r.drawn = shown;
                        r.links = links;
                    }
                    r.result = Some(PrDetailResult { detail, ..result });
                }
                Err(e) => {
                    tracing::debug!(error = %e, "a pull request wasn't read");
                    r.failed = true;
                }
            }
        }
        changed();
    });
}

/// Opens a changed file in the diff view.
pub fn file(path: String) {
    let opened = {
        let s = lock();
        let r = &s.review;
        let Some(detail) = r.detail().filter(|d| d.files.iter().any(|f| f.path == path)) else { return };
        let scope = history::PrScope { repo: detail.repo.clone(), number: detail.pr.number };
        let thread = r.thread_id.clone().or_else(|| detail.thread_id.clone()).unwrap_or_default();
        (scope, thread, diff_files(detail), detail.changed)
    };
    let (scope, thread, files, total) = opened;
    history_page::pr_diff_open(scope, thread, files, total, path);
}

/// "Ask <agent> to review it": one message in the view's thread, which then
/// shows. The host writes the message from the number; nothing else goes.
pub fn ask() {
    let Some((thread_id, number)) = ({
        let mut s = lock();
        let thread = view_thread(&s);
        let number = s.review.detail().map(|d| d.pr.number);
        let gh = s.review.result.as_ref().map(|r| r.gh.gh);
        match (thread, number) {
            (Some(t), Some(number))
                if ask_button(Some(t), true, gh) == (AskButton::Thread { busy: false })
                    && !s.review.asking =>
            {
                let id = t.id.clone();
                s.review.asking = true;
                Some((id, number))
            }
            _ => None,
        }
    }) else {
        return;
    };
    changed();
    source::with_client(move |client| async move {
        let sent = client
            .thread_send_with(ThreadSendParams::ask_review(thread_id.clone(), number, Origin::launcher()))
            .await;
        lock().review.asking = false;
        match sent {
            Ok(_) => {
                close();
                open_thread(thread_id);
            }
            Err(e) => {
                changed();
                source::tell(&source::reason_of(&e));
            }
        }
    });
}

/// "Ask in a new thread…": the new-thread form's Start, with the chip
/// "Review pull request #N" in place of the prompt. The host checks the
/// pull request is one it listed or read, and that the folder is a
/// checkout of its repository.
pub fn new_thread(agent: String, folder_id: String, mode: String, repo: String, number: u32) {
    let Some(agent) = AgentId::parse(&agent) else { return };
    if repo.is_empty() || repo.len() > MAX_REPO || number == 0 || !lock().review.cap {
        return;
    }
    let mut params = new_thread_params(agent, folder_id, String::new(), &mode);
    params.purpose = Some(Purpose::Review);
    params.pr = Some(number);
    params.repo = Some(repo.clone());
    busy(1);
    source::with_client(move |client| async move {
        let started = client.thread_start(params).await;
        busy(-1);
        match started {
            Ok(info) => {
                bridge().remember(&info);
                lists_changed();
                close();
                open_thread(info.id);
            }
            Err(e) if source::reason_of(&e) == "wrong_repo" => {
                crate::bridge::app::show_message(tr!("Agents", "That folder isn't a checkout of %1.", repo));
            }
            Err(e) => crate::bridge::app::show_message(request_error(&e, Request::Start)),
        }
    });
}

#[cfg(test)]
mod tests {
    use g7_agents::api::{PrInfo, PrState, State};

    use super::*;
    use crate::agents::history_tests::a_thread;

    fn a_detail(files: &[(&str, &str)], changed: u32) -> PrDetail {
        PrDetail {
            pr: PrInfo {
                number: 42,
                title: "Stock sync".into(),
                state: PrState::Open,
                draft: false,
                url: "https://github.com/o/r/pull/42".into(),
                base: "main".into(),
                head: "feat/sync".into(),
                head_oid: "0".repeat(40),
                review: None,
                mergeable: None,
                blocked: false,
                updated_ms: 1,
                checks: Vec::new(),
                pass: 0,
                fail: 0,
                pending: 0,
                read_ms: 1,
            },
            repo: "o/r".into(),
            author: "someone".into(),
            body: String::new(),
            body_cut: false,
            adds: 7,
            dels: 2,
            changed,
            files: files
                .iter()
                .map(|(path, change)| PrFile {
                    path: (*path).into(),
                    change: (*change).into(),
                    adds: 1,
                    dels: 1,
                })
                .collect(),
            files_more: (changed as usize) > files.len(),
            comments: Vec::new(),
            comments_older: 0,
            thread_id: None,
            folder_id: None,
            read_ms: 1,
        }
    }

    #[test]
    fn the_button_asks_in_the_thread_and_waits_while_it_works() {
        let mut thread = a_thread("t");
        thread.state = State::Done;
        assert_eq!(ask_button(Some(&thread), true, Some(GhUse::Ok)), AskButton::Thread { busy: false });
        thread.state = State::Working;
        assert_eq!(ask_button(Some(&thread), true, Some(GhUse::Ok)), AskButton::Thread { busy: true });
        thread.state = State::Waiting;
        assert_eq!(ask_button(Some(&thread), true, None), AskButton::Thread { busy: true });
    }

    #[test]
    fn without_a_thread_the_button_starts_one() {
        assert_eq!(ask_button(None, true, Some(GhUse::Ok)), AskButton::NewThread);
        let mut readonly = a_thread("t");
        readonly.readonly = Some(api::Readonly::FolderMissing);
        assert_eq!(
            ask_button(Some(&readonly), true, Some(GhUse::Ok)),
            AskButton::NewThread,
            "it can't take a message"
        );
    }

    #[test]
    fn nothing_is_offered_before_a_read_or_when_gh_cant_read() {
        let thread = a_thread("t");
        assert_eq!(ask_button(Some(&thread), false, Some(GhUse::Ok)), AskButton::Hidden);
        assert_eq!(ask_button(Some(&thread), true, Some(GhUse::SignedOut)), AskButton::Hidden);
        assert_eq!(ask_button(None, true, Some(GhUse::Missing)), AskButton::Hidden);
        assert_eq!(ask_button(None, true, Some(GhUse::Old)), AskButton::Hidden);
    }

    #[test]
    fn the_files_become_the_diffs_pull_request_scope() {
        let detail = a_detail(
            &[
                ("src/a.rs", "modified"),
                ("new.md", "added"),
                ("old.txt", "deleted"),
                ("b.rs", "renamed"),
                ("c", "copied"),
            ],
            310,
        );
        let listed = diff_files(&detail);
        let statuses: Vec<FileStatus> = listed.files.iter().map(|f| f.status).collect();
        assert_eq!(
            statuses,
            [
                FileStatus::Modified,
                FileStatus::Added,
                FileStatus::Deleted,
                FileStatus::Renamed,
                FileStatus::Modified
            ]
        );
        assert!(listed.more, "the files GitHub listed past 300 are counted");
        assert_eq!((listed.adds, listed.dels), (7, 2));
        assert_eq!(change_letter("copied"), "C");
        assert_eq!(change_letter("changed"), "M");
    }

    #[test]
    fn a_description_is_drawn_escaped_with_its_links_kept_aside() {
        let mut links = HashMap::new();
        let palette = CodePalette::default();
        let drawn = blocks(
            "# Fix\n\n<img src=\"https://x.example/p.png\"> see [the docs](https://example.com/d)\n\n```sh\nrm -rf x\n```",
            "pr:1:b",
            &palette,
            &mut links,
        );
        let markup: Vec<&str> = drawn.iter().filter_map(|b| b["markup"].as_str()).collect();
        assert_eq!(drawn[0]["kind"], "heading");
        assert!(markup.iter().all(|m| !m.contains("<img")), "raw HTML stays text: {markup:?}");
        assert!(markup.iter().any(|m| m.contains("&lt;img")));
        let key = drawn.iter().find(|b| b["markup"].as_str().is_some_and(|m| m.contains("<a href"))).unwrap()
            ["key"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            links.get(&key).and_then(|l| l.first()).map(|l| l.href.as_str()),
            Some("https://example.com/d")
        );
        assert!(drawn.iter().any(|b| b["kind"] == "code"));
    }
}
