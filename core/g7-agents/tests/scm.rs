// SPDX-License-Identifier: MPL-2.0
//! Track G (phase 4 §SC3, §SC11.1 G): source control's git side, every case on a throwaway
//! repository this file makes in a temporary folder, with `HOME`, `XDG_CONFIG_HOME` and
//! `GIT_CONFIG_GLOBAL` pointed into it and `GIT_CONFIG_NOSYSTEM=1`, a bare repository beside it as
//! the remote, and hooks the test writes. The user's own configuration and repositories are never
//! read.
#![cfg(feature = "host")]

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::time::Duration;

use g7_agents::host::git::{self, Capture, Git, Place};
use g7_agents::host::scm::{
    Change, Doing, GitStatus, Lines, MAX_JOB_BYTES, MAX_JOB_LINES, NoRepo, Picks, Reason, RepoState, Scm,
    ScmError, Spot, Staged, Stop, SwitchTo,
};

// ---- The lab ----

/// The path of `git`, or `None` (and a printed reason) when there is none. CI's runners have git:
/// there a missing one fails.
fn real_git() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let found = std::env::split_paths(&path).map(|d| d.join("git")).find(|p| p.is_file());
    if found.is_none() {
        assert!(std::env::var("CI").as_deref() != Ok("true"), "git is not on PATH in CI");
        eprintln!("skipped: git is not on PATH");
    }
    found
}

struct Lab {
    _dir: tempfile::TempDir,
    root: PathBuf,
    env: Vec<(OsString, OsString)>,
    git: Arc<Git>,
    scm: Scm,
    real: PathBuf,
}

const GLOBAL: &str =
    "[init]\n\tdefaultBranch = main\n[user]\n\tuseConfigOnly = true\n[advice]\n\tdetachedHead = false\n";

impl Lab {
    fn new() -> Option<Lab> {
        Lab::with_env(&[])
    }

    fn with_env(extra: &[(&str, &str)]) -> Option<Lab> {
        let real = real_git()?;
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for d in ["home", "xdg", "data"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::write(root.join("gitconfig"), GLOBAL).unwrap();
        let mut env: Vec<(OsString, OsString)> = vec![
            ("PATH".into(), std::env::var_os("PATH").unwrap_or_default()),
            ("HOME".into(), root.join("home").into()),
            ("XDG_CONFIG_HOME".into(), root.join("xdg").into()),
            ("GIT_CONFIG_GLOBAL".into(), root.join("gitconfig").into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_CEILING_DIRECTORIES".into(), root.clone().into()),
        ];
        env.extend(extra.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))));
        let git = Git::with_bin(real.clone(), &env);
        let scm = Scm::new(git.clone(), &env);
        Some(Lab { _dir: dir, root, env, git, scm, real })
    }

    /// The same lab's git and configuration with another binary in front (a shim).
    fn scm_with(&self, bin: PathBuf) -> Scm {
        Scm::new(Git::with_bin(bin, &self.env), &self.env)
    }

    fn try_git(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(&self.real)
            .args(args)
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .env("GIT_OPTIONAL_LOCKS", "0")
            .current_dir(cwd)
            .output()
            .unwrap()
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let out = self.try_git(cwd, args);
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
    }

    /// A repository with an identity and one commit: `tracked.txt` and `sub/s.txt`.
    fn repo(&self, name: &str) -> PathBuf {
        let repo = self.bare_repo(name);
        self.git(&repo, &["config", "user.name", "Lab"]);
        self.git(&repo, &["config", "user.email", "lab@localhost"]);
        write(&repo, "tracked.txt", "a\n");
        write(&repo, "sub/s.txt", "s\n");
        self.commit(&repo, "one");
        repo
    }

    /// A repository with no identity and no commit.
    fn bare_repo(&self, name: &str) -> PathBuf {
        let repo = self.root.join(name);
        fs::create_dir_all(&repo).unwrap();
        self.git(&repo, &["init", "-q"]);
        repo
    }

    /// A bare repository to push to.
    fn remote(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        self.git(&self.root, &["init", "-q", "--bare", path.to_str().unwrap()]);
        path
    }

    fn commit(&self, repo: &Path, message: &str) {
        self.git(repo, &["add", "-A"]);
        self.git(repo, &["commit", "-q", "--no-verify", "--allow-empty", "-m", message]);
    }

    fn head(&self, repo: &Path) -> String {
        String::from_utf8_lossy(&self.try_git(repo, &["rev-parse", "-q", "--verify", "HEAD"]).stdout)
            .trim()
            .to_owned()
    }

    fn staged(&self, repo: &Path) -> String {
        self.git(repo, &["diff", "--cached", "--name-only"])
    }

    async fn spot(&self, folder: &Path) -> Spot {
        self.scm.spot(folder).await.unwrap()
    }

    async fn status(&self, folder: &Path) -> (Spot, GitStatus) {
        let spot = self.spot(folder).await;
        let status = self.scm.status(&spot).await.unwrap();
        (spot, status)
    }
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn hook(repo: &Path, name: &str, body: &str) {
    script(&repo.join(".git/hooks").join(name), body);
}

fn paths(list: &[&str]) -> Picks {
    Picks::Paths(list.iter().map(|s| (*s).to_owned()).collect())
}

fn reason<T: std::fmt::Debug>(result: Result<T, ScmError>) -> Reason {
    result.expect_err("expected a refusal").reason
}

fn files(status: &GitStatus) -> Vec<(String, Change)> {
    status.files.iter().map(|f| (f.path.clone(), f.status)).collect()
}

/// A git that records each call's arguments (one line each) before running the real one.
fn recording_git(lab: &Lab) -> (PathBuf, PathBuf) {
    let log = lab.root.join("calls.log");
    let bin = lab.root.join("bin/git");
    script(
        &bin,
        &format!(
            "printf '%s ' \"$@\" >> '{}'\necho >> '{}'\nexec '{}' \"$@\"",
            log.display(),
            log.display(),
            lab.real.display()
        ),
    );
    (bin, log)
}

fn calls(log: &Path) -> Vec<String> {
    fs::read_to_string(log).unwrap_or_default().lines().map(str::to_owned).collect()
}

// ---- Not a repository ----

#[tokio::test]
async fn a_plain_folder_is_none_a_removed_one_reads_none_and_an_unsafe_one_is_refused() {
    let Some(lab) = Lab::new() else { return };
    let plain = lab.root.join("plain");
    fs::create_dir_all(&plain).unwrap();
    assert_eq!(lab.scm.spot(&plain).await.unwrap_err(), NoRepo::None);
    assert_eq!(lab.scm.spot(Path::new("relative")).await.unwrap_err(), NoRepo::None);
    assert_eq!(lab.scm.spot(&lab.root.join("missing")).await.unwrap_err(), NoRepo::None);
    assert_eq!(GitStatus::without(NoRepo::None.repo()).repo, RepoState::None);

    let repo = lab.repo("r");
    let spot = lab.spot(&repo).await;
    fs::remove_dir_all(&repo).unwrap();
    assert_eq!(lab.scm.status(&spot).await.unwrap().repo, RepoState::None);

    let Some(odd) = Lab::with_env(&[("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1")]) else { return };
    let repo = odd.root.join("theirs");
    fs::create_dir_all(&repo).unwrap();
    odd.git(&repo, &["-c", "safe.directory=*", "init", "-q"]);
    assert_eq!(odd.scm.spot(&repo).await.unwrap_err(), NoRepo::Refused);
}

#[tokio::test]
async fn a_folder_outside_the_agent_folders_repository_is_refused() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let nested = lab.repo("r/inner");
    let worktrees = lab.root.join("data/worktrees");
    assert!(lab.scm.spot_within(&repo.join("sub"), &repo, &worktrees).await.is_ok());
    assert_eq!(lab.scm.spot_within(&nested, &repo, &worktrees).await.unwrap_err(), NoRepo::Refused);
}

// ---- HEAD shapes ----

#[tokio::test]
async fn an_unborn_repository_takes_its_first_commit_and_refuses_a_push() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.bare_repo("r");
    lab.git(&repo, &["config", "user.name", "Lab"]);
    lab.git(&repo, &["config", "user.email", "lab@localhost"]);
    write(&repo, "a.txt", "a\n");
    let (spot, status) = lab.status(&repo).await;
    assert!(status.unborn && status.branch.as_deref() == Some("main") && status.last.is_none());
    assert_eq!(files(&status), [("a.txt".to_owned(), Change::Untracked)]);
    let patch = lab.scm.patch(&spot, &status, "a.txt", 0).await.unwrap();
    assert!(patch.text.contains("+a"), "{}", patch.text);

    let out = Lines::new();
    assert_eq!(reason(lab.scm.push(&spot, None, &out, &Stop::new()).await), Reason::NoCommits);
    let made =
        lab.scm.commit(&spot, &status, &paths(&["a.txt"]), "First", None, &out, &Stop::new()).await.unwrap();
    assert_eq!((made.subject.as_str(), made.files), ("First", 1));
    assert_eq!(made.oid7, lab.head(&repo)[..7]);
}

#[tokio::test]
async fn a_detached_head_is_said_refuses_commit_and_push_and_a_new_branch_keeps_it() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let head = lab.head(&repo);
    lab.git(&repo, &["checkout", "-q", "--detach"]);
    write(&repo, "tracked.txt", "b\n");
    let (spot, status) = lab.status(&repo).await;
    assert_eq!(status.detached.as_deref(), Some(&head[..7]));
    assert!(status.branch.is_none());
    let out = Lines::new();
    let stop = Stop::new();
    assert_eq!(
        reason(lab.scm.commit(&spot, &status, &paths(&["tracked.txt"]), "m", None, &out, &stop).await),
        Reason::Detached
    );
    assert_eq!(reason(lab.scm.push(&spot, None, &out, &stop).await), Reason::Detached);
    let name = lab.scm.switch(&spot, &SwitchTo::New("keep/it".into()), true, &out).await.unwrap();
    assert_eq!(name, "keep/it");
    assert_eq!(lab.git(&repo, &["symbolic-ref", "--short", "HEAD"]), "keep/it");
    assert_eq!(lab.head(&repo), head);
    assert_eq!(fs::read_to_string(repo.join("tracked.txt")).unwrap(), "b\n");
}

#[tokio::test]
async fn without_an_upstream_there_are_no_counts_and_a_push_sets_one_once() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let bare = lab.remote("remote.git");
    lab.git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    let (spot, status) = lab.status(&repo).await;
    assert_eq!((status.upstream.as_deref(), status.ahead, status.behind), (None, None, None));
    assert_eq!(status.remote.as_deref(), Some("origin"));
    assert_eq!(status.remotes.len(), 1);
    assert!(status.remotes[0].local);

    let out = Lines::new();
    let pushed = lab.scm.push(&spot, None, &out, &Stop::new()).await.unwrap();
    assert!(pushed.set_upstream && !pushed.up_to_date);
    assert_eq!((pushed.branch.as_str(), pushed.remote.as_str()), ("main", "origin"));
    let status = lab.scm.status(&spot).await.unwrap();
    assert_eq!(
        (status.upstream.as_deref(), status.ahead, status.behind),
        (Some("origin/main"), Some(0), Some(0))
    );
    let again = lab.scm.push(&spot, None, &out, &Stop::new()).await.unwrap();
    assert!(!again.set_upstream && again.up_to_date);
    assert!(out.lines().iter().all(|l| !l.contains(bare.to_str().unwrap())), "{:?}", out.lines());
}

// ---- Status ----

#[tokio::test]
async fn status_lists_every_kind_of_change_from_the_folder_only() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    write(&repo, "sub/b.txt", "b\n");
    write(&repo, "sub/t.txt", "t\n");
    write(&repo, "sub/d.txt", "d\n");
    lab.commit(&repo, "two");
    write(&repo, "sub/s.txt", "changed\n");
    write(&repo, "sub/new.txt", "n\n");
    lab.git(&repo, &["add", "sub/new.txt"]);
    write(&repo, "sub/docs/a.md", "a\n");
    write(&repo, "sub/docs/img/p", "p\n");
    fs::remove_file(repo.join("sub/d.txt")).unwrap();
    lab.git(&repo, &["mv", "sub/b.txt", "sub/b2.txt"]);
    fs::remove_file(repo.join("sub/t.txt")).unwrap();
    std::os::unix::fs::symlink("s.txt", repo.join("sub/t.txt")).unwrap();
    write(&repo, "tracked.txt", "outside\n");

    let (spot, status) = lab.status(&repo.join("sub")).await;
    let mut got = files(&status);
    got.sort();
    assert_eq!(
        got,
        [
            ("b2.txt".to_owned(), Change::Renamed),
            ("d.txt".to_owned(), Change::Deleted),
            ("docs/".to_owned(), Change::Untracked),
            ("new.txt".to_owned(), Change::Added),
            ("s.txt".to_owned(), Change::Modified),
            ("t.txt".to_owned(), Change::Type),
        ]
    );
    let renamed = status.files.iter().find(|f| f.path == "b2.txt").unwrap();
    assert_eq!((renamed.from.as_deref(), renamed.staged), (Some("b.txt"), Staged::All));
    let folder = status.files.iter().find(|f| f.path == "docs/").unwrap();
    assert!(folder.folder);
    let modified = status.files.iter().find(|f| f.path == "s.txt").unwrap();
    assert_eq!((modified.adds, modified.dels, modified.staged), (Some(1), Some(1), Staged::None));
    assert_eq!(status.total, 6);
    assert!(!status.more);

    let patch = lab.scm.patch(&spot, &status, "docs/", 0).await.unwrap();
    assert_eq!(patch.names, ["docs/a.md", "docs/img/p"]);
    let patch = lab.scm.patch(&spot, &status, "b2.txt", 0).await.unwrap();
    assert!(patch.text.contains("rename from sub/b.txt"), "{}", patch.text);
    assert_eq!(
        lab.scm.patch(&spot, &status, "../tracked.txt", 0).await.unwrap_err().reason,
        Reason::NotFound
    );
    assert_eq!(lab.scm.patch(&spot, &status, "nope", 0).await.unwrap_err().reason, Reason::NotFound);
}

#[tokio::test]
async fn a_conflict_is_counted_and_a_merge_in_progress_stops_commit_and_switch() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    lab.git(&repo, &["checkout", "-q", "-b", "other"]);
    write(&repo, "tracked.txt", "theirs\n");
    lab.commit(&repo, "theirs");
    lab.git(&repo, &["checkout", "-q", "main"]);
    write(&repo, "tracked.txt", "ours\n");
    lab.commit(&repo, "ours");
    assert!(!lab.try_git(&repo, &["merge", "-q", "other"]).status.success());

    let (spot, status) = lab.status(&repo).await;
    assert_eq!(status.doing, Some(Doing::Merge));
    assert_eq!(status.conflicts, 1);
    assert_eq!(files(&status), [("tracked.txt".to_owned(), Change::Conflict)]);
    let out = Lines::new();
    let picks = paths(&["tracked.txt"]);
    assert_eq!(
        reason(lab.scm.commit(&spot, &status, &picks, "m", None, &out, &Stop::new()).await),
        Reason::Doing
    );
    assert_eq!(
        reason(lab.scm.switch(&spot, &SwitchTo::Local("other".into()), true, &out).await),
        Reason::Doing
    );
    fs::remove_file(repo.join(".git/MERGE_HEAD")).unwrap();
    assert_eq!(
        reason(lab.scm.commit(&spot, &status, &picks, "m", None, &out, &Stop::new()).await),
        Reason::Conflicts
    );
}

#[tokio::test]
async fn past_2000_files_the_rest_are_counted_and_the_fingerprint_follows_edits() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    for i in 0..2001 {
        fs::write(repo.join(format!("f{i:04}.txt")), "x").unwrap();
    }
    let (spot, status) = lab.status(&repo).await;
    assert_eq!((status.files.len(), status.total, status.more), (2000, 2001, true));
    let again = lab.scm.status(&spot).await.unwrap();
    assert_eq!(again.fingerprint, status.fingerprint);
    assert_eq!(status.fingerprint.len(), 16);

    write(&repo, "tracked.txt", "b\n");
    let edited = lab.scm.status(&spot).await.unwrap();
    assert_ne!(edited.fingerprint, status.fingerprint);
    std::thread::sleep(Duration::from_millis(20));
    write(&repo, "tracked.txt", "c\n");
    let same_line = lab.scm.status(&spot).await.unwrap();
    assert_ne!(
        same_line.fingerprint, edited.fingerprint,
        "an edit that leaves git's line alike still counts"
    );
}

#[tokio::test]
async fn odd_paths_are_listed_and_committed_byte_for_byte() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let newline = "two\nlines.txt";
    let quote = "say \"hi\".txt";
    let raw: &[u8] = b"bytes-\xff\xfe.txt";
    write(&repo, newline, "n\n");
    write(&repo, quote, "q\n");
    fs::write(repo.join(std::ffi::OsStr::from_bytes(raw)), "r\n").unwrap();
    let (spot, status) = lab.status(&repo).await;
    let mut names: Vec<String> = status.files.iter().map(|f| f.path.clone()).collect();
    names.sort();
    assert_eq!(names, ["bytes-\u{fffd}\u{fffd}.txt", quote, newline]);
    let review = lab.scm.review(&spot, &status, &Picks::Paths(names.clone())).await.unwrap();
    let out = Lines::new();
    let made = lab
        .scm
        .commit(&spot, &status, &Picks::Paths(names), "Odd names", Some(&review.id), &out, &Stop::new())
        .await;
    assert_eq!(made.unwrap().files, 3);
    let listed = lab.try_git(&repo, &["ls-files", "-z"]).stdout;
    assert!(listed.split(|&b| b == 0).any(|p| p == raw), "the raw name is committed as it is");
    assert!(listed.split(|&b| b == 0).any(|p| p == newline.as_bytes()));
    assert_eq!(lab.scm.status(&spot).await.unwrap().total, 0);
}

// ---- Hooks ----

#[tokio::test]
async fn commit_hooks_run_in_order_and_their_lines_reach_the_job() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let marker = lab.root.join("marker");
    let m = marker.display();
    hook(
        &repo,
        "pre-commit",
        &format!("echo pre >> '{m}'\necho 'pre-commit says hi'\ncat > /dev/null\necho 'stdin ended' >&2"),
    );
    hook(&repo, "commit-msg", &format!("echo msg >> '{m}'\necho 'commit-msg checked' >&2"));
    write(&repo, "tracked.txt", "b\n");
    let (spot, status) = lab.status(&repo).await;
    let out = Lines::new();
    let made = tokio::time::timeout(
        Duration::from_secs(60),
        lab.scm.commit(&spot, &status, &paths(&["tracked.txt"]), "With hooks", None, &out, &Stop::new()),
    )
    .await
    .expect("a hook reading stdin gets end-of-file")
    .unwrap();
    assert_eq!(made.subject, "With hooks");
    assert_eq!(fs::read_to_string(&marker).unwrap(), "pre\nmsg\n");
    let lines = out.lines();
    let at = |s: &str| lines.iter().position(|l| l.contains(s)).unwrap_or_else(|| panic!("{s} in {lines:?}"));
    assert!(at("pre-commit says hi") < at("commit-msg checked"));
    assert!(at("stdin ended") < at("commit-msg checked"));
}

#[tokio::test]
async fn a_hook_that_says_no_leaves_no_commit_and_the_picked_files_staged() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    hook(&repo, "pre-commit", "echo 'lint failed: x' >&2\nexit 1");
    write(&repo, "tracked.txt", "b\n");
    write(&repo, "sub/s.txt", "other\n");
    let head = lab.head(&repo);
    let (spot, status) = lab.status(&repo).await;
    let out = Lines::new();
    let failed =
        lab.scm.commit(&spot, &status, &paths(&["tracked.txt"]), "m", None, &out, &Stop::new()).await;
    assert_eq!(reason(failed), Reason::Hook);
    assert!(out.lines().iter().any(|l| l == "lint failed: x"), "{:?}", out.lines());
    assert!(!out.lines().iter().any(|l| l.contains("nothing to commit")));
    assert_eq!(lab.head(&repo), head);
    assert_eq!(lab.staged(&repo), "tracked.txt");
    assert_eq!(fs::read_to_string(repo.join("sub/s.txt")).unwrap(), "other\n");
}

#[tokio::test]
async fn stop_kills_a_sleeping_hook_and_no_commit_is_made() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let child = lab.root.join("child.pid");
    hook(&repo, "pre-commit", &format!("sleep 300 &\necho $! > '{}'\nwait", child.display()));
    write(&repo, "tracked.txt", "b\n");
    let head = lab.head(&repo);
    let (spot, status) = lab.status(&repo).await;
    let out = Lines::new();
    let stop = Stop::new();
    let stopper = stop.clone();
    let pid_file = child.clone();
    tokio::spawn(async move {
        for _ in 0..200 {
            if pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        stopper.stop();
    });
    let started = std::time::Instant::now();
    let stopped = lab.scm.commit(&spot, &status, &paths(&["tracked.txt"]), "m", None, &out, &stop).await;
    assert_eq!(reason(stopped), Reason::Stopped);
    assert!(started.elapsed() < Duration::from_secs(30));
    assert_eq!(lab.head(&repo), head);
    let pid: i32 = fs::read_to_string(&child).unwrap().trim().parse().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let alive = Path::new(&format!("/proc/{pid}")).exists()
        && !fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default().contains(") Z ");
    assert!(!alive, "the hook's child was killed with the group");
}

#[tokio::test]
async fn reads_never_run_a_hook_a_monitor_an_external_diff_or_a_text_converter() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let marker = lab.root.join("ran");
    let m = marker.display();
    for name in
        ["pre-commit", "post-checkout", "post-index-change", "reference-transaction", "fsmonitor-watchman"]
    {
        hook(&repo, name, &format!("echo {name} >> '{m}'\nexit 1"));
    }
    let tool = lab.root.join("bin/tool");
    script(&tool, &format!("echo tool >> '{m}'\nexit 1"));
    let tool = tool.to_str().unwrap();
    lab.git(&repo, &["config", "core.fsmonitor", tool]);
    lab.git(&repo, &["config", "diff.external", tool]);
    lab.git(&repo, &["config", "diff.conv.textconv", tool]);
    lab.git(&repo, &["config", "log.showSignature", "true"]);
    lab.git(&repo, &["config", "gpg.program", tool]);
    write(&repo, ".gitattributes", "*.txt diff=conv\n");
    write(&repo, "tracked.txt", "b\n");
    write(&repo, "new.txt", "n\n");
    write(&repo, "dir/inner.txt", "i\n");

    let (spot, status) = lab.status(&repo).await;
    assert_eq!(status.total, 4);
    for path in ["tracked.txt", "new.txt", "dir/"] {
        lab.scm.patch(&spot, &status, path, 0).await.unwrap();
    }
    lab.scm.branches(&spot, None).await.unwrap();
    lab.scm.review(&spot, &status, &paths(&["tracked.txt", "new.txt", "dir/"])).await.unwrap();
    assert!(!marker.exists(), "ran: {}", fs::read_to_string(&marker).unwrap_or_default());
}

// ---- What a commit takes ----

#[tokio::test]
async fn a_commit_takes_only_the_ticked_files_and_leaves_the_users_staging_alone() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    write(&repo, "sub/b.txt", "b\n");
    write(&repo, "gone.txt", "g\n");
    write(&repo, "removed.txt", "r\n");
    lab.commit(&repo, "two");
    lab.git(&repo, &["rm", "-q", "removed.txt"]);
    write(&repo, "tracked.txt", "ticked\n");
    write(&repo, "mine.txt", "staged by the user\n");
    lab.git(&repo, &["add", "mine.txt"]);
    fs::remove_file(repo.join("gone.txt")).unwrap();
    lab.git(&repo, &["mv", "sub/b.txt", "sub/b2.txt"]);
    write(&repo, "docs/a.md", "a\n");
    write(&repo, "docs/b/c.md", "c\n");
    write(&repo, "untouched.txt", "u\n");
    let (spot, status) = lab.status(&repo).await;
    let out = Lines::new();
    let picks = paths(&["tracked.txt", "gone.txt", "removed.txt", "sub/b2.txt", "docs/"]);
    let made =
        lab.scm.commit(&spot, &status, &picks, "Ticked\n\nBody", None, &out, &Stop::new()).await.unwrap();
    assert_eq!(made.files, 5);
    let committed = lab.git(&repo, &["show", "--name-status", "--format=", "HEAD"]);
    let mut committed: Vec<&str> = committed.lines().collect();
    committed.sort();
    assert_eq!(
        committed,
        [
            "A\tdocs/a.md",
            "A\tdocs/b/c.md",
            "D\tgone.txt",
            "D\tremoved.txt",
            "M\ttracked.txt",
            "R100\tsub/b.txt\tsub/b2.txt"
        ]
    );
    assert_eq!(lab.staged(&repo), "mine.txt");
    assert_eq!(lab.git(&repo, &["log", "-1", "--format=%B"]), "Ticked\n\nBody");
    let status = lab.scm.status(&spot).await.unwrap();
    let mut left = files(&status);
    left.sort();
    assert_eq!(
        left,
        [("mine.txt".to_owned(), Change::Added), ("untouched.txt".to_owned(), Change::Untracked)]
    );
}

#[tokio::test]
async fn all_commits_the_folder_and_nothing_to_commit_runs_nothing() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let (bin, log) = recording_git(&lab);
    let scm = lab.scm_with(bin);
    let spot = scm.spot(&repo).await.unwrap();
    let clean = scm.status(&spot).await.unwrap();
    let out = Lines::new();
    let before = calls(&log).len();
    assert_eq!(
        reason(scm.commit(&spot, &clean, &Picks::Paths(vec![]), "m", None, &out, &Stop::new()).await),
        Reason::Nothing
    );
    assert_eq!(
        reason(scm.commit(&spot, &clean, &Picks::All, "m", None, &out, &Stop::new()).await),
        Reason::Nothing
    );
    assert_eq!(
        reason(scm.commit(&spot, &clean, &paths(&["x"]), "m", None, &out, &Stop::new()).await),
        Reason::NotFound
    );
    assert_eq!(
        reason(scm.commit(&spot, &clean, &Picks::All, "   ", None, &out, &Stop::new()).await),
        Reason::BadMessage
    );
    let later = calls(&log);
    assert!(later[before..].iter().all(|c| !c.starts_with("add ") && !c.starts_with("commit ")), "{later:?}");

    write(&repo, "a.txt", "a\n");
    write(&repo, "sub/s.txt", "s2\n");
    let status = scm.status(&spot).await.unwrap();
    let made = scm.commit(&spot, &status, &Picks::All, "Everything", None, &out, &Stop::new()).await.unwrap();
    assert_eq!(made.files, 2);
    assert_eq!(scm.status(&spot).await.unwrap().total, 0);
}

// ---- index.lock, identity, signing ----

#[tokio::test]
async fn a_held_index_lock_lets_status_run_and_refuses_every_act_before_git_runs() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    lab.git(&repo, &["branch", "other"]);
    write(&repo, "tracked.txt", "b\n");
    let (bin, log) = recording_git(&lab);
    let scm = lab.scm_with(bin);
    let spot = scm.spot(&repo).await.unwrap();
    fs::write(repo.join(".git/index.lock"), "").unwrap();
    let index = fs::read(repo.join(".git/index")).unwrap();
    let status = scm.status(&spot).await.unwrap();
    assert!(status.locked);
    let before = calls(&log).len();
    let out = Lines::new();
    let picks = paths(&["tracked.txt"]);
    assert_eq!(
        reason(scm.commit(&spot, &status, &picks, "m", None, &out, &Stop::new()).await),
        Reason::GitBusy
    );
    assert_eq!(
        reason(scm.switch(&spot, &SwitchTo::Local("other".into()), true, &out).await),
        Reason::GitBusy
    );
    let root = lab.root.join("data/worktrees/f1");
    assert_eq!(reason(scm.worktree_add(&spot, &root, "g7/x", None, &out).await), Reason::GitBusy);
    assert_eq!(reason(scm.push(&spot, None, &out, &Stop::new()).await), Reason::GitBusy);
    assert_eq!(calls(&log).len(), before, "nothing ran: {:?}", &calls(&log)[before..]);
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
}

#[tokio::test]
async fn a_lock_that_appears_after_the_checks_fails_the_commit_and_leaves_the_index() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    write(&repo, "tracked.txt", "b\n");
    let bin = lab.root.join("bin/git");
    let lock = repo.join(".git/index.lock");
    script(
        &bin,
        &format!("case \"$1\" in add) : > '{}';; esac\nexec '{}' \"$@\"", lock.display(), lab.real.display()),
    );
    let scm = lab.scm_with(bin);
    let spot = scm.spot(&repo).await.unwrap();
    let status = scm.status(&spot).await.unwrap();
    let index = fs::read(repo.join(".git/index")).unwrap();
    let out = Lines::new();
    let failed = scm.commit(&spot, &status, &paths(&["tracked.txt"]), "m", None, &out, &Stop::new()).await;
    assert_eq!(reason(failed), Reason::Failed);
    assert!(out.lines().iter().any(|l| l.contains("index.lock")), "{:?}", out.lines());
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
}

#[tokio::test]
async fn no_identity_is_refused_before_git_runs_and_a_failing_signer_is_gits_own_line() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let anon = lab.bare_repo("anon");
    write(&anon, "a.txt", "a\n");
    let (spot, status) = lab.status(&anon).await;
    let out = Lines::new();
    assert_eq!(
        reason(lab.scm.commit(&spot, &status, &paths(&["a.txt"]), "m", None, &out, &Stop::new()).await),
        Reason::Identity
    );
    assert!(out.lines().is_empty());

    lab.git(&repo, &["config", "commit.gpgsign", "true"]);
    lab.git(&repo, &["config", "gpg.program", "/bin/false"]);
    write(&repo, "tracked.txt", "b\n");
    let head = lab.head(&repo);
    let (spot, status) = lab.status(&repo).await;
    let failed =
        lab.scm.commit(&spot, &status, &paths(&["tracked.txt"]), "m", None, &out, &Stop::new()).await;
    assert_eq!(reason(failed), Reason::Failed);
    assert!(out.lines().iter().any(|l| l.contains("gpg failed to sign the data")), "{:?}", out.lines());
    assert_eq!(lab.head(&repo), head);
}

// ---- Review ----

#[tokio::test]
async fn a_review_pins_the_files_and_any_change_since_is_refused() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    write(&repo, "tracked.txt", "b\n");
    write(&repo, "docs/a.md", "a\n");
    let (spot, status) = lab.status(&repo).await;
    let picks = paths(&["tracked.txt", "docs/"]);
    let review = lab.scm.review(&spot, &status, &picks).await.unwrap();
    assert_eq!(review.id.len(), 16);
    assert_eq!(review.fingerprint, status.fingerprint);
    assert_eq!(review.files.len(), 2);
    let docs = review.files.iter().find(|f| f.path == "docs/").unwrap();
    assert!(docs.folder && docs.entries == 1 && docs.bytes == 2);
    assert_eq!(lab.scm.review(&spot, &status, &picks).await.unwrap(), review, "a review is stable");

    let out = Lines::new();
    let head = lab.head(&repo);
    for change in 0..3 {
        match change {
            0 => write(&repo, "tracked.txt", "edited\n"),
            1 => write(&repo, "docs/new.md", "n\n"),
            _ => {
                lab.git(&repo, &["commit", "-q", "--allow-empty", "-m", "moved"]);
            }
        }
        let head_now = lab.head(&repo);
        let refused = lab.scm.commit(&spot, &status, &picks, "m", Some(&review.id), &out, &Stop::new()).await;
        assert_eq!(reason(refused), Reason::Changed, "change {change}");
        assert_eq!(lab.head(&repo), head_now);
    }
    assert_ne!(lab.head(&repo), head);
    let status = lab.scm.status(&spot).await.unwrap();
    let fresh = lab.scm.review(&spot, &status, &picks).await.unwrap();
    lab.scm.commit(&spot, &status, &picks, "Reviewed", Some(&fresh.id), &out, &Stop::new()).await.unwrap();
}

// ---- Push and fetch ----

#[tokio::test]
async fn pushes_new_fast_forward_up_to_date_and_rejected() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let bare = lab.remote("remote.git");
    lab.git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    let spot = lab.spot(&repo).await;
    let out = Lines::new();
    assert!(lab.scm.push(&spot, None, &out, &Stop::new()).await.unwrap().set_upstream);
    lab.commit(&repo, "two");
    let pushed = lab.scm.push(&spot, None, &out, &Stop::new()).await.unwrap();
    assert!(!pushed.up_to_date && !pushed.set_upstream);
    assert!(lab.scm.push(&spot, None, &out, &Stop::new()).await.unwrap().up_to_date);

    let other = lab.root.join("other");
    lab.git(&lab.root, &["clone", "-q", bare.to_str().unwrap(), other.to_str().unwrap()]);
    lab.git(
        &other,
        &[
            "-c",
            "user.name=O",
            "-c",
            "user.email=o@localhost",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "theirs",
        ],
    );
    lab.git(&other, &["push", "-q", "origin", "main"]);
    lab.commit(&repo, "ours");
    let ours = lab.head(&repo);
    let theirs = lab.git(&bare, &["rev-parse", "main"]);
    assert_eq!(reason(lab.scm.push(&spot, None, &out, &Stop::new()).await), Reason::Behind);
    assert_eq!((lab.head(&repo), lab.git(&bare, &["rev-parse", "main"])), (ours, theirs));
}

#[tokio::test]
async fn a_refusing_remote_a_pre_push_hook_and_a_missing_remote_are_told_apart() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let bare = lab.remote("remote.git");
    lab.git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    let spot = lab.spot(&repo).await;
    script(&bare.join("hooks/pre-receive"), "echo 'policy: no pushes today' >&2\nexit 1");
    let out = Lines::new();
    assert_eq!(reason(lab.scm.push(&spot, None, &out, &Stop::new()).await), Reason::RemoteRefused);
    assert!(out.lines().iter().any(|l| l.contains("policy: no pushes today")), "{:?}", out.lines());
    fs::remove_file(bare.join("hooks/pre-receive")).unwrap();

    hook(&repo, "pre-push", "echo 'tests failed' >&2\nexit 1");
    let out = Lines::new();
    assert_eq!(reason(lab.scm.push(&spot, None, &out, &Stop::new()).await), Reason::Hook);
    fs::remove_file(repo.join(".git/hooks/pre-push")).unwrap();

    lab.git(&repo, &["remote", "set-url", "origin", lab.root.join("nowhere.git").to_str().unwrap()]);
    assert_eq!(reason(lab.scm.push(&spot, None, &Lines::new(), &Stop::new()).await), Reason::Failed);
}

#[tokio::test]
async fn remotes_are_chosen_or_asked_for_and_a_secret_in_a_url_never_shows() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let spot = lab.spot(&repo).await;
    let out = Lines::new();
    assert_eq!(reason(lab.scm.push(&spot, None, &out, &Stop::new()).await), Reason::NoRemote);
    let a = lab.remote("a.git");
    let b = lab.remote("b.git");
    lab.git(&repo, &["remote", "add", "alpha", a.to_str().unwrap()]);
    lab.git(&repo, &["remote", "add", "beta", b.to_str().unwrap()]);
    let asked = lab.scm.push(&spot, None, &out, &Stop::new()).await.unwrap_err();
    assert_eq!(
        (asked.reason, asked.lines),
        (Reason::PickRemote, vec!["alpha".to_owned(), "beta".to_owned()])
    );
    assert_eq!(lab.scm.push(&spot, Some("beta"), &out, &Stop::new()).await.unwrap().remote, "beta");
    assert_eq!(reason(lab.scm.push(&spot, Some("--mirror"), &out, &Stop::new()).await), Reason::NotFound);

    let secret = lab.repo("secret");
    lab.git(&secret, &["remote", "add", "origin", "http://kim:s3cret-word@127.0.0.1:9/x.git"]);
    let (spot, status) = lab.status(&secret).await;
    assert_eq!(status.remotes[0].label, "127.0.0.1/x");
    let out = Lines::new();
    let failed = lab.scm.push(&spot, None, &out, &Stop::new()).await.unwrap_err();
    assert!(matches!(failed.reason, Reason::Failed | Reason::Network | Reason::SignIn), "{failed:?}");
    let shown = format!("{:?} {:?} {:?}", out.lines(), failed, status);
    assert!(!shown.contains("s3cret-word") && !shown.contains("kim:"), "{shown}");
}

#[tokio::test]
async fn a_fetch_moves_behind_and_fetch_head_and_no_local_ref() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let bare = lab.remote("remote.git");
    lab.git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    let spot = lab.spot(&repo).await;
    lab.scm.push(&spot, None, &Lines::new(), &Stop::new()).await.unwrap();
    lab.scm.fetch(&spot, &Lines::new()).await.unwrap();
    let first = lab.scm.status(&spot).await.unwrap().fetched_ms.unwrap();

    let other = lab.root.join("other");
    lab.git(&lab.root, &["clone", "-q", bare.to_str().unwrap(), other.to_str().unwrap()]);
    lab.git(
        &other,
        &[
            "-c",
            "user.name=O",
            "-c",
            "user.email=o@localhost",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "theirs",
        ],
    );
    lab.git(&other, &["push", "-q", "origin", "main"]);
    let locals = lab.git(&repo, &["for-each-ref", "refs/heads", "refs/tags"]);
    std::thread::sleep(Duration::from_millis(20));
    let out = Lines::new();
    lab.scm.fetch(&spot, &out).await.unwrap();
    let status = lab.scm.status(&spot).await.unwrap();
    assert_eq!((status.ahead, status.behind), (Some(0), Some(1)));
    assert!(status.fetched_ms.unwrap() > first);
    assert_eq!(status.default_branch.as_deref(), Some("main"));
    assert!(status.on_default);
    assert_eq!(lab.git(&repo, &["for-each-ref", "refs/heads", "refs/tags"]), locals);
    assert!(out.lines().iter().all(|l| !l.contains(bare.to_str().unwrap())), "{:?}", out.lines());
}

// ---- Branches and switching ----

#[tokio::test]
async fn switching_carries_changes_only_when_asked_and_never_overwrites() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    lab.git(&repo, &["branch", "plain"]);
    lab.git(&repo, &["checkout", "-q", "-b", "clash"]);
    write(&repo, "tracked.txt", "on clash\n");
    lab.commit(&repo, "clash");
    lab.git(&repo, &["checkout", "-q", "main"]);
    let marker = lab.root.join("checked-out");
    hook(&repo, "post-checkout", &format!("echo 'post-checkout ran'\necho x >> '{}'", marker.display()));
    let spot = lab.spot(&repo).await;
    let out = Lines::new();

    assert_eq!(lab.scm.switch(&spot, &SwitchTo::Local("plain".into()), false, &out).await.unwrap(), "plain");
    assert!(out.lines().iter().any(|l| l == "post-checkout ran"));
    assert!(marker.exists());
    lab.scm.switch(&spot, &SwitchTo::Local("main".into()), false, &out).await.unwrap();

    write(&repo, "sub/s.txt", "carried\n");
    assert_eq!(
        reason(lab.scm.switch(&spot, &SwitchTo::Local("plain".into()), false, &out).await),
        Reason::Dirty
    );
    assert_eq!(lab.git(&repo, &["symbolic-ref", "--short", "HEAD"]), "main");
    lab.scm.switch(&spot, &SwitchTo::Local("plain".into()), true, &out).await.unwrap();
    assert_eq!(fs::read_to_string(repo.join("sub/s.txt")).unwrap(), "carried\n");
    lab.scm.switch(&spot, &SwitchTo::Local("main".into()), true, &out).await.unwrap();

    write(&repo, "tracked.txt", "mine\n");
    let out = Lines::new();
    assert_eq!(
        reason(lab.scm.switch(&spot, &SwitchTo::Local("clash".into()), true, &out).await),
        Reason::WouldOverwrite
    );
    assert!(out.lines().iter().any(|l| l.contains("tracked.txt")), "{:?}", out.lines());
    assert_eq!(lab.git(&repo, &["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(fs::read_to_string(repo.join("tracked.txt")).unwrap(), "mine\n");
}

#[tokio::test]
async fn names_are_looked_up_and_new_names_checked_before_git_runs() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    lab.git(&repo, &["branch", "taken"]);
    let (bin, log) = recording_git(&lab);
    let scm = lab.scm_with(bin);
    let spot = scm.spot(&repo).await.unwrap();
    scm.branches(&spot, None).await.unwrap();
    let before = calls(&log).len();
    let out = Lines::new();
    for name in ["missing", "--discard-changes", "-f", "origin/main"] {
        assert_eq!(
            reason(scm.switch(&spot, &SwitchTo::Local(name.into()), true, &out).await),
            Reason::NotFound,
            "{name}"
        );
        assert_eq!(
            reason(scm.switch(&spot, &SwitchTo::Track(name.into()), true, &out).await),
            Reason::NotFound,
            "{name}"
        );
    }
    let long = "a".repeat(101);
    for name in ["-x", "@", "a..b", "x.lock", "a b", "HEAD", long.as_str()] {
        assert_eq!(
            reason(scm.switch(&spot, &SwitchTo::New(name.into()), true, &out).await),
            Reason::BadName,
            "{name}"
        );
    }
    assert_eq!(reason(scm.switch(&spot, &SwitchTo::New("taken".into()), true, &out).await), Reason::Exists);
    let ran = calls(&log);
    assert!(ran[before..].iter().all(|c| !c.starts_with("switch")), "{:?}", &ran[before..]);
    assert_eq!(scm.switch(&spot, &SwitchTo::New("feat/new".into()), false, &out).await.unwrap(), "feat/new");
    let listed = scm.branches(&spot, Some("FEAT")).await.unwrap();
    assert_eq!(listed.branches.len(), 1);
    assert_eq!(listed.current.as_deref(), Some("feat/new"));
}

#[tokio::test]
async fn a_branch_open_in_another_worktree_is_listed_and_not_switched_to() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let other = lab.root.join("elsewhere");
    lab.git(&repo, &["worktree", "add", "-q", "-b", "busy", other.to_str().unwrap()]);
    let spot = lab.spot(&repo).await;
    let listed = lab.scm.branches(&spot, None).await.unwrap();
    let busy = listed.branches.iter().find(|b| b.name == "busy").unwrap();
    assert_eq!(busy.worktree.as_deref(), Some("elsewhere"));
    let out = Lines::new();
    assert_eq!(
        reason(lab.scm.switch(&spot, &SwitchTo::Local("busy".into()), false, &out).await),
        Reason::InWorktree
    );
}

#[tokio::test]
async fn a_remote_branch_is_tracked_into_a_local_one() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let bare = lab.remote("remote.git");
    lab.git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    lab.git(&repo, &["push", "-q", "origin", "main", "main:theirs"]);
    lab.git(&repo, &["fetch", "-q", "origin"]);
    let spot = lab.spot(&repo).await;
    let listed = lab.scm.branches(&spot, None).await.unwrap();
    assert!(
        listed.branches.iter().any(|b| b.name == "origin/theirs" && b.remote.as_deref() == Some("origin"))
    );
    let out = Lines::new();
    assert_eq!(
        lab.scm.switch(&spot, &SwitchTo::Track("origin/theirs".into()), false, &out).await.unwrap(),
        "theirs"
    );
    assert_eq!(lab.git(&repo, &["rev-parse", "--abbrev-ref", "theirs@{upstream}"]), "origin/theirs");
    assert_eq!(
        reason(lab.scm.switch(&spot, &SwitchTo::Track("origin/main".into()), false, &out).await),
        Reason::Exists
    );
}

// ---- Worktrees ----

#[tokio::test]
async fn worktrees_are_added_under_the_root_and_removed_only_when_clean() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    lab.git(&repo, &["branch", "base"]);
    let bare = lab.remote("remote.git");
    lab.git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    lab.git(&repo, &["push", "-q", "origin", "main"]);
    let spot = lab.spot(&repo).await;
    let root = lab.root.join("data/g7-agents/worktrees/f1");
    let out = Lines::new();
    let from_head = lab.scm.worktree_add(&spot, &root, "g7/x", None, &out).await.unwrap();
    let from_base = lab.scm.worktree_add(&spot, &root, "g7-x", Some("base"), &out).await.unwrap();
    assert_ne!(from_head, from_base);
    for path in [&from_head, &from_base] {
        assert!(path.starts_with(&root));
        assert!(path.file_name().unwrap().to_string_lossy().starts_with("g7-x-"));
        assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o700);
    }
    assert_eq!(fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(reason(lab.scm.worktree_add(&spot, &root, "g7/x", None, &out).await), Reason::Exists);
    assert_eq!(
        reason(lab.scm.worktree_add(&spot, &root, "g7/y", Some("nope"), &out).await),
        Reason::NotFound
    );
    assert_eq!(lab.git(&from_head, &["symbolic-ref", "--short", "HEAD"]), "g7/x");
    assert!(lab.try_git(&from_head, &["rev-parse", "--abbrev-ref", "g7/x@{upstream}"]).stdout.is_empty());

    let there = lab.spot(&from_head).await;
    assert!(there.linked());
    let status = lab.scm.status(&there).await.unwrap();
    assert_eq!(status.branch.as_deref(), Some("g7/x"));
    lab.git(
        &from_head,
        &["-c", "user.name=W", "-c", "user.email=w@localhost", "commit", "-q", "--allow-empty", "-m", "w1"],
    );
    lab.git(
        &from_head,
        &["-c", "user.name=W", "-c", "user.email=w@localhost", "commit", "-q", "--allow-empty", "-m", "w2"],
    );
    write(&from_head, "untracked.txt", "u\n");
    assert_eq!(reason(lab.scm.worktree_remove(&spot, &from_head).await), Reason::Dirty);
    assert!(from_head.join("untracked.txt").exists());
    fs::remove_file(from_head.join("untracked.txt")).unwrap();
    let listed = lab.scm.worktrees(&spot).await.unwrap();
    assert_eq!(listed.len(), 3);
    assert!(listed[0].main);
    let unpushed = lab.scm.worktree_remove(&spot, &from_head).await.unwrap();
    assert_eq!((unpushed.commits, unpushed.branch.as_deref()), (2, Some("g7/x")));
    assert!(!from_head.exists());
    assert_eq!(lab.git(&repo, &["rev-list", "--count", "main..g7/x"]), "2");
    assert_eq!(reason(lab.scm.worktree_remove(&spot, &from_head).await), Reason::NotFound);
    assert_eq!(reason(lab.scm.worktree_remove(&spot, &repo).await), Reason::NotFound);
}

// ---- Bounds ----

#[tokio::test]
async fn a_hook_printing_10000_long_lines_keeps_400_and_counts_the_rest() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    hook(
        &repo,
        "pre-commit",
        "i=0\nwhile [ $i -lt 10000 ]; do printf '%0600d\\n' $i; i=$((i+1)); done\nexit 1",
    );
    write(&repo, "tracked.txt", "b\n");
    let (spot, status) = lab.status(&repo).await;
    let out = Lines::new();
    let failed =
        lab.scm.commit(&spot, &status, &paths(&["tracked.txt"]), "m", None, &out, &Stop::new()).await;
    assert_eq!(reason(failed), Reason::Hook);
    let lines = out.lines();
    assert!(lines.len() <= MAX_JOB_LINES);
    assert!(lines.iter().map(String::len).sum::<usize>() <= MAX_JOB_BYTES);
    assert!(lines.iter().all(|l| l.chars().count() <= 512));
    assert_eq!(out.dropped() as usize + lines.len(), out.next() as usize);
    assert!(out.next() >= 10_000);
}

#[tokio::test]
async fn a_20_mib_patch_is_cut_and_flagged() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let line = "y".repeat(99) + "\n";
    fs::write(repo.join("big.txt"), line.repeat(200_000)).unwrap();
    lab.commit(&repo, "big");
    fs::write(repo.join("big.txt"), line.replace('y', "z").repeat(200_000)).unwrap();
    let (spot, status) = lab.status(&repo).await;
    let mut offset = 0;
    let mut whole = 0;
    loop {
        let page = lab.scm.patch(&spot, &status, "big.txt", offset).await.unwrap();
        assert!(page.truncated && !page.binary);
        whole += page.text.len();
        match page.next {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert!(whole <= g7_agents::api::MAX_PATCH_FILE && whole > 0);
}

// ---- A restore point inside a linked worktree is in tests/git.rs ----

#[tokio::test]
async fn restore_points_still_work_beside_source_control() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("r");
    let place =
        Place::new(lab.git.clone(), &git::checkpoints_dir(&lab.root.join("data")), "f1", &repo).unwrap();
    let pre = git::pre_ref("0123456789abcdef0123456789abcdef", 1).unwrap();
    assert!(matches!(git::capture(&place, &pre, git::PRE_BUDGET).await, Capture::Ready { .. }));
}

// ---- The quote chip's text form (§SC9.1): the fixtures Q and P implement against ----

/// The one text form of a quote chip: a line `path:first–last`, then the lines in a fence one
/// backtick longer than the longest run inside them (at least three), the text ending in a newline
/// before the closing fence.
fn quote_text(path: &str, first: u64, last: u64, text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat((longest + 1).max(3));
    let body = if text.ends_with('\n') { text.to_owned() } else { format!("{text}\n") };
    format!("{path}:{first}\u{2013}{last}\n{fence}\n{body}{fence}")
}

#[test]
fn the_six_quote_fixtures_follow_one_text_form() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scm/quote");
    let mut seen = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let fixture: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let quote = &fixture["quote"];
        let text = quote["text"].as_str().unwrap();
        assert!(text.chars().count() <= 2000 && quote["path"].as_str().unwrap().len() <= 512);
        let want = fixture["text"].as_str().unwrap();
        let got = quote_text(
            quote["path"].as_str().unwrap(),
            quote["first"].as_u64().unwrap(),
            quote["last"].as_u64().unwrap(),
            text,
        );
        assert_eq!(got, want, "{}", path.display());
        seen += 1;
    }
    assert_eq!(seen, 6);
}
