// SPDX-License-Identifier: MPL-2.0
//! The only git commands source control runs (§SC3.1): [`Op`] is the closed
//! list and [`argv`] the only place in `host/scm` that names a git
//! subcommand. Every value an `Op` carries was looked up or built by the
//! host before it gets here; [`Op::sound`] checks the shapes once more.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::time::Duration;

use crate::host::git::run::{MAX_OUT, MAX_PATCH_OUT, Profile};

/// The diff options of every read that diffs (§SC3.1).
const DIFF_OPTIONS: [&str; 3] = ["--no-color", "--no-ext-diff", "--no-textconv"];

/// `for-each-ref`'s fields, each followed by a NUL so a record is six
/// fields whatever a worktree's path holds.
pub(super) const REFS_FORMAT: &str = "--format=%(refname)%00%(upstream)%00%(upstream:track,nobracket)%00%(committerdate:unix)%00%(HEAD)%00%(worktreepath)%00";
pub(super) const REFS_FIELDS: usize = 6;

/// One git command of source control's closed list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Op {
    /// The folder's top level, git directory, common directory and its
    /// place under the top level.
    Top,
    Status,
    Numstat,
    /// A tracked file's patch; a rename names its old path too.
    Patch {
        path: OsString,
        from: Option<OsString>,
    },
    PatchNew {
        path: OsString,
    },
    /// Object ids of the paths on stdin.
    Ids,
    Ident,
    Refs,
    Remotes,
    RemoteHead {
        remote: String,
    },
    RefOk {
        name: String,
    },
    Unpushed,
    LastCommit,
    Worktrees,
    /// Paths on stdin ([`pathspecs`]).
    Add,
    /// Paths on stdin ([`pathspecs`]).
    Commit {
        message: String,
    },
    Push {
        remote: String,
        branch: String,
        set_upstream: bool,
    },
    Fetch {
        remote: String,
    },
    Switch {
        branch: String,
    },
    SwitchNew {
        name: String,
    },
    SwitchTrack {
        remote_branch: String,
    },
    WorktreeAdd {
        path: PathBuf,
        branch: String,
        base: String,
    },
    WorktreeRemove {
        path: PathBuf,
    },
}

impl Op {
    pub fn profile(&self) -> Profile {
        match self {
            Op::Add
            | Op::Commit { .. }
            | Op::Push { .. }
            | Op::Fetch { .. }
            | Op::Switch { .. }
            | Op::SwitchNew { .. }
            | Op::SwitchTrack { .. }
            | Op::WorktreeAdd { .. }
            | Op::WorktreeRemove { .. } => Profile::Act,
            _ => Profile::Read,
        }
    }

    /// The call's time limit (§SC3.1).
    pub fn limit(&self) -> Duration {
        let secs = match self {
            Op::Patch { .. } | Op::PatchNew { .. } => 20,
            Op::Add
            | Op::Fetch { .. }
            | Op::Switch { .. }
            | Op::SwitchNew { .. }
            | Op::SwitchTrack { .. } => 60,
            Op::WorktreeRemove { .. } => 60,
            Op::WorktreeAdd { .. } => 120,
            Op::Push { .. } => 300,
            Op::Commit { .. } => 600,
            _ => 10,
        };
        Duration::from_secs(secs)
    }

    /// Standard output kept.
    pub fn cap(&self) -> usize {
        match self {
            Op::Patch { .. } | Op::PatchNew { .. } => MAX_PATCH_OUT,
            _ => MAX_OUT,
        }
    }

    /// Whether the values this carries can be placed in argv: no name or
    /// path that could read as an option, no NUL, no empty value.
    pub fn sound(&self) -> bool {
        let name =
            |s: &str| !s.is_empty() && !s.starts_with('-') && !s.contains(['\0', '\n']) && s.len() <= 255;
        let file = |p: &OsStr| {
            let b = p.as_bytes();
            !b.is_empty() && !b.starts_with(b"-") && !b.contains(&0)
        };
        match self {
            Op::Patch { path, from } => file(path) && from.as_deref().is_none_or(file),
            Op::PatchNew { path } => file(path),
            Op::RemoteHead { remote } | Op::Fetch { remote } => name(remote),
            Op::RefOk { name: n } => !n.is_empty() && !n.contains('\0') && n.len() <= 255,
            Op::Commit { message } => !message.trim().is_empty() && !message.contains('\0'),
            Op::Push { remote, branch, .. } => name(remote) && name(branch) && !branch.contains(':'),
            Op::Switch { branch } => name(branch),
            Op::SwitchNew { name: n } => name(n),
            Op::SwitchTrack { remote_branch } => name(remote_branch),
            Op::WorktreeAdd { path, branch, base } => path.is_absolute() && name(branch) && name(base),
            Op::WorktreeRemove { path } => path.is_absolute(),
            _ => true,
        }
    }
}

/// The arguments after the binary and the profile's options.
pub(crate) fn argv(op: &Op) -> Vec<OsString> {
    let words = |w: &[&str]| w.iter().map(OsString::from).collect::<Vec<_>>();
    match op {
        Op::Top => words(&[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
            "--show-prefix",
        ]),
        Op::Status => {
            words(&["status", "--porcelain=2", "--branch", "-z", "--untracked-files=normal", "--", "."])
        }
        Op::Numstat => {
            let mut v = words(&["diff-index", "-M", "--numstat", "-z"]);
            v.extend(words(&DIFF_OPTIONS));
            v.extend(words(&["HEAD", "--", "."]));
            v
        }
        Op::Patch { path, from } => {
            let mut v = words(&["diff", "--patch", "-M"]);
            v.extend(words(&DIFF_OPTIONS));
            v.extend(words(&["HEAD", "--"]));
            v.push(path.clone());
            v.extend(from.iter().cloned());
            v
        }
        Op::PatchNew { path } => {
            let mut v = words(&["diff", "--no-index"]);
            v.extend(words(&DIFF_OPTIONS));
            v.extend(words(&["--", "/dev/null"]));
            v.push(path.clone());
            v
        }
        Op::Ids => words(&["hash-object", "--no-filters", "--stdin-paths"]),
        Op::Ident => words(&["var", "GIT_AUTHOR_IDENT"]),
        Op::Refs => {
            words(&["for-each-ref", REFS_FORMAT, "--sort=-committerdate", "refs/heads", "refs/remotes"])
        }
        Op::Remotes => words(&["config", "--get-regexp", "^remote\\..*\\.url$"]),
        Op::RemoteHead { remote } => {
            let mut v = words(&["symbolic-ref", "--quiet"]);
            v.push(format!("refs/remotes/{remote}/HEAD").into());
            v
        }
        Op::RefOk { name } => {
            let mut v = words(&["check-ref-format", "--branch"]);
            v.push(name.into());
            v
        }
        Op::Unpushed => words(&["rev-list", "--count", "HEAD", "--not", "--remotes"]),
        Op::LastCommit => {
            words(&["log", "-z", "--max-count=1", "--no-show-signature", "--format=%H%x1f%ct%x1f%s"])
        }
        Op::Worktrees => words(&["worktree", "list", "--porcelain", "-z"]),
        Op::Add => words(&["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"]),
        Op::Commit { message } => {
            let mut v = words(&["commit", "--only", "--pathspec-from-file=-", "--pathspec-file-nul", "-m"]);
            v.push(message.into());
            v
        }
        Op::Push { remote, branch, set_upstream } => {
            let mut v = words(&["push", "--porcelain"]);
            if *set_upstream {
                v.push("--set-upstream".into());
            }
            v.push("--".into());
            v.push(remote.into());
            v.push(refspec(branch).into());
            v
        }
        Op::Fetch { remote } => {
            let mut v = words(&["fetch", "--"]);
            v.push(remote.into());
            v
        }
        Op::Switch { branch } => {
            let mut v = words(&["switch", "--no-guess"]);
            v.push(branch.into());
            v
        }
        Op::SwitchNew { name } => {
            let mut v = words(&["switch", "--create"]);
            v.push(name.into());
            v
        }
        Op::SwitchTrack { remote_branch } => {
            let mut v = words(&["switch", "--track"]);
            v.push(remote_branch.into());
            v
        }
        Op::WorktreeAdd { path, branch, base } => {
            let mut v = words(&["worktree", "add", "--no-track", "-b"]);
            v.push(branch.into());
            v.push(path.into());
            v.push(base.into());
            v
        }
        Op::WorktreeRemove { path } => {
            let mut v = words(&["worktree", "remove"]);
            v.push(path.into());
            v
        }
    }
}

/// The one ref a push sends: the current branch to the branch of the same
/// name, never forced.
pub(crate) fn refspec(branch: &str) -> String {
    format!("HEAD:refs/heads/{branch}")
}

/// Paths from the top level for `--pathspec-from-file`, each read
/// literally (`:(top,literal)`), NUL-separated. An Act call can't set
/// `GIT_LITERAL_PATHSPECS`: the user's hooks would inherit it.
pub(crate) fn pathspecs<'a>(paths: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    for path in paths {
        out.extend_from_slice(b":(top,literal)");
        out.extend_from_slice(path);
        out.push(0);
    }
    out
}

/// A path's bytes as an argument.
pub(crate) fn arg_path(raw: &[u8]) -> OsString {
    OsString::from_vec(raw.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §SC3.1's table: the first word of every command source control runs.
    const SUBCOMMANDS: [&str; 18] = [
        "rev-parse",
        "status",
        "diff-index",
        "diff",
        "hash-object",
        "var",
        "for-each-ref",
        "config",
        "symbolic-ref",
        "check-ref-format",
        "rev-list",
        "log",
        "worktree",
        "add",
        "commit",
        "push",
        "fetch",
        "switch",
    ];

    /// `SCM_NEVER` of `tests/forbidden_strings.rs`, unquoted.
    const NEVER: [&str; 49] = [
        "clean",
        "reset",
        "checkout",
        "stash",
        "restore",
        "rebase",
        "merge",
        "pull",
        "cherry-pick",
        "revert",
        "rm",
        "mv",
        "branch",
        "tag",
        "gc",
        "prune",
        "filter-branch",
        "update-ref",
        "--force",
        "-f",
        "--force-with-lease",
        "--force-if-includes",
        "--hard",
        "--no-verify",
        "-n",
        "--amend",
        "--mirror",
        "--all",
        "--tags",
        "--delete",
        "-d",
        "-D",
        "-C",
        "--force-create",
        "--orphan",
        "--discard-changes",
        "--detach",
        "--prune",
        "--allow-empty",
        "--no-gpg-sign",
        "-c",
        "--admin",
        "--auto",
        "--delete-branch",
        "--fill",
        "--web",
        "--editor",
        "api",
        "token",
    ];

    fn every_op() -> Vec<Op> {
        vec![
            Op::Top,
            Op::Status,
            Op::Numstat,
            Op::Patch { path: "a.txt".into(), from: Some("b.txt".into()) },
            Op::PatchNew { path: "new.txt".into() },
            Op::Ids,
            Op::Ident,
            Op::Refs,
            Op::Remotes,
            Op::RemoteHead { remote: "origin".into() },
            Op::RefOk { name: "feat/x".into() },
            Op::Unpushed,
            Op::LastCommit,
            Op::Worktrees,
            Op::Add,
            Op::Commit { message: "Fix the retry\n\nBody".into() },
            Op::Push { remote: "origin".into(), branch: "feat/x".into(), set_upstream: true },
            Op::Fetch { remote: "origin".into() },
            Op::Switch { branch: "main".into() },
            Op::SwitchNew { name: "feat/y".into() },
            Op::SwitchTrack { remote_branch: "origin/z".into() },
            Op::WorktreeAdd { path: "/w/g7-x-abc123".into(), branch: "g7/x".into(), base: "HEAD".into() },
            Op::WorktreeRemove { path: "/w/g7-x-abc123".into() },
        ]
    }

    #[test]
    fn every_op_is_on_the_list() {
        let ops = every_op();
        assert_eq!(ops.len(), 23, "one example of each Op");
        let mut firsts = std::collections::BTreeSet::new();
        for op in &ops {
            assert!(op.sound(), "{op:?}");
            let args = argv(op);
            let first = args[0].to_str().unwrap();
            assert!(SUBCOMMANDS.contains(&first), "{op:?} starts with {first}");
            firsts.insert(first.to_owned());
            for word in &args {
                let word = word.to_string_lossy();
                assert!(!NEVER.contains(&word.as_ref()), "{op:?} has {word}");
            }
        }
        assert_eq!(firsts.len(), SUBCOMMANDS.len(), "every listed subcommand is used");
    }

    #[test]
    fn a_refspec_is_one_plain_ref() {
        let op = Op::Push { remote: "origin".into(), branch: "feat/x".into(), set_upstream: false };
        let args: Vec<String> = argv(&op).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["push", "--porcelain", "--", "origin", "HEAD:refs/heads/feat/x"]);
        let specs: Vec<&String> = args.iter().filter(|a| a.contains(':')).collect();
        assert_eq!(specs.len(), 1);
        assert!(!specs[0].starts_with('+') && specs[0].starts_with("HEAD:refs/heads/"));
        for bad in ["-f", "a:b", "--force", "", "x\0y"] {
            let op = Op::Push { remote: "origin".into(), branch: bad.into(), set_upstream: false };
            assert!(!op.sound(), "{bad:?}");
        }
        let plus = refspec("+main");
        assert!(!plus.starts_with('+') && plus == "HEAD:refs/heads/+main");
    }

    #[test]
    fn values_that_read_as_options_are_not_sound() {
        assert!(!Op::Switch { branch: "--discard-changes".into() }.sound());
        assert!(!Op::Switch { branch: "-f".into() }.sound());
        assert!(!Op::Fetch { remote: "--upload-pack=x".into() }.sound());
        assert!(!Op::PatchNew { path: "-x".into() }.sound());
        assert!(!Op::WorktreeAdd { path: "rel".into(), branch: "g7/a".into(), base: "HEAD".into() }.sound());
        assert!(!Op::Commit { message: "  \n".into() }.sound());
        assert!(!Op::Commit { message: "a\0b".into() }.sound());
    }

    #[test]
    fn the_act_profile_adds_no_config() {
        for op in every_op() {
            let args = argv(&op);
            if op.profile() == Profile::Act {
                assert!(
                    !args.iter().any(|a| a == "-c" || a.to_string_lossy().contains("hooksPath")),
                    "{op:?}"
                );
            }
        }
        let acts: Vec<_> = every_op().into_iter().filter(|op| op.profile() == Profile::Act).collect();
        assert_eq!(acts.len(), 9);
        assert_eq!(pathspecs([&b"a b"[..], b"*.rs"]), b":(top,literal)a b\0:(top,literal)*.rs\0");
    }
}
