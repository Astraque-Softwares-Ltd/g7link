// SPDX-License-Identifier: GPL-3.0-or-later
//! Phase 3 with the page's own state and a host on a test socket: the
//! timeline with a thread's turns, rewinds and their undo, marks, and what
//! phones are relayed (never files, never a `cwd`).

use g7_agents::api::{
    Checkpoint, CheckpointWhy, FileStatus, ItemKind, NoticeKind, TurnFile, TurnInfo, UndoInfo,
};
use g7link_core::TurnRange as WireRange;

use super::*;

pub(super) fn a_thread(id: &str) -> ThreadInfo {
    ThreadInfo {
        id: id.into(),
        agent: AgentId::Claude,
        folder_id: "f0123456789ab".into(),
        folder_label: "duka-pos".into(),
        title: "Stock sync".into(),
        state: State::Done,
        mode: Mode::Supervised,
        origin: Origin::launcher(),
        created_ms: 1,
        updated_ms: 2,
        last_seq: 3,
        pending: 0,
        summary: None,
        error: None,
        native_id: None,
        step: None,
        plan: None,
        state_since_ms: 0,
        subdir: None,
        continued: false,
        model: None,
        effort: None,
        answered_model: None,
        queued: Vec::new(),
        queue_held: false,
        context: None,
        mcp: None,
        asking: 0,
        pinned: false,
        archived: false,
        unread: false,
        snoozed_until_ms: None,
        turns: 2,
        forked_from: None,
        found: false,
        readonly: None,
        undo: None,
        branch: None,
        worktree: None,
        changed: None,
        wrote: None,
        ports: Vec::new(),
        runs: Vec::new(),
    }
}

fn a_turn(n: u32, checkpoint: Checkpoint) -> TurnInfo {
    TurnInfo {
        turn: n,
        prompt_seq: u64::from(n) * 10,
        last_seq: Some(u64::from(n) * 10 + 2),
        started_ms: 1,
        ended_ms: Some(2),
        ended: Some(api::TurnEnd::Done),
        checkpoint,
        why: None,
        files: 2,
        adds: 5,
        dels: 1,
        more_files: false,
        skipped: 0,
        can_fork: true,
        can_rewind: true,
        can_restore: true,
    }
}

fn user(seq: u64, turn: u32, text: &str) -> Item {
    Item {
        text: Some(text.into()),
        turn: Some(turn),
        ..Item::new(seq, format!("u{seq}"), ItemKind::User, seq)
    }
}

fn answer(seq: u64, turn: u32) -> Item {
    Item {
        text: Some("Done.".into()),
        turn: Some(turn),
        phase: Some(api::Phase::Final),
        end_ms: Some(seq + 1),
        ..Item::new(seq, format!("m{seq}"), ItemKind::Assistant, seq)
    }
}

fn notice(seq: u64, turn: u32, kind: NoticeKind) -> Item {
    Item {
        text: Some("Rewound to here".into()),
        turn: Some(turn),
        notice: Some(kind),
        ..Item::new(seq, format!("n{seq}"), ItemKind::Notice, seq)
    }
}

fn file(path: &str) -> TurnFile {
    TurnFile { path: path.into(), status: FileStatus::Modified, from: None, adds: 3, dels: 1, binary: false }
}

/// A page state with `items` open in thread "t" and its turns.
fn page_with(items: &[Item], turns: Vec<TurnInfo>, undo: bool) -> BridgeState {
    let mut s = BridgeState::default();
    let mut view = OpenView::new(7, OpenKind::Thread, "t".into());
    for item in items {
        view.apply(Entry::from_item(item));
    }
    view.turns = Arc::new(turns);
    view.turn_paths =
        Arc::new(HashMap::from([(1, vec![file("src/sync/queue.rs"), file("src/sync/mod.rs")])]));
    s.open = Some(view);
    let mut info = a_thread("t");
    info.undo = undo.then_some(UndoInfo { until_ms: now_ms() + 60_000, files: true });
    s.threads.insert("t".into(), info);
    s
}

fn rows_of(s: &BridgeState) -> Vec<Arc<ModelRow>> {
    let snap = snapshot(s).expect("a view is open");
    let mut render = Render::default();
    render.reset(snap.generation);
    render.update(&snap).expect("its own view");
    render.shown.clone()
}

fn find<'a>(rows: &'a [Arc<ModelRow>], view: &str) -> Vec<&'a Arc<ModelRow>> {
    rows.iter().filter(|r| r.text(Role::View) == view).collect()
}

#[test]
fn a_recorded_turn_opens_its_diff_and_only_the_newest_rewind_carries_undo() {
    let items = [
        user(1, 1, "Fix the sync"),
        answer(2, 1),
        notice(3, 1, NoticeKind::Rewound),
        user(4, 2, "Add backoff"),
        answer(5, 2),
        notice(6, 2, NoticeKind::EditFromHere),
    ];
    let mut s = page_with(&items, vec![a_turn(1, Checkpoint::Ready), a_turn(2, Checkpoint::Ready)], true);
    s.open.as_mut().unwrap().left = vec![("Cargo.lock".into(), "changed since you looked".into())];
    let rows = rows_of(&s);
    let changes = find(&rows, "changes");
    assert!(!changes.is_empty(), "{rows:?}");
    assert_eq!(changes[0].get(Role::Recorded), &RoleValue::Flag(true));
    let notices = find(&rows, "notice");
    assert_eq!(notices.len(), 2);
    assert_eq!(notices[0].get(Role::Undo), &RoleValue::Flag(false), "an older rewind's notice");
    assert_eq!(notices[1].get(Role::Undo), &RoleValue::Flag(true));
    assert_eq!(notices[1].text(Role::Notice), "edit_from_here");
    let at = rows.iter().position(|r| r.key() == notices[1].key()).unwrap();
    assert_eq!(rows[at + 1].text(Role::View), "left", "what was left follows its notice");
    // Actions are offered on your message and the final answer.
    let mine = rows.iter().find(|r| r.text(Role::View) == "user").unwrap();
    assert_eq!(mine.text(Role::ActRewind), "on");

    // Once it can't be undone, no notice carries Undo.
    let s = page_with(&items, vec![a_turn(1, Checkpoint::Ready), a_turn(2, Checkpoint::Ready)], false);
    assert!(find(&rows_of(&s), "notice").iter().all(|r| r.get(Role::Undo) == &RoleValue::Flag(false)));
}

#[test]
fn a_turn_not_recorded_says_why() {
    let mut edit = Item { turn: Some(1), end_ms: Some(3), ..Item::new(2, "toolu_1", ItemKind::Tool, 2) };
    edit.tool = Some(api::Tool {
        name: "Edit".into(),
        title: "Edit a.rs".into(),
        class: Some(api::ToolClass::Edit),
        detail: Some("-a\n+b".into()),
        path: Some("a.rs".into()),
        status: api::ToolStatus::Ok,
        output: None,
        by_rule: false,
    });
    let items = [user(1, 1, "Fix it"), edit, answer(4, 1)];
    let mut turn = a_turn(1, Checkpoint::Missing);
    turn.why = Some(CheckpointWhy::Busy);
    let rows = rows_of(&page_with(&items, vec![turn], false));
    let notes = find(&rows, "changes_note");
    assert_eq!(notes.len(), 1, "{rows:?}");
    assert_eq!((notes[0].text(Role::Note), notes[0].text(Role::Why)), ("missing", "busy"));
}

#[test]
fn a_cut_leaves_the_view_and_its_undo_clears_what_was_left() {
    let items = [user(1, 1, "a"), answer(2, 1), user(3, 2, "b"), answer(4, 2)];
    let mut s = page_with(&items, Vec::new(), false);
    let open = s.open.as_mut().unwrap();
    open.left = vec![("x".into(), "not a file".into())];
    history_page::apply_cut(open, 3, 4, false);
    assert_eq!(open.entries.iter().map(|e| e.item.seq).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(open.left.len(), 1, "a cut keeps what the rewind left to say");
    history_page::apply_cut(open, 3, 4, true);
    assert!(open.left.is_empty());
    // A hit's message that isn't loaded is read; one that is, isn't.
    open.focus = Some(2);
    assert_eq!(history_page::focus_missing(open), None);
    open.focus = Some(40);
    assert_eq!(history_page::focus_missing(open), Some(40));
}

#[test]
fn marks_are_words_for_up_to_fifty_threads_and_a_snooze_must_be_ahead() {
    let ids = |n: usize| (0..n).map(|i| format!("t{i}")).collect::<Vec<_>>();
    let p = history_page::mark_params(vec!["a".into(), "a".into(), "b".into()], "pin", 0, 1_000).unwrap();
    assert_eq!((p.thread_ids.len(), p.pinned), (2, Some(true)));
    assert_eq!(history_page::mark_params(ids(60), "archive", 0, 1_000).unwrap().thread_ids.len(), 50);
    assert_eq!(history_page::mark_params(ids(1), "read", 0, 1_000).unwrap().unread, Some(false));
    assert!(history_page::mark_params(ids(1), "snooze", 999, 1_000).is_none(), "in the past");
    assert!(history_page::mark_params(ids(1), "snooze", 1_000 + api::MAX_SNOOZE_MS + 1, 1_000).is_none());
    assert_eq!(
        history_page::mark_params(ids(1), "snooze", 5_000, 1_000).unwrap().snooze_until_ms,
        Some(5_000)
    );
    assert_eq!(history_page::mark_params(ids(1), "unsnooze", 0, 1_000).unwrap().snooze_until_ms, Some(0));
    // Unsnooze, one thread or several picked: 0 ends the snooze, and nothing else changes.
    let back = history_page::mark_params(ids(3), "unsnooze", 0, 1_000).unwrap();
    assert_eq!((back.thread_ids.len(), back.snooze_until_ms), (3, Some(0)));
    assert_eq!((back.pinned, back.archived, back.unread), (None, None, None));
    assert_eq!(serde_json::to_value(&back).unwrap()["snooze_until_ms"], 0, "0 goes to the host");
    assert!(history_page::mark_params(ids(1), "delete", 0, 1_000).is_none());
    assert!(history_page::mark_params(Vec::new(), "pin", 0, 1_000).is_none());
}

#[test]
fn archived_and_snoozed_threads_leave_the_main_list() {
    let mut threads = HashMap::new();
    for (id, archived, snoozed) in
        [("a", false, None), ("b", true, None), ("c", false, Some(u64::MAX)), ("d", false, Some(1))]
    {
        let mut t = a_thread(id);
        t.archived = archived;
        t.snoozed_until_ms = snoozed;
        threads.insert(id.to_owned(), t);
    }
    let mut listed: Vec<&str> = listed_threads(&threads, 100).iter().map(|t| t.id.as_str()).collect();
    listed.sort_unstable();
    assert_eq!(listed, ["a", "d"], "a snooze that ended is back");
    let mut fork = a_thread("e");
    fork.forked_from =
        Some(Box::new(api::ForkedFrom { thread_id: "a".into(), turn: Some(1), title: "Stock".into() }));
    fork.unread = true;
    let row = full_thread_row(&fork, &[], 100);
    assert_eq!((row["forkOf"]["title"].as_str(), row["unread"].as_bool()), (Some("Stock"), Some(true)));
    let wire = thread(&fork, None);
    assert_eq!(wire.forked_from.map(|f| f.title), Some("Stock".into()));
}

// ---- Phones, through a host on a test socket ----

type Log = Arc<Mutex<Vec<(String, Value)>>>;

/// A host that offers history and undo, answers phase 3's methods with
/// fixed results, and logs each request with its params.
async fn history_host(path: std::path::PathBuf, log: Log) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let thread = serde_json::to_value(a_thread("0123456789abcdef0123456789abcdef")).unwrap();
    loop {
        let Ok((stream, _)) = listener.accept().await else { return };
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let request: Value = serde_json::from_str(&line).unwrap();
            let method = request["method"].as_str().unwrap_or_default().to_owned();
            log.lock().unwrap().push((method.clone(), request["params"].clone()));
            let result = match method.as_str() {
                "hello" => {
                    json!({ "version": 1, "host_version": "test", "pid": 1, "caps": ["history", "undo"] })
                }
                "rewind.preview" => json!({ "plan": {
                    "token": "ab".repeat(16), "expires_ms": 9, "thread_id": "t", "turn": 2,
                    "cut": { "turns": 1, "messages": 3 },
                } }),
                "sessions.scan" => json!({
                    "found": [{
                        "id": "sx0123456789abcdef0123456789abcdef", "agent": "codex", "title": "Soil import",
                        "folder_label": "shamba-api", "cwd": "/home/me/secret/shamba-api", "updated_ms": 5,
                        "bytes": 10, "source": "terminal",
                    }],
                    "total": 1, "scanning": false, "partial": false,
                }),
                "thread.turns" => {
                    json!({ "turns": [serde_json::to_value(a_turn(1, Checkpoint::Ready)).unwrap()] })
                }
                "thread.send" => json!({ "duplicate": true }),
                "turn.files" => json!({ "files": [], "more": false, "adds": 0, "dels": 0 }),
                "rewind.undo" | "rewind.apply" | "thread.fork" | "thread.rename" => {
                    json!({ "thread": thread })
                }
                "thread.mark" => json!({ "threads": [thread] }),
                _ => json!({}),
            };
            let reply = json!({ "id": request["id"], "result": result });
            write.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
        }
    }
}

async fn connected(log: &Log) -> (tempfile::TempDir, Client) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("host.sock");
    tokio::spawn(history_host(path.clone(), log.clone()));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(tokio::time::Instant::now() < deadline, "the host never listened");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (client, _events) = Client::connect(&path, hello()).await.unwrap();
    (dir, client)
}

fn params_of(log: &Log, method: &str) -> Option<Value> {
    log.lock().unwrap().iter().rev().find(|(m, _)| m == method).map(|(_, p)| p.clone())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_rewinds_the_conversation_only_and_never_sees_a_folder_path() {
    let log: Log = Arc::default();
    let (_dir, client) = connected(&log).await;
    let phone = DeviceId([7; 32]);

    let reply =
        history_page::phone(phone, &client, AgentRequest::RewindPreview { thread: "t".into(), turn: 2 })
            .await
            .unwrap();
    let AgentReply::RewindPlan(plan) = reply else { panic!("{reply:?}") };
    assert_eq!((plan.turns, plan.messages), (1, 3));
    let asked = params_of(&log, "rewind.preview").unwrap();
    assert_eq!((asked["conversation"].as_bool(), asked["files"].as_bool()), (Some(true), Some(false)));
    assert_eq!(asked["by"]["kind"], "phone");

    history_page::phone(phone, &client, AgentRequest::RewindApply { token: "ab".repeat(16), refill: true })
        .await
        .unwrap();
    let applied = params_of(&log, "rewind.apply").unwrap();
    assert_eq!((applied["include_later"].as_bool(), applied["refill"].as_bool()), (Some(false), Some(true)));

    let reply = history_page::phone(
        phone,
        &client,
        AgentRequest::Found {
            query: None,
            agent: Some("codex".into()),
            show: None,
            before_ms: None,
            limit: 50,
        },
    )
    .await
    .unwrap();
    let AgentReply::Found(found) = reply else { panic!("{reply:?}") };
    assert_eq!(found.found[0].folder, "shamba-api");
    assert!(
        !format!("{:?}", serde_json::to_string(&found)).contains("/home/me"),
        "a cwd never leaves the PC"
    );

    let reply = history_page::phone(
        phone,
        &client,
        AgentRequest::Turns { thread: "t".into(), before: None, limit: 30 },
    )
    .await
    .unwrap();
    let AgentReply::Turns(turns) = reply else { panic!("{reply:?}") };
    assert!(serde_json::to_value(&turns[0]).unwrap().get("can_restore").is_none());

    let range = WireRange::Span { from: 1, to: 2 };
    history_page::phone(phone, &client, AgentRequest::DiffFiles { thread: "t".into(), range }).await.unwrap();
    let files = params_of(&log, "turn.files").unwrap();
    assert_eq!((files["from_turn"].as_u64(), files["to_turn"].as_u64()), (Some(1), Some(2)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_never_undoes_a_rewind_that_put_files_back() {
    let log: Log = Arc::default();
    let (_dir, client) = connected(&log).await;
    let phone = DeviceId([8; 32]);
    let id = "fedcba9876543210fedcba9876543210";
    let mut info = a_thread(id);
    info.undo = Some(UndoInfo { until_ms: u64::MAX, files: true });
    bridge().remember(&info);
    let refused = history_page::phone(phone, &client, AgentRequest::RewindUndo { thread: id.into() }).await;
    assert_eq!(refused.unwrap_err(), AgentFailure::Denied);
    assert!(params_of(&log, "rewind.undo").is_none(), "the host is never asked");

    info.undo = Some(UndoInfo { until_ms: u64::MAX, files: false });
    bridge().remember(&info);
    history_page::phone(phone, &client, AgentRequest::RewindUndo { thread: id.into() }).await.unwrap();
    assert_eq!(params_of(&log, "rewind.undo").unwrap()["by"]["kind"], "phone");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_marks_and_forks_as_itself() {
    let log: Log = Arc::default();
    let (_dir, client) = connected(&log).await;
    let phone = DeviceId([9; 32]);
    let mark = AgentRequest::Mark {
        threads: vec!["t".into()],
        pinned: Some(true),
        archived: None,
        unread: None,
        snooze: None,
    };
    let reply = history_page::phone(phone, &client, mark).await.unwrap();
    assert!(matches!(reply, AgentReply::Threads(ref t) if t.len() == 1), "{reply:?}");
    let marked = params_of(&log, "thread.mark").unwrap();
    assert_eq!((marked["pinned"].as_bool(), marked["by"]["kind"].as_str()), (Some(true), Some("phone")));
    history_page::phone(phone, &client, AgentRequest::Fork { thread: "t".into(), turn: Some(1) })
        .await
        .unwrap();
    assert_eq!(params_of(&log, "thread.fork").unwrap()["turn"], 1);
    let bad = AgentRequest::ThreadsPage { show: Some("trash".into()), before_ms: None };
    assert_eq!(history_page::phone(phone, &client, bad).await.unwrap_err(), AgentFailure::BadMessage);
}
