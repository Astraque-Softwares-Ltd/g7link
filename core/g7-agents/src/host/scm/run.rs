// SPDX-License-Identifier: MPL-2.0
//! How source control runs its commands (§SC3.1, §SC4.1, §SC5.2): git
//! through [`Git::spawn`](crate::host::git::Git) with an [`Op`] from the
//! closed list, `gh` through [`run_program`]; and the two things a job
//! hands in, [`Lines`] (its bounded output) and [`Stop`].

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{Notify, watch};

use super::argv::{Op, argv};
use super::clean::{self, MAX_LINE_CHARS};
use super::{Reason, Scm, ScmError};
use crate::host::git::GitError;
use crate::host::git::run::{Call, LineSink, MAX_ERR, Out, Profile};

/// Lines a job keeps (§SC5.2), oldest dropped first.
pub const MAX_JOB_LINES: usize = 400;
/// Bytes of lines a job keeps.
pub const MAX_JOB_BYTES: usize = 64 << 10;
/// Bytes of one line read from a program before the rest is dropped.
const READ_LINE: usize = 4096;
/// After a kill, how long the reap may take.
const REAP_GRACE: Duration = Duration::from_secs(2);

// ---- A job's output ----

#[derive(Debug, Default)]
struct Kept {
    lines: VecDeque<String>,
    bytes: usize,
    dropped: u32,
    /// Lines ever pushed: the next line's number.
    next: u64,
}

/// A job's output: at most [`MAX_JOB_LINES`] lines and [`MAX_JOB_BYTES`],
/// each at most 512 characters, the oldest dropped and counted. Lines are
/// numbered from 0 in the order they arrived, so a reader can ask for what
/// is new since the last line it saw. Cheap to clone; clones share.
#[derive(Debug, Clone, Default)]
pub struct Lines {
    kept: Arc<Mutex<Kept>>,
    notify: Arc<Notify>,
}

impl Lines {
    pub fn new() -> Lines {
        Lines::default()
    }

    /// Adds one line (already cleaned; cut again to 512 characters).
    pub fn push(&self, line: &str) {
        let line = match line.char_indices().nth(MAX_LINE_CHARS) {
            Some((end, _)) => &line[..end],
            None => line,
        };
        {
            let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
            kept.bytes += line.len();
            kept.lines.push_back(line.to_owned());
            kept.next += 1;
            while kept.lines.len() > MAX_JOB_LINES || kept.bytes > MAX_JOB_BYTES {
                let Some(old) = kept.lines.pop_front() else { break };
                kept.bytes -= old.len();
                kept.dropped = kept.dropped.saturating_add(1);
            }
        }
        self.notify.notify_waiters();
    }

    /// The lines kept, oldest first.
    pub fn lines(&self) -> Vec<String> {
        self.kept.lock().unwrap_or_else(PoisonError::into_inner).lines.iter().cloned().collect()
    }

    /// Lines dropped to stay within the bounds.
    pub fn dropped(&self) -> u32 {
        self.kept.lock().unwrap_or_else(PoisonError::into_inner).dropped
    }

    /// The number the next line will have (lines pushed so far).
    pub fn next(&self) -> u64 {
        self.kept.lock().unwrap_or_else(PoisonError::into_inner).next
    }

    /// The kept lines numbered `from` and later, and the number after the
    /// last of them.
    pub fn since(&self, from: u64) -> (Vec<String>, u64) {
        let kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        let first = kept.next - kept.lines.len() as u64;
        let skip = usize::try_from(from.saturating_sub(first)).unwrap_or(usize::MAX);
        (kept.lines.iter().skip(skip).cloned().collect(), kept.next)
    }

    /// Waits until a line numbered `from` or later has been pushed.
    pub async fn wait_past(&self, from: u64) {
        loop {
            let notified = self.notify.notified();
            if self.next() > from {
                return;
            }
            notified.await;
        }
    }
}

/// A flag the host sets to stop a running job; the job's process group is
/// killed. Cheap to clone; clones share.
#[derive(Debug, Clone)]
pub struct Stop(Arc<watch::Sender<bool>>);

impl Default for Stop {
    fn default() -> Self {
        Stop(Arc::new(watch::Sender::new(false)))
    }
}

impl Stop {
    pub fn new() -> Stop {
        Stop::default()
    }

    pub fn stop(&self) {
        self.0.send_replace(true);
    }

    pub fn is_stopped(&self) -> bool {
        *self.0.borrow()
    }

    /// Returns once [`Stop::stop`] has been called.
    pub async fn wait(&self) {
        let mut seen = self.0.subscribe();
        let _ = seen.wait_for(|stopped| *stopped).await;
    }
}

// ---- git ----

/// What an Act call did.
#[derive(Debug)]
pub(crate) struct Acted {
    pub out: Out,
    /// Lines that arrived on either stream.
    pub seen: u32,
    /// Stopped by the person: the process group was killed.
    pub stopped: bool,
}

impl Scm {
    /// Runs a read. A non-zero exit is not an error here.
    pub(crate) async fn read(&self, op: &Op, cwd: &Path, input: Option<Vec<u8>>) -> Result<Out, ScmError> {
        if op.profile() != Profile::Read || !op.sound() {
            return Err(ScmError::new(Reason::BadRequest));
        }
        let mut call = Call::new(cwd, op.limit()).args(argv(op)).cap(op.cap());
        if let Some(input) = input {
            call = call.input(input);
        }
        let deadline = Instant::now() + op.limit();
        self.git
            .spawn(Profile::Read, call, deadline, None, std::future::pending())
            .await
            .map_err(|e| failure(e, cwd))
    }

    /// Runs a person's request: every line of either stream goes to `out`
    /// as it arrives (cleaned, `hide` written as the remote's name), and
    /// `stop` kills the process group.
    pub(crate) async fn act(
        &self,
        op: &Op,
        cwd: &Path,
        input: Option<Vec<u8>>,
        out: &Lines,
        stop: Option<&Stop>,
        hide: &[(String, String)],
    ) -> Result<Acted, ScmError> {
        if op.profile() != Profile::Act || !op.sound() {
            return Err(ScmError::new(Reason::BadRequest));
        }
        let mut call = Call::new(cwd, op.limit()).args(argv(op)).cap(op.cap());
        if let Some(input) = input {
            call = call.input(input);
        }
        if let Some(locale) = &self.locale {
            call = call.env("LC_ALL", locale);
        }
        let deadline = Instant::now() + op.limit();
        let mut seen = 0u32;
        let home = self.home.clone();
        let mut on_line = |raw: &[u8]| {
            seen = seen.saturating_add(1);
            out.push(&redact(&clean::line(raw, home.as_deref()), hide, home.as_deref()));
        };
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stopped.clone();
        let wait = async move {
            match stop {
                Some(stop) => stop.wait().await,
                None => std::future::pending().await,
            }
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        };
        let sink: LineSink<'_> = &mut on_line;
        let result = self.git.spawn(Profile::Act, call, deadline, Some(sink), wait).await;
        let stopped = stopped.load(std::sync::atomic::Ordering::SeqCst);
        match result {
            Ok(out) => Ok(Acted { out, seen, stopped }),
            Err(e) => Err(failure(e, cwd)),
        }
    }
}

/// `(url, name)` pairs: every URL in `line` written as its remote's name,
/// so a remote's URL never leaves the host in a job's lines.
pub(crate) fn redact(line: &str, hide: &[(String, String)], home: Option<&str>) -> String {
    let mut line = line.to_owned();
    for (url, name) in hide {
        let cleaned = clean::line(url.as_bytes(), home);
        for form in [url.as_str(), cleaned.as_str()] {
            if !form.is_empty() && line.contains(form) {
                line = line.replace(form, name);
            }
        }
    }
    line
}

fn failure(e: GitError, cwd: &Path) -> ScmError {
    match e {
        GitError::Slow => ScmError::new(Reason::Slow),
        GitError::Io(_) if cwd.symlink_metadata().is_err() => ScmError::new(Reason::NotRepo),
        GitError::Io(_) => ScmError::new(Reason::NoGit),
        _ => ScmError::new(Reason::Failed),
    }
}

// ---- Other programs (gh) ----

/// One run of a program other than git: argv only, the given environment,
/// stdin null unless `input`, the process group killed at `limit`.
#[derive(Debug)]
pub(crate) struct Program<'a> {
    pub bin: &'a Path,
    pub args: Vec<OsString>,
    pub env: &'a [(OsString, OsString)],
    pub cwd: PathBuf,
    pub input: Option<Vec<u8>>,
    pub limit: Duration,
    pub cap: usize,
}

/// How a program ended.
#[derive(Debug)]
pub(crate) enum Ran {
    Exited {
        /// `None`: killed by a signal.
        code: Option<i32>,
        stdout: Vec<u8>,
        /// Standard output passed the cap (the process group was killed).
        over: bool,
        stderr: Vec<u8>,
    },
    /// Past its limit: killed.
    Late,
}

/// Runs `p`; with `on_line`, each line of either stream as it arrives.
pub(crate) async fn run_program(p: Program<'_>, mut on_line: Option<LineSink<'_>>) -> io::Result<Ran> {
    let mut cmd = Command::new(p.bin);
    cmd.args(&p.args)
        .env_clear()
        .envs(p.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&p.cwd)
        .stdin(if p.input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = crate::host::git::run::spawn_retrying(&mut cmd).await?;
    let pid = child.id();
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let input = p.input;
    let until = tokio::time::Instant::now() + p.limit;
    let streaming = on_line.is_some();
    let ended = {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
        let out_tx = streaming.then(|| tx.clone());
        let err_tx = streaming.then(|| tx.clone());
        drop(tx);
        let feed = async move {
            if let (Some(mut w), Some(bytes)) = (stdin, input) {
                let _ = w.write_all(&bytes).await;
                let _ = w.shutdown().await;
            }
        };
        let deliver = async {
            while let Some(line) = rx.recv().await {
                if let Some(f) = on_line.as_mut() {
                    f(&line);
                }
            }
        };
        let work = async {
            let (_, out, err, ()) = tokio::join!(
                feed,
                read(stdout, p.cap, Some(pid), out_tx),
                read(stderr, MAX_ERR, None, err_tx),
                deliver
            );
            (out, err, child.wait().await)
        };
        tokio::time::timeout_at(until, work).await
    };
    match ended {
        Ok(((stdout, over), (stderr, _), status)) => {
            Ok(Ran::Exited { code: status?.code(), stdout, over, stderr })
        }
        Err(_) => {
            // `work` lost the race while waiting, so the child isn't reaped
            // and the group's id can't have been reused.
            kill_group(pid);
            let _ = tokio::time::timeout(REAP_GRACE, child.wait()).await;
            Ok(Ran::Late)
        }
    }
}

/// Reads to the end, keeping `cap` bytes; past it, kills `kill`'s group.
/// With `lines`, sends each line as it arrives.
async fn read(
    from: Option<impl AsyncRead + Unpin>,
    cap: usize,
    kill: Option<Option<u32>>,
    lines: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
) -> (Vec<u8>, bool) {
    let Some(mut from) = from else { return (Vec::new(), false) };
    let mut kept = Vec::new();
    let mut over = false;
    let mut chunk = vec![0u8; 64 << 10];
    let mut line = Vec::new();
    let mut skipping = false;
    loop {
        let n = match from.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let room = cap - kept.len();
        kept.extend_from_slice(&chunk[..n.min(room)]);
        if n > room && !over {
            over = true;
            if let Some(pid) = kill {
                kill_group(pid);
                break;
            }
        }
        let Some(lines) = &lines else { continue };
        for &b in &chunk[..n] {
            if b == b'\n' {
                if !skipping {
                    let _ = lines.send(std::mem::take(&mut line)).await;
                }
                line.clear();
                skipping = false;
            } else if !skipping {
                line.push(b);
                if line.len() >= READ_LINE {
                    let _ = lines.send(std::mem::take(&mut line)).await;
                    skipping = true;
                }
            }
        }
    }
    if let Some(lines) = &lines
        && !line.is_empty()
        && !skipping
    {
        let _ = lines.send(line).await;
    }
    (kept, over)
}

fn kill_group(pid: Option<u32>) {
    let Some(pid) = pid.and_then(|id| i32::try_from(id).ok()).and_then(rustix::process::Pid::from_raw) else {
        return;
    };
    let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_keep_400_and_64_kib_and_count_what_they_drop() {
        let lines = Lines::new();
        for i in 0..10_000 {
            lines.push(&format!("{i} {}", "x".repeat(600)));
        }
        let kept = lines.lines();
        assert!(kept.len() <= MAX_JOB_LINES);
        assert!(kept.iter().map(String::len).sum::<usize>() <= MAX_JOB_BYTES);
        assert!(kept.iter().all(|l| l.chars().count() <= MAX_LINE_CHARS));
        assert_eq!(lines.dropped() as usize + kept.len(), 10_000);
        assert!(kept.last().unwrap().starts_with("9999 "));
        assert_eq!(lines.next(), 10_000);
        let (new, next) = lines.since(9_998);
        assert_eq!((new.len(), next), (2, 10_000));
        let (all, _) = lines.since(0);
        assert_eq!(all.len(), kept.len());
    }

    #[test]
    fn short_lines_keep_400() {
        let lines = Lines::new();
        for i in 0..1000 {
            lines.push(&i.to_string());
        }
        assert_eq!(lines.lines().len(), MAX_JOB_LINES);
        assert_eq!(lines.dropped(), 600);
        assert_eq!(lines.lines()[0], "600");
    }

    #[tokio::test]
    async fn stop_wakes_a_waiter_and_stays_set() {
        let stop = Stop::new();
        let waiter = stop.clone();
        let task = tokio::spawn(async move { waiter.wait().await });
        tokio::task::yield_now().await;
        assert!(!stop.is_stopped());
        stop.stop();
        tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap();
        assert!(stop.is_stopped());
        tokio::time::timeout(Duration::from_secs(1), stop.wait()).await.unwrap();
    }

    #[test]
    fn urls_are_written_as_their_remote() {
        let hide = vec![("https://kim:pw@example.test/o/r.git".to_owned(), "origin".to_owned())];
        assert_eq!(redact("To https://\u{2026}@example.test/o/r.git", &hide, None), "To origin");
        assert_eq!(redact("To https://kim:pw@example.test/o/r.git", &hide, None), "To origin");
        assert_eq!(redact("nothing here", &hide, None), "nothing here");
        let local = vec![("/home/kim/bare.git".to_owned(), "backup".to_owned())];
        assert_eq!(
            redact(&clean::line(b"To /home/kim/bare.git", Some("/home/kim")), &local, Some("/home/kim")),
            "To backup"
        );
    }
}
