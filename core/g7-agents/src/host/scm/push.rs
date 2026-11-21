// SPDX-License-Identifier: MPL-2.0
//! Push and fetch (§SC3.5). A push sends one ref, the current branch, to
//! the branch of the same name on one remote, never forced; what happened
//! is read from `--porcelain` lines and the exit code only. A remote's URL
//! is written as its name in every line.

use super::argv::{Op, refspec};
use super::{Lines, Reason, Scm, ScmError, Spot, Stop, pick_remote, remote_of};

/// Git's English for a sign-in failure: it only refines a failure's word.
const SIGN_IN: [&str; 3] =
    ["terminal prompts disabled", "Authentication failed", "Permission denied (publickey)"];
/// Git's English for a remote it couldn't reach. ("Could not read from
/// remote repository" follows every failed connection, a missing local
/// folder's included, so it says nothing about the network.)
const NETWORK: [&str; 1] = ["Could not resolve host"];

/// A push that went through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pushed {
    pub branch: String,
    pub remote: String,
    /// The remote had it already.
    pub up_to_date: bool,
    /// The branch had no upstream; it has one now.
    pub set_upstream: bool,
}

impl Scm {
    /// Pushes the current branch (§SC3.5). `remote`: a name the client
    /// picked after `pick_remote`; otherwise the branch's own.
    pub async fn push(
        &self,
        spot: &Spot,
        remote: Option<&str>,
        out: &Lines,
        stop: &Stop,
    ) -> Result<Pushed, ScmError> {
        if spot.git_dir().join("index.lock").symlink_metadata().is_ok() {
            return Err(ScmError::new(Reason::GitBusy));
        }
        let fresh = self.porcelain(spot, 0).await?;
        if fresh.oid.is_none() {
            return Err(ScmError::new(Reason::NoCommits));
        }
        let Some(branch) = fresh.head.clone() else { return Err(ScmError::new(Reason::Detached)) };
        let remotes = self.remotes(spot).await?;
        let upstream_ref = fresh.upstream.as_ref().map(|u| format!("refs/remotes/{u}"));
        let upstream_remote = upstream_ref.as_deref().and_then(|u| remote_of(u, &remotes));
        let remote = pick_remote(&remotes, upstream_remote.as_deref(), remote)?;
        let set_upstream = fresh.upstream.is_none();
        let hide: Vec<(String, String)> = remotes.iter().map(|(n, u)| (u.clone(), n.clone())).collect();
        let op = Op::Push { remote: remote.clone(), branch: branch.clone(), set_upstream };
        let pushed = self.act(&op, spot.folder(), None, out, Some(stop), &hide).await?;
        self.forget_names(spot);
        if pushed.stopped {
            return Err(ScmError::new(Reason::Stopped));
        }
        let target = refspec(&branch);
        let line = ref_line(&pushed.out.stdout, &target);
        match (pushed.out.code, line) {
            (Some(0), Some((flag, _))) if matches!(flag, b' ' | b'*' | b'=') => {
                self.gh.forget_pr(spot, &branch);
                Ok(Pushed { branch, remote, up_to_date: flag == b'=', set_upstream })
            }
            (_, Some((b'!', summary))) if summary.starts_with("[rejected]") => {
                Err(ScmError::new(Reason::Behind))
            }
            (_, Some((b'!', summary))) if summary.starts_with("[remote rejected]") => {
                Err(ScmError::new(Reason::RemoteRefused))
            }
            (Some(1), None) if pushed.seen > 0 => Err(ScmError::new(Reason::Hook)),
            _ => Err(ScmError::new(refine(&out.lines()))),
        }
    }

    /// Fetches the branch's remote (§SC3.5): no prune, no tags, no other
    /// remote. The host reads the status again afterwards.
    pub async fn fetch(&self, spot: &Spot, out: &Lines) -> Result<(), ScmError> {
        let fresh = self.porcelain(spot, 0).await?;
        let remotes = self.remotes(spot).await?;
        let upstream_ref = fresh.upstream.as_ref().map(|u| format!("refs/remotes/{u}"));
        let upstream_remote = upstream_ref.as_deref().and_then(|u| remote_of(u, &remotes));
        let remote = pick_remote(&remotes, upstream_remote.as_deref(), None)?;
        let hide: Vec<(String, String)> = remotes.iter().map(|(n, u)| (u.clone(), n.clone())).collect();
        let fetched = self.act(&Op::Fetch { remote }, spot.folder(), None, out, None, &hide).await?;
        self.forget_names(spot);
        if fetched.out.code == Some(0) { Ok(()) } else { Err(ScmError::new(refine(&out.lines()))) }
    }
}

/// The `--porcelain` line for `target` (`HEAD:refs/heads/<branch>`): its
/// flag and summary.
fn ref_line(stdout: &[u8], target: &str) -> Option<(u8, String)> {
    let text = String::from_utf8_lossy(stdout);
    text.lines().find_map(|line| {
        let mut fields = line.split('\t');
        let flag = fields.next()?.as_bytes();
        let refs = fields.next()?;
        let summary = fields.next().unwrap_or("");
        (flag.len() == 1 && refs == target).then(|| (flag[0], summary.to_owned()))
    })
}

/// A failure's word: `sign_in` or `network` when a line holds one of git's
/// fixed phrases, else `failed`.
pub(crate) fn refine(lines: &[String]) -> Reason {
    if lines.iter().any(|l| SIGN_IN.iter().any(|p| l.contains(p))) {
        Reason::SignIn
    } else if lines.iter().any(|l| NETWORK.iter().any(|p| l.contains(p))) {
        Reason::Network
    } else {
        Reason::Failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_lines_are_found_by_their_ref() {
        let out = b"To origin\n*\tHEAD:refs/heads/main\t[new branch]\nDone\n";
        assert_eq!(ref_line(out, "HEAD:refs/heads/main"), Some((b'*', "[new branch]".to_owned())));
        let out = b"To origin\n!\tHEAD:refs/heads/main\t[rejected] (fetch first)\nDone\n";
        assert_eq!(ref_line(out, "HEAD:refs/heads/main").unwrap().1, "[rejected] (fetch first)");
        assert_eq!(ref_line(out, "HEAD:refs/heads/other"), None);
    }

    #[test]
    fn only_the_fixed_phrases_refine_a_failure() {
        let lines = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            refine(&lines(&["fatal: could not read Username: terminal prompts disabled"])),
            Reason::SignIn
        );
        assert_eq!(
            refine(&lines(&[
                "git@x: Permission denied (publickey).",
                "fatal: Could not read from remote repository."
            ])),
            Reason::SignIn
        );
        assert_eq!(
            refine(&lines(&["fatal: unable to access 'x': Could not resolve host: x"])),
            Reason::Network
        );
        assert_eq!(
            refine(&lines(&[
                "fatal: '/nope' does not appear to be a git repository",
                "fatal: Could not read from remote repository."
            ])),
            Reason::Failed
        );
        assert_eq!(refine(&lines(&["error: something else"])), Reason::Failed);
    }
}
