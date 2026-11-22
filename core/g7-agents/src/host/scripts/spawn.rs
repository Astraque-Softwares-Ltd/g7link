// SPDX-License-Identifier: MPL-2.0
//! How a script runs (phase 6, §X3.2), and the only place in `host/scripts`
//! that starts a process: the user's shell with `-c` and the line exactly as
//! they typed it, in the folder, as them, with the host's own environment
//! less G7's and the agents' variables. G7 builds no part of the line and
//! adds no flag.
//!
//! The run's task owns the shell. It reads both streams to their end and
//! only then reaps it, so while a stop may still be sent the group id can't
//! belong to anything else. A stop is `TERM` to the group, then `KILL` once
//! the grace is over, then the reap.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use rustix::process::{Pid, Signal};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use crate::host::git::run::spawn_retrying;
use crate::host::scm::Stop;

/// The shell when the host's `SHELL` isn't an absolute path to an executable file.
pub const FALLBACK_SHELL: &str = "/bin/sh";
/// Bytes of one line read; the rest of that line is dropped.
pub const READ_LINE: usize = 4096;
/// After the last signal, how long the reap may take.
const REAP_GRACE: Duration = Duration::from_secs(2);
/// Lines read and not yet taken.
const LINES_QUEUED: usize = 256;

/// Variables that never reach a script, by name and by start.
const UNSET: [&str; 4] = ["CLAUDECODE", "GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"];
const UNSET_STARTS: [&str; 2] = ["G7_AGENTS_", "CLAUDE_CODE_"];

/// The shell a script runs in: the host's `SHELL` when it is an absolute
/// path to an executable regular file, else [`FALLBACK_SHELL`].
pub fn shell(env: &[(OsString, OsString)]) -> PathBuf {
    env.iter()
        .find(|(name, _)| name == "SHELL")
        .map(|(_, value)| PathBuf::from(value))
        .filter(|path| path.is_absolute() && is_executable_file(path))
        .unwrap_or_else(|| PathBuf::from(FALLBACK_SHELL))
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The host's environment `env` as a script gets it: every variable but
/// G7's own, the agents' and git's repository overrides. Nothing is added.
pub fn environment(env: &[(OsString, OsString)]) -> Vec<(OsString, OsString)> {
    env.iter().filter(|(name, _)| passes(name)).cloned().collect()
}

fn passes(name: &OsStr) -> bool {
    let name = name.as_encoded_bytes();
    !UNSET.iter().any(|unset| name == unset.as_bytes())
        && !UNSET_STARTS.iter().any(|start| name.starts_with(start.as_bytes()))
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The shell exited by itself with this code.
    Exit(i32),
    /// A signal G7 didn't send ended it.
    Signal,
    /// It was stopped.
    Stopped,
}

/// A started shell, held unreaped until [`Running::finish`] ends.
#[derive(Debug)]
pub struct Running {
    child: Child,
    pid: Option<Pid>,
}

/// Starts `command` with the user's `shell` in `cwd`: exactly the shell,
/// `-c` and the line; stdin empty, both streams read; its own process group.
pub async fn start(
    shell: &Path,
    command: &str,
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> io::Result<Running> {
    let mut cmd = Command::new(shell);
    cmd.arg("-c")
        .arg(command)
        .env_clear()
        .envs(environment(env))
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let child = spawn_retrying(&mut cmd).await?;
    let pid = child.id().and_then(|id| i32::try_from(id).ok()).and_then(Pid::from_raw);
    Ok(Running { child, pid })
}

impl Running {
    /// The shell's pid (its group's id), while it isn't reaped.
    pub fn pid(&self) -> Option<u32> {
        self.pid.and_then(|pid| u32::try_from(pid.as_raw_nonzero().get()).ok())
    }

    /// Runs to the end: every line of either stream goes to `on_line` as it
    /// arrives (at most [`READ_LINE`] bytes of it), then the shell is
    /// reaped. Once `stop` is set the group gets `TERM`, and `KILL` when
    /// `grace()` has passed without the run ending.
    pub async fn finish(
        mut self,
        stop: &Stop,
        grace: impl Fn() -> Duration,
        mut on_line: impl FnMut(Vec<u8>),
    ) -> Ended {
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(LINES_QUEUED);
        let streams = {
            let (out, err) = (self.child.stdout.take(), self.child.stderr.take());
            let err_tx = tx.clone();
            async move {
                tokio::join!(read_lines(out, tx), read_lines(err, err_tx));
            }
        };
        let mut streams = std::pin::pin!(streams);
        let mut streams_done = false;
        // Both streams at their end and every line taken.
        let mut open = true;
        let mut kill_at: Option<tokio::time::Instant> = None;
        let status = loop {
            let kill_due = kill_at.unwrap_or_else(tokio::time::Instant::now);
            tokio::select! {
                () = &mut streams, if !streams_done => streams_done = true,
                line = rx.recv(), if open => match line {
                    Some(line) => on_line(line),
                    None => open = false,
                },
                status = self.child.wait(), if !open => break status.ok(),
                () = stop.wait(), if kill_at.is_none() => {
                    signal(self.pid, Signal::TERM);
                    kill_at = Some(tokio::time::Instant::now() + grace());
                }
                () = tokio::time::sleep_until(kill_due), if kill_at.is_some() => {
                    // Still not reaped: the group is still the run's.
                    signal(self.pid, Signal::KILL);
                    let _ = tokio::time::timeout(REAP_GRACE, self.child.wait()).await;
                    while let Ok(line) = rx.try_recv() {
                        on_line(line);
                    }
                    return Ended::Stopped;
                }
            }
        };
        if kill_at.is_some() {
            return Ended::Stopped;
        }
        match status.and_then(|s| s.code()) {
            Some(code) => Ended::Exit(code),
            None => Ended::Signal,
        }
    }
}

/// `signal` to the run's process group. Only called before the shell is
/// reaped, so the group id is still the run's.
fn signal(pid: Option<Pid>, signal: Signal) {
    if let Some(pid) = pid {
        let _ = rustix::process::kill_process_group(pid, signal);
    }
}

/// Reads `from` to its end, sending each line as it arrives: at most
/// [`READ_LINE`] bytes of it, without its newline; a last line without one
/// at the end.
async fn read_lines(from: Option<impl AsyncRead + Unpin>, to: mpsc::Sender<Vec<u8>>) {
    let Some(mut from) = from else { return };
    let mut chunk = vec![0u8; 16 << 10];
    let mut line = Vec::new();
    let mut skipping = false;
    loop {
        let n = match from.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for part in chunk[..n].split_inclusive(|b| *b == b'\n') {
            let (text, ends) = match part.strip_suffix(b"\n") {
                Some(text) => (text, true),
                None => (part, false),
            };
            if !skipping {
                let room = READ_LINE - line.len();
                line.extend_from_slice(&text[..text.len().min(room)]);
                if text.len() >= room && line.len() >= READ_LINE {
                    if to.send(std::mem::take(&mut line)).await.is_err() {
                        return;
                    }
                    skipping = !ends;
                    continue;
                }
            }
            if ends {
                if !skipping && to.send(std::mem::take(&mut line)).await.is_err() {
                    return;
                }
                line.clear();
                skipping = false;
            }
        }
    }
    if !line.is_empty() && !skipping {
        let _ = to.send(line).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(list: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        list.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect()
    }

    #[test]
    fn the_environment_is_the_hosts_less_g7s_and_the_agents() {
        let env = vars(&[
            ("PATH", "/usr/bin"),
            ("HOME", "/home/u"),
            ("G7_AGENTS_SOCKET", "/run/x"),
            ("CLAUDE_CODE_ENTRYPOINT", "cli"),
            ("CLAUDECODE", "1"),
            ("GIT_DIR", "/x/.git"),
            ("GIT_WORK_TREE", "/x"),
            ("GIT_INDEX_FILE", "/x/i"),
            ("GIT_AUTHOR_NAME", "Me"),
            ("CLAUDECODEX", "kept"),
            ("NODE_ENV", "development"),
        ]);
        let names: Vec<String> =
            environment(&env).iter().map(|(k, _)| k.to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["PATH", "HOME", "GIT_AUTHOR_NAME", "CLAUDECODEX", "NODE_ENV"]);
    }

    #[test]
    fn the_shell_is_the_users_when_it_can_be_run() {
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("myshell");
        std::fs::write(&ok, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&ok, std::fs::Permissions::from_mode(0o755)).unwrap();
        let plain = dir.path().join("plain");
        std::fs::write(&plain, "").unwrap();
        let ok_str = ok.to_str().unwrap();
        assert_eq!(shell(&vars(&[("SHELL", ok_str)])), ok);
        for bad in ["zsh", "", plain.to_str().unwrap(), dir.path().to_str().unwrap(), "/nonexistent/sh"] {
            assert_eq!(shell(&vars(&[("SHELL", bad)])), Path::new(FALLBACK_SHELL), "{bad}");
        }
        assert_eq!(shell(&[]), Path::new(FALLBACK_SHELL));
    }

    async fn lines_of(bytes: &[u8]) -> Vec<Vec<u8>> {
        let (tx, mut rx) = mpsc::channel(1024);
        read_lines(Some(bytes), tx).await;
        let mut out = Vec::new();
        while let Some(line) = rx.recv().await {
            out.push(line);
        }
        out
    }

    #[tokio::test]
    async fn lines_are_cut_at_4096_bytes_and_the_rest_of_them_dropped() {
        assert_eq!(lines_of(b"a\nb\n\nc").await, [b"a".to_vec(), b"b".to_vec(), Vec::new(), b"c".to_vec()]);
        let mut long = vec![b'x'; 10_000];
        long.extend_from_slice(b"\nnext\n");
        let got = lines_of(&long).await;
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].len(), READ_LINE);
        assert_eq!(got[1], b"next");
        let exact = [vec![b'y'; READ_LINE], b"\nz".to_vec()].concat();
        let got = lines_of(&exact).await;
        assert_eq!((got.len(), got[0].len(), got[1].as_slice()), (2, READ_LINE, b"z".as_slice()));
        assert!(lines_of(b"").await.is_empty());
    }
}
