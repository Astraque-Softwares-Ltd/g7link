// SPDX-License-Identifier: GPL-3.0-or-later
//! Source control's reducers: the quote chip's text form (the shared
//! fixtures), the commit box, the agent's answer, the job output, the pull
//! request card, the merge dialog's guard, and what the phone relay drops.

use std::collections::BTreeSet;

use g7_agents::api::{
    GitChange, GitFile, GitStatus, Job, PrInfo, PrMethodsResult, Purpose, Remote, RepoState, Staged,
    WorktreeTag, Wrote,
};
use serde_json::{Value, json};

use super::source::*;

fn fixtures() -> Vec<(String, Quote, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../core/g7-agents/tests/fixtures/scm/quote");
    let mut paths: Vec<_> =
        std::fs::read_dir(&dir).expect("the quote fixtures").flatten().map(|e| e.path()).collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let value: Value =
                serde_json::from_slice(&std::fs::read(&path).expect("a fixture")).expect("JSON");
            let quote: Quote = serde_json::from_value(value["quote"].clone()).expect("a quote");
            let text = value["text"].as_str().expect("its text").to_owned();
            (path.display().to_string(), quote, text)
        })
        .collect()
}

#[test]
fn the_quote_text_form_matches_every_shared_fixture() {
    let all = fixtures();
    assert_eq!(all.len(), 6, "the six fixtures the phone shares");
    for (name, quote, text) in all {
        assert_eq!(quote.text_form(), text, "{name}");
    }
}

#[test]
fn quotes_go_before_the_words() {
    let q = Quote { path: "a.rs".into(), first: 1, last: 2, text: "x\ny".into() };
    assert_eq!(with_quotes(&[], "hi"), "hi");
    assert_eq!(with_quotes(std::slice::from_ref(&q), "Why?"), format!("{}\n\nWhy?", q.text_form()));
    assert_eq!(with_quotes(std::slice::from_ref(&q), "  "), q.text_form(), "a chip alone is a message");
    assert_eq!(with_quotes(&[q.clone(), q], "x").matches("a.rs:1").count(), 2);
}

fn row(old: u32, new: u32, text: &str) -> QuoteRow {
    QuoteRow { old, new, hunk: false, text: text.into() }
}

#[test]
fn a_quote_takes_the_rows_picked_by_their_line_numbers() {
    let rows = vec![
        QuoteRow { hunk: true, text: "@@ -1,3 +1,3 @@".into(), ..Default::default() },
        row(1, 1, "a"),
        row(2, 0, "b"),
        row(0, 2, "B"),
        row(3, 3, "c"),
    ];
    let q = quote_rows("f.rs", &rows, 4, 0).expect("a quote");
    assert_eq!((q.first, q.last, q.text.as_str()), (1, 3, "a\nb\nB\nc"));
    let q = quote_rows("f.rs", &rows, 2, 2).expect("a removed line");
    assert_eq!((q.first, q.last), (2, 2));
    assert!(quote_rows("f.rs", &rows, 0, 0).is_none(), "a hunk header alone");
    assert!(quote_rows("f.rs", &rows, 9, 12).is_none(), "past the end");
    assert!(quote_rows("f.rs", &[], 0, 0).is_none());
    let long = vec![row(1, 1, &"x".repeat(2_050))];
    assert_eq!(quote_rows("f.rs", &long, 0, 0).expect("cut").text.chars().count(), 2_000);
}

#[test]
fn the_agents_answer_fills_only_a_field_not_typed_in_since_asking() {
    let mut f = AskField::default();
    f.ask(1);
    assert!(f.answered(3, 10, "Fix the retry"));
    assert_eq!((f.text.as_str(), f.set_n), ("Fix the retry", 1));
    assert!(!f.answered(3, 10, "again"), "one answer, taken once");

    let mut f = AskField::default();
    f.typed("My own".into());
    f.ask(1);
    assert!(f.answered(4, 11, "Agent's"), "typed before asking: replaced");

    let mut f = AskField::default();
    f.ask(1);
    f.typed("Mine, while it wrote".into());
    assert!(!f.answered(5, 12, "Agent's"));
    assert_eq!(f.text, "Mine, while it wrote");
    assert_eq!(f.offer.as_deref(), Some("Agent's"), "offered instead");

    let mut f = AskField::default();
    assert!(!f.answered(6, 13, "unasked"), "nothing asked, nothing taken");
    assert!(f.text.is_empty());
}

#[test]
fn an_answer_after_the_turns_end_still_fills_the_field() {
    let mut f = AskField::default();
    f.ask(100);
    f.gave_up();
    assert!(!f.answered(2, 50, "older"), "written before the ask");
    assert!(f.answered(2, 150, "Fix the retry"), "written after it, though the turn's end came first");
    assert_eq!(f.text, "Fix the retry");
    assert!(!f.answered(3, 200, "again"), "one answer per ask");

    let mut f = AskField::default();
    f.ask(100);
    f.gave_up();
    f.typed("Mine".into());
    assert!(!f.answered(2, 150, "Agent's"));
    assert_eq!(f.offer.as_deref(), Some("Agent's"), "typed since asking: offered");
}

#[test]
fn the_threads_title_is_put_in_and_its_echo_isnt_typing() {
    let mut f = AskField::default();
    f.put("Stock sync drops rows".into());
    f.ask(1);
    f.typed("Stock sync drops rows".into());
    assert!(!f.typed, "the page's echo of what was put in");
    assert_eq!(f.set_n, 1);
}

#[test]
fn wrote_fills_the_commit_message_or_the_pull_requests_fields() {
    let mut p = Panel { thread_id: "t".into(), ..Default::default() };
    p.message.ask(0);
    let wrote = Wrote { purpose: Purpose::Commit, turn: 2, text: "Keep rows\n\nBody".into(), at_ms: 1 };
    assert!(!take_wrote(&mut p, "other", &wrote), "another thread's");
    assert!(take_wrote(&mut p, "t", &wrote));
    assert_eq!(p.message.text, "Keep rows\n\nBody");

    p.form.title.ask(0);
    p.form.body.ask(0);
    let wrote = Wrote { purpose: Purpose::Pr, turn: 3, text: "Stock sync\n\n- drain waits".into(), at_ms: 2 };
    assert!(take_wrote(&mut p, "t", &wrote));
    assert_eq!((p.form.title.text.as_str(), p.form.body.text.as_str()), ("Stock sync", "- drain waits"));
    assert!(!p.form.body.asking);
}

#[test]
fn a_long_subject_is_flagged_never_refused() {
    assert!(!subject_long(
        "Fix it\n\nA body line that goes on and on and on and on and on and on and on and on"
    ));
    assert!(subject_long(&"x".repeat(73)));
    assert!(!subject_long(&"é".repeat(72)), "characters, not bytes");
}

#[test]
fn new_branch_names_follow_the_hosts_rules() {
    for good in ["fix/ack-timeout", "g7/try-1", "a", "v1.2_x"] {
        assert!(branch_name_ok(good), "{good}");
    }
    let long = "x".repeat(101);
    for bad in [
        "-x", "@", "a..b", "x.lock", "a b", "HEAD", &long, "", "/a", "a/", "a//b", "a@{1}", ".a", "a.",
        "a/.b",
    ] {
        assert!(!branch_name_ok(bad), "{bad}");
    }
}

#[test]
fn a_worktree_branch_comes_from_the_threads_words() {
    assert_eq!(worktree_branch("Try acking per batch instead"), "g7/try-acking-per-batch-instead");
    assert_eq!(worktree_branch("!!!"), "g7/new-thread");
    let long = worktree_branch(&"word ".repeat(30));
    assert!(long.len() <= 43 && branch_name_ok(&long), "{long}");
}

fn file(path: &str, status: GitChange) -> GitFile {
    GitFile {
        path: path.into(),
        from: None,
        status,
        staged: Staged::None,
        folder: false,
        adds: None,
        dels: None,
        binary: false,
    }
}

fn status(files: Vec<GitFile>) -> GitStatus {
    GitStatus { repo: RepoState::Ok, branch: Some("feat/x".into()), files, ..Default::default() }
}

#[test]
fn commit_says_why_it_cant_run() {
    let st = status(vec![file("a.rs", GitChange::Modified)]);
    assert_eq!(commit_blocked(&st, 1, false, false), None);
    assert_eq!(commit_blocked(&st, 0, false, false), Some("nothing"));
    assert_eq!(commit_blocked(&st, 1, true, false), Some("thread_busy"));
    assert_eq!(commit_blocked(&st, 1, false, true), Some("job"));
    assert_eq!(commit_blocked(&GitStatus { locked: true, ..st.clone() }, 1, false, false), Some("git_busy"));
    let mid = GitStatus { doing: Some(g7_agents::api::Doing::Merge), ..st.clone() };
    assert_eq!(commit_blocked(&mid, 1, false, false), Some("doing"));
    let detached = GitStatus { detached: Some("db48c89".into()), ..st.clone() };
    assert_eq!(commit_blocked(&detached, 1, false, false), Some("detached"));
    assert_eq!(commit_blocked(&GitStatus { conflicts: 1, ..st.clone() }, 1, false, false), Some("conflicts"));
    assert_eq!(commit_blocked(&GitStatus { repo: RepoState::None, ..st }, 1, false, false), Some("not_git"));
}

#[test]
fn a_commit_takes_the_ticked_files_and_never_a_conflict() {
    let st = status(vec![
        file("a.rs", GitChange::Modified),
        file("b.rs", GitChange::Untracked),
        file("c.rs", GitChange::Conflict),
    ]);
    let mut unticked = BTreeSet::new();
    assert_eq!(picked_paths(&st, &unticked), vec!["a.rs", "b.rs"]);
    unticked.insert("a.rs".to_owned());
    assert_eq!(picked_paths(&st, &unticked), vec!["b.rs"]);
}

fn job(value: Value) -> Job {
    let mut base =
        json!({ "id": "0123456789abcdef", "thread_id": "t", "started_ms": 1, "by": { "kind": "launcher" } });
    if let (Some(base), Some(more)) = (base.as_object_mut(), value.as_object()) {
        base.extend(more.clone());
    }
    serde_json::from_value(base).expect("a job")
}

#[test]
fn the_job_output_ends_in_words() {
    let done = job(
        json!({ "kind": "commit", "phase": "done", "result": { "oid7": "9c41e7a", "subject": "Keep rows" } }),
    );
    assert_eq!(job_words(&done), ("committed", vec!["9c41e7a".into(), "Keep rows".into()]));
    let hook =
        job(json!({ "kind": "commit", "phase": "failed", "error": { "reason": "hook", "message": "" } }));
    assert_eq!(job_words(&hook).0, "commit_hook");
    assert_eq!(job_words(&job(json!({ "kind": "commit", "phase": "stopped" }))).0, "commit_stopped");
    let behind =
        job(json!({ "kind": "push", "phase": "failed", "error": { "reason": "behind", "message": "" } }));
    assert_eq!(job_words(&behind).0, "push_behind");
    let odd = job(json!({ "kind": "push", "phase": "failed", "error": { "reason": "odd", "message": "" } }));
    assert_eq!(job_words(&odd).0, "push_failed", "an unknown word is the plain failure");
    let same = job(json!({ "kind": "push", "phase": "done", "result": { "up_to_date": true } }));
    assert_eq!(job_words(&same).0, "up_to_date");
    let created =
        job(json!({ "kind": "switch", "phase": "done", "result": { "branch": "fix/x", "created": true } }));
    assert_eq!(job_words(&created), ("created", vec!["fix/x".into()]));
    assert_eq!(job_json(&job(json!({ "kind": "switch", "phase": "running" })))["stoppable"], false);
    assert_eq!(job_json(&job(json!({ "kind": "push", "phase": "running" })))["stoppable"], true);
}

#[test]
fn progress_appends_what_follows_and_asks_for_the_rest() {
    let mut shown = job(json!({ "kind": "commit", "phase": "running", "lines": ["a", "b"], "from": 0 }));
    let next = job(json!({ "kind": "commit", "phase": "running", "lines": ["c"], "from": 2 }));
    assert!(!take_progress(&mut shown, next));
    assert_eq!(shown.lines, vec!["a", "b", "c"]);
    let again = job(json!({ "kind": "commit", "phase": "running", "lines": ["b", "c", "d"], "from": 1 }));
    assert!(!take_progress(&mut shown, again), "lines already shown aren't doubled");
    assert_eq!(shown.lines, vec!["a", "b", "c", "d"]);
    let gap = job(json!({ "kind": "commit", "phase": "done", "lines": ["z"], "from": 9 }));
    assert!(take_progress(&mut shown, gap), "a gap: read the job whole");
    assert_eq!(shown.lines.len(), 4);
    assert_eq!(job_json(&shown)["phase"], "done", "its end still shows");
    let other = job(json!({ "id": "ffffffffffffffff", "kind": "commit", "phase": "done" }));
    assert!(!take_progress(&mut shown, other), "another job's");

    let mut long = job(json!({ "kind": "commit", "phase": "running", "from": 0, "dropped": 0 }));
    let flood = job(json!({ "kind": "commit", "phase": "running", "from": 0,
        "lines": (0..450).map(|i| format!("line {i}")).collect::<Vec<_>>() }));
    take_progress(&mut long, flood);
    let shown = job_json(&long);
    assert_eq!(shown["lines"].as_array().map(Vec::len), Some(400), "400 kept");
    assert_eq!(shown["lines"][0], "line 50", "the newest");
    assert_eq!(shown["dropped"], 50, "\"%n earlier lines not kept\"");
}

fn pr(value: Value) -> PrInfo {
    let mut base = json!({
        "number": 212, "title": "Stock sync", "state": "open", "draft": false, "url": "https://github.com/o/r/pull/212",
        "base": "main", "head": "feat/x", "head_oid": "9c41e7a0000000000000000000000000000000ff", "blocked": false,
        "updated_ms": 1, "checks": [], "pass": 0, "fail": 0, "pending": 0, "read_ms": 1,
    });
    if let (Some(base), Some(more)) = (base.as_object_mut(), value.as_object()) {
        base.extend(more.clone());
    }
    serde_json::from_value(base).expect("a pull request")
}

#[test]
fn the_checks_line_leaves_skipped_out_and_lists_failing_first() {
    assert_eq!(checks_line(&pr(json!({ "pass": 7 }))), ("passed", 7, 7));
    assert_eq!(checks_line(&pr(json!({ "pass": 5, "fail": 2 }))), ("failed", 2, 7));
    assert_eq!(checks_line(&pr(json!({ "pass": 4, "pending": 3 }))), ("running", 3, 7));
    assert_eq!(checks_line(&pr(json!({}))), ("none", 0, 0));
    let listed = pr(json!({ "pass": 1, "fail": 1, "checks": [
        { "name": "ok", "state": "pass" }, { "name": "skip", "state": "skipped" }, { "name": "bad", "state": "fail" },
    ] }));
    let card = pr_json(&listed);
    assert_eq!(card["checks"][0]["name"], "bad", "failing first");
    assert_eq!(card["checks"].as_array().map(Vec::len), Some(2), "skipped left out");
    assert_eq!(card["oid7"], "9c41e7a");
}

#[test]
fn only_https_links_on_a_signed_in_host_open() {
    let hosts = vec!["github.com".to_owned()];
    assert!(safe_url("https://github.com/o/r/pull/1", &hosts));
    assert!(!safe_url("http://github.com/o/r/pull/1", &hosts));
    assert!(!safe_url("https://evil.example/o", &hosts));
    assert!(!safe_url("https://user@github.com/o", &hosts));
    assert!(!safe_url("https://github.com:444/o", &hosts));
    assert!(!safe_url("javascript:alert(1)", &hosts));
    assert!(!safe_url("https://github.com/o", &[]));
}

#[test]
fn a_merge_needs_a_click_or_the_tabbed_button_and_the_tick_on_failures() {
    let shown = pr(json!({ "pass": 7 }));
    let mut m = Merge {
        number: 212,
        head: shown.head_oid.clone(),
        methods: Some(PrMethodsResult { squash: true, merge: true, rebase: false }),
        method: Some(g7_agents::api::MergeMethod::Squash),
        ..Default::default()
    };
    assert!(merge_ready(&m, &shown, "click"));
    assert!(!merge_ready(&m, &shown, "key"), "a bare Enter never merges");
    m.focused = true;
    assert!(merge_ready(&m, &shown, "key"));
    m.method = Some(g7_agents::api::MergeMethod::Rebase);
    assert!(!merge_ready(&m, &shown, "click"), "a method the repository doesn't allow");
    m.method = Some(g7_agents::api::MergeMethod::Squash);
    let failing = pr(json!({ "pass": 5, "fail": 2 }));
    assert!(!merge_ready(&m, &failing, "click"), "failed checks need the tick");
    m.looked = true;
    assert!(merge_ready(&m, &failing, "click"));
    let moved = pr(json!({ "head_oid": "1111111000000000000000000000000000000000" }));
    assert!(!merge_ready(&m, &moved, "click"), "pinned to the commit shown");
}

#[test]
fn the_relay_drops_what_a_phone_may_not_get() {
    let mut files: Vec<GitFile> = (0..700).map(|i| file(&format!("f{i}"), GitChange::Modified)).collect();
    files.insert(0, file("/home/someone/secret", GitChange::Modified));
    let mut st = status(files);
    st.remotes = vec![
        Remote { name: "origin".into(), label: "github.com/o/r".into(), local: false },
        Remote { name: "here".into(), label: "/srv/git/r.git".into(), local: true },
    ];
    st.worktree = Some(WorktreeTag {
        id: "abcdef012345".into(),
        branch: "g7/x".into(),
        label: "x".into(),
        missing: false,
    });
    let phone = phone_status(&st);
    assert_eq!(phone.files.len(), 500);
    assert!(phone.more);
    assert!(phone.files.iter().all(|f| !f.path.starts_with('/')));
    assert_eq!(phone.remotes[0].label, "github.com/o/r");
    assert!(phone.remotes[1].label.is_empty(), "a folder on this PC");
    let w = phone.worktree.as_ref().expect("label and branch");
    assert_eq!((w.id.as_str(), w.branch.as_str(), w.label.as_str()), ("", "g7/x", "x"));
    let text = serde_json::to_string(&phone).expect("JSON");
    assert!(!text.contains("/srv/") && !text.contains("/home/"));

    let lines: Vec<String> = (0..100).map(|i| format!("{i:0>300}")).collect();
    let long = job(json!({ "kind": "commit", "phase": "running", "lines": lines, "from": 0 }));
    let p = phone_job(&long);
    assert!(p.lines.len() <= 40);
    assert!(p.lines.iter().map(String::len).sum::<usize>() <= 8 * 1024);
    assert_eq!(p.lines.last(), long.lines.last(), "the newest are kept");
    assert_eq!(p.dropped as usize, 100 - p.lines.len());
    assert_eq!(p.from as usize, 100 - p.lines.len());
}
