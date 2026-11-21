// SPDX-License-Identifier: MPL-2.0
//! Strings that name something G7 must never send, call or open (phase 2 contract §Q7). They may
//! only appear in tests that check they are absent, so this looks at shipped code only: Rust before
//! its first `#[cfg(test)]`, and the desktop and Android sources outside their test folders.

use std::path::{Path, PathBuf};

const FORBIDDEN: [&str; 14] = [
    "apply_flag_settings",
    "thinking-display",
    "rateLimitResetCredit",
    "account/usage/read",
    ".mcp.json",
    "claude mcp",
    "auth token",
    "hosts.yml",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "api.github.com",
    "--show-token",
];

/// The two app updaters read G7's own release feed from GitHub's API; nothing else may name it.
const UPDATE_FEED_HOST: &str = "api.github.com";
const UPDATE_FEED_FILES: [&str; 2] = ["desktop/app/src/updater.rs", "update/AppUpdater.kt"];

/// `auth token` must start a word, so `oauth token` is not it; every other word is a plain match.
fn names(line: &str, word: &str) -> bool {
    line.match_indices(word).any(|(at, _)| {
        word != "auth token" || !line[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric())
    })
}

const ROOTS: [&str; 5] =
    ["core", "desktop/app/src", "desktop/app/qml", "desktop/app/cpp", "android/app/src/main"];
const EXTENSIONS: [&str; 7] = ["rs", "kt", "qml", "js", "cpp", "hpp", "h"];
const SKIPPED_DIRS: [&str; 5] = ["tests", "target", "build", "fixtures", "node_modules"];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            let name = entry.file_name();
            if !SKIPPED_DIRS.iter().any(|skip| name == *skip) {
                sources(&path, out);
            }
        } else if kind.is_file()
            && path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.contains(&e))
        {
            out.push(path);
        }
    }
}

fn shipped(path: &Path, text: &str) -> String {
    if path.extension().is_some_and(|e| e == "rs") {
        return text.find("#[cfg(test)]").map_or(text, |end| &text[..end]).to_owned();
    }
    text.to_owned()
}

#[test]
fn shipped_code_never_names_what_g7_must_not_use() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for root in ROOTS {
        sources(&workspace.join(root), &mut files);
    }
    assert!(files.len() > 50, "the scan found only {} files; the roots moved", files.len());

    let mut found = Vec::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        let code = shipped(file, &text);
        for word in FORBIDDEN {
            for (number, line) in code.lines().enumerate() {
                let path = file.to_string_lossy();
                if word == UPDATE_FEED_HOST && UPDATE_FEED_FILES.iter().any(|f| path.ends_with(f)) {
                    continue;
                }
                if names(line, word) {
                    found.push(format!("{}:{}: {word}", file.display(), number + 1));
                }
            }
        }
    }
    assert!(found.is_empty(), "forbidden strings in shipped code:\n{}", found.join("\n"));
}

/// Git subcommands and flags that rewrite the user's work tree, index or branches (phase 3 contract
/// §U5.5), plus every command and flag that source control (`host/scm`) owns (phase 4 contract
/// §SC3.6). Restore points only read the user's repository and write G7's own, so none of these may
/// appear as an argument in `host/git`. `"commit"` is left out on purpose: `checkpoint.rs` compares
/// an object's type with that word, and `--only` and `--amend` cover the command. Never shorten this.
const GIT_NEVER: [&str; 18] = [
    "\"clean\"",
    "\"reset\"",
    "\"checkout\"",
    "\"stash\"",
    "\"--staged\"",
    "\"--force\"",
    "\"-f\"",
    "\"push\"",
    "\"fetch\"",
    "\"pull\"",
    "\"switch\"",
    "\"merge\"",
    "\"rebase\"",
    "\"branch\"",
    "\"worktree\"",
    "\"gh\"",
    "\"--only\"",
    "\"--amend\"",
];

/// Words that `host/scm` never writes as an argument, in any file (phase 4 contract §SC3.6 item 2).
const SCM_NEVER: [&str; 49] = [
    "\"clean\"",
    "\"reset\"",
    "\"checkout\"",
    "\"stash\"",
    "\"restore\"",
    "\"rebase\"",
    "\"merge\"",
    "\"pull\"",
    "\"cherry-pick\"",
    "\"revert\"",
    "\"rm\"",
    "\"mv\"",
    "\"branch\"",
    "\"tag\"",
    "\"gc\"",
    "\"prune\"",
    "\"filter-branch\"",
    "\"update-ref\"",
    "\"--force\"",
    "\"-f\"",
    "\"--force-with-lease\"",
    "\"--force-if-includes\"",
    "\"--hard\"",
    "\"--no-verify\"",
    "\"-n\"",
    "\"--amend\"",
    "\"--mirror\"",
    "\"--all\"",
    "\"--tags\"",
    "\"--delete\"",
    "\"-d\"",
    "\"-D\"",
    "\"-C\"",
    "\"--force-create\"",
    "\"--orphan\"",
    "\"--discard-changes\"",
    "\"--detach\"",
    "\"--prune\"",
    "\"--allow-empty\"",
    "\"--no-gpg-sign\"",
    "\"-c\"",
    "\"--admin\"",
    "\"--auto\"",
    "\"--delete-branch\"",
    "\"--fill\"",
    "\"--web\"",
    "\"--editor\"",
    "\"api\"",
    "\"token\"",
];

/// One word one file may hold: `gh pr merge` is `gh`'s own subcommand. `git merge` can't happen
/// because `only_argv_names_git_commands` pins every git command to `argv.rs`.
const SCM_EXEMPT: [(&str, &str); 1] = [("host/scm/gh.rs", "\"merge\"")];

fn banned_arguments(
    dir: &Path,
    words: &[&str],
    exempt: &[(&str, &str)],
    floor: usize,
    label: &str,
) -> Vec<String> {
    let mut files = Vec::new();
    sources(dir, &mut files);
    assert!(files.len() >= floor, "the scan found only {} files in {label}; it moved", files.len());

    let mut found = Vec::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        found.extend(banned_lines(file, &shipped(file, &text), words, exempt));
    }
    found
}

/// The lines of `code` (from `file`) that hold a banned word, comment lines skipped.
fn banned_lines(file: &Path, code: &str, words: &[&str], exempt: &[(&str, &str)]) -> Vec<String> {
    let mut found = Vec::new();
    for (number, line) in code.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        for word in words {
            let allowed = exempt.iter().any(|(path, w)| w == word && file.ends_with(path));
            if line.contains(word) && !allowed {
                found.push(format!("{}:{}: {word}", file.display(), number + 1));
            }
        }
    }
    found
}

#[test]
fn restore_points_never_run_a_destructive_git_command() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/host/git");
    let found = banned_arguments(&dir, &GIT_NEVER, &[], 4, "host/git");
    assert!(found.is_empty(), "destructive git arguments in host/git:\n{}", found.join("\n"));
}

#[test]
fn source_control_never_runs_a_destructive_or_unlisted_argument() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/host/scm");
    let mut files = Vec::new();
    sources(&dir, &mut files);
    if files.is_empty() {
        eprintln!("host/scm has no sources yet: the SCM_NEVER guard is skipped until track G adds them");
        return;
    }
    let found = banned_arguments(&dir, &SCM_NEVER, &SCM_EXEMPT, 3, "host/scm");
    assert!(found.is_empty(), "banned arguments in host/scm:\n{}", found.join("\n"));
}

#[test]
fn the_merge_word_is_allowed_in_gh_rs_only() {
    let code = "let v = [\"pr\", \"merge\"];\nlet w = \"rebase\";\n// \"reset\" in a comment\n";
    let in_gh = banned_lines(Path::new("src/host/scm/gh.rs"), code, &SCM_NEVER, &SCM_EXEMPT);
    assert_eq!(in_gh, ["src/host/scm/gh.rs:2: \"rebase\""], "only the exempt word passes in gh.rs");
    for other in ["src/host/scm/argv.rs", "src/host/scm/status.rs", "src/host/scm/xgh.rs", "src/host/gh.rs"] {
        let found = banned_lines(Path::new(other), code, &SCM_NEVER, &SCM_EXEMPT);
        assert_eq!(found.len(), 2, "{other}: {found:?}");
        assert!(found[0].ends_with(": \"merge\""), "{other}: {found:?}");
    }
    let in_git = banned_lines(Path::new("src/host/git/run.rs"), code, &GIT_NEVER, &[]);
    assert!(in_git.iter().any(|l| l.ends_with(": \"merge\"")), "host/git has no exemption");
}

/// Every subcommand word of source control's closed git list (phase 4 contract §SC3.1, with
/// `worktree`'s own `list`, `add` and `remove`), and of its `gh` list (§SC4.1). Quoted, they may
/// appear in `host/scm/argv.rs` (git's) and `host/scm/gh.rs` (gh's) and nowhere else in `host/scm`,
/// so every command source control can run is one of those two files' lists.
const SCM_GIT_WORDS: [&str; 20] = [
    "\"rev-parse\"",
    "\"status\"",
    "\"diff-index\"",
    "\"diff\"",
    "\"hash-object\"",
    "\"var\"",
    "\"for-each-ref\"",
    "\"config\"",
    "\"symbolic-ref\"",
    "\"check-ref-format\"",
    "\"rev-list\"",
    "\"log\"",
    "\"worktree\"",
    "\"list\"",
    "\"add\"",
    "\"remove\"",
    "\"commit\"",
    "\"push\"",
    "\"fetch\"",
    "\"switch\"",
];
const SCM_GH_WORDS: [&str; 12] = [
    "\"--version\"",
    "\"auth\"",
    "\"status\"",
    "\"pr\"",
    "\"list\"",
    "\"create\"",
    "\"search\"",
    "\"prs\"",
    "\"repo\"",
    "\"view\"",
    "\"ready\"",
    "\"merge\"",
];

#[test]
fn only_argv_names_git_commands() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/host/scm");
    let mut files = Vec::new();
    sources(&dir, &mut files);
    if files.is_empty() {
        eprintln!("host/scm has no sources yet: the subcommand guard is skipped until track G adds them");
        return;
    }
    assert!(files.len() >= 3, "the scan found only {} files in host/scm; it moved", files.len());
    for named in ["argv.rs", "gh.rs"] {
        assert!(
            files.iter().any(|f| f.ends_with(named)),
            "host/scm/{named} is missing; the guard's lists moved"
        );
    }

    let mut found = Vec::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        let code = shipped(file, &text);
        let allowed: &[&str] = if file.ends_with("argv.rs") {
            &SCM_GIT_WORDS
        } else if file.ends_with("gh.rs") {
            &SCM_GH_WORDS
        } else {
            &[]
        };
        for (number, line) in code.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for word in SCM_GIT_WORDS.iter().chain(&SCM_GH_WORDS) {
                if line.contains(word) && !allowed.contains(word) {
                    found.push(format!("{}:{}: {word}", file.display(), number + 1));
                }
            }
        }
    }
    found.dedup();
    assert!(
        found.is_empty(),
        "subcommand words outside argv.rs and gh.rs in host/scm:\n{}",
        found.join("\n")
    );
}
