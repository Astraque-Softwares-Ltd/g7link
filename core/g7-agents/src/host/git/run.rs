// SPDX-License-Identifier: MPL-2.0
//! How git is run (§U5.1). The `git` on the user's `PATH`, resolved once to
//! an absolute path; argv only, never a shell; the user's environment with
//! prompts, optional locks and the variables that would point git somewhere
//! else taken out; every call bounded in time (the process group is killed)
//! and in output; at most two git processes in the host at once, and one
//! operation per repository (see [`Git::lock`]).
//!
//! Two git behaviours the rest of this module is built around:
//!
//! - **Freshening.** When git is about to write an object that already
//!   exists, in G7's repository or in an alternate (the user's object
//!   directory), it updates that file's mtime instead (`freshen_object`): a
//!   loose object's file, or the whole pack holding it. So a capture can move
//!   the mtimes of objects and packs the user's repository already has. It
//!   adds, removes and rewrites nothing there; it is what the user's own
//!   `git add` does, and no option turns it off.
//! - **`git restore` clears the way.** Restoring a path where a directory now
//!   stands deletes that directory and everything in it, untracked files
//!   included, and a file where a parent directory is needed is unlinked,
//!   `--overlay` or not. [`super::restore`] therefore checks every path's
//!   parents itself before git is asked to write it.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{OwnedMutexGuard, Semaphore};

/// Standard output kept from one call, and from a patch.
pub const MAX_OUT: usize = 4 << 20;
pub const MAX_PATCH_OUT: usize = 10 << 20;
/// Standard error kept from one call. It is logged, never sent to a client.
pub const MAX_ERR: usize = 16 << 10;
/// Git processes the host runs at once.
const SLOTS: usize = 2;
/// A folder found not to be in a git work tree is not asked again for this long.
const NOT_GIT_FOR: Duration = Duration::from_secs(60);
const NOT_GIT_KEPT: usize = 256;
/// After a kill, how long the reap may take before the child is left to tokio.
const REAP_GRACE: Duration = Duration::from_secs(2);

/// Taken out of the environment git inherits: they would point it at another
/// repository, index or object store, or change how pathspecs and diffs read.
const STRIP: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_NAMESPACE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_PREFIX",
    "GIT_QUARANTINE_PATH",
    "GIT_SHALLOW_FILE",
    "GIT_REPLACE_REF_BASE",
    "GIT_LITERAL_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
    "GIT_EXTERNAL_DIFF",
    "GIT_DIFF_OPTS",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
];

/// Added to the environment of every call.
const ADD: &[(&str, &str)] = &[
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_ASKPASS", ""),
    ("SSH_ASKPASS_REQUIRE", "never"),
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_ADVICE", "0"),
    ("LC_ALL", "C"),
];

/// `-c` options of every call. `core.hooksPath=/dev/null` keeps every hook,
/// the global hooks path's included, from running.
const CONFIG: &[&str] = &[
    "core.fsmonitor=false",
    "core.untrackedCache=false",
    "core.quotePath=false",
    "core.hooksPath=/dev/null",
    "core.logAllRefUpdates=false",
    "core.splitIndex=false",
    "gc.auto=0",
    "maintenance.auto=false",
    "commit.gpgSign=false",
];

/// Why a git operation didn't happen. The text is for the host's log; a
/// client gets a reason word chosen by the caller.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("the folder is not in a git work tree")]
    NotGit,
    #[error("git is in the middle of something in the folder")]
    Busy,
    #[error("git took too long")]
    Slow,
    #[error("too many or too large files")]
    Large,
    #[error("the restore point is no longer kept")]
    Gone,
    #[error("the files changed since the preview")]
    Changed,
    #[error("the folder's repository is not the one recorded")]
    Moved,
    #[error("no such path in this diff")]
    NotFound,
    #[error("git failed: {0}")]
    Failed(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// The git binary and what the host allows of it. One per host; cheap to share.
#[derive(Debug)]
pub struct Git {
    bin: PathBuf,
    env: Vec<(OsString, OsString)>,
    slots: Semaphore,
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    not_git: Mutex<HashMap<PathBuf, Instant>>,
}

impl Git {
    /// The `git` on `env`'s `PATH`; `None` when there is none.
    pub fn new(env: &[(OsString, OsString)]) -> Option<Arc<Git>> {
        let path = env.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v)?;
        let bin = std::env::split_paths(path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join("git"))
            .find(|bin| is_executable(bin))?;
        Some(Self::with_bin(bin, env))
    }

    /// A given binary, with `env` as the environment it starts from.
    pub fn with_bin(bin: PathBuf, env: &[(OsString, OsString)]) -> Arc<Git> {
        let mut env: Vec<(OsString, OsString)> = env
            .iter()
            .filter(|(k, _)| !STRIP.iter().any(|s| k == s) && !ADD.iter().any(|(a, _)| k == a))
            .cloned()
            .collect();
        env.extend(ADD.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))));
        Arc::new(Git {
            bin,
            env,
            slots: Semaphore::new(SLOTS),
            locks: Mutex::default(),
            not_git: Mutex::default(),
        })
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// One operation per key at a time (a repository's top level, or G7's
    /// repository), waited for until `deadline`.
    pub(crate) async fn lock(&self, key: &Path, deadline: Instant) -> Result<OwnedMutexGuard<()>, GitError> {
        let lock = {
            let mut locks = self.locks.lock().unwrap_or_else(PoisonError::into_inner);
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
            locks.entry(key.to_owned()).or_default().clone()
        };
        tokio::time::timeout_at(deadline.into(), lock.lock_owned()).await.map_err(|_| GitError::Slow)
    }

    pub(crate) fn recently_not_git(&self, folder: &Path) -> bool {
        let seen = self.not_git.lock().unwrap_or_else(PoisonError::into_inner);
        seen.get(folder).is_some_and(|at| at.elapsed() < NOT_GIT_FOR)
    }

    pub(crate) fn set_not_git(&self, folder: &Path) {
        let mut seen = self.not_git.lock().unwrap_or_else(PoisonError::into_inner);
        seen.retain(|_, at| at.elapsed() < NOT_GIT_FOR);
        if seen.len() < NOT_GIT_KEPT {
            seen.insert(folder.to_owned(), Instant::now());
        }
    }

    /// Runs one git command. Fails with [`GitError::Slow`] past the call's
    /// limit or `deadline`, whichever is first (the process group is killed
    /// first), and with [`GitError::Io`] when git can't be started. A
    /// non-zero exit is not an error here: the caller reads [`Out::code`].
    pub(crate) async fn run(&self, call: Call, deadline: Instant) -> Result<Out, GitError> {
        let now = Instant::now();
        let limit = call.limit.min(deadline.saturating_duration_since(now));
        if limit.is_zero() {
            return Err(GitError::Slow);
        }
        let until = tokio::time::Instant::from_std(now + limit);
        let _slot = tokio::time::timeout_at(until, self.slots.acquire())
            .await
            .map_err(|_| GitError::Slow)?
            .map_err(|_| GitError::Slow)?;

        let mut cmd = Command::new(&self.bin);
        for config in CONFIG {
            cmd.arg("-c").arg(config);
        }
        cmd.args(&call.args)
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .envs(call.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&call.cwd)
            .stdin(if call.input.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        let mut child = cmd.spawn()?;
        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let input = call.input;
        let cap = call.cap;
        let io = async move {
            let feed = async move {
                if let (Some(mut w), Some(bytes)) = (stdin, input) {
                    // A child that exits without reading it all closes the pipe: not an error.
                    let _ = w.write_all(&bytes).await;
                    let _ = w.shutdown().await;
                }
            };
            let (_, out, err) =
                tokio::join!(feed, read_capped(stdout, cap, Some(pid)), read_capped(stderr, MAX_ERR, None));
            (out, err)
        };
        // Every path below signals the group before `wait` has reaped the
        // child, so its id (the group's) can't have been reused.
        let ((stdout, over), (stderr, _)) = match tokio::time::timeout_at(until, io).await {
            Ok(read) => read,
            Err(_) => {
                kill_group(pid);
                reap(&mut child).await;
                return Err(GitError::Slow);
            }
        };
        let status = match tokio::time::timeout_at(until, child.wait()).await {
            Ok(status) => status?,
            Err(_) => {
                kill_group(pid);
                reap(&mut child).await;
                return Err(GitError::Slow);
            }
        };
        Ok(Out { code: status.code(), stdout, over, stderr: String::from_utf8_lossy(&stderr).into_owned() })
    }
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn kill_group(pid: Option<u32>) {
    let Some(pid) = pid.and_then(|id| i32::try_from(id).ok()).and_then(rustix::process::Pid::from_raw) else {
        return;
    };
    let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
}

async fn reap(child: &mut tokio::process::Child) {
    // Killed with SIGKILL: this is quick unless the child is stuck in the
    // kernel; then `kill_on_drop` and tokio's reaper take it from here.
    let _ = tokio::time::timeout(REAP_GRACE, child.wait()).await;
}

/// Reads `from` to its end, keeping at most `cap` bytes. `kill`: past the
/// cap, stop reading and kill that process group (it may be blocked on a
/// full pipe); otherwise keep reading and drop the rest. Returns the bytes
/// and whether there were more.
async fn read_capped(
    from: Option<impl AsyncRead + Unpin>,
    cap: usize,
    kill: Option<Option<u32>>,
) -> (Vec<u8>, bool) {
    let Some(mut from) = from else { return (Vec::new(), false) };
    let mut kept = Vec::new();
    let mut chunk = vec![0u8; 64 << 10];
    let mut over = false;
    loop {
        match from.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = cap - kept.len();
                kept.extend_from_slice(&chunk[..n.min(room)]);
                if n > room {
                    over = true;
                    if let Some(pid) = kill {
                        kill_group(pid);
                        break;
                    }
                }
            }
        }
    }
    (kept, over)
}

/// One git command: its arguments after the `-c` options, its directory, the
/// variables it adds, what it reads on stdin, its time limit and the output kept.
#[derive(Debug)]
pub(crate) struct Call {
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: Vec<(&'static str, OsString)>,
    pub input: Option<Vec<u8>>,
    pub limit: Duration,
    pub cap: usize,
}

impl Call {
    pub fn new(cwd: &Path, limit: Duration) -> Call {
        Call { args: Vec::new(), cwd: cwd.to_owned(), env: Vec::new(), input: None, limit, cap: MAX_OUT }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Call {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(mut self, args: I) -> Call {
        self.args.extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    pub fn env(mut self, key: &'static str, value: impl AsRef<OsStr>) -> Call {
        self.env.push((key, value.as_ref().to_owned()));
        self
    }

    pub fn input(mut self, bytes: Vec<u8>) -> Call {
        self.input = Some(bytes);
        self
    }

    pub fn cap(mut self, cap: usize) -> Call {
        self.cap = cap;
        self
    }
}

/// What a call printed and how it exited.
#[derive(Debug)]
pub(crate) struct Out {
    /// `None`: killed by a signal.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    /// Standard output passed the call's cap: `stdout` is its start.
    pub over: bool,
    pub stderr: String,
}

impl Out {
    pub fn ok(&self) -> bool {
        self.code == Some(0) && !self.over
    }

    /// The first line of standard error, short, for the log.
    pub fn why(&self) -> String {
        let line = self.stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        let short: String = line.chars().take(200).collect();
        format!("exit {:?}: {short}", self.code)
    }

    /// Standard output as one trimmed line.
    pub fn line(&self) -> &[u8] {
        self.stdout.strip_suffix(b"\n").unwrap_or(&self.stdout)
    }

    /// Fails with the first line of standard error unless the call succeeded.
    pub fn check(self) -> Result<Out, GitError> {
        if self.ok() { Ok(self) } else { Err(GitError::Failed(self.why())) }
    }
}
