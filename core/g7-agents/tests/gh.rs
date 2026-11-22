// SPDX-License-Identifier: MPL-2.0
//! Track G (phase 4 §SC4, §SC11.1 G): pull requests through `gh`, with a fake `gh` (a shell script
//! this file writes) first on a `PATH` that holds nothing else but a few shell tools, so the real
//! `gh` can never run and nothing reaches the network. The fake records each call's argv, its
//! environment, its stdin and where it ran, and prints a hand-written fixture with fictional values.
//! Phase 6 (§X6.1, §X12 G): a pull request's detail and diff, and `split_diff`'s fixtures.
#![cfg(feature = "host")]

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use g7_agents::api::{PrDetail, is_stripped_char};
use g7_agents::host::git::Git;
use g7_agents::host::scm::{
    CheckState, GhUse, GitStatus, Lines, Mergeable, Method, NewPr, PrState, Reason, ReviewState, Scm, Show,
    Spot, split_diff,
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
pwd > "$d/cwd-$n"
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
const DETAIL_FIELDS: &str = "number,title,state,isDraft,url,reviewDecision,mergeable,mergeStateStatus,baseRefName,headRefName,headRefOid,updatedAt,statusCheckRollup,author,body,additions,deletions,changedFiles,files,comments,reviews";
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_HEAD: &str = "ffffffffffffffffffffffffffffffffffffffff";
const TOKEN_VARIABLES: [&str; 4] =
    ["GH_TOKEN=", "GITHUB_TOKEN=", "GH_ENTERPRISE_TOKEN=", "GITHUB_ENTERPRISE_TOKEN="];

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

fn diff_fixture(name: &str) -> Vec<u8> {
    fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scm/prdiff").join(name)).unwrap()
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

    fn cwd_of(&self, n: usize) -> PathBuf {
        PathBuf::from(fs::read_to_string(self.calls.join(format!("cwd-{n}"))).unwrap().trim_end())
    }

    /// The calls whose argv starts `first second`.
    fn calls_of(&self, first: &str, second: &str) -> Vec<usize> {
        (1..=self.count()).filter(|&n| is(&self.argv(n), first, second)).collect()
    }

    /// The answer to `key` from a file this test wrote (for answers too large to inline).
    fn answer_file(&self, key: &str, bytes: &[u8], code: i32) {
        let a = self.calls.join("answers").join(key);
        fs::write(a.with_extension("out"), bytes).unwrap();
        fs::write(a.with_extension("code"), code.to_string()).unwrap();
    }

    async fn detail(&self, refresh: bool) -> Result<PrDetail, Reason> {
        self.scm.pr_detail(self.spot.folder(), "fictional/repo", 212, refresh).await.map_err(|e| e.reason)
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

// ---- One pull request, read-only (phase 6 §X6.1) ----

/// One part of a generated diff: an added text file of `lines` lines of 101 bytes.
fn added_part(path: &str, lines: usize) -> Vec<u8> {
    let mut part =
        format!("diff --git a/{path} b/{path}\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{lines} @@\n")
            .into_bytes();
    for i in 0..lines {
        part.extend_from_slice(format!("+{i:099}\n").as_bytes());
    }
    part
}

#[tokio::test]
async fn a_detail_reads_the_card_the_description_the_files_and_the_conversation() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    fake.answer("pr-view", &fixture("pr-detail.json"), 0);
    let detail = fake.detail(false).await.unwrap();
    let view = fake.calls_of("pr", "view");
    assert_eq!(view.len(), 1);
    let mut want = words(&["pr", "view", "212", "--repo", "fictional/repo", "--json"]);
    want.push(DETAIL_FIELDS.to_owned());
    assert_eq!(fake.argv(view[0]), want);
    assert_eq!(fake.cwd_of(view[0]), fake.spot.folder());

    let pr = &detail.pr;
    assert_eq!((pr.number, pr.state, pr.draft, pr.blocked), (212, PrState::Open, false, true));
    assert_eq!((pr.review, pr.mergeable), (Some(ReviewState::ChangesRequested), Some(Mergeable::Yes)));
    assert_eq!((pr.head_oid.as_str(), pr.pass, pr.checks[0].name.as_str()), (HEAD, 1, "CI / test"));
    assert_eq!((detail.repo.as_str(), detail.author.as_str()), ("fictional/repo", "fictional-author"));
    assert_eq!(
        detail.body, "Reads `config.toml` once.\n\n- [x] tests\n\n<img src=\"https://example.test/x.png\">\n",
        "carriage returns dropped, raw HTML left as text"
    );
    assert!(!detail.body_cut);
    assert_eq!((detail.adds, detail.dels, detail.changed), (120, 30, 3));
    let files: Vec<(&str, &str, u32, u32)> =
        detail.files.iter().map(|f| (f.path.as_str(), f.change.as_str(), f.adds, f.dels)).collect();
    assert_eq!(
        files,
        [
            ("src/config.rs", "modified", 100, 20),
            ("docs/config.md", "added", 20, 0),
            ("src/old.rs", "deleted", 0, 10)
        ]
    );
    assert!(!detail.files_more);

    let said: Vec<(&str, &str, Option<&str>)> =
        detail.comments.iter().map(|c| (c.author.as_str(), c.body.as_str(), c.review.as_deref())).collect();
    assert_eq!(
        said,
        [
            ("fictional-third", "Superseded.", Some("dismissed")),
            ("fictional-reviewer", "Looks close.", None),
            ("fictional-reviewer", "Please split the parser.", Some("changes_requested")),
            ("fictional-author", "Fixed in the next push.", None),
            ("fictional-second", "", Some("approved")),
            ("fictional-fourth", "One note inline.", Some("commented")),
            ("fictional-fifth", "Not sent yet.", None),
        ],
        "oldest first; a review that is only a comment with nothing written is left out"
    );
    assert!(detail.comments.windows(2).all(|w| w[0].at_ms <= w[1].at_ms));
    assert_eq!(
        detail.comments[1].url.as_deref(),
        Some("https://github.com/fictional/repo/pull/212#issuecomment-1")
    );
    assert_eq!(detail.comments[3].url, None, "a link off the signed-in hosts is dropped");
    assert!(detail.comments.iter().all(|c| !c.cut));
    assert_eq!(
        (detail.comments_older, detail.thread_id.as_deref(), detail.folder_id.as_deref()),
        (0, None, None)
    );
    assert!(detail.read_ms > 0);

    let n = fake.count();
    assert_eq!(fake.detail(false).await.unwrap(), detail);
    assert_eq!(fake.count(), n, "kept a minute");
    fake.detail(true).await.unwrap();
    assert_eq!(fake.count(), n + 1, "refresh reads it again");
    assert!(fake.scm.cached_pr_detail("fictional/repo", 212).is_some());
    assert!(fake.scm.cached_pr_detail("github.com/fictional/repo", 212).is_some());
    assert!(fake.scm.cached_pr_detail("fictional/repo", 213).is_none());

    let gone = fake.root.join("no-such-folder");
    fake.scm.pr_detail(&gone, "fictional/repo", 212, true).await.unwrap();
    assert_eq!(fake.cwd_of(fake.count()), fake.root.join("home"), "no folder: the home folder");
}

#[tokio::test]
async fn a_detail_keeps_its_bounds() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    let base: serde_json::Value = serde_json::from_str(&fixture("pr-detail.json")).unwrap();

    let mut quiet = base.clone();
    quiet["comments"] = serde_json::json!([]);
    quiet["reviews"] = serde_json::json!([]);
    quiet["body"] = serde_json::json!("");
    fake.answer("pr-view", &quiet.to_string(), 0);
    let d = fake.detail(true).await.unwrap();
    assert!(d.comments.is_empty() && d.comments_older == 0 && d.body.is_empty() && !d.body_cut);

    let mut busy = base.clone();
    busy["comments"] = (0..150)
        .map(|i| {
            let at = format!("2026-11-20T{:02}:{:02}:00Z", 10 + i / 60, i % 60);
            serde_json::json!({"author": {"login": format!("p{i}")}, "body": format!("comment {i}"), "createdAt": at})
        })
        .collect();
    busy["reviews"] = serde_json::json!([]);
    busy["body"] = serde_json::json!("\u{e9}".repeat(20 * 1024));
    busy["files"] = (0..400)
        .map(|i| serde_json::json!({"path": format!("src/f{i}.rs"), "additions": 1, "deletions": 0, "changeType": "RENAMED"}))
        .collect();
    busy["changedFiles"] = serde_json::json!(400);
    fake.answer("pr-view", &busy.to_string(), 0);
    let d = fake.detail(true).await.unwrap();
    assert_eq!((d.comments.len(), d.comments_older), (100, 50));
    assert_eq!((d.comments[0].body.as_str(), d.comments[99].body.as_str()), ("comment 50", "comment 149"));
    assert!(d.body_cut && d.body.len() <= 32_768 && d.body.len() >= 32_767, "{}", d.body.len());
    assert!(d.body.chars().all(|c| c == '\u{e9}'), "cut at a character");
    assert_eq!((d.files.len(), d.files_more, d.files[0].change.as_str()), (300, true, "renamed"));

    let mut few = base.clone();
    few["changedFiles"] = serde_json::json!(1000);
    fake.answer("pr-view", &few.to_string(), 0);
    let d = fake.detail(true).await.unwrap();
    assert_eq!((d.files.len(), d.files_more, d.changed), (3, true, 1000), "gh listed fewer than changed");

    let mut odd = base;
    odd["comments"] = serde_json::json!([
        {"author": {"login": "long"}, "body": "b".repeat(5000), "createdAt": "2026-11-20T10:00:00Z"},
        {"author": null, "body": "no author", "createdAt": "2026-11-20T10:00:00Z"},
        {"author": {"login": "no-body"}, "createdAt": "2026-11-20T10:00:00Z"},
        {"author": {"login": "blank"}, "body": " \n ", "createdAt": "2026-11-20T10:00:00Z"}
    ]);
    odd["reviews"] = serde_json::json!(null);
    odd["files"] = serde_json::json!([
        {"path": "../escape", "changeType": "MODIFIED"},
        {"path": "a\u{1}b", "changeType": "MODIFIED"},
        {"path": "ok.rs", "changeType": "SOMETHING_NEW"},
        {"path": null}
    ]);
    fake.answer("pr-view", &odd.to_string(), 0);
    let d = fake.detail(true).await.unwrap();
    assert_eq!(d.comments.len(), 1, "{:?}", d.comments);
    assert!(d.comments[0].cut && d.comments[0].body.len() == 4096);
    let files: Vec<(&str, &str)> = d.files.iter().map(|f| (f.path.as_str(), f.change.as_str())).collect();
    assert_eq!(files, [("ok.rs", "changed")], "a path G7 can't offer is left out");
    assert!(d.files_more, "three changed files aren't listed");
}

#[tokio::test]
async fn control_and_bidi_characters_never_reach_a_client() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    let mut hostile: serde_json::Value = serde_json::from_str(&fixture("pr-detail.json")).unwrap();
    hostile["author"] = serde_json::json!({"login": "fict\u{202e}ional\u{1b}[31m-author\u{7}"});
    hostile["title"] = serde_json::json!("Title\u{1b}[1m\u{202e} here");
    hostile["body"] =
        serde_json::json!("line\u{1b}]8;;https://evil.test\u{7}one\u{0}\r\n\u{2066}two\u{2069}\u{9b}");
    hostile["comments"] = serde_json::json!([
        {"author": {"login": "re\u{200f}viewer"}, "body": "bad\u{1b}\u{202a}text\u{85}", "createdAt": "2026-11-20T10:00:00Z"}
    ]);
    hostile["reviews"] = serde_json::json!([
        {"author": {"login": "app\u{2067}rover"}, "body": "ok\u{1b}[2J\u{8}", "state": "APPROVED", "submittedAt": "2026-11-20T11:00:00Z"}
    ]);
    fake.answer("pr-view", &hostile.to_string(), 0);
    let d = fake.detail(true).await.unwrap();
    let mut texts = vec![d.author.clone(), d.body.clone(), d.pr.title.clone()];
    texts.extend(d.comments.iter().flat_map(|c| [c.author.clone(), c.body.clone()]));
    for text in &texts {
        assert!(!text.chars().any(is_stripped_char), "{text:?}");
    }
    let authors: Vec<&str> = d.comments.iter().map(|c| c.author.as_str()).collect();
    assert_eq!((d.author.as_str(), authors), ("fictional-author", vec!["reviewer", "approver"]));
    assert!(d.body.contains("one\ntwo"), "{:?}", d.body);
}

#[tokio::test]
async fn a_token_or_an_unknown_key_in_a_detail_is_never_kept_or_logged() {
    let Some(fake) = Fake::new(true, &[("GH_TOKEN", "the-users-own-value")]).await else { return };
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    fake.signed_in();
    fake.answer("pr-view", &fixture("pr-detail.json"), 0);
    fake.answer("pr-diff", std::str::from_utf8(&diff_fixture("pr-212.diff")).unwrap(), 0);
    let detail = fake.detail(true).await.unwrap();
    fake.scm.pr_diff(fake.spot.folder(), "fictional/repo", 212, HEAD, "src/config.rs", 0).await.unwrap();
    let shown = format!("{detail:?} {} {:?}", serde_json::to_string(&detail).unwrap(), fake.scm);
    let logged = String::from_utf8_lossy(&log.0.lock().unwrap()).into_owned();
    for secret in
        ["gho_FICTIONAL", "fictional-unknown-value", "unknownKey", "authorAssociation", "the-users-own"]
    {
        assert!(!shown.contains(secret), "{secret} in {shown}");
        assert!(!logged.contains(secret), "{secret} in the log: {logged}");
    }
    assert!(!logged.contains("fictional-author"), "an author is never logged: {logged}");
    let reads: Vec<usize> =
        fake.calls_of("pr", "view").into_iter().chain(fake.calls_of("pr", "diff")).collect();
    assert_eq!(reads.len(), 2);
    for n in reads {
        let env = fake.env_of(n);
        let tokens: Vec<&String> =
            env.iter().filter(|l| TOKEN_VARIABLES.iter().any(|t| l.starts_with(t))).collect();
        assert_eq!(tokens, ["GH_TOKEN=the-users-own-value"], "the user's own passes as it is; none is added");
        assert!(!env.iter().any(|l| l.starts_with("GH_REPO=")), "GH_REPO stays unset");
    }
}

#[tokio::test]
async fn bad_or_slow_detail_answers_are_errors_never_panics() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    let mut another: serde_json::Value = serde_json::from_str(&fixture("pr-detail.json")).unwrap();
    another["number"] = serde_json::json!(213);
    let mut wrong_type: serde_json::Value = serde_json::from_str(&fixture("pr-detail.json")).unwrap();
    wrong_type["additions"] = serde_json::json!("many");
    let huge = format!("{{\"number\": 212, \"state\": \"OPEN\", \"body\": \"{}\"}}", "x".repeat(2 << 20));
    let cases = [
        ("{ not json".to_owned(), 0, Reason::Failed),
        ("null".to_owned(), 0, Reason::Failed),
        ("[]".to_owned(), 0, Reason::Failed),
        (another.to_string(), 0, Reason::Failed),
        (wrong_type.to_string(), 0, Reason::Failed),
        (huge, 0, Reason::Failed),
        (String::new(), 1, Reason::Network),
    ];
    for (out, code, want) in cases {
        fake.answer("pr-view", &out, code);
        assert_eq!(fake.detail(true).await.unwrap_err(), want, "{}", &out[..out.len().min(40)]);
    }

    fake.answer("pr-view", "", 4);
    assert_eq!(fake.detail(true).await.unwrap_err(), Reason::SignedOut);
    let auths = fake.calls_of("auth", "status").len();
    fake.answer("auth", "", 1);
    assert_eq!(fake.detail(true).await.unwrap_err(), Reason::SignedOut, "exit 4 flips the state");
    assert_eq!(fake.calls_of("auth", "status").len(), auths + 1, "and it is read again");

    fake.signed_in();
    fake.answer("pr-view", &fixture("pr-detail.json"), 0);
    fake.answer_sleep("pr-view", 60);
    let started = std::time::Instant::now();
    assert_eq!(fake.detail(true).await.unwrap_err(), Reason::Network);
    assert!(started.elapsed() < Duration::from_secs(40));
}

#[tokio::test]
async fn a_repository_number_head_or_path_is_checked_before_gh_runs() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    let folder = fake.spot.folder().to_owned();
    for repo in ["-x/y", "a/b/c/d", "../x", "x/--web", "o/r --web", "", "o"] {
        let err = fake.scm.pr_detail(&folder, repo, 212, true).await.unwrap_err();
        assert_eq!(err.reason, Reason::BadRequest, "{repo}");
        let err = fake.scm.pr_diff(&folder, repo, 212, HEAD, "src/config.rs", 0).await.unwrap_err();
        assert_eq!(err.reason, Reason::BadRequest, "{repo}");
    }
    let zero = fake.scm.pr_detail(&folder, "fictional/repo", 0, true).await.unwrap_err();
    assert_eq!(zero.reason, Reason::BadRequest);
    for (head, path) in
        [("abc", "src/config.rs"), (HEAD, "../x"), (HEAD, "/etc/x"), (HEAD, ""), (HEAD, "a//b")]
    {
        let err = fake.scm.pr_diff(&folder, "fictional/repo", 212, head, path, 0).await.unwrap_err();
        assert_eq!(err.reason, Reason::BadRequest, "{head} {path}");
    }
    let zero = fake.scm.pr_diff(&folder, "fictional/repo", 0, HEAD, "src/config.rs", 0).await.unwrap_err();
    assert_eq!(zero.reason, Reason::BadRequest);
    assert_eq!(fake.count(), 0, "nothing ran, not even gh's state");

    let off = "ghe.example.test/fictional/repo";
    assert_eq!(fake.scm.pr_detail(&folder, off, 212, true).await.unwrap_err().reason, Reason::NotGithub);
    let err = fake.scm.pr_diff(&folder, off, 212, HEAD, "src/config.rs", 0).await.unwrap_err();
    assert_eq!(err.reason, Reason::NotGithub);
    assert!(fake.count() > 0, "gh's state was read");
    assert!(fake.calls_of("pr", "view").is_empty() && fake.calls_of("pr", "diff").is_empty());

    fake.answer("pr-view", &fixture("pr-detail.json"), 0);
    let d = fake.scm.pr_detail(&folder, "GitHub.com/fictional/repo", 212, true).await.unwrap();
    assert_eq!(d.repo, "fictional/repo");
    assert_eq!(fake.argv(fake.count())[4], "fictional/repo");
}

#[tokio::test]
async fn a_diff_is_read_once_per_head_and_served_by_file() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    fake.answer_file("pr-diff", &diff_fixture("pr-212.diff"), 0);
    let folder = fake.spot.folder().to_owned();
    let diff = |path: &'static str, head: &'static str| {
        let (scm, folder) = (&fake.scm, folder.clone());
        async move { scm.pr_diff(&folder, "fictional/repo", 212, head, path, 0).await }
    };

    let config = diff("src/config.rs", HEAD).await.unwrap();
    assert!(config.text.starts_with("diff --git a/src/config.rs b/src/config.rs\n"), "{}", config.text);
    assert!(config.text.ends_with(" fn main() {}\n") && config.text.contains("+fn read()"));
    assert!(!config.truncated && !config.binary && config.next.is_none());
    let calls = fake.calls_of("pr", "diff");
    assert_eq!(calls.len(), 1);
    assert_eq!(
        fake.argv(calls[0]),
        words(&["pr", "diff", "212", "--repo", "fictional/repo", "--color", "never"])
    );
    assert_eq!(fake.cwd_of(calls[0]), folder);

    assert!(diff("docs/config.md", HEAD).await.unwrap().text.contains("+# Config\n"));
    let old = diff("src/old.rs", HEAD).await.unwrap();
    assert!(old.text.starts_with("diff --git a/src/old.rs") && old.text.ends_with("-// old\n"));
    let logo = diff("img/logo.png", HEAD).await.unwrap();
    assert!(logo.binary && logo.text.is_empty());
    assert_eq!(fake.calls_of("pr", "diff").len(), 1, "read once, served by file");
    assert_eq!(diff("src/missing.rs", HEAD).await.unwrap_err().reason, Reason::NotFound);
    let far = fake.scm.pr_diff(&folder, "fictional/repo", 212, HEAD, "src/config.rs", 1 << 20).await;
    assert_eq!(far.unwrap_err().reason, Reason::BadRequest, "an offset no page gave");

    diff("src/config.rs", OTHER_HEAD).await.unwrap();
    assert_eq!(fake.calls_of("pr", "diff").len(), 2, "another head is another diff");
    diff("src/config.rs", HEAD).await.unwrap();
    assert_eq!(fake.calls_of("pr", "diff").len(), 2, "two are kept");

    fake.answer("pr-view", &fixture("pr-detail.json"), 0);
    fake.detail(true).await.unwrap();
    diff("src/config.rs", HEAD).await.unwrap();
    assert_eq!(fake.calls_of("pr", "diff").len(), 3, "a refreshed detail drops its kept diffs");
}

#[tokio::test]
async fn a_diff_pages_like_a_patch_and_stops_at_a_megabyte_a_file() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    let mut whole = added_part("mid.txt", 3_000);
    let mid_len = whole.len();
    whole.extend(added_part("big.txt", 15_000));
    fake.answer_file("pr-diff", &whole, 0);
    let page = |path: &'static str, offset: u64| {
        let (scm, folder) = (&fake.scm, fake.spot.folder().to_owned());
        async move { scm.pr_diff(&folder, "fictional/repo", 212, HEAD, path, offset).await.unwrap() }
    };

    let first = page("mid.txt", 0).await;
    assert!(first.text.len() <= 196_608 && first.text.ends_with('\n') && !first.truncated);
    let next = first.next.expect("a second page");
    let second = page("mid.txt", next).await;
    assert_eq!(second.next, None);
    assert_eq!(format!("{}{}", first.text, second.text).as_bytes(), &whole[..mid_len]);

    let mut offset = Some(0);
    let mut served = 0;
    while let Some(at) = offset {
        let p = page("big.txt", at).await;
        assert!(p.truncated, "the file passes 1 MiB");
        served += p.text.len();
        offset = p.next;
    }
    assert!(served <= 1 << 20 && served > (1 << 20) - 200, "{served}");
}

#[tokio::test]
async fn a_diff_past_4_mib_is_cut_and_the_files_it_reaches_are_flagged() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    let mut whole = added_part("small.rs", 10);
    whole.extend(added_part("a.rs", 39_400));
    let b_start = whole.len();
    whole.extend(added_part("b.rs", 5_000));
    whole.extend(added_part("c.rs", 10));
    assert!(b_start < 4 << 20 && whole.len() > (4 << 20) + 200_000);
    fake.answer_file("pr-diff", &whole, 0);
    let page = |path: &'static str| {
        let (scm, folder) = (&fake.scm, fake.spot.folder().to_owned());
        async move { scm.pr_diff(&folder, "fictional/repo", 212, HEAD, path, 0).await.unwrap() }
    };

    let small = page("small.rs").await;
    assert!(!small.truncated && small.text.ends_with(&format!("+{:099}\n", 9)));
    let b = page("b.rs").await;
    assert!(b.truncated, "its part runs into the cut");
    assert!(b.text.ends_with('\n') && b.text.len() < (4 << 20) - b_start + 1, "cut at a line end");
    let c = page("c.rs").await;
    assert!(c.truncated && c.text.is_empty(), "past the cut: nothing, flagged");
    assert_eq!(fake.calls_of("pr", "diff").len(), 1);
}

#[tokio::test]
async fn a_diff_github_refuses_as_too_large_or_cant_send_is_not_kept() {
    let Some(fake) = Fake::new(true, &[]).await else { return };
    fake.signed_in();
    let folder = fake.spot.folder().to_owned();
    let read = || fake.scm.pr_diff(&folder, "fictional/repo", 212, HEAD, "src/config.rs", 0);
    fake.answer("pr-diff", "", 1);
    for err in [
        "could not find pull request diff: HTTP 406: Sorry, the diff is too large to show (https://example.test)\n",
        "HTTP 406: Sorry, the diff exceeded the maximum number of files (300).\nPullRequest.diff too_large\n",
    ] {
        fake.answer_err("pr-diff", err);
        assert_eq!(read().await.unwrap_err().reason, Reason::Large, "{err}");
    }
    fake.answer_err("pr-diff", "error connecting to the server\n");
    assert_eq!(read().await.unwrap_err().reason, Reason::Network);
    fake.answer("pr-diff", "", 4);
    assert_eq!(read().await.unwrap_err().reason, Reason::SignedOut);
    fake.signed_in();
    fake.answer_err("pr-diff", "");
    fake.answer_file("pr-diff", &diff_fixture("pr-212.diff"), 0);
    let before = fake.calls_of("pr", "diff").len();
    assert!(read().await.unwrap().text.contains("+fn read()"));
    assert_eq!(fake.calls_of("pr", "diff").len(), before + 1, "a failure kept nothing");
}

// ---- A diff, by file ----

#[test]
fn split_diff_names_each_file_by_its_own_lines() {
    let diff = diff_fixture("parts.diff");
    let parts = split_diff(&diff);
    let paths: Vec<&str> = parts.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        [
            "src/added.rs",
            "src/gone.rs",
            "docs/new name.md",
            "img/logo.png",
            "run.sh",
            "caf\u{e9} \"q\".txt",
            "x b/y.txt",
            "x b/mode only",
            "tail.txt",
        ]
    );
    for (path, range) in &parts {
        assert!(diff[range.clone()].starts_with(b"diff --git "), "{path}");
    }
    for pair in parts.windows(2) {
        assert!(pair[0].1.end <= pair[1].1.start);
    }
    assert_eq!(parts[0].1.start, 0);
    assert_eq!(parts.last().unwrap().1.end, diff.len(), "the last part runs to the end, newline or not");
    assert!(
        diff[parts[1].1.clone()].ends_with(b"-++ b/evil\n"),
        "a hunk line like a header stays in its part"
    );

    let outside = diff.windows(22).position(|w| w == b"diff --git a/../outsid").unwrap();
    assert!(parts.iter().all(|(_, r)| !r.contains(&outside)), "a path with `..` is not offered");
    assert_eq!(parts[7].1.end, outside);

    assert!(split_diff(b"").is_empty() && split_diff(b"not a diff\n").is_empty());
    assert_eq!(split_diff(b"preamble\ndiff --git a/x b/x\n"), [("x".to_owned(), 9..28)]);
    assert!(split_diff(b"diff --git a/x b/y\n").is_empty(), "two names and no other line: no path");
}
