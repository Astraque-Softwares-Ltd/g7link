// SPDX-License-Identifier: MPL-2.0
//! The thread store (§U3): the bundled SQLite, the one-time import of the
//! file store (the export byte-for-byte the same), crashes during it, an
//! unreadable database, batching, rewinds' cuts, search, the limits, message
//! keys, forks and the sessions found.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use g7_agents::api::*;
use g7_agents::host::db::{
    self, CutRange, Db, FoundFile, FoundQuery, FoundRow, Interrupt, SearchQuery, ThreadCols, TurnRow,
    import_v1,
};
use g7_agents::host::export::{self, Header};
use serde_json::json;

const A: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
const B: &str = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/store-v1")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A data directory holding a copy of the v1 fixture tree.
fn v1_root() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_tree(&fixture_root(), dir.path());
    dir
}

fn tid(n: u32) -> String {
    format!("{n:032x}")
}

fn info(id: &str, title: &str, state: &str, updated_ms: u64) -> ThreadInfo {
    serde_json::from_value(json!({
        "id": id, "agent": "claude", "folder_id": "f0123456789ab", "folder_label": "g7linux",
        "title": title, "state": state, "mode": "supervised", "origin": {"kind": "launcher"},
        "created_ms": 1, "updated_ms": updated_ms, "last_seq": 0, "pending": 0
    }))
    .unwrap()
}

fn cols() -> ThreadCols {
    ThreadCols { next_seq: 1, next_turn: 1, ..ThreadCols::default() }
}

fn user(seq: u64, turn: u32, text: &str, ts: u64) -> Item {
    Item {
        text: Some(text.into()),
        turn: Some(turn),
        origin: Some(Origin::launcher()),
        ..Item::new(seq, format!("user-{seq}"), ItemKind::User, ts)
    }
}

fn answer(seq: u64, turn: u32, text: &str, ts: u64) -> Item {
    Item {
        text: Some(text.into()),
        turn: Some(turn),
        phase: Some(Phase::Final),
        end_ms: Some(ts + 1),
        ..Item::new(seq, format!("msg-{seq}"), ItemKind::Assistant, ts)
    }
}

/// A thread whose turn `n` is a prompt and an answer, each turn with a
/// recorded restore point and a native mark.
fn thread_with_turns(db: &mut Db, id: &str, title: &str, turns: &[(&str, &str)]) {
    let mut thread = info(id, title, "done", 1000);
    thread.native_id = Some("native-1".into());
    db.save_thread(&thread, &cols()).unwrap();
    let mut seq = 1;
    for (n, (prompt, reply)) in (1u32..).zip(turns) {
        let ts = u64::from(n) * 100;
        db.append_item(id, &user(seq, n, prompt, ts)).unwrap();
        let row = TurnRow {
            checkpoint: Checkpoint::Ready,
            pre_oid: Some(format!("{n:040x}")),
            post_oid: Some(format!("{:040x}", n + 1)),
            native_mark: Some(format!("mark-{n}")),
            files: 1,
            adds: 2,
            ..TurnRow::new(n, seq, ts)
        };
        let file = TurnFile {
            path: format!("src/{n}.rs"),
            status: FileStatus::Modified,
            from: None,
            adds: 2,
            dels: 0,
            binary: false,
        };
        db.save_turn(id, &row, Some(&[file])).unwrap();
        db.append_item(id, &answer(seq + 1, n, reply, ts + 10)).unwrap();
        seq += 2;
    }
    db.flush().unwrap();
}

fn search(db: &Db, query: &str) -> db::SearchPage {
    db.search(&SearchQuery { query: query.into(), limit: 20, ..SearchQuery::default() }, &Interrupt::new())
        .unwrap()
}

fn seqs(page: &db::SearchPage) -> Vec<(String, u64)> {
    page.hits.iter().map(|h| (h.thread_id.clone(), h.seq)).collect()
}

// ---- The database ----

#[test]
fn the_bundled_sqlite_has_fts5_trigram() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE t USING fts5(x, tokenize = 'trigram remove_diacritics 1', content = '', contentless_delete = 1);
         INSERT INTO t (rowid, x) VALUES (1, 'Le café crème est prêt');",
    )
    .unwrap();
    let hit = |q: &str| -> Vec<i64> {
        conn.prepare("SELECT rowid FROM t WHERE t MATCH ?1")
            .unwrap()
            .query_map([q], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(hit("\"cafe\""), [1]);
    assert_eq!(hit("\"CAFÉ\""), [1]);
    assert_eq!(hit("\"pret\""), [1]);
    assert!(hit("\"tea\"").is_empty());
    conn.execute("DELETE FROM t WHERE rowid = 1", []).unwrap();
    assert!(hit("\"cafe\"").is_empty(), "a contentless row can be deleted");
}

#[test]
fn a_new_store_is_private_and_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("g7-agents");
    {
        let mut db = Db::open(&root).unwrap();
        assert!(db.imported().is_none() && db.set_aside().is_none());
        db.save_thread(&info(&tid(1), "One", "done", 5), &cols()).unwrap();
    }
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(fs::metadata(root.join("agents.db")).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700);
    let db = Db::open(&root).unwrap();
    let threads = db.load_threads().unwrap();
    assert_eq!(threads.len(), 1, "a dropped store commits what it held");
    assert_eq!(threads[0].info.title, "One");
}

// ---- The import of the file store ----

fn render(info: &ThreadInfo, items: &[Item], approvals: &[db::ApprovalRecord]) -> String {
    let device = |by: &Origin| by.device_name.clone().unwrap_or_else(|| "guchu".to_owned());
    let who = |origin: Option<&Origin>| match origin {
        Some(by) => format!("You on {}", device(by)),
        None => "You".to_owned(),
    };
    let header = Header {
        title: info.title.clone(),
        agent: info.agent,
        folder_label: info.folder_label.clone(),
        started_ms: info.created_ms,
    };
    export::render(&header, items, approvals, &who, &device).0
}

#[test]
fn the_file_store_is_imported_once_and_exports_the_same() {
    let dir = v1_root();
    let root = dir.path();
    // What the file store reads and exports, before the import.
    let mut before = Vec::new();
    for id in [A, B] {
        let thread = import_v1::read_thread(&root.join("threads").join(id)).unwrap().unwrap();
        let items: Vec<Item> =
            import_v1::read_items(&root.join("threads").join(id)).unwrap().into_values().collect();
        let approvals = import_v1::read_approvals(&root.join("threads").join(id)).unwrap();
        before.push((thread.info.clone(), items.len(), render(&thread.info, &items, &approvals)));
    }
    assert_eq!(before[0].1, 9, "the unreadable line is skipped, the later kind kept");

    let db = Db::open(root).unwrap();
    let report = db.imported().unwrap().clone();
    assert_eq!((report.readable, report.threads, report.items), (2, 2, 13));
    assert!(report.failed.is_empty());
    assert!(!root.join("threads").exists());
    assert!(root.join("threads.v1-imported").join(A).join("items.jsonl").is_file(), "the files are kept");
    assert!(!root.join("agents.db.importing").exists());

    for (info, count, text) in &before {
        let items: Vec<Item> = db.all_items(&info.id).unwrap().into_values().collect();
        assert_eq!(items.len(), *count);
        let approvals = db.read_approvals(&info.id).unwrap();
        assert_eq!(&render(info, &items, &approvals), text, "the export is byte-for-byte the same");
    }

    // The later kind's line is stored as it was written.
    let conn = rusqlite::Connection::open(root.join("agents.db")).unwrap();
    let body: String =
        conn.query_row("SELECT body FROM items WHERE thread_id = ?1 AND seq = 4", [A], |r| r.get(0)).unwrap();
    assert_eq!(
        body,
        r#"{"seq":4,"id":"x-1","kind":"hologram","ts_ms":1795080906000,"rev":0,"turn":1,"shape":[1,2,3]}"#
    );
    let kind: String =
        conn.query_row("SELECT kind FROM items WHERE thread_id = ?1 AND seq = 4", [A], |r| r.get(0)).unwrap();
    assert_eq!(kind, "hologram");
    drop(conn);

    // Turns are rebuilt from the items' numbers; nothing shows as unread.
    let turns = db.turns(A, None, 10).unwrap();
    assert_eq!(
        turns.iter().map(|t| (t.turn, t.prompt_seq, t.last_seq)).collect::<Vec<_>>(),
        [(2, 5, Some(8)), (1, 1, Some(4))]
    );
    assert!(
        turns.iter().all(|t| t.checkpoint == Checkpoint::Missing && t.why == Some(CheckpointWhy::Before))
    );
    assert_eq!(turns[0].ended, Some(TurnEnd::Done));
    assert!(db.turns(B, None, 10).unwrap().is_empty(), "a thread from before turns has none");
    let loaded = db.load_threads().unwrap();
    let a = loaded.iter().find(|t| t.info.id == A).unwrap();
    assert_eq!((a.cols.next_seq, a.cols.next_turn, a.max_turn, a.users, a.cols.read_seq), (10, 3, 2, 2, 9));
    let b = loaded.iter().find(|t| t.info.id == B).unwrap();
    assert_eq!((b.cols.next_seq, b.cols.next_turn, b.max_turn, b.users), (5, 3, 0, 2));
    assert_eq!(b.info.state, State::Failed);
    assert_eq!(a.info.native_id.as_deref(), Some("0b7e2c3a-1d2e-4f50-8a9b-0c1d2e3f4a5b"));
    assert!(loaded.iter().all(|t| t.cols.pending_fork.is_none()), "the import leaves no pending fork");
    assert!(
        loaded.iter().all(|t| t.cols.read_seq == t.info.last_seq),
        "read up to its last item: the update leaves nothing unread"
    );

    // Imported text is searchable.
    assert_eq!(seqs(&search(&db, "cafe")), [(A.to_owned(), 3)]);
    drop(db);

    // Once: a second start imports nothing.
    let db = Db::open(root).unwrap();
    assert!(db.imported().is_none());
    assert_eq!(db.load_threads().unwrap().len(), 2);
}

#[test]
fn a_crash_during_the_import_is_safe_at_each_step() {
    // A half-made database: deleted, and the files imported afresh.
    let dir = v1_root();
    fs::write(dir.path().join("agents.db.importing"), b"half a database").unwrap();
    let db = Db::open(dir.path()).unwrap();
    assert_eq!(db.imported().unwrap().threads, 2);
    assert_eq!(db.load_threads().unwrap().len(), 2);
    drop(db);

    // Built and checked, not renamed: the files are still the truth.
    let dir = v1_root();
    import_v1::build(dir.path()).unwrap();
    assert!(dir.path().join("threads").is_dir() && !dir.path().join("agents.db").exists());
    let db = Db::open(dir.path()).unwrap();
    assert_eq!(db.imported().unwrap().threads, 2);
    assert_eq!(db.load_threads().unwrap().len(), 2, "imported once, not twice");
    drop(db);

    // Between the two renames: the database is the truth; the folder is retired.
    let dir = v1_root();
    import_v1::build(dir.path()).unwrap();
    fs::rename(dir.path().join("agents.db.importing"), dir.path().join("agents.db")).unwrap();
    let db = Db::open(dir.path()).unwrap();
    assert!(db.imported().is_none());
    assert_eq!(db.load_threads().unwrap().len(), 2);
    assert!(!dir.path().join("threads").exists());
    assert!(dir.path().join("threads.v1-imported").join(B).is_dir());
    drop(db);

    // A `threads/` that turns up again later is set aside, never read.
    fs::create_dir_all(dir.path().join("threads").join(tid(9))).unwrap();
    let db = Db::open(dir.path()).unwrap();
    assert_eq!(db.load_threads().unwrap().len(), 2);
    assert!(!dir.path().join("threads").exists());
    let aside = fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("threads.v1-imported-"))
        .count();
    assert_eq!(aside, 1);
}

#[test]
fn an_unreadable_database_is_moved_aside_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let junk = vec![0x5a_u8; 8192];
    fs::write(dir.path().join("agents.db"), &junk).unwrap();
    let mut db = Db::open(dir.path()).unwrap();
    let aside = db.set_aside().unwrap().to_owned();
    assert!(aside.file_name().unwrap().to_string_lossy().starts_with("agents.db.unreadable-"));
    assert_eq!(fs::read(&aside).unwrap(), junk);
    db.save_thread(&info(&tid(1), "New", "done", 1), &cols()).unwrap();
    db.flush().unwrap();
    assert_eq!(db.load_threads().unwrap().len(), 1, "a new store works");
}

#[test]
fn a_pending_fork_survives_a_restart_and_stays_in_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let id = tid(1);
    let hint = "claude:fork-session;resume=/run/user/1000/g7-agents/cut/0123456789abcdef.jsonl";
    {
        let mut db = Db::open(dir.path()).unwrap();
        let thread = info(&id, "Pending", "done", 1);
        db.save_thread(&thread, &ThreadCols { pending_fork: Some(hint.into()), ..cols() }).unwrap();
        let loaded = db.load_thread(&id).unwrap().unwrap();
        assert_eq!(loaded.cols.pending_fork.as_deref(), Some(hint), "the writer sees it at once");
        db.copy_thread(&id, 1, &info(&tid(2), "Fork", "done", 2)).unwrap();
    }
    let mut db = Db::open(dir.path()).unwrap();
    let loaded = db.load_thread(&id).unwrap().unwrap();
    assert_eq!(loaded.cols.pending_fork.as_deref(), Some(hint), "kept across a restart");
    assert!(
        db.load_thread(&tid(2)).unwrap().unwrap().cols.pending_fork.is_none(),
        "a fork doesn't inherit it"
    );
    let conn = rusqlite::Connection::open(dir.path().join("agents.db")).unwrap();
    let info_json: String =
        conn.query_row("SELECT info FROM threads WHERE id = ?1", [&id], |r| r.get(0)).unwrap();
    assert!(
        !info_json.contains("fork-session") && !info_json.contains("pending_fork"),
        "never in ThreadInfo"
    );
    drop(conn);
    assert!(!serde_json::to_string(&loaded.info).unwrap().contains("fork-session"));
    db.save_thread(&loaded.info, &ThreadCols { pending_fork: None, ..loaded.cols.clone() }).unwrap();
    db.flush().unwrap();
    drop(db);
    let db = Db::open(dir.path()).unwrap();
    assert!(db.load_thread(&id).unwrap().unwrap().cols.pending_fork.is_none(), "and cleared");
}

// ---- Batching ----

#[test]
fn writes_are_batched_and_committed_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open(dir.path()).unwrap();
    let reader = db.reader().unwrap();
    let id = tid(1);
    db.save_thread(&info(&id, "Batch", "working", 1), &cols()).unwrap();
    for rev in 0..3 {
        db.append_item(&id, &Item { rev, ..answer(1, 1, &format!("rev {rev}"), 5) }).unwrap();
    }
    db.append_item(&id, &Item { rev: 1, ..answer(1, 1, "older", 5) }).unwrap();
    assert!(db.flush_deadline().is_some());
    assert!(reader.read_items(&id, None, 10).unwrap().is_empty(), "nothing is committed yet");
    assert_eq!(
        db.read_items(&id, None, 10).unwrap()[0].text.as_deref(),
        Some("rev 2"),
        "the writer sees its batch"
    );
    db.flush().unwrap();
    assert!(db.flush_deadline().is_none());
    let items = reader.read_items(&id, None, 10).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].text.as_deref(),
        Some("rev 2"),
        "the newest rev wins; an older one doesn't replace it"
    );

    // A full batch commits by itself.
    for seq in 2..=(u64::from(db::BATCH_OPS) + 1) {
        db.append_item(&id, &user(seq, 1, "q", seq)).unwrap();
    }
    assert_eq!(reader.read_items(&id, None, 100).unwrap().len(), 1 + db::BATCH_OPS as usize);
    db.append_item(&id, &user(1000, 1, "late", 1000)).unwrap();
    std::thread::sleep(db::BATCH_WINDOW + Duration::from_millis(20));
    db.tick().unwrap();
    let items = reader.read_items(&id, None, 200).unwrap();
    assert_eq!(items.last().unwrap().seq, 1000, "the window commits it");
    assert!(items.windows(2).all(|w| w[0].seq < w[1].seq));
    assert_eq!(reader.read_items(&id, Some(3), 1).unwrap()[0].seq, 2);
    assert_eq!(
        reader.read_items_after(&id, 64, 2).unwrap().iter().map(|i| i.seq).collect::<Vec<_>>(),
        [65, 1000]
    );

    // A failed write leaves nothing of itself.
    assert!(db.append_item(&tid(77), &user(1, 1, "no thread", 1)).is_err());
    db.flush().unwrap();
    assert!(db.read_items(&tid(77), None, 10).unwrap().is_empty());
}

// ---- Rewinds ----

#[test]
fn cut_uncut_and_purge_follow_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open(dir.path()).unwrap();
    let id = tid(1);
    thread_with_turns(
        &mut db,
        &id,
        "Rewind",
        &[("first apple", "red apple"), ("second banana", "yellow banana"), ("third cherry", "dark cherry")],
    );
    assert_eq!(search(&db, "banana").hits.len(), 2);

    let preview = db.cut_preview(&id, 2).unwrap();
    assert_eq!(preview, CutRange { from_seq: 3, to_seq: 6, turns: 2, messages: 4 });
    assert!(db.cut(&id, 9, "native-2").is_err(), "no such turn");
    let cut = db.cut(&id, 2, "native-2").unwrap();
    assert_eq!(cut, preview);
    assert_eq!(db.read_items(&id, None, 50).unwrap().iter().map(|i| i.seq).collect::<Vec<_>>(), [1, 2]);
    assert!(search(&db, "banana").hits.is_empty() && search(&db, "cherry").hits.is_empty());
    assert_eq!(search(&db, "apple").hits.len(), 2);
    assert_eq!(db.turns(&id, None, 10).unwrap().iter().map(|t| t.turn).collect::<Vec<_>>(), [1]);
    let loaded = db.load_thread(&id).unwrap().unwrap();
    assert_eq!(loaded.info.native_id.as_deref(), Some("native-2"));
    assert_eq!(loaded.prior_native_id.as_deref(), Some("native-1"));
    assert_eq!(loaded.cols.next_turn, 4, "turn numbers are never reused");
    assert_eq!(loaded.cols.next_seq, 7);
    assert!(db.cut(&id, 1, "native-3").is_err(), "one rewind at a time");

    // The notice the rewind writes goes when it is undone.
    let notice =
        Item { text: Some("Rewound to here".into()), ..Item::new(7, "notice-7", ItemKind::Notice, 900) };
    db.append_item(&id, &notice).unwrap();
    let undone = db.uncut(&id).unwrap();
    assert_eq!(undone, CutRange { from_seq: 3, to_seq: 7, turns: 2, messages: 4 });
    assert_eq!(
        db.read_items(&id, None, 50).unwrap().iter().map(|i| i.seq).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6]
    );
    assert_eq!(search(&db, "banana").hits.len(), 2, "back in the index");
    let loaded = db.load_thread(&id).unwrap().unwrap();
    assert_eq!(loaded.info.native_id.as_deref(), Some("native-1"));
    assert!(loaded.prior_native_id.is_none());
    assert!(db.uncut(&id).is_err(), "nothing to undo");

    // Purged for good: the restore points' refs come back to delete.
    db.cut(&id, 3, "native-4").unwrap();
    let refs = db.purge_cut(&id).unwrap();
    assert_eq!(
        refs,
        [format!("refs/g7/{id}/3/pre"), format!("refs/g7/{id}/3/post"), format!("refs/g7/{id}/undo")]
    );
    assert!(db.uncut(&id).is_err());
    assert!(db.turn(&id, 3).unwrap().is_none());
    assert!(search(&db, "cherry").hits.is_empty());
    assert_eq!(db.load_thread(&id).unwrap().unwrap().info.native_id.as_deref(), Some("native-4"));

    // Rewinding to before turn 1 starts a new session.
    db.cut(&id, 1, "").unwrap();
    assert!(db.load_thread(&id).unwrap().unwrap().info.native_id.is_none());
    assert!(db.read_items(&id, None, 50).unwrap().is_empty());
}

#[test]
fn turns_and_their_files_round_trip_and_expire() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open(dir.path()).unwrap();
    let id = tid(1);
    thread_with_turns(&mut db, &id, "Turns", &[("a1", "b1"), ("a2", "b2"), ("a3", "b3")]);
    let mut row = db.turn(&id, 2).unwrap().unwrap();
    assert_eq!(db.turn_files(&id, 2).unwrap()[0].path, "src/2.rs");
    row.ended = Some(TurnEnd::Failed);
    row.skipped = (0..250).map(|n| format!("big-{n}.bin")).collect();
    db.save_turn(&id, &row, None).unwrap();
    let back = db.turn(&id, 2).unwrap().unwrap();
    assert_eq!(back.ended, Some(TurnEnd::Failed));
    assert_eq!(back.skipped.len(), db::MAX_SKIPPED);
    assert_eq!(db.turn_files(&id, 2).unwrap().len(), 1, "files are kept unless given");
    let info = back.info();
    assert_eq!((info.turn, info.prompt_seq, info.skipped, info.more_files), (2, 3, 200, false));
    assert_eq!(db.turns(&id, Some(3), 1).unwrap()[0].turn, 2);

    let refs = db.expire_turns(&id, 2).unwrap();
    assert_eq!(refs, [format!("refs/g7/{id}/1/pre"), format!("refs/g7/{id}/1/post")]);
    assert_eq!(db.turn(&id, 1).unwrap().unwrap().checkpoint, Checkpoint::Expired);
    assert_eq!(db.turn(&id, 2).unwrap().unwrap().checkpoint, Checkpoint::Ready);
    assert!(db.expire_turns(&id, 2).unwrap().is_empty(), "already expired");
}

// ---- Search ----

#[test]
fn search_finds_phrases_terms_and_titles() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open(dir.path()).unwrap();
    let (one, two) = (tid(1), tid(2));
    thread_with_turns(
        &mut db,
        &one,
        "Fix the login test",
        &[
            ("The login test fails on CI", "It was a [flaky helper](https://ci.example/login)."),
            ("Now the signup test", "Done: the signup test passes with the helper."),
        ],
    );
    thread_with_turns(&mut db, &two, "東京の旅", &[("東京タワーに行きました", "Le café crème est prêt")]);
    // A streaming answer is not indexed until it ends; tools and thinking never are.
    db.append_item(&one, &Item { end_ms: None, ..answer(9, 2, "unfinished zebra", 900) }).unwrap();
    let tool = Item {
        tool: Some(Tool {
            name: "Bash".into(),
            title: "zebra".into(),
            detail: Some("echo zebra".into()),
            status: ToolStatus::Ok,
            output: Some("zebra".into()),
            class: None,
            path: None,
            by_rule: false,
        }),
        text: Some("zebra".into()),
        ..Item::new(10, "tool-10", ItemKind::Tool, 901)
    };
    db.append_item(&one, &tool).unwrap();
    db.append_item(
        &one,
        &Item { text: Some("zebra".into()), ..Item::new(11, "r", ItemKind::Reasoning, 902) },
    )
    .unwrap();
    assert!(search(&db, "zebra").hits.is_empty());
    db.append_item(&one, &Item { rev: 1, ..answer(9, 2, "finished zebra", 900) }).unwrap();
    assert_eq!(seqs(&search(&db, "zebra")), [(one.clone(), 9)]);

    // A phrase; two terms; a link's target is not searched, its text is.
    assert_eq!(seqs(&search(&db, "signup test")), [(one.clone(), 4), (one.clone(), 3)], "newest first");
    assert_eq!(seqs(&search(&db, "passes helper")), [(one.clone(), 4)]);
    assert!(search(&db, "ci.example").hits.is_empty());
    assert_eq!(seqs(&search(&db, "flaky")), [(one.clone(), 2)]);

    // The title comes first for its thread.
    let page = search(&db, "login");
    assert_eq!(seqs(&page), [(one.clone(), 0), (one.clone(), 1)]);
    assert_eq!(page.hits[0].snippet, "Fix the login test");
    assert_eq!(page.hits[0].marks, [(8, 5)]);
    assert_eq!(page.hits[0].folder_label, "g7linux");

    // One or two letters: titles only.
    let page = search(&db, "lo");
    assert!(page.titles_only);
    assert_eq!(seqs(&page), [(one.clone(), 0)]);
    let page = search(&db, "東京");
    assert!(page.titles_only);
    assert_eq!(seqs(&page), [(two.clone(), 0)]);
    // A short term beside a long one must appear in the hit or its title.
    assert_eq!(seqs(&search(&db, "signup ok")), Vec::<(String, u64)>::new());
    assert_eq!(seqs(&search(&db, "passes he")), [(one.clone(), 4)]);

    // CJK and diacritics.
    assert_eq!(seqs(&search(&db, "京タワ")), [(two.clone(), 1)]);
    let page = search(&db, "CAFE creme");
    assert_eq!(seqs(&page), [(two.clone(), 2)]);
    assert_eq!(page.hits[0].marks, [(3, 4), (8, 5)]);
    assert!(!search(&db, "prêt").hits.is_empty());

    // Quotes and operators are text.
    db.append_item(&one, &answer(12, 2, r#"She said "hi" AND NEAR(x y) left* -- done"#, 950)).unwrap();
    assert_eq!(seqs(&search(&db, r#""hi" AND"#)), [(one.clone(), 12)]);
    assert_eq!(seqs(&search(&db, "NEAR(x")), [(one.clone(), 12)]);
    assert_eq!(seqs(&search(&db, "left*")), [(one.clone(), 12)]);
    assert!(search(&db, r#"""""#).hits.is_empty());
    assert!(search(&db, "OR").titles_only);
    assert!(search(&db, "   ").hits.is_empty());
}

#[test]
fn snippets_stay_in_bounds_on_wide_text() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open(dir.path()).unwrap();
    let id = tid(1);
    let text = format!("{}needle{}", "😀".repeat(100), "é".repeat(300));
    thread_with_turns(&mut db, &id, "Wide", &[("q", &text)]);
    let hit = &search(&db, "needle").hits[0];
    assert_eq!(hit.snippet.chars().count(), MAX_SNIPPET_CHARS);
    assert!(hit.snippet.starts_with(&"😀".repeat(SNIPPET_LEAD_CHARS)));
    assert_eq!(hit.marks, [(48, 6)], "UTF-16 units");
    let short = &search(&db, "q").hits;
    assert!(short.is_empty(), "a one-letter query matches titles only");
}

#[test]
fn search_caps_hits_per_thread_and_honours_archived() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open(dir.path()).unwrap();
    let (busy, archived, other) = (tid(1), tid(2), tid(3));
    thread_with_turns(
        &mut db,
        &busy,
        "needle title",
        &[("needle one", "needle two"), ("needle three", "needle four")],
    );
    thread_with_turns(&mut db, &archived, "Archived", &[("needle archived", "x")]);
    let mut shelf = info(&archived, "Archived", "done", 2000);
    shelf.archived = true;
    db.save_thread(&shelf, &cols()).unwrap();
    thread_with_turns(&mut db, &other, "Other", &[("needle other", "y")]);

    let page = search(&db, "needle");
    let busy_hits: Vec<_> = page.hits.iter().filter(|h| h.thread_id == busy).collect();
    assert_eq!(busy_hits.len(), MAX_HITS_PER_THREAD);
    assert_eq!(busy_hits[0].seq, 0, "the title first");
    assert!(busy_hits.iter().all(|h| h.more_in_thread == 2));
    assert!(page.hits.iter().all(|h| h.thread_id != archived), "archived threads are off by default");

    let q = SearchQuery { query: "needle".into(), limit: 20, archived: true, ..SearchQuery::default() };
    let page = db.search(&q, &Interrupt::new()).unwrap();
    assert!(page.hits.iter().any(|h| h.thread_id == archived && h.archived));

    // Paging by thread.
    let q = SearchQuery { query: "needle".into(), limit: 3, ..SearchQuery::default() };
    let first = db.search(&q, &Interrupt::new()).unwrap();
    let next = first.next.clone().unwrap();
    let second = db.search(&SearchQuery { cursor: Some(next), ..q.clone() }, &Interrupt::new()).unwrap();
    let threads: HashSet<_> = first.hits.iter().chain(&second.hits).map(|h| h.thread_id.clone()).collect();
    assert_eq!(threads, HashSet::from([busy.clone(), other.clone()]));
    assert!(second.next.is_none());
    assert!(db.search(&SearchQuery { cursor: Some("x".into()), ..q.clone() }, &Interrupt::new()).is_err());

    // Only what the asking client may read.
    let visible = SearchQuery { visible: Some(HashSet::from([other.clone()])), ..q };
    let page = db.search(&visible, &Interrupt::new()).unwrap();
    assert!(page.hits.iter().all(|h| h.thread_id == other) && !page.hits.is_empty());
}

#[test]
fn a_superseded_search_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open(dir.path()).unwrap();
    thread_with_turns(&mut db, &tid(1), "T", &[("needle", "needle")]);
    let reader = db.reader().unwrap();
    let stop = Interrupt::new();
    let q = SearchQuery { query: "needle".into(), limit: 5, ..SearchQuery::default() };
    assert_eq!(reader.search(&q, &stop).unwrap().hits.len(), 2);
    stop.stop();
    assert_eq!(reader.search(&q, &stop).unwrap_err().kind(), std::io::ErrorKind::Interrupted);
    assert_eq!(db.search(&q, &stop).unwrap_err().kind(), std::io::ErrorKind::Interrupted);
    assert_eq!(reader.search(&q, &Interrupt::new()).unwrap().hits.len(), 2, "the connection is fine after");
}

// ---- Limits ----

#[test]
fn past_five_thousand_threads_the_oldest_idle_go() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open(dir.path()).unwrap();
    let total = MAX_THREADS_STORED as u32 + 3;
    for n in 1..=total {
        let mut thread = info(&tid(n), "t", "done", 10_000 + u64::from(n));
        match n {
            1 => thread.pinned = true,
            2 => thread.state = State::Working,
            50 | 60 => thread.archived = true,
            _ => {}
        }
        db.save_thread(&thread, &cols()).unwrap();
    }
    db.flush().unwrap();
    assert!(db.prune_threads().unwrap().len() == 3);
    let left: HashSet<String> = db.load_threads().unwrap().into_iter().map(|t| t.info.id).collect();
    assert_eq!(left.len(), MAX_THREADS_STORED);
    for gone in [50, 60, 3] {
        assert!(!left.contains(&tid(gone)), "{gone}: archived first, then the oldest idle");
    }
    for kept in [1, 2, 4] {
        assert!(left.contains(&tid(kept)), "{kept}: pinned and working ones stay");
    }
    assert!(db.prune_threads().unwrap().is_empty());
    assert_eq!(db.info().unwrap().threads, MAX_THREADS_STORED as u32);
}

#[test]
fn past_twenty_thousand_items_the_oldest_go() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open(dir.path()).unwrap();
    let id = tid(1);
    db.save_thread(&info(&id, "Long", "done", 1), &cols()).unwrap();
    let total = MAX_ITEMS_STORED as u64 + 1;
    for seq in 1..=total {
        db.append_item(
            &id,
            &Item { text: Some("x".into()), ..Item::new(seq, format!("n{seq}"), ItemKind::Notice, seq) },
        )
        .unwrap();
    }
    db.flush().unwrap();
    let items = db.all_items(&id).unwrap();
    assert!(items.len() <= MAX_ITEMS_STORED);
    let (_, first) = items.iter().next().unwrap();
    assert_eq!((first.kind, first.id.as_str()), (ItemKind::Notice, "left-out"));
    assert_eq!(first.text.as_deref(), Some("Earlier messages were left out."));
    assert_eq!(items.keys().next_back(), Some(&total), "the newest are kept");
    let second = items.values().nth(1).unwrap();
    assert!(first.seq < second.seq);
    assert_eq!(db.load_thread(&id).unwrap().unwrap().cols.next_seq, total + 1);
}

// ---- Message keys ----

#[test]
fn a_message_key_is_taken_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open(dir.path()).unwrap();
    let id = tid(1);
    db.save_thread(&info(&id, "Keys", "done", 1), &cols()).unwrap();
    let key = "0123456789abcdef";
    assert!(db.take_send_key(&id, key, 1_000).unwrap());
    assert!(!db.take_send_key(&id, key, 2_000).unwrap(), "a duplicate");
    assert!(db.take_send_key(&id, "fedcba9876543210", 2_000).unwrap());
    assert!(db.take_send_key(&id, key, 1_000 + SEND_KEY_MS + 1).unwrap(), "after a day it is new again");
    // A send refused after taking its key gives it back: the retry goes through.
    let retry = "00000000000000aa";
    assert!(db.take_send_key(&id, retry, 5_000).unwrap());
    db.release_send_key(&id, retry).unwrap();
    assert!(db.take_send_key(&id, retry, 5_001).unwrap(), "taken again after it was given back");
    assert!(!db.take_send_key(&id, retry, 5_002).unwrap());
    db.release_send_key(&id, "ffffffffffffffff").unwrap();
    db.release_send_key(&tid(2), retry).unwrap();
    drop(db);
    let mut db = Db::open(dir.path()).unwrap();
    assert!(!db.take_send_key(&id, retry, 5_003).unwrap(), "a taken key survives a restart");
    assert!(db.take_send_key(&id, "short", 1).is_err());
    assert_eq!(db.take_send_key(&tid(2), key, 1).unwrap_err().kind(), std::io::ErrorKind::NotFound);
    db.delete_thread(&id).unwrap();
    assert!(db.load_thread(&id).unwrap().is_none());
}

// ---- Forks ----

#[test]
fn a_fork_copies_its_turns_with_new_seqs() {
    let dir = v1_root();
    let mut db = Db::open(dir.path()).unwrap();
    let mut fork = info(&tid(5), "Fix the login test", "done", 5);
    fork.forked_from =
        Some(ForkedFrom { thread_id: A.into(), turn: Some(1), title: "Fix the login test".into() }.into());
    db.copy_thread(A, 1, &fork).unwrap();
    let items = db.all_items(&fork.id).unwrap();
    assert_eq!(items.keys().copied().collect::<Vec<_>>(), [1, 2, 3, 4], "turn 1 only, renumbered");
    assert_eq!(items[&3].text.as_deref().unwrap().len(), 95);
    let loaded = db.load_thread(&fork.id).unwrap().unwrap();
    assert_eq!((loaded.cols.next_seq, loaded.cols.next_turn), (5, 2));
    let turns = db.turns(&fork.id, None, 10).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(
        (turns[0].prompt_seq, turns[0].last_seq, turns[0].checkpoint),
        (1, Some(4), Checkpoint::Missing)
    );
    let approvals = db.read_approvals(&fork.id).unwrap();
    assert_eq!(approvals.len(), 1, "only the approval of a copied step");
    assert_eq!(approvals[0].tool_seq, Some(2));
    assert_eq!(seqs(&search(&db, "cafe")).len(), 2, "the copy is searchable");
    assert!(db.copy_thread(A, 1, &fork).is_err(), "the new thread must not exist");
    assert!(db.copy_thread(&tid(99), 1, &info(&tid(6), "x", "done", 1)).is_err());

    // Deleting the parent leaves the fork as it is.
    db.delete_thread(A).unwrap();
    assert_eq!(db.all_items(&fork.id).unwrap().len(), 4);
    assert!(search(&db, "signup").hits.is_empty());
}

// ---- Sessions found ----

fn found(id: &str, title: &str, cwd: &str, updated_ms: u64) -> FoundRow {
    FoundRow {
        id: id.into(),
        agent: AgentId::Codex,
        native_id: format!("native-{id}"),
        cwd: Some(cwd.into()),
        folder_id: None,
        title: title.into(),
        first_ms: Some(1),
        updated_ms,
        bytes: 10,
        source: Some(SessionSource::Terminal),
        base_thread: None,
        base_end: None,
        damaged: false,
        thread_id: None,
        files: vec![FoundFile {
            path: format!("/x/rollout-{id}.jsonl.zst"),
            dev: u64::MAX,
            ino: 7,
            bytes: 10,
            mtime_ms: updated_ms,
            stamp: "2026-11-20T10-00-00".into(),
            packed: true,
        }],
    }
}

#[test]
fn sessions_found_are_listed_searched_and_linked() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open(dir.path()).unwrap();
    let rows = vec![
        found("sx01", "Refactor the parser", "/home/u/g7linux", 300),
        found("sx02", "Write docs", "/home/u/website", 200),
        found("sx03", "Release notes", "/home/u/g7linux", 200),
        found("sx04", "Old thing", "/home/u/other", 100),
    ];
    db.upsert_found(&rows).unwrap();
    let all = |db: &Db, q: FoundQuery| db.found(&FoundQuery { limit: 50, ..q }).unwrap();
    let page = all(&db, FoundQuery::default());
    assert_eq!(page.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["sx01", "sx02", "sx03", "sx04"]);
    assert_eq!(page.total, 4);
    assert_eq!(page.rows[0].files[0].dev, u64::MAX);
    assert!(page.rows[0].packed());

    let by = |db: &Db, q: &str| -> Vec<String> {
        all(db, FoundQuery { query: Some(q.into()), ..FoundQuery::default() })
            .rows
            .into_iter()
            .map(|r| r.id)
            .collect()
    };
    assert_eq!(by(&db, "parser"), ["sx01"]);
    assert_eq!(by(&db, "g7linux"), ["sx01", "sx03"], "the folder's name matches");
    assert_eq!(by(&db, "Re"), ["sx01", "sx03"], "under 3 characters: the start of a title");
    assert_eq!(by(&db, "notes g7"), ["sx03"]);

    // Paging keeps rows that share a time together.
    let page = db.found(&FoundQuery { limit: 2, ..FoundQuery::default() }).unwrap();
    assert_eq!(page.rows.len(), 3);
    assert_eq!(page.next_ms, Some(200));
    let rest = db.found(&FoundQuery { limit: 2, before_ms: page.next_ms, ..FoundQuery::default() }).unwrap();
    assert_eq!(rest.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["sx04"]);
    assert_eq!((rest.next_ms, rest.total), (None, 4));

    // Brought in: linked, kept across a rescan, out of "new"; G7's own sessions are left out.
    let id = tid(1);
    let mut thread = info(&id, "Refactor the parser", "done", 1);
    thread.native_id = Some("native-sx02".into());
    db.save_thread(&thread, &cols()).unwrap();
    db.set_found_thread("sx01", Some(&id)).unwrap();
    db.upsert_found(&[FoundRow { title: "Refactor the parser again".into(), ..rows[0].clone() }]).unwrap();
    let page = all(&db, FoundQuery::default());
    assert_eq!(page.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["sx01", "sx03", "sx04"]);
    assert_eq!(page.rows[0].thread_id.as_deref(), Some(id.as_str()));
    assert_eq!(page.rows[0].title, "Refactor the parser again");
    let new = all(&db, FoundQuery { show: FoundShow::New, ..FoundQuery::default() });
    assert_eq!(new.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["sx03", "sx04"]);
    assert!(db.set_found_thread("nope", None).is_err());

    // One row by id, even one a listing hides because G7 already has its session.
    let one = db.found_row("sx02").unwrap().expect("sx02 is stored");
    assert_eq!((one.title.as_str(), one.files.len()), ("Write docs", 1));
    assert_eq!(db.found_row("sx01").unwrap().and_then(|r| r.thread_id).as_deref(), Some(id.as_str()));
    assert!(db.found_row("nope").unwrap().is_none());

    // A file gone from disk drops its row; the session stays.
    db.forget_found_files(&[("sx04".into(), "/x/rollout-sx04.jsonl.zst".into())]).unwrap();
    assert_eq!(db.found_files().unwrap().len(), 3);
    db.delete_thread(&id).unwrap();
    assert!(all(&db, FoundQuery::default()).rows[0].thread_id.is_none(), "deleting the thread unlinks it");
    assert_eq!(db.info().unwrap().found, 4);
}

#[test]
fn store_info_counts() {
    let dir = v1_root();
    let db = Db::open(dir.path()).unwrap();
    let info = db.info().unwrap();
    assert_eq!((info.threads, info.archived, info.items, info.found), (2, 0, 13, 0));
    assert!(info.bytes > 0);
    assert_eq!(db.reader().unwrap().info().unwrap().threads, 2);
}
