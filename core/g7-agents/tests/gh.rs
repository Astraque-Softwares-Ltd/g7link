// SPDX-License-Identifier: MPL-2.0
//! Track G (phase 4 §SC4, §SC11.1 G): pull requests through `gh`, with a fake `gh` (a shell script
//! this file writes) first on a `PATH` that holds nothing else but a few shell tools, so the real
//! `gh` can never run and nothing reaches the network. The fake records each call's argv, its
//! environment and its stdin, and prints a hand-written fixture with fictional values.
#![cfg(feature = "host")]

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use g7_agents::host::git::Git;
use g7_agents::host::scm::{
    CheckState, GhUse, GitStatus, Lines, Mergeable, Method, NewPr, PrState, Reason, ReviewState, Scm, Show,
    Spot,
};

const FAKE_GH: &str = r#"#!/bin/sh
d="$FAKE_GH_DIR"
case "$1" in
  --version) key=version ;;
  auth) key=auth ;;
  pr) case "$2" in
        list) case " $* " in *" --head "*) key=pr-head ;; *) key=pr-open ;; esac ;;
        *) key="pr-$2" ;;
      esac ;;
  search) case " $* " in *review-requested*) key=search-review ;; *) key=search-mine ;; esac ;;
  repo) key=repo ;;
  *) key=other ;;
esac
n=$(cat "$d/n" 2>/dev/null || echo 0); n=$((n+1)); echo "$n" > "$d/n"
c=$(cat "$d/count-$key" 2>/dev/null || echo 0); c=$((c+1)); echo "$c" > "$d/count-$key"
printf '%s\0' "$@" > "$d/argv-$n"
env > "$d/env-$n"
cat > "$d/stdin-$n"
a="$d/answers/$key"
if [ -e "$a.$c.out" ] || [ -e "$a.$c.code" ]; then a="$a.$c"; fi
if [ -f "$a.sleep" ]; then sleep "$(cat "$a.sleep")"; fi
if [ -f "$a.err" ]; then cat "$a.err" >&2; fi
if [ -f "$a.out" ]; then cat "$a.out"; fi
code=0
if [ -f "$a.code" ]; then code=$(cat "$a.code"); fi
exit "$code"
"#;

const PR_FIELDS: &str = "number,title,state,isDraft,url,reviewDecision,mergeable,mergeStateStatus,baseRefName,headRefName,headRefOid,updatedAt,statusCheckRollup";
const SEARCH_FIELDS: &str = "number,title,repository,state,isDraft,updatedAt,url";
const LIST_FIELDS: &str = "number,title,state,isDraft,updatedAt,url,headRefName,statusCheckRollup";
const REPO_FIELDS: &str =
    "nameWithOwner,mergeCommitAllowed,squashMergeAllowed,rebaseMergeAllowed,defaultBranchRef";

fn real_git() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let found = std::env::split_paths(&path).map(|d| d.join("git")).find(|p| p.is_file());
    if found.is_none() {
        assert!(std::env::var("CI").as_deref() != Ok("true"), "git is not on PATH in CI");
        eprintln!("skipped: git is not on PATH");
    }
    found
}

fn fixture(name: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scm/gh").join(name))
        .unwrap()
}

struct Fake {
    _dir: tempfile::TempDir,
    root: PathBuf,
    calls: PathBuf,
    scm: Scm,
    spot: Spot,
    real: PathBuf,
    env: Vec<(OsString, OsString)>,
}

impl Fake {
    /// A repository whose `origin` is on github.com (fictional; never contacted), its branch
    /// pushed as far as git knows, and `gh` faked. `with_gh: false`: no `gh` on `PATH` at all.
    async fn new(with_gh: bool, extra: &[(&str, &str)]) -> Option<Fake> {
        let real = real_git()?;
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let (bin, tools, calls) = (root.join("bin"), root.join("tools"), root.join("calls"));
        for d in [&bin, &tools, &calls, &calls.join("answers"), &root.join("home"), &root.join("xdg")] {
            fs::create_dir_all(d).unwrap();
        }
        let system = std::env::var_os("PATH").unwrap_or_default();
        for tool in ["cat", "env", "sleep"] {
            let found = std::env::split_paths(&system).map(|d| d.join(tool)).find(|p| p.is_file()).unwrap();
            std::os::unix::fs::symlink(found, tools.join(tool)).unwrap();
        }
        if with_gh {
            fs::write(bin.join("gh"), FAKE_GH).unwrap();
            fs::set_permissions(bin.join("gh"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(root.join("gitconfig"), "[init]\n\tdefaultBranch = main\n").unwrap();
        let path = std::env::join_paths([&bin, &tools]).unwrap();
        let mut env: Vec<(OsString, OsString)> = vec![
            ("PATH".into(), path),
            ("HOME".into(), root.join("home").into()),
            ("XDG_CONFIG_HOME".into(), root.join("xdg").into()),
            ("GIT_CONFIG_GLOBAL".into(), root.join("gitconfig").into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_CEILING_DIRECTORIES".into(), root.clone().into()),
            ("FAKE_GH_DIR".into(), calls.clone().into()),
        ];
        env.extend(extra.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))));
        let fake_root = root.clone();
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new(&real)
                .args(args)
                .env_clear()
                .envs(env.iter().map(|(k, v)| (k, v)))
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&[
            "-c",
            "user.name=Lab",
            "-c",
            "user.email=lab@localhost",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "one",
        ]);
        git(&["remote", "add", "origin", "https://github.com/fictional/repo.git"]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&["branch", "-q", "--set-upstream-to=origin/main", "main"]);
        let scm = Scm::new(Git::with_bin(real.clone(), &env), &env);
        let spot = scm.spot(&repo).await.unwrap();
        Some(Fake { _dir: dir, root: fake_root, calls, scm, spot, real, env })
    }

    fn answer(&self, key: &str, out: &str, code: i32) {
        let a = self.calls.join("answers").join(key);
        fs::write(a.with_extension("out"), out).unwrap();
        fs::write(a.with_extension("code"), code.to_string()).unwrap();
    }

    /// The answer to the `n`th call of `key` only.
    fn answer_nth(&self, key: &str, n: u32, out: &str, code: i32) {
        let a = self.calls.join("answers").join(format!("{key}.{n}"));
        fs::write(format!("{}.out", a.display()), out).unwrap();
        fs::write(format!("{}.code", a.display()), code.to_string()).unwrap();
    }

    fn answer_err(&self, key: &str, err: &str) {
        fs::write(self.calls.join("answers").join(key).with_extension("err"), err).unwrap();
    }

    fn answer_sleep(&self, key: &str, secs: u32) {
        fs::write(self.calls.join("answers").join(key).with_extension("sleep"), secs.to_string()).unwrap();
    }

    fn signed_in(&self) {
        self.answer(
            "version",
            "gh version 2.102.0 (2026-09-30)\nhttps://github.com/cli/cli/releases/tag/v2.102.0\n",
            0,
        );
        self.answer("auth", &fixture("auth-signed-in.json"), 0);
    }

    fn count(&self) -> usize {
        fs::read_to_string(self.calls.join("n")).map_or(0, |n| n.trim().parse().unwrap())
    }

    fn argv(&self, n: usize) -> Vec<String> {
        let raw = fs::read(self.calls.join(format!("argv-{n}"))).unwrap();
        raw.split(|&b| b == 0)
            .filter(|a| !a.is_empty())
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect()
    }

    fn all_argv(&self) -> Vec<Vec<String>> {
        (1..=self.count()).map(|n| self.argv(n)).collect()
    }

    fn env_of(&self, n: usize) -> Vec<String> {
        fs::read_to_string(self.calls.join(format!("env-{n}"))).unwrap().lines().map(str::to_owned).collect()
    }

    fn stdin_of(&self, n: usize) -> Vec<u8> {
        fs::read(self.calls.join(format!("stdin-{n}"))).unwrap()
    }

    async fn status(&self) -> GitStatus {
        self.scm.status(&self.spot).await.unwrap()
    }

    fn git(&self, args: &[&str]) {
        let out = Command::new(&self.real)
            .args(args)
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .current_dir(self.spot.folder())
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
}

fn is(argv: &[String], first: &str, second: &str) -> bool {
    argv.len() >= 2 && argv[0] == first && argv[1] == second
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// Debug log lines written while `f` runs.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// ---- State ----

#[tokio::test]
async fn without_gh_everything_github_is_missing_and_git_still_works() {
    let Some(fake) = Fake::new(false, &[]).await else { return };
    let state = fake.scm.gh_state(false).await;
    assert_eq!((state.gh, state.hosts.len()), (GhUse::Missing, 0));
    assert_eq!(fake.scm.pr_list(Show::Mine, None, false).await.unwrap_err().reason, Reason::NoGh);
    let status = fake.status().await;
    assert_eq!(fake.scm.pr_for_branch(&fake.spot, &status, false).await.unwrap_err().reason, Reason::NoGh);
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert!(status.github.is_none());
}

#[tokio::test]
async fn signed_out_is_read_from_the_exit_code_and_exit_4_flips_the_state() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.answer("version", "gh version 2.102.0 (2026-09-30)\n", 0);
    fake.answer("auth", "", 1);
    assert_eq!(fake.scm.gh_state(false).await.gh, GhUse::SignedOut);

    fake.signed_in();
    let state = fake.scm.gh_state(true).await;
    assert_eq!(
        (state.gh, state.hosts.clone(), state.version.as_deref()),
        (GhUse::Ok, words(&["github.com"]), Some("2.102.0"))
    );
    let status = fake.status().await;
    assert_eq!(status.github.as_ref().map(|g| g.repo.as_str()), Some("fictional/repo"));

    fake.answer("pr-head", "", 4);
    let err = fake.scm.pr_for_branch(&fake.spot, &status, true).await.unwrap_err();
    assert_eq!(err.reason, Reason::SignedOut);
    let auths_before = fake.all_argv().iter().filter(|a| a[0] == "auth").count();
    fake.answer("auth", "", 1);
    assert_eq!(fake.scm.gh_state(false).await.gh, GhUse::SignedOut);
    let auths_after = fake.all_argv().iter().filter(|a| a[0] == "auth").count();
    assert_eq!(auths_after, auths_before + 1, "after exit 4 the state is read again");
}

#[tokio::test]
async fn an_account_name_or_a_credential_in_gh_output_is_never_kept_or_logged() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    fake.signed_in();
    fake.answer_err("auth", "Logged in to github.com account fictional-login-zq (keyring)\n");
    let state = fake.scm.gh_state(true).await;
    assert_eq!(state.hosts, words(&["github.com"]), "only hosts whose state is success");
    fake.answer("pr-head", &fixture("pr-open.json"), 0);
    let status = fake.status().await;
    let pr = fake.scm.pr_for_branch(&fake.spot, &status, true).await.unwrap();
    let shown = format!("{state:?} {pr:?} {status:?} {}", serde_json::to_string(&state).unwrap());
    let logged = String::from_utf8_lossy(&log.0.lock().unwrap()).into_owned();
    for secret in ["fictional-login-zq", "gho_FICTIONAL", "keyring", "scopes"] {
        assert!(!shown.contains(secret), "{secret} in {shown}");
        assert!(!logged.contains(secret), "{secret} in the log: {logged}");
    }
}

#[tokio::test]
async fn gh_runs_with_its_quiet_settings_and_never_gains_or_loses_a_token_variable() {
    let extra = [
        ("GH_DEBUG", "api"),
        ("GH_FORCE_TTY", "1"),
        ("GH_REPO", "someone/else"),
        ("GH_HOST", "elsewhere.test"),
    ];
    let Some(fake) = Fake::new(true, &extra).await else { return };
    fake.signed_in();
    fake.scm.gh_state(true).await;
    for n in 1..=fake.count() {
        let env = fake.env_of(n);
        for set in [
            "GH_PROMPT_DISABLED=1",
            "GH_NO_UPDATE_NOTIFIER=1",
            "GH_NO_EXTENSION_UPDATE_NOTIFIER=1",
            "GH_SPINNER_DISABLED=1",
            "GH_PAGER=cat",
            "NO_COLOR=1",
        ] {
            assert!(env.iter().any(|l| l == set), "{set} missing");
        }
        for gone in ["GH_DEBUG=", "GH_FORCE_TTY=", "GH_REPO=", "GH_HOST="] {
            assert!(!env.iter().any(|l| l.starts_with(gone)), "{gone} kept");
        }
        for token in ["GH_TOKEN=", "GITHUB_TOKEN=", "GH_ENTERPRISE_TOKEN=", "GITHUB_ENTERPRISE_TOKEN="] {
            assert!(!env.iter().any(|l| l.starts_with(token)), "{token} was added");
        }
    }

    let Some(theirs) = Fake::new(true, &[("GH_TOKEN", "the-users-own-value")]).await else { return };
    theirs.signed_in();
    theirs.scm.gh_state(true).await;
    assert!(
        theirs.env_of(1).iter().any(|l| l == "GH_TOKEN=the-users-own-value"),
        "the user's own is passed as it is"
    );
}

#[tokio::test]
async fn an_unknown_flag_is_an_old_gh() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.answer("version", "gh version 1.0.0 (2020-01-01)\n", 0);
    fake.answer("auth", "", 1);
    fake.answer_err("auth", "unknown flag: --json\n\nUsage:  gh auth status [flags]\n");
    let state = fake.scm.gh_state(true).await;
    assert_eq!((state.gh, state.version.as_deref()), (GhUse::Old, Some("1.0.0")));
    assert_eq!(fake.scm.pr_list(Show::Mine, None, false).await.unwrap_err().reason, Reason::OldGh);
}

// ---- The branch's pull request ----

#[tokio::test]
async fn the_branchs_pull_request_none_one_and_the_open_one_first() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    let status = fake.status().await;
    fake.answer("pr-head", &fixture("pr-none.json"), 0);
    assert_eq!(fake.scm.pr_for_branch(&fake.spot, &status, true).await.unwrap(), None);

    fake.answer("pr-head", &fixture("pr-open.json"), 0);
    let pr = fake.scm.pr_for_branch(&fake.spot, &status, true).await.unwrap().unwrap();
    assert_eq!((pr.number, pr.state, pr.draft), (212, PrState::Open, false));
    assert_eq!((pr.review, pr.mergeable, pr.blocked), (None, Some(Mergeable::Yes), false));
    assert_eq!(pr.head_oid, "0123456789abcdef0123456789abcdef01234567");
    assert_eq!(pr.url, "https://github.com/fictional/repo/pull/212");
    let states: Vec<CheckState> = pr.checks.iter().map(|c| c.state).collect();
    assert_eq!(
        states,
        [CheckState::Fail, CheckState::Pending, CheckState::Pass, CheckState::Skipped, CheckState::Cancelled]
    );
    assert_eq!((pr.pass, pr.fail, pr.pending), (1, 1, 1));
    assert_eq!(pr.checks[0].name, "CI / test");
    assert_eq!(pr.checks[1].url, None, "a link off the signed-in hosts is dropped");
    let n = fake.count();
    assert_eq!(fake.scm.pr_for_branch(&fake.spot, &status, false).await.unwrap().unwrap().number, 212);
    assert_eq!(fake.count(), n, "kept for a minute");

    fake.answer("pr-head", &fixture("pr-merged-and-open.json"), 0);
    let pr = fake.scm.pr_for_branch(&fake.spot, &status, true).await.unwrap().unwrap();
    assert_eq!((pr.number, pr.state, pr.draft, pr.blocked), (9, PrState::Open, true, true));
    assert_eq!((pr.review, pr.mergeable), (Some(ReviewState::ChangesRequested), Some(Mergeable::Conflicts)));
    assert_eq!(pr.title, "Draft with colour");
    let checks: Vec<(String, CheckState, bool)> =
        pr.checks.iter().map(|c| (c.name.clone(), c.state, c.url.is_some())).collect();
    assert_eq!(
        checks,
        [
            ("ci/old".to_owned(), CheckState::Fail, false),
            ("ci/legacy".to_owned(), CheckState::Pending, true),
            ("queued".to_owned(), CheckState::Pending, false),
            ("future".to_owned(), CheckState::Pending, false),
        ]
    );
    let argv = fake.argv(fake.count());
    let mut want = words(&["pr", "list", "--head", "main", "--state", "all", "--limit", "5", "--json"]);
    want.push(PR_FIELDS.to_owned());
    assert_eq!(argv, want);
}

#[tokio::test]
async fn a_hundred_checks_are_kept_failing_first_and_bad_answers_are_errors() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    let status = fake.status().await;
    let checks: Vec<serde_json::Value> = (0..150)
        .map(|i| {
            let conclusion = if i % 10 == 9 { "FAILURE" } else { "SUCCESS" };
            serde_json::json!({"__typename": "CheckRun", "name": format!("c{i}"), "status": "COMPLETED", "conclusion": conclusion, "workflowName": "", "detailsUrl": ""})
        })
        .collect();
    let mut prs: serde_json::Value = serde_json::from_str(&fixture("pr-open.json")).unwrap();
    prs[0]["statusCheckRollup"] = serde_json::Value::Array(checks);
    fake.answer("pr-head", &prs.to_string(), 0);
    let pr = fake.scm.pr_for_branch(&fake.spot, &status, true).await.unwrap().unwrap();
    assert_eq!(pr.checks.len(), 100);
    assert!(pr.checks[..15].iter().all(|c| c.state == CheckState::Fail));
    assert!(pr.checks[15..].iter().all(|c| c.state == CheckState::Pass));
    assert_eq!((pr.pass, pr.fail, pr.pending), (135, 15, 0));

    fake.answer("pr-head", "{ not json", 0);
    assert_eq!(fake.scm.pr_for_branch(&fake.spot, &status, true).await.unwrap_err().reason, Reason::Failed);
    fake.answer("pr-head", &format!("[{}]", " ".repeat(2 << 20)), 0);
    assert_eq!(fake.scm.pr_for_branch(&fake.spot, &status, true).await.unwrap_err().reason, Reason::Failed);
    fake.answer("pr-head", "", 1);
    assert_eq!(fake.scm.pr_for_branch(&fake.spot, &status, true).await.unwrap_err().reason, Reason::Network);

    fake.answer("pr-head", "[]", 0);
    fake.answer_sleep("pr-head", 60);
    let started = std::time::Instant::now();
    let err = fake.scm.pr_for_branch(&fake.spot, &status, true).await.unwrap_err();
    assert_eq!(err.reason, Reason::Network);
    assert!(started.elapsed() < Duration::from_secs(40));
}

#[tokio::test]
async fn a_folder_off_github_or_not_pushed_or_detached_never_asks_gh() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    fake.scm.gh_state(true).await;
    let n = fake.count();
    let new = NewPr { title: "T".into(), body: String::new(), base: None, draft: false };

    let mut status = fake.status().await;
    status.ahead = Some(1);
    assert_eq!(
        fake.scm.pr_create(&fake.spot, &status, &new, &Lines::new()).await.unwrap_err().reason,
        Reason::NotPushed
    );
    status.upstream = None;
    assert_eq!(
        fake.scm.pr_create(&fake.spot, &status, &new, &Lines::new()).await.unwrap_err().reason,
        Reason::NotPushed
    );
    let mut detached = fake.status().await;
    detached.branch = None;
    assert_eq!(
        fake.scm.pr_for_branch(&fake.spot, &detached, true).await.unwrap_err().reason,
        Reason::Detached
    );
    assert_eq!(fake.count(), n);

    fake.git(&["remote", "set-url", "origin", "https://elsewhere.example.test/fictional/repo.git"]);
    let status = fake.status().await;
    assert!(status.github.is_none());
    assert_eq!(
        fake.scm.pr_for_branch(&fake.spot, &status, true).await.unwrap_err().reason,
        Reason::NotGithub
    );
    assert_eq!(fake.count(), n, "only gh's state was read");
    let _ = &fake.root;
}

// ---- Open a pull request ----

#[tokio::test]
async fn a_new_pull_request_gets_its_body_on_stdin_byte_for_byte() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    let status = fake.status().await;
    fake.answer_nth("pr-head", 1, &fixture("pr-none.json"), 0);
    fake.answer("pr-head", &fixture("pr-created.json"), 0);
    fake.answer("pr-create", &fixture("create-out.txt"), 0);
    let body = "-leading dash\nquotes \" ' and `backticks` and $(echo nope) and \\ back\n\ttab\n";
    let new =
        NewPr { title: "--web".into(), body: body.into(), base: Some("origin/main".into()), draft: true };
    let out = Lines::new();
    let pr = fake.scm.pr_create(&fake.spot, &status, &new, &out).await.unwrap();
    assert_eq!(pr.number, 215, "the number in gh's URL line");
    let create = (1..=fake.count()).find(|&n| is(&fake.argv(n), "pr", "create")).unwrap();
    assert_eq!(fake.stdin_of(create), body.as_bytes());
    assert_eq!(
        fake.argv(create),
        words(&[
            "pr",
            "create",
            "--head",
            "main",
            "--base",
            "main",
            "--title=--web",
            "--body-file",
            "-",
            "--draft"
        ])
    );
    assert!(
        out.lines().iter().any(|l| l == "https://github.com/fictional/repo/pull/215"),
        "{:?}",
        out.lines()
    );

    let Some(other) = Fake::new(true, &[]).await else { return };
    other.signed_in();
    let status = other.status().await;
    other.answer_nth("pr-head", 1, &fixture("pr-none.json"), 0);
    other.answer("pr-head", &fixture("pr-created.json"), 0);
    other.answer("pr-create", "https://phish.example.test/fictional/repo/pull/215\n", 0);
    let new = NewPr { title: "T".into(), body: String::new(), base: None, draft: false };
    let pr = other.scm.pr_create(&other.spot, &status, &new, &Lines::new()).await.unwrap();
    assert_eq!(pr.number, 214, "a URL off the signed-in hosts isn't trusted: the list decides");
    let create = (1..=other.count()).find(|&n| is(&other.argv(n), "pr", "create")).unwrap();
    assert_eq!(
        other.argv(create),
        words(&["pr", "create", "--head", "main", "--title=T", "--body-file", "-"])
    );
}

#[tokio::test]
async fn an_open_pull_request_or_a_bad_title_stops_a_create_before_it_runs() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    let status = fake.status().await;
    fake.answer("pr-head", &fixture("pr-open.json"), 0);
    let new = NewPr { title: "T".into(), body: String::new(), base: None, draft: false };
    assert_eq!(
        fake.scm.pr_create(&fake.spot, &status, &new, &Lines::new()).await.unwrap_err().reason,
        Reason::PrExists
    );
    for title in ["", "  ", "two\nlines", &"x".repeat(257)] {
        let new = NewPr { title: title.into(), body: String::new(), base: None, draft: false };
        assert_eq!(
            fake.scm.pr_create(&fake.spot, &status, &new, &Lines::new()).await.unwrap_err().reason,
            Reason::BadRequest
        );
    }
    let new = NewPr { title: "T".into(), body: "b".repeat(32_769), base: None, draft: false };
    assert_eq!(
        fake.scm.pr_create(&fake.spot, &status, &new, &Lines::new()).await.unwrap_err().reason,
        Reason::BadRequest
    );
    let new = NewPr { title: "T".into(), body: String::new(), base: Some("--web".into()), draft: false };
    assert_eq!(
        fake.scm.pr_create(&fake.spot, &status, &new, &Lines::new()).await.unwrap_err().reason,
        Reason::NotFound
    );
    assert!(fake.all_argv().iter().all(|a| !is(a, "pr", "create")));
}

// ---- Merge and ready ----

#[tokio::test]
async fn a_merge_checks_the_method_and_the_commit_before_anything_runs() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    fake.answer("repo", &fixture("repo-view.json"), 0);
    let methods = fake.scm.pr_methods(&fake.spot).await.unwrap();
    assert!(methods.squash && methods.rebase && !methods.merge);
    let head = "0123456789abcdef0123456789abcdef01234567";
    let out = Lines::new();
    let refused = fake.scm.pr_merge(&fake.spot, 212, Method::Merge, head, &out).await.unwrap_err();
    assert_eq!(refused.reason, Reason::NotAllowed);
    let upper = "A".repeat(40);
    for (number, head) in [(0, head), (212, "abc"), (212, "--admin"), (212, upper.as_str())] {
        let bad = fake.scm.pr_merge(&fake.spot, number, Method::Squash, head, &out).await.unwrap_err();
        assert_eq!(bad.reason, Reason::BadRequest);
    }
    let mut repo_view = words(&["repo", "view", "--json"]);
    repo_view.push(REPO_FIELDS.to_owned());
    for argv in fake.all_argv() {
        assert!(!is(&argv, "pr", "merge"), "nothing merged: {argv:?}");
        if argv[0] == "repo" {
            assert_eq!(argv, repo_view);
        }
    }
}

#[tokio::test]
async fn a_merge_is_pinned_to_the_head_shown_and_never_automatic() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    fake.answer("repo", &fixture("repo-view.json"), 0);
    fake.answer("pr-head", &fixture("pr-open.json"), 0);
    let status = fake.status().await;
    fake.scm.pr_for_branch(&fake.spot, &status, true).await.unwrap();
    let head = "0123456789abcdef0123456789abcdef01234567";
    fake.answer("pr-merge", "Merged pull request fictional/repo#212\n", 0);
    let out = Lines::new();
    fake.scm.pr_merge(&fake.spot, 212, Method::Squash, head, &out).await.unwrap();
    assert_eq!(
        fake.argv(fake.count()),
        words(&["pr", "merge", "212", "--squash", "--match-head-commit", head])
    );
    assert!(out.lines().iter().any(|l| l == "Merged pull request fictional/repo#212"), "{:?}", out.lines());
    let n = fake.count();
    fake.scm.pr_for_branch(&fake.spot, &status, false).await.unwrap();
    assert_eq!(fake.count(), n + 1, "a merge drops the kept pull request");

    fake.scm.pr_merge(&fake.spot, 212, Method::Rebase, head, &Lines::new()).await.unwrap();
    let merge = fake.argv(fake.count());
    assert_eq!(merge, words(&["pr", "merge", "212", "--rebase", "--match-head-commit", head]));

    fake.answer("pr-merge", "", 1);
    fake.answer_err(
        "pr-merge",
        "\u{1b}[31mGraphQL: Head branch was modified.\u{7} Review and try the merge again.\n",
    );
    let out = Lines::new();
    let failed = fake.scm.pr_merge(&fake.spot, 212, Method::Squash, head, &out).await.unwrap_err();
    assert_eq!(failed.reason, Reason::Failed);
    assert_eq!(out.lines(), ["GraphQL: Head branch was modified. Review and try the merge again."]);

    for argv in fake.all_argv().iter().filter(|a| is(a, "pr", "merge")) {
        assert!(argv.iter().any(|a| a == "--match-head-commit"), "{argv:?}");
        for never in ["--auto", "--admin", "--delete-branch", "-d", "--body", "--subject"] {
            assert!(!argv.iter().any(|a| a == never), "{argv:?}");
        }
    }
}

#[tokio::test]
async fn ready_names_the_number_only() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    fake.answer("pr-ready", "", 0);
    fake.scm.pr_ready(&fake.spot, 9).await.unwrap();
    assert_eq!(fake.argv(fake.count()), words(&["pr", "ready", "9"]));
    fake.answer("pr-ready", "", 1);
    assert_eq!(fake.scm.pr_ready(&fake.spot, 9).await.unwrap_err().reason, Reason::Network);
    assert_eq!(fake.scm.pr_ready(&fake.spot, 0).await.unwrap_err().reason, Reason::BadRequest);
}

// ---- Lists ----

#[tokio::test]
async fn the_three_lists_use_their_own_argv_cap_50_rows_and_clean_titles() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    fake.answer("search-mine", &fixture("search-mine.json"), 0);
    let mine = fake.scm.pr_list(Show::Mine, None, false).await.unwrap();
    assert_eq!(mine.len(), 2);
    assert_eq!((mine[0].number, mine[0].repo.as_str(), mine[0].state), (3, "fictional/repo", PrState::Open));
    assert!(mine[1].draft);
    let mut want = words(&["search", "prs", "--author=@me", "--state=open", "--limit", "50", "--json"]);
    want.push(SEARCH_FIELDS.to_owned());
    assert_eq!(fake.argv(fake.count()), want);
    let n = fake.count();
    fake.scm.pr_list(Show::Mine, None, false).await.unwrap();
    assert_eq!(fake.count(), n, "kept 30 seconds");

    let many: Vec<serde_json::Value> = (1..=80)
        .map(|i| serde_json::json!({"number": i, "title": format!("Row\u{1b}[1m {i}\u{7}"), "repository": {"name": "r", "nameWithOwner": "fictional/r"}, "state": "open", "isDraft": false, "updatedAt": "2026-11-19T08:00:00Z", "url": format!("https://github.com/fictional/r/pull/{i}")}))
        .collect();
    fake.answer("search-review", &serde_json::Value::Array(many).to_string(), 0);
    let review = fake.scm.pr_list(Show::Review, None, false).await.unwrap();
    assert_eq!(review.len(), 50);
    assert_eq!(review[0].title, "Row 1");
    let mut want =
        words(&["search", "prs", "--review-requested=@me", "--state=open", "--limit", "50", "--json"]);
    want.push(SEARCH_FIELDS.to_owned());
    assert_eq!(fake.argv(fake.count()), want);

    fake.answer("pr-open", &fixture("pr-merged-and-open.json"), 0);
    let here = fake.scm.pr_list(Show::Folder, Some(&fake.spot), false).await.unwrap();
    assert_eq!(here.len(), 2);
    assert_eq!(here[1].repo, "fictional/repo");
    assert_eq!(here[1].branch.as_deref(), Some("main"));
    let counts = here[1].checks.unwrap();
    assert_eq!((counts.pass, counts.fail, counts.pending), (0, 1, 3));
    let mut want = words(&["pr", "list", "--state", "open", "--limit", "50", "--json"]);
    want.push(LIST_FIELDS.to_owned());
    assert_eq!(fake.argv(fake.count()), want);
    assert_eq!(fake.scm.pr_list(Show::Folder, None, false).await.unwrap_err().reason, Reason::BadRequest);
}

#[tokio::test]
async fn every_call_is_a_row_of_the_closed_list() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    fake.answer("pr-head", &fixture("pr-open.json"), 0);
    fake.answer("search-mine", &fixture("search-mine.json"), 0);
    fake.answer("repo", &fixture("repo-view.json"), 0);
    let status = fake.status().await;
    fake.scm.pr_for_branch(&fake.spot, &status, true).await.unwrap();
    fake.scm.pr_list(Show::Mine, None, true).await.unwrap();
    fake.scm.pr_methods(&fake.spot).await.unwrap();
    let firsts: Vec<(String, String)> =
        fake.all_argv().iter().map(|a| (a[0].clone(), a.get(1).cloned().unwrap_or_default())).collect();
    let allowed =
        [("--version", ""), ("auth", "status"), ("pr", "list"), ("search", "prs"), ("repo", "view")];
    for (a, b) in &firsts {
        assert!(allowed.iter().any(|(x, y)| x == a && y == b), "{a} {b}");
    }
    assert_eq!(fake.argv(1), words(&["--version"]));
    assert_eq!(fake.argv(2), words(&["auth", "status", "--json", "hosts"]));
    for argv in fake.all_argv() {
        for never in [
            "api",
            "--admin",
            "--auto",
            "--delete-branch",
            "--fill",
            "--web",
            "--editor",
            "--show-token",
            "token",
        ] {
            assert!(!argv.iter().any(|a| a == never), "{argv:?}");
        }
    }
}
