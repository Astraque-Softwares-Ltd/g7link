// SPDX-License-Identifier: MPL-2.0
//! Track G (§U5, §U11.1 G; reconcile §1-§4): restore points, diffs and
//! restores, every case on a throwaway repository this file makes in a
//! temporary folder, with `HOME`, `XDG_CONFIG_HOME` and `GIT_CONFIG_GLOBAL`
//! pointed into it so the user's own git configuration is never read.
#![cfg(feature = "host")]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::time::{Duration, Instant};

use g7_agents::api::{CheckpointWhy, FileStatus, MAX_PATCH_FILE, RewindAction, RewindBlock};
use g7_agents::host::git::{
    self, Blocked, Capture, Git, GitError, LeftWhy, PRE_BUDGET, Place, RestorePlan, apply_restore,
    delete_refs, patch, plan_restore, post_ref, pre_ref, thread_ids, turn_refs, turn_stats, undo_ref,
};

const THREAD: &str = "0123456789abcdef0123456789abcdef";
const OTHER: &str = "fedcba9876543210fedcba9876543210";
const MIB: u64 = 1 << 20;

// ---- The lab: a temporary home, git configuration and repositories ----

/// The path of `git`, or `None` (and a printed reason) when there is none.
/// CI's runners have git: there a missing one fails.
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
    real: PathBuf,
}

impl Lab {
    fn new() -> Option<Lab> {
        let real = real_git()?;
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for d in ["home", "xdg", "data"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::write(root.join("gitconfig"), "[init]\n\tdefaultBranch = main\n").unwrap();
        let env: Vec<(OsString, OsString)> = vec![
            ("PATH".into(), std::env::var_os("PATH").unwrap_or_default()),
            ("HOME".into(), root.join("home").into()),
            ("XDG_CONFIG_HOME".into(), root.join("xdg").into()),
            ("GIT_CONFIG_GLOBAL".into(), root.join("gitconfig").into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_CEILING_DIRECTORIES".into(), root.clone().into()),
        ];
        let git = Git::with_bin(real.clone(), &env);
        Some(Lab { _dir: dir, root, env, git, real })
    }

    fn checkpoints(&self) -> PathBuf {
        git::checkpoints_dir(&self.root.join("data"))
    }

    fn place(&self, folder: &Path) -> Place {
        self.place_with(self.git.clone(), folder)
    }

    fn place_with(&self, git: Arc<Git>, folder: &Path) -> Place {
        let id = format!("f{}", &fnv(folder.as_os_str().as_bytes())[..12]);
        Place::new(git, &self.checkpoints(), &id, folder).unwrap()
    }

    /// Git as the user would run it, in `cwd`, with the lab's configuration.
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

    /// A repository with one commit: `tracked.txt`, `sub/s.txt`, and a
    /// `.gitignore` that ignores `build/`.
    fn repo(&self, name: &str) -> PathBuf {
        let repo = self.root.join(name);
        fs::create_dir_all(&repo).unwrap();
        self.git(&repo, &["init", "-q"]);
        self.git(&repo, &["config", "user.name", "Lab"]);
        self.git(&repo, &["config", "user.email", "lab@localhost"]);
        write(&repo, "tracked.txt", "a\n");
        write(&repo, "sub/s.txt", "s\n");
        write(&repo, ".gitignore", "build/\n");
        self.commit(&repo, "one");
        repo
    }

    fn commit(&self, repo: &Path, message: &str) {
        self.git(repo, &["add", "-A"]);
        self.git(repo, &["commit", "-q", "--no-verify", "--allow-empty", "-m", message]);
    }

    /// `git ls-tree -r` of a restore point in G7's repository: path → blob id.
    fn tree(&self, place: &Place, rev: &str) -> BTreeMap<String, String> {
        let dir = format!("--git-dir={}", place.repo().display());
        let out = self.try_git(&self.root, &[&dir, "ls-tree", "-r", "-z", "--full-tree", rev]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        out.stdout
            .split(|&b| b == 0)
            .filter(|l| !l.is_empty())
            .map(|line| {
                let line = String::from_utf8_lossy(line);
                let (head, path) = line.split_once('\t').unwrap();
                (path.to_owned(), head.split(' ').nth(2).unwrap().to_owned())
            })
            .collect()
    }

    /// A blob's bytes from G7's repository.
    fn blob(&self, place: &Place, oid: &str) -> Vec<u8> {
        let dir = format!("--git-dir={}", place.repo().display());
        self.try_git(&self.root, &[&dir, "cat-file", "blob", oid]).stdout
    }

    /// The blob id git gives `body`.
    fn hash(&self, body: &str) -> String {
        let out = Command::new(&self.real)
            .args(["hash-object", "--stdin"])
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&self.root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(body.as_bytes())?;
                child.wait_with_output()
            })
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// A file of `len` bytes that takes no space (a hole).
fn sparse(root: &Path, rel: &str, len: u64) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::File::create(path).unwrap().set_len(len).unwrap();
}

fn ready(capture: Capture) -> (String, String, String, Vec<String>) {
    match capture {
        Capture::Ready { oid, tree, head, skipped } => (oid, tree, head, skipped),
        other => panic!("expected a restore point, got {other:?}"),
    }
}

/// Every entry under `dir`: its size, its content's hash (files), its link
/// text (links) and its mtime, except mtimes of objects and packs, which
/// git freshens (see `host/git/run.rs`).
fn listing(dir: &Path) -> BTreeMap<PathBuf, (u64, String, Option<i64>)> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, (u64, String, Option<i64>)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let rel = path.strip_prefix(root).unwrap().to_owned();
            let freshened = rel.starts_with("objects")
                && !rel.starts_with("objects/info")
                && meta.is_file()
                && (rel.components().count() == 3 || rel.starts_with("objects/pack"));
            let mtime = if freshened || meta.is_dir() {
                None
            } else {
                use std::os::unix::fs::MetadataExt;
                Some(meta.mtime() * 1_000_000_000 + meta.mtime_nsec())
            };
            let content = if meta.is_file() {
                fnv(&fs::read(&path).unwrap())
            } else if meta.is_symlink() {
                fs::read_link(&path).unwrap().display().to_string()
            } else {
                String::new()
            };
            out.insert(rel, (meta.len(), content, mtime));
            if meta.is_dir() {
                walk(root, &path, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn fnv(bytes: &[u8]) -> String {
    let h = bytes
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3));
    format!("{h:016x}")
}

/// What the user's repository looks like from the outside.
fn user_state(lab: &Lab, repo: &Path) -> (Vec<u8>, String, String, String, String) {
    (
        fs::read(repo.join(".git/index")).unwrap_or_default(),
        lab.try_git(repo, &["rev-parse", "-q", "--verify", "HEAD"]).stdout.escape_ascii().to_string(),
        lab.try_git(repo, &["symbolic-ref", "-q", "HEAD"]).stdout.escape_ascii().to_string(),
        lab.git(repo, &["stash", "list"]),
        lab.git(repo, &["status", "--porcelain=v2", "--branch", "--untracked-files=all"]),
    )
}

// ---- Capture leaves the user's state alone (§U11.1 G, reconcile §1) ----

#[tokio::test]
async fn capture_leaves_the_users_repository_alone_and_records_the_work_tree() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    write(&repo, "stashed.txt", "x\n");
    lab.git(&repo, &["add", "stashed.txt"]);
    lab.git(&repo, &["stash", "push", "-q", "-m", "kept"]);
    write(&repo, "tracked.txt", "a\nstaged\n");
    lab.git(&repo, &["add", "tracked.txt"]);
    write(&repo, "tracked.txt", "a\nstaged\nunstaged\n");
    write(&repo, "sub/new.txt", "untracked\n");
    write(&repo, "build/out.bin", "ignored\n");
    let state = user_state(&lab, &repo);
    let files = listing(&repo.join(".git"));

    let place = lab.place(&repo);
    let (oid, tree, head, skipped) =
        ready(git::capture(&place, &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);
    assert!(skipped.is_empty());
    assert!(head.starts_with("refs/heads/main ") && head.len() == "refs/heads/main ".len() + 40, "{head}");
    assert_eq!(user_state(&lab, &repo), state);
    assert_eq!(listing(&repo.join(".git")), files);

    let paths = lab.tree(&place, &oid);
    assert!(paths.contains_key("sub/new.txt"));
    assert!(!paths.contains_key("build/out.bin"));
    assert_eq!(lab.blob(&place, &paths["tracked.txt"]), b"a\nstaged\nunstaged\n");
    let whole = lab.git(
        &lab.root,
        &[&format!("--git-dir={}", place.repo().display()), "rev-parse", &format!("{oid}^{{tree}}")],
    );
    assert_eq!(tree, whole);
    assert_eq!(
        lab.git(
            &lab.root,
            &[&format!("--git-dir={}", place.repo().display()), "rev-parse", &pre_ref(THREAD, 1).unwrap()]
        ),
        oid
    );

    // Nothing changed: the same files make the same tree.
    let (_, again, _, _) = ready(git::capture(&place, &post_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);
    assert_eq!(again, tree);
    assert_eq!(listing(&repo.join(".git")), files);
}

#[tokio::test]
async fn a_sub_folder_capture_records_that_folder_from_the_top_level() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    let sub = repo.join("sub");
    let place = lab.place(&sub);
    let (a, tree_a, _, _) = ready(git::capture(&place, &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);
    write(&repo, "tracked.txt", "changed outside\n");
    write(&repo, "sub/s.txt", "changed inside\n");
    let (b, tree_b, _, _) = ready(git::capture(&place, &post_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);
    assert_ne!(tree_a, tree_b);
    let paths = lab.tree(&place, &b);
    assert_eq!(lab.blob(&place, &paths["sub/s.txt"]), b"changed inside\n");
    // Outside the folder the tree holds the index's version, not the work tree's.
    assert_eq!(lab.blob(&place, &paths["tracked.txt"]), b"a\n");
    let stats = turn_stats(&place, &a, &b).await.unwrap();
    let rows: Vec<(&str, FileStatus)> = stats.rows.iter().map(|r| (r.path.as_str(), r.status)).collect();
    assert_eq!(rows, vec![("s.txt", FileStatus::Modified)]);
    assert_eq!((stats.files, stats.adds, stats.dels), (1, 1, 1));
}

#[tokio::test]
async fn objects_the_user_has_are_borrowed_not_copied_and_nothing_can_be_pushed() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    let remote = lab.root.join("remote.git");
    lab.git(&lab.root, &["init", "-q", "--bare", remote.to_str().unwrap()]);
    lab.git(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    let place = lab.place(&repo);
    let (oid, _, _, _) = ready(git::capture(&place, &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);
    let blob = lab.git(&repo, &["rev-parse", "HEAD:tracked.txt"]);
    assert_eq!(lab.tree(&place, &oid)["tracked.txt"], blob);
    assert!(!place.repo().join("objects").join(&blob[..2]).join(&blob[2..]).exists());
    let alternates = fs::read_to_string(place.repo().join("objects/info/alternates")).unwrap();
    assert_eq!(alternates.trim(), repo.join(".git/objects").to_str().unwrap());
    assert_eq!(fs::metadata(place.repo()).unwrap().permissions().mode() & 0o777, 0o700);

    lab.git(&repo, &["push", "-q", "--mirror", "origin"]);
    let sent = lab.git(&remote, &["for-each-ref", "--format=%(refname)"]);
    assert!(sent.contains("refs/heads/main"));
    assert!(!sent.contains("g7"), "{sent}");
    assert!(lab.git(&repo, &["for-each-ref"]).lines().all(|l| !l.contains("g7")));
}

// ---- Hooks, signing, identity (§U11.1 G; reconcile §4) ----

const HOOKS: [&str; 7] = [
    "pre-commit",
    "commit-msg",
    "post-commit",
    "post-checkout",
    "reference-transaction",
    "pre-auto-gc",
    "post-index-change",
];

fn plant_hooks(dir: &Path, marker: &Path) {
    fs::create_dir_all(dir).unwrap();
    for hook in HOOKS {
        let path = dir.join(hook);
        fs::write(&path, format!("#!/bin/sh\necho {hook} >> '{}'\n", marker.display())).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[tokio::test]
async fn no_hook_of_the_users_runs_and_signing_or_identity_settings_dont_matter() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    let marker = lab.root.join("hook-ran");
    plant_hooks(&repo.join(".git/hooks"), &marker);
    plant_hooks(&lab.root.join("global-hooks"), &marker);
    fs::write(
        lab.root.join("gitconfig"),
        format!(
            "[init]\n\tdefaultBranch = main\n[core]\n\thooksPath = {}\n[commit]\n\tgpgsign = true\n[gpg]\n\tprogram = /bin/false\n[user]\n\tuseConfigOnly = true\n",
            lab.root.join("global-hooks").display()
        ),
    )
    .unwrap();
    // The repository's own identity goes: there is none anywhere now.
    lab.git(&repo, &["config", "--unset", "user.name"]);
    lab.git(&repo, &["config", "--unset", "user.email"]);
    write(&repo, "tracked.txt", "changed\n");
    let place = lab.place(&repo);
    let (a, _, _, _) = ready(git::capture(&place, &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);
    ready(git::capture(&place, &post_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);
    assert!(delete_refs(&place, &turn_refs(THREAD, 1).unwrap()).await.unwrap() == 2);
    assert!(!marker.exists(), "a hook ran: {:?}", fs::read_to_string(&marker));
    assert!(!a.is_empty());
    // The hooks are live: the user's own ref update runs one.
    lab.git(&repo, &["update-ref", "refs/heads/probe", "HEAD"]);
    assert!(fs::read_to_string(&marker).unwrap().contains("reference-transaction"));
}

// ---- Locks and work in progress (reconcile §4) ----

#[tokio::test]
async fn a_held_index_lock_doesnt_stop_a_capture_but_a_rebase_or_merge_does() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    let place = lab.place(&repo);
    fs::write(repo.join(".git/index.lock"), "someone else's").unwrap();
    ready(git::capture(&place, &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);
    assert_eq!(fs::read(repo.join(".git/index.lock")).unwrap(), b"someone else's");
    fs::remove_file(repo.join(".git/index.lock")).unwrap();

    // A rebase stopped on a conflict.
    lab.git(&repo, &["checkout", "-q", "-b", "side"]);
    write(&repo, "tracked.txt", "side\n");
    lab.commit(&repo, "side");
    lab.git(&repo, &["checkout", "-q", "main"]);
    write(&repo, "tracked.txt", "main\n");
    lab.commit(&repo, "main");
    let rebase = lab.try_git(&repo, &["rebase", "side"]);
    assert!(!rebase.status.success());
    assert!(repo.join(".git/rebase-merge").exists() || repo.join(".git/rebase-apply").exists());
    assert_eq!(
        git::capture(&place, &pre_ref(THREAD, 2).unwrap(), PRE_BUDGET).await,
        Capture::Missing(CheckpointWhy::Busy)
    );
    lab.git(&repo, &["rebase", "--abort"]);

    // A merge stopped on a conflict.
    assert!(!lab.try_git(&repo, &["merge", "-q", "side"]).status.success());
    assert!(repo.join(".git/MERGE_HEAD").exists());
    assert_eq!(
        git::capture(&place, &pre_ref(THREAD, 3).unwrap(), PRE_BUDGET).await,
        Capture::Missing(CheckpointWhy::Busy)
    );
    lab.git(&repo, &["merge", "--abort"]);
    ready(git::capture(&place, &pre_ref(THREAD, 4).unwrap(), PRE_BUDGET).await);
}

// ---- HEAD shapes ----

#[tokio::test]
async fn detached_unborn_and_moved_heads_are_recorded_as_they_are() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    let place = lab.place(&repo);
    let commit = lab.git(&repo, &["rev-parse", "HEAD"]);
    lab.git(&repo, &["checkout", "-q", "--detach"]);
    let (_, _, head, _) = ready(git::capture(&place, &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);
    assert_eq!(head, format!("detached {commit}"));
    lab.git(&repo, &["checkout", "-q", "main"]);

    // A branch switched and a commit made during the turn.
    let (pre, _, head_a, _) = ready(git::capture(&place, &pre_ref(THREAD, 2).unwrap(), PRE_BUDGET).await);
    lab.git(&repo, &["checkout", "-q", "-b", "agent"]);
    write(&repo, "committed.txt", "c\n");
    lab.commit(&repo, "agent");
    let (post, _, head_b, _) = ready(git::capture(&place, &post_ref(THREAD, 2).unwrap(), PRE_BUDGET).await);
    assert_ne!(head_a, head_b);
    assert!(head_b.starts_with("refs/heads/agent "));
    let stats = turn_stats(&place, &pre, &post).await.unwrap();
    assert_eq!(
        stats.rows.iter().map(|r| (r.path.as_str(), r.status)).collect::<Vec<_>>(),
        vec![("committed.txt", FileStatus::Added)]
    );

    // No commit at all, one file.
    let empty = lab.root.join("empty");
    fs::create_dir_all(&empty).unwrap();
    lab.git(&empty, &["init", "-q"]);
    write(&empty, "only.txt", "1\n");
    let place = lab.place(&empty);
    let (oid, _, head, _) = ready(git::capture(&place, &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);
    assert_eq!(head, "refs/heads/main unborn");
    assert_eq!(lab.tree(&place, &oid).keys().collect::<Vec<_>>(), vec!["only.txt"]);
}

// ---- Not git ----

#[tokio::test]
async fn a_folder_outside_git_or_refused_for_its_owner_or_gone_is_not_git() {
    let Some(lab) = Lab::new() else { return };
    let plain = lab.root.join("plain");
    write(&plain, "a.txt", "a\n");
    assert_eq!(
        git::capture(&lab.place(&plain), &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await,
        Capture::NotGit
    );
    // Cached: asked again at once, git isn't run (the answer is the same).
    assert_eq!(
        git::capture(&lab.place(&plain), &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await,
        Capture::NotGit
    );

    let repo = lab.repo("u");
    let mut env = lab.env.clone();
    env.push(("GIT_TEST_ASSUME_DIFFERENT_OWNER".into(), "1".into()));
    let other_owner = lab.place_with(Git::with_bin(lab.real.clone(), &env), &repo);
    assert_eq!(git::capture(&other_owner, &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await, Capture::NotGit);
    assert!(!other_owner.repo().exists());

    let gone = lab.root.join("gone");
    assert_eq!(
        git::capture(&lab.place(&gone), &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await,
        Capture::NotGit
    );
}

/// A `git` earlier on `PATH` that runs `script` before the real one.
fn shim(lab: &Lab, name: &str, script: &str) -> Arc<Git> {
    let dir = lab.root.join(name);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("git");
    fs::write(&path, format!("#!/bin/sh\n{script}\nexec '{}' \"$@\"\n", lab.real.display())).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    // A process another test forked while the script was open for writing
    // holds it until it execs, and running the script then fails as busy
    // (which the probe reads as "not git"). Once one run gets through, no
    // such process is left.
    for _ in 0..200 {
        match Command::new(&path).arg("--version").output() {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => panic!("the shim doesn't run: {e}"),
        }
    }
    let mut env = lab.env.clone();
    let path_var =
        std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&env[0].1))).unwrap();
    env[0].1 = path_var;
    let git = Git::new(&env).unwrap();
    assert_eq!(git.bin(), lab.root.join(name).join("git"));
    git
}

#[tokio::test]
async fn a_folder_removed_mid_capture_is_not_git_and_leaves_no_lock() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    write(&repo, "sub/new.txt", "n\n");
    let sub = repo.join("sub");
    let git =
        shim(&lab, "shim", &format!("case \" $* \" in *\" ls-files \"*) rm -rf '{}';; esac", sub.display()));
    let place = lab.place_with(git, &sub);
    assert_eq!(git::capture(&place, &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await, Capture::NotGit);
    assert!(no_locks(place.repo()));
}

fn no_locks(repo: &Path) -> bool {
    fn walk(dir: &Path) -> bool {
        fs::read_dir(dir).into_iter().flatten().flatten().all(|e| {
            let p = e.path();
            !p.to_string_lossy().ends_with(".lock") && (!p.is_dir() || walk(&p))
        })
    }
    walk(repo)
}

// ---- Size rules (reconcile §2) ----

#[tokio::test]
async fn large_new_and_changed_files_are_left_out_and_listed() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    sparse(&repo, "tracked-big.bin", 11 * MIB);
    lab.commit(&repo, "big");
    let place = lab.place(&repo);
    ready(git::capture(&place, &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);

    sparse(&repo, "new-big.bin", 11 * MIB);
    sparse(&repo, "tracked-big.bin", 11 * MIB + 1);
    write(&repo, "small.txt", "s\n");
    let (oid, _, _, mut skipped) =
        ready(git::capture(&place, &post_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);
    skipped.sort();
    assert_eq!(skipped, vec!["new-big.bin", "tracked-big.bin"]);
    let paths = lab.tree(&place, &oid);
    assert!(paths.contains_key("small.txt"));
    assert!(!paths.contains_key("new-big.bin"));
    let stats = turn_stats(&place, &pre_ref(THREAD, 1).unwrap(), &oid).await.unwrap();
    let rows: Vec<(&str, FileStatus)> = stats.rows.iter().map(|r| (r.path.as_str(), r.status)).collect();
    assert_eq!(
        rows,
        vec![
            ("small.txt", FileStatus::Added),
            ("new-big.bin", FileStatus::Unrecorded),
            ("tracked-big.bin", FileStatus::Unrecorded)
        ]
    );
    assert_eq!((stats.files, stats.skipped), (1, 2));
}

#[tokio::test]
async fn past_48_mib_the_largest_are_left_out_and_too_many_left_out_is_large() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    for name in ["a", "b", "c", "d", "e"] {
        sparse(&repo, &format!("nine/{name}.bin"), 9 * MIB);
    }
    sparse(&repo, "nine/five.bin", 5 * MIB);
    let place = lab.place(&repo);
    let (_, _, _, skipped) = ready(git::capture(&place, &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);
    assert_eq!(skipped, vec!["nine/e.bin"]);

    let many = lab.repo("many");
    for i in 0..=200 {
        sparse(&many, &format!("big/{i:03}.bin"), 11 * MIB);
    }
    let place = lab.place(&many);
    assert_eq!(
        git::capture(&place, &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await,
        Capture::Missing(CheckpointWhy::Large)
    );
    assert!(no_locks(place.repo()));
}

#[tokio::test]
async fn more_than_50000_new_files_is_large() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    for d in 0..=50 {
        let dir = repo.join(format!("gen/{d:02}"));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..1000 {
            if d * 1000 + f > 50_000 {
                break;
            }
            fs::File::create(dir.join(format!("{f:03}"))).unwrap();
        }
    }
    let place = lab.place(&repo);
    assert_eq!(
        git::capture(&place, &pre_ref(THREAD, 1).unwrap(), Duration::from_secs(20)).await,
        Capture::Missing(CheckpointWhy::Large)
    );
}

// ---- The time budget ----

#[tokio::test]
async fn a_git_past_the_budget_is_slow_its_process_group_killed_and_no_lock_left() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    write(&repo, "new.txt", "n\n");
    let pid_file = lab.root.join("sleeper.pid");
    let git = shim(
        &lab,
        "slow",
        &format!("case \" $* \" in *\" add \"*) sleep 30 & echo $! > '{}'; wait;; esac", pid_file.display()),
    );
    let place = lab.place_with(git, &repo);
    let started = Instant::now();
    let got = git::capture(&place, &pre_ref(THREAD, 1).unwrap(), Duration::from_secs(2)).await;
    assert_eq!(got, Capture::Missing(CheckpointWhy::Slow));
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    let pid = fs::read_to_string(&pid_file).unwrap();
    let proc = PathBuf::from(format!("/proc/{}", pid.trim()));
    let deadline = Instant::now() + Duration::from_secs(3);
    while proc.exists() && Instant::now() < deadline {
        let state = fs::read_to_string(proc.join("stat")).unwrap_or_default();
        if state.split(' ').nth(2) == Some("Z") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let state = fs::read_to_string(proc.join("stat")).unwrap_or_default();
    assert!(state.is_empty() || state.split(' ').nth(2) == Some("Z"), "the sleeper lives: {state}");
    assert!(no_locks(place.repo()));
    // The repository is still usable afterwards.
    ready(git::capture(&lab.place(&repo), &pre_ref(THREAD, 2).unwrap(), PRE_BUDGET).await);
}

// ---- Turn numbers ----

#[tokio::test]
async fn a_turns_numbers_cover_add_modify_delete_rename_mode_and_binary() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    write(&repo, "gone.txt", "bye\n");
    write(&repo, "moved.txt", "one\ntwo\nthree\nfour\nfive\nsix\n");
    write(&repo, "mode.sh", "echo\n");
    lab.commit(&repo, "base");
    let place = lab.place(&repo);
    let (pre, _, _, _) = ready(git::capture(&place, &pre_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);
    write(&repo, "tracked.txt", "a\nb\nc\n");
    fs::remove_file(repo.join("gone.txt")).unwrap();
    fs::rename(repo.join("moved.txt"), repo.join("renamed.txt")).unwrap();
    fs::set_permissions(repo.join("mode.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(repo.join("blob.bin"), b"\0\x01binary\0").unwrap();
    write(&repo, "with space.txt", "x\n");
    write(&repo, "-dash.txt", "x\n");
    write(&repo, "new\nline.txt", "x\n");
    fs::write(repo.join(std::ffi::OsStr::from_bytes(b"latin\xe9.txt")), "x\n").unwrap();
    let (post, _, _, _) = ready(git::capture(&place, &post_ref(THREAD, 1).unwrap(), PRE_BUDGET).await);
    let stats = turn_stats(&place, &pre, &post).await.unwrap();
    let rows: BTreeMap<&str, (FileStatus, Option<&str>, u32, u32, bool)> = stats
        .rows
        .iter()
        .map(|r| (r.path.as_str(), (r.status, r.from.as_deref(), r.adds, r.dels, r.binary)))
        .collect();
    assert_eq!(rows["tracked.txt"], (FileStatus::Modified, None, 2, 0, false));
    assert_eq!(rows["gone.txt"], (FileStatus::Deleted, None, 0, 1, false));
    assert_eq!(rows["renamed.txt"], (FileStatus::Renamed, Some("moved.txt"), 0, 0, false));
    assert_eq!(rows["mode.sh"], (FileStatus::Modified, None, 0, 0, false));
    assert_eq!(rows["blob.bin"], (FileStatus::Added, None, 0, 0, true));
    assert_eq!(rows["with space.txt"].0, FileStatus::Added);
    assert_eq!(rows["-dash.txt"].0, FileStatus::Added);
    assert_eq!(rows["new\nline.txt"].0, FileStatus::Added);
    assert_eq!(rows["latin\u{fffd}.txt"].0, FileStatus::Added);
    assert_eq!(stats.files, 9);
    assert!(!stats.more);
    // A mode change alone is a change of the file, nothing else.
    assert_eq!(lab.hash("echo\n"), lab.tree(&place, &post)["mode.sh"]);
}

// ---- Keeping and pruning (§U5.6) ----

#[tokio::test]
async fn deleting_refs_removes_those_and_no_others_and_a_gone_point_reads_gone() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    let place = lab.place(&repo);
    let files = listing(&repo.join(".git"));
    for thread in [THREAD, OTHER] {
        for turn in 1..=2 {
            ready(git::capture(&place, &pre_ref(thread, turn).unwrap(), PRE_BUDGET).await);
            write(&repo, "tracked.txt", &format!("{thread}{turn}\n"));
            ready(git::capture(&place, &post_ref(thread, turn).unwrap(), PRE_BUDGET).await);
        }
    }
    let mut both = vec![THREAD.to_owned(), OTHER.to_owned()];
    both.sort();
    assert_eq!(thread_ids(&place).await.unwrap(), both);
    assert_eq!(delete_refs(&place, &turn_refs(THREAD, 1).unwrap()).await.unwrap(), 2);
    assert!(matches!(
        turn_stats(&place, &pre_ref(THREAD, 1).unwrap(), &post_ref(THREAD, 1).unwrap()).await,
        Err(GitError::Gone)
    ));
    turn_stats(&place, &pre_ref(THREAD, 2).unwrap(), &post_ref(THREAD, 2).unwrap()).await.unwrap();
    assert_eq!(delete_refs(&place, &git::thread_refs(OTHER).unwrap()).await.unwrap(), 4);
    assert_eq!(thread_ids(&place).await.unwrap(), vec![THREAD.to_owned()]);
    assert_eq!(delete_refs(&place, &git::thread_refs(OTHER).unwrap()).await.unwrap(), 0);
    assert!(delete_refs(&place, "refs/heads/").await.is_err());
    git::gc(&place).await.unwrap();
    turn_stats(&place, &pre_ref(THREAD, 2).unwrap(), &post_ref(THREAD, 2).unwrap()).await.unwrap();
    // Prune and gc touch only G7's repository.
    assert_eq!(listing(&repo.join(".git")), files);
    assert!(git::store_bytes(&lab.checkpoints()) > 0);
    git::remove_repo(&place).await.unwrap();
    assert!(!place.repo().exists());
}

// ---- Never ----

#[test]
fn the_git_module_never_names_a_destructive_git_command() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/host/git");
    let dir = if dir.exists() {
        dir
    } else {
        Path::new(file!()).parent().unwrap().parent().unwrap().join("src/host/git")
    };
    let mut seen = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let text = fs::read_to_string(&path).unwrap();
        seen += 1;
        for word in [
            "\"clean\"",
            "\"reset\"",
            "\"checkout\"",
            "\"stash\"",
            "\"--force\"",
            "\"-f\"",
            "\"--staged\"",
            "\"--hard\"",
        ] {
            assert!(!text.contains(word), "{} names {word}", path.display());
        }
    }
    assert!(seen >= 2);
}

// ---- Diff (§U5.4) ----

#[tokio::test]
async fn a_patch_is_served_for_listed_paths_only_and_pages_reassemble() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    write(&repo, "moved.txt", &"line\n".repeat(20));
    let six = "0123456789abcdef".repeat(4) + "\n";
    write(&repo, "wide.txt", &six.repeat(100_000));
    lab.commit(&repo, "base");
    let place = lab.place(&repo);
    let pre = pre_ref(THREAD, 1).unwrap();
    let post = post_ref(THREAD, 1).unwrap();
    ready(git::capture(&place, &pre, PRE_BUDGET).await);
    write(&repo, "tracked.txt", "a\nadded\n");
    fs::rename(repo.join("moved.txt"), repo.join("renamed.txt")).unwrap();
    write(&repo, "renamed.txt", &("line\n".repeat(20) + "more\n"));
    fs::write(repo.join("blob.bin"), b"\0\x01\x02binary").unwrap();
    // A whole 6.5 MiB file rewritten: a 13 MiB patch.
    write(&repo, "wide.txt", &six.to_uppercase().repeat(100_000));
    for name in ["with space.txt", "-dash.txt", "new\nline.txt"] {
        write(&repo, name, "x\n");
    }
    fs::write(repo.join(std::ffi::OsStr::from_bytes(b"latin\xe9.txt")), "x\n").unwrap();
    sparse(&repo, "big.bin", 11 * MIB);
    ready(git::capture(&place, &post, PRE_BUDGET).await);

    let p = patch(&place, &pre, &post, "tracked.txt").await.unwrap();
    assert!(p.text.contains("@@ -1 +1,2 @@\n a\n+added\n"), "{}", p.text);
    assert!(!p.binary && !p.truncated && !p.unrecorded);
    let p = patch(&place, &pre, &post, "renamed.txt").await.unwrap();
    assert!(p.text.contains("rename from moved.txt") && p.text.contains("+more"), "{}", p.text);
    assert!(patch(&place, &pre, &post, "blob.bin").await.unwrap().binary);
    assert!(patch(&place, &pre, &post, "big.bin").await.unwrap().unrecorded);
    for name in ["with space.txt", "-dash.txt", "new\nline.txt", "latin\u{fffd}.txt"] {
        assert!(patch(&place, &pre, &post, name).await.unwrap().text.contains("+x\n"), "{name}");
    }
    for refused in ["sub/s.txt", "nothing.txt", "../tracked.txt", "/etc/passwd", "build/out.bin"] {
        assert!(matches!(patch(&place, &pre, &post, refused).await, Err(GitError::NotFound)), "{refused}");
    }

    let wide = patch(&place, &pre, &post, "wide.txt").await.unwrap();
    assert!(wide.truncated && wide.text.len() <= MAX_PATCH_FILE && wide.text.ends_with('\n'));
    let (mut offset, mut whole, mut pages) = (0, String::new(), 0);
    loop {
        let (page, next) = wide.page(offset).unwrap();
        assert!(page.ends_with('\n'));
        whole.push_str(page);
        pages += 1;
        match next {
            Some(n) => offset = n,
            None => break,
        }
    }
    assert_eq!(whole, wide.text);
    assert!(pages >= 5);
}

// ---- Restore (§U5.5; reconcile §2-§4) ----

/// A repository at a restore point (turn 1's *pre*), then changed by "the
/// agent" and recorded (turn 1's *post*).
struct Turned {
    lab: Lab,
    repo: PathBuf,
    place: Place,
    pre: String,
    post: String,
}

async fn turned(folder: Option<&str>, before: impl Fn(&Path), agent: impl Fn(&Path)) -> Option<Turned> {
    let lab = Lab::new()?;
    let repo = lab.repo("u");
    before(&repo);
    lab.commit(&repo, "base");
    let place = lab.place(&folder.map_or(repo.clone(), |f| repo.join(f)));
    let (pre, post) = (pre_ref(THREAD, 1).unwrap(), post_ref(THREAD, 1).unwrap());
    ready(git::capture(&place, &pre, PRE_BUDGET).await);
    agent(&repo);
    ready(git::capture(&place, &post, PRE_BUDGET).await);
    Some(Turned { lab, repo, place, pre, post })
}

fn actions(plan: &RestorePlan) -> BTreeMap<String, (RewindAction, bool)> {
    plan.files.iter().map(|f| (f.path.clone(), (f.action, f.later))).collect()
}

fn read(repo: &Path, rel: &str) -> Option<String> {
    fs::read_to_string(repo.join(rel)).ok()
}

#[tokio::test]
async fn a_rewind_puts_back_exactly_what_it_listed_and_undo_puts_back_the_rest() {
    let Some(t) = turned(
        None,
        |repo| {
            write(repo, "gone.txt", "was here\n");
            write(repo, "deep/keep/k.txt", "k\n");
            write(repo, "link-target.txt", "target\n");
            std::os::unix::fs::symlink("link-target.txt", repo.join("was-link")).unwrap();
        },
        |repo| {
            write(repo, "tracked.txt", "agent\n");
            fs::remove_file(repo.join("gone.txt")).unwrap();
            write(repo, "made/by/agent.txt", "new\n");
            write(repo, "build/out.bin", "ignored, never touched\n");
            fs::remove_file(repo.join("was-link")).unwrap();
            write(repo, "was-link", "now a file\n");
        },
    )
    .await
    else {
        return;
    };
    let (lab, repo, place) = (&t.lab, &t.repo, &t.place);
    // After the agent's last turn: the user edits, and a link to outside the folder appears.
    write(repo, "sub/s.txt", "the user's own edit\n");
    let outside = lab.root.join("outside");
    write(&outside, "precious.txt", "p\n");
    std::os::unix::fs::symlink(&outside, repo.join("made/link-out")).unwrap();
    let fifo = repo.join("made/fifo");
    assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let state = user_state(lab, repo);
    let files = listing(&repo.join(".git"));

    let plan = plan_restore(place, THREAD, &t.pre, Some(&t.post), &[]).await.unwrap();
    let want: BTreeMap<String, (RewindAction, bool)> = [
        ("tracked.txt", (RewindAction::Restore, false)),
        ("gone.txt", (RewindAction::BringBack, false)),
        ("made/by/agent.txt", (RewindAction::Delete, false)),
        ("was-link", (RewindAction::Restore, false)),
        ("sub/s.txt", (RewindAction::Restore, true)),
        ("made/link-out", (RewindAction::Delete, true)),
    ]
    .into_iter()
    .map(|(p, a)| (p.to_owned(), a))
    .collect();
    assert_eq!(actions(&plan), want);
    assert!(!plan.head_moved);
    let wire = plan.wire();
    assert_eq!((wire.change.len(), wire.more, wire.later, wire.skipped), (6, 0, 2, 0));
    assert_eq!(wire.tree.len(), 40);
    // A preview changes nothing.
    assert_eq!(read(repo, "tracked.txt").as_deref(), Some("agent\n"));
    assert_eq!(user_state(lab, repo), state);

    let left = apply_restore(place, &plan, false).await.unwrap();
    let left: BTreeMap<&str, LeftWhy> = left.iter().map(|l| (l.path.as_str(), l.why)).collect();
    assert_eq!(left, BTreeMap::from([("made/link-out", LeftWhy::Later), ("sub/s.txt", LeftWhy::Later)]));
    assert_eq!(read(repo, "tracked.txt").as_deref(), Some("a\n"));
    assert_eq!(read(repo, "gone.txt").as_deref(), Some("was here\n"));
    assert!(!repo.join("made/by").exists(), "emptied folders go");
    assert!(repo.join("made").is_dir(), "a folder that isn't empty stays");
    assert_eq!(fs::read_link(repo.join("was-link")).unwrap(), Path::new("link-target.txt"));
    assert_eq!(read(repo, "sub/s.txt").as_deref(), Some("the user's own edit\n"));
    assert_eq!(read(repo, "build/out.bin").as_deref(), Some("ignored, never touched\n"));
    assert!(fs::symlink_metadata(&fifo).unwrap().file_type().is_fifo_like());
    assert_eq!(read(&outside, "precious.txt").as_deref(), Some("p\n"));
    assert_eq!(read(repo, "deep/keep/k.txt").as_deref(), Some("k\n"));
    // The user's index, HEAD, branch and stash: untouched. Status shows the restored files.
    let after = user_state(lab, repo);
    assert_eq!((&after.0, &after.1, &after.2, &after.3), (&state.0, &state.1, &state.2, &state.3));
    assert_eq!(listing(&repo.join(".git")), files);

    // Undo: everything back, the `later` files included.
    let undo = plan_restore(place, THREAD, &undo_ref(THREAD).unwrap(), None, &[]).await.unwrap();
    assert!(apply_restore(place, &undo, true).await.unwrap().is_empty());
    assert_eq!(read(repo, "tracked.txt").as_deref(), Some("agent\n"));
    assert!(!repo.join("gone.txt").exists());
    assert_eq!(read(repo, "made/by/agent.txt").as_deref(), Some("new\n"));
    assert_eq!(read(repo, "was-link").as_deref(), Some("now a file\n"));
    assert!(fs::symlink_metadata(repo.join("was-link")).unwrap().is_file());
    assert_eq!(user_state(lab, repo), state);

    // A second rewind with the later ones included, then undo: every byte returns.
    let plan = plan_restore(place, THREAD, &t.pre, Some(&t.post), &[]).await.unwrap();
    assert!(apply_restore(place, &plan, true).await.unwrap().is_empty());
    assert_eq!(read(repo, "sub/s.txt").as_deref(), Some("s\n"));
    assert!(fs::symlink_metadata(repo.join("made/link-out")).is_err());
    assert_eq!(read(&outside, "precious.txt").as_deref(), Some("p\n"));
    let undo = plan_restore(place, THREAD, &undo_ref(THREAD).unwrap(), None, &[]).await.unwrap();
    assert!(apply_restore(place, &undo, true).await.unwrap().is_empty());
    assert_eq!(read(repo, "sub/s.txt").as_deref(), Some("the user's own edit\n"));
    assert_eq!(fs::read_link(repo.join("made/link-out")).unwrap(), outside);
    assert_eq!(user_state(lab, repo), state);
}

trait FifoLike {
    fn is_fifo_like(&self) -> bool;
}

impl FifoLike for fs::FileType {
    fn is_fifo_like(&self) -> bool {
        use std::os::unix::fs::FileTypeExt;
        self.is_fifo()
    }
}

#[tokio::test]
async fn a_sub_folder_rewind_never_touches_the_rest_of_the_repository() {
    let Some(t) = turned(
        Some("sub"),
        |_| {},
        |repo| {
            write(repo, "sub/s.txt", "agent\n");
            write(repo, "tracked.txt", "agent outside the folder\n");
            write(repo, "elsewhere.txt", "made outside the folder\n");
        },
    )
    .await
    else {
        return;
    };
    let plan = plan_restore(&t.place, THREAD, &t.pre, Some(&t.post), &[]).await.unwrap();
    assert_eq!(actions(&plan), BTreeMap::from([("s.txt".to_owned(), (RewindAction::Restore, false))]));
    assert!(apply_restore(&t.place, &plan, true).await.unwrap().is_empty());
    assert_eq!(read(&t.repo, "sub/s.txt").as_deref(), Some("s\n"));
    assert_eq!(read(&t.repo, "tracked.txt").as_deref(), Some("agent outside the folder\n"));
    assert_eq!(read(&t.repo, "elsewhere.txt").as_deref(), Some("made outside the folder\n"));
}

#[tokio::test]
async fn files_left_out_are_in_no_list_and_untouched_while_their_neighbours_go_back() {
    let Some(t) = turned(
        None,
        |repo| sparse(repo, "big.bin", 11 * MIB),
        |repo| {
            sparse(repo, "big.bin", 11 * MIB + 7);
            write(repo, "tracked.txt", "agent\n");
            sparse(repo, "new-big.bin", 12 * MIB);
            write(repo, "named.txt", "the caller says it was left out\n");
        },
    )
    .await
    else {
        return;
    };
    let plan =
        plan_restore(&t.place, THREAD, &t.pre, Some(&t.post), &["named.txt".to_owned()]).await.unwrap();
    assert_eq!(actions(&plan), BTreeMap::from([("tracked.txt".to_owned(), (RewindAction::Restore, false))]));
    assert_eq!(plan.wire().skipped, 3);
    assert!(apply_restore(&t.place, &plan, true).await.unwrap().is_empty());
    assert_eq!(read(&t.repo, "tracked.txt").as_deref(), Some("a\n"));
    assert_eq!(fs::metadata(t.repo.join("big.bin")).unwrap().len(), 11 * MIB + 7);
    assert_eq!(fs::metadata(t.repo.join("new-big.bin")).unwrap().len(), 12 * MIB);
    assert!(t.repo.join("named.txt").exists());
}

#[tokio::test]
async fn filters_round_trip_hooks_dont_run_and_a_moved_branch_is_said() {
    let Some(lab) = Lab::new() else { return };
    let marker = lab.root.join("hook-ran");
    fs::write(
        lab.root.join("gitconfig"),
        format!(
            "[init]\n\tdefaultBranch = main\n[filter \"up\"]\n\tclean = tr a-z A-Z\n\tsmudge = tr A-Z a-z\n[core]\n\thooksPath = {}\n",
            lab.root.join("global-hooks").display()
        ),
    )
    .unwrap();
    plant_hooks(&lab.root.join("global-hooks"), &marker);
    let repo = lab.repo("u");
    write(&repo, ".gitattributes", "*.up filter=up\n");
    write(&repo, "x.up", "hello\n");
    lab.commit(&repo, "filters");
    let place = lab.place(&repo);
    let (pre, post) = (pre_ref(THREAD, 1).unwrap(), post_ref(THREAD, 1).unwrap());
    let (oid, _, _, _) = ready(git::capture(&place, &pre, PRE_BUDGET).await);
    assert_eq!(lab.blob(&place, &lab.tree(&place, &oid)["x.up"]), b"HELLO\n");
    lab.git(&repo, &["checkout", "-q", "-b", "agent"]);
    write(&repo, "x.up", "changed\n");
    ready(git::capture(&place, &post, PRE_BUDGET).await);
    fs::remove_file(&marker).ok();

    let plan = plan_restore(&place, THREAD, &pre, Some(&post), &[]).await.unwrap();
    assert!(plan.head_moved);
    assert_eq!(actions(&plan), BTreeMap::from([("x.up".to_owned(), (RewindAction::Restore, false))]));
    assert!(apply_restore(&place, &plan, false).await.unwrap().is_empty());
    assert_eq!(read(&repo, "x.up").as_deref(), Some("hello\n"));
    assert!(!marker.exists(), "a hook ran: {:?}", fs::read_to_string(&marker));
    assert_eq!(lab.git(&repo, &["symbolic-ref", "HEAD"]), "refs/heads/agent");
}

#[tokio::test]
async fn files_changed_since_the_preview_stop_the_apply_before_anything_changes() {
    let Some(t) = turned(
        None,
        |_| {},
        |repo| {
            write(repo, "tracked.txt", "agent\n");
            write(repo, "made.txt", "agent\n");
        },
    )
    .await
    else {
        return;
    };
    let plan = plan_restore(&t.place, THREAD, &t.pre, Some(&t.post), &[]).await.unwrap();
    write(&t.repo, "made.txt", "edited after the preview\n");
    assert!(matches!(apply_restore(&t.place, &plan, true).await, Err(GitError::Changed)));
    assert_eq!(read(&t.repo, "tracked.txt").as_deref(), Some("agent\n"));
    assert_eq!(read(&t.repo, "made.txt").as_deref(), Some("edited after the preview\n"));
    // The undo ref is moved only by an apply that goes ahead.
    let dir = format!("--git-dir={}", t.place.repo().display());
    assert!(
        !t.lab
            .try_git(&t.lab.root, &[&dir, "rev-parse", "-q", "--verify", &undo_ref(THREAD).unwrap()])
            .status
            .success()
    );
}

#[tokio::test]
async fn paths_that_change_type_in_the_last_moment_are_left_and_nothing_is_followed() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    let outside = lab.root.join("outside");
    write(&outside, "precious.txt", "p\n");
    let place = lab.place(&repo);
    let (pre, post) = (pre_ref(THREAD, 1).unwrap(), post_ref(THREAD, 1).unwrap());
    ready(git::capture(&place, &pre, PRE_BUDGET).await);
    for name in ["del-dir.txt", "del-fifo.txt", "del-link.txt", "del-ok.txt", "parent/child.txt"] {
        write(&repo, name, "agent\n");
    }
    write(&repo, "tracked.txt", "agent\n");
    write(&repo, "sub/s.txt", "agent\n");
    ready(git::capture(&place, &post, PRE_BUDGET).await);
    let plan = plan_restore(&place, THREAD, &pre, Some(&post), &[]).await.unwrap();
    assert_eq!(plan.files.len(), 7);

    // Between the apply's last look at the tree and its checks of each path.
    let mark = lab.root.join("raced");
    let script = format!(
        "case \" $* \" in *\" update-ref refs/g7/\"*\"/undo \"*) if [ ! -e '{mark}' ]; then touch '{mark}'; cd '{repo}' && \
         rm del-dir.txt && mkdir del-dir.txt && echo keep > del-dir.txt/inner && \
         rm del-fifo.txt && mkfifo del-fifo.txt && \
         rm del-link.txt && ln -s '{outside}' del-link.txt && \
         echo sneaky > tracked.txt && \
         rm -r parent && ln -s '{outside}' parent; fi;; esac",
        mark = mark.display(),
        repo = repo.display(),
        outside = outside.display()
    );
    let racy = lab.place_with(shim(&lab, "racy", &script), &repo);
    let left = apply_restore(&racy, &plan, true).await.unwrap();
    let left: BTreeMap<&str, LeftWhy> = left.iter().map(|l| (l.path.as_str(), l.why)).collect();
    assert_eq!(
        left,
        BTreeMap::from([
            ("del-dir.txt", LeftWhy::Folder),
            ("del-fifo.txt", LeftWhy::NotFile),
            ("del-link.txt", LeftWhy::Changed),
            ("parent/child.txt", LeftWhy::NotFile),
            ("tracked.txt", LeftWhy::Changed),
        ])
    );
    assert!(mark.exists());
    assert_eq!(read(&repo, "del-dir.txt/inner").as_deref(), Some("keep\n"));
    assert!(fs::symlink_metadata(repo.join("del-link.txt")).unwrap().is_symlink());
    assert_eq!(read(&repo, "tracked.txt").as_deref(), Some("sneaky\n"));
    assert_eq!(read(&outside, "precious.txt").as_deref(), Some("p\n"));
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    assert!(!repo.join("del-ok.txt").exists());
    assert_eq!(read(&repo, "sub/s.txt").as_deref(), Some("s\n"));
}

#[tokio::test]
async fn a_busy_repository_blocks_the_preview_and_the_apply() {
    let Some(t) = turned(None, |_| {}, |repo| write(repo, "tracked.txt", "agent\n")).await else { return };
    let lock = t.repo.join(".git/index.lock");
    fs::write(&lock, "").unwrap();
    let started = Instant::now();
    assert_eq!(plan_restore(&t.place, THREAD, &t.pre, None, &[]).await.unwrap_err(), Blocked::Busy);
    assert!(started.elapsed() >= Duration::from_millis(1900));
    assert_eq!(Blocked::Busy.block(), RewindBlock::Busy);

    // A lock that goes while the preview waits.
    let gone = lock.clone();
    let remover = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        fs::remove_file(gone).unwrap();
    });
    let plan = plan_restore(&t.place, THREAD, &t.pre, None, &[]).await.unwrap();
    remover.await.unwrap();

    fs::write(&lock, "").unwrap();
    assert!(matches!(apply_restore(&t.place, &plan, true).await, Err(GitError::Busy)));
    assert_eq!(read(&t.repo, "tracked.txt").as_deref(), Some("agent\n"));
    assert!(lock.exists(), "G7 never removes the user's lock");
    fs::remove_file(&lock).unwrap();

    // A rebase stopped on a conflict.
    let (lab, repo) = (&t.lab, &t.repo);
    lab.git(repo, &["stash", "-q"]);
    lab.git(repo, &["checkout", "-q", "-b", "side"]);
    write(repo, "tracked.txt", "side\n");
    lab.commit(repo, "side");
    lab.git(repo, &["checkout", "-q", "main"]);
    write(repo, "tracked.txt", "main\n");
    lab.commit(repo, "main");
    assert!(!lab.try_git(repo, &["rebase", "side"]).status.success());
    assert_eq!(plan_restore(&t.place, THREAD, &t.pre, None, &[]).await.unwrap_err(), Blocked::Busy);
    assert!(matches!(apply_restore(&t.place, &plan, true).await, Err(GitError::Busy)));
}

#[tokio::test]
async fn objects_the_users_gc_pruned_make_the_restore_point_expired_never_a_partial_restore() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    lab.git(&repo, &["checkout", "-q", "-b", "feature"]);
    write(&repo, "feature.txt", "only on the feature branch\n");
    write(&repo, "tracked.txt", "feature version\n");
    lab.commit(&repo, "feature");
    let place = lab.place(&repo);
    let (pre, post) = (pre_ref(THREAD, 1).unwrap(), post_ref(THREAD, 1).unwrap());
    let (oid, _, _, _) = ready(git::capture(&place, &pre, PRE_BUDGET).await);
    let blob = lab.git(&repo, &["rev-parse", "HEAD:feature.txt"]);
    assert_eq!(lab.tree(&place, &oid)["feature.txt"], blob);
    write(&repo, "feature.txt", "the agent's\n");
    ready(git::capture(&place, &post, PRE_BUDGET).await);
    let plan = plan_restore(&place, THREAD, &pre, Some(&post), &[]).await.unwrap();

    // The user drops the branch and prunes.
    fs::remove_file(repo.join("feature.txt")).unwrap();
    lab.git(&repo, &["checkout", "-q", "-f", "main"]);
    lab.git(&repo, &["branch", "-q", "-D", "feature"]);
    lab.git(&repo, &["reflog", "expire", "--expire=now", "--all"]);
    lab.git(&repo, &["gc", "-q", "--prune=now"]);
    assert!(!lab.try_git(&repo, &["cat-file", "-e", &blob]).status.success());
    write(&repo, "feature.txt", "the agent's\n");

    assert!(matches!(apply_restore(&place, &plan, true).await, Err(GitError::Gone)));
    assert_eq!(read(&repo, "feature.txt").as_deref(), Some("the agent's\n"));
    assert_eq!(read(&repo, "tracked.txt").as_deref(), Some("a\n"));
    assert_eq!(plan_restore(&place, THREAD, &pre, Some(&post), &[]).await.unwrap_err(), Blocked::Expired);
    assert_eq!(Blocked::Expired.block(), RewindBlock::Expired);
    // Refs deleted by hand: expired too.
    delete_refs(&place, &turn_refs(THREAD, 1).unwrap()).await.unwrap();
    assert_eq!(plan_restore(&place, THREAD, &pre, None, &[]).await.unwrap_err(), Blocked::Expired);
}

#[tokio::test]
async fn names_that_leave_the_folder_in_a_crafted_restore_point_are_never_listed() {
    let Some(t) = turned(None, |_| {}, |repo| write(repo, "tracked.txt", "agent\n")).await else { return };
    let (lab, place) = (&t.lab, &t.place);
    let dir = format!("--git-dir={}", place.repo().display());
    let blob = lab.hash("evil\n");
    let run = |args: &[&str], input: &[u8]| {
        use std::io::Write;
        let mut child = Command::new(&lab.real)
            .args(args)
            .env_clear()
            .envs(lab.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&lab.root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    run(&[&dir, "hash-object", "-w", "--stdin"], b"evil\n");
    let raw = |hex: &str| {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect::<Vec<u8>>()
    };
    let mut tree = Vec::new();
    for name in ["../evil", "/abs"] {
        tree.extend_from_slice(format!("100644 {name}\0").as_bytes());
        tree.extend_from_slice(&raw(&blob));
    }
    let tree = run(&[&dir, "hash-object", "-t", "tree", "-w", "--stdin", "--literally"], &tree);
    let message = format!(
        "G7 Agents restore point\n\ng7-top {}\ng7-prefix \ng7-head x\n",
        t.repo.as_os_str().as_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let commit =
        run(&[&dir, "-c", "user.name=x", "-c", "user.email=x@x", "commit-tree", &tree], message.as_bytes());
    let plan = plan_restore(place, THREAD, &commit, None, &[]).await.unwrap();
    assert!(
        plan.files.iter().all(|f| !f.path.contains("evil") && !f.path.contains("abs")),
        "{:?}",
        actions(&plan)
    );
    assert_eq!(plan.hidden, 2);
}

#[tokio::test]
async fn a_file_that_changes_type_while_it_is_hashed_is_left_and_the_rest_still_go_back() {
    let Some(lab) = Lab::new() else { return };
    let repo = lab.repo("u");
    let place = lab.place(&repo);
    let (pre, post) = (pre_ref(THREAD, 1).unwrap(), post_ref(THREAD, 1).unwrap());
    ready(git::capture(&place, &pre, PRE_BUDGET).await);
    for name in ["a.txt", "b.txt", "c.txt"] {
        write(&repo, name, "agent\n");
    }
    write(&repo, "tracked.txt", "agent\n");
    ready(git::capture(&place, &post, PRE_BUDGET).await);
    let plan = plan_restore(&place, THREAD, &pre, Some(&post), &[]).await.unwrap();
    assert_eq!(plan.files.len(), 4);
    // `b.txt` becomes a folder after it was looked at, before git hashes it.
    let mark = lab.root.join("raced");
    let script = format!(
        "case \" $* \" in *\" hash-object -- \"*) if [ ! -e '{mark}' ]; then touch '{mark}'; cd '{repo}' && rm b.txt && mkdir b.txt; fi;; esac",
        mark = mark.display(),
        repo = repo.display()
    );
    let racy = lab.place_with(shim(&lab, "racy", &script), &repo);
    let left = apply_restore(&racy, &plan, true).await.unwrap();
    assert_eq!(
        left.iter().map(|l| (l.path.as_str(), l.why)).collect::<Vec<_>>(),
        vec![("b.txt", LeftWhy::Changed)]
    );
    assert!(repo.join("b.txt").is_dir());
    assert!(!repo.join("a.txt").exists() && !repo.join("c.txt").exists());
    assert_eq!(read(&repo, "tracked.txt").as_deref(), Some("a\n"));
}
