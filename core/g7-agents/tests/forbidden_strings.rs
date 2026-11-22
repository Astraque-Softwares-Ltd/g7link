// SPDX-License-Identifier: MPL-2.0
//! Strings that name something G7 must never send, call or open (phase 2 contract §Q7). They may
//! only appear in tests that check they are absent, so this looks at shipped code only: Rust before
//! its first `#[cfg(test)]`, and the desktop and Android sources outside their test folders.

use std::path::{Path, PathBuf};

/// Phase 6 §X6.2 item 3 adds GitHub's write commands: G7 posts nothing to GitHub. Shipped comments
/// and copy say "GitHub's write commands" instead of naming them.
const FORBIDDEN: [&str; 21] = [
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
    "pr review",
    "pr comment",
    "pr edit",
    "pr close",
    "pr reopen",
    "gh api",
    "issue comment",
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

/// Words that `host/scm` never writes as an argument, in any file (phase 4 contract §SC3.6 item 2),
/// and from phase 6 (§X6.2 item 2) every `gh` word that would post, edit or reach past the two pull
/// request reads.
const SCM_NEVER: [&str; 83] = [
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
    "\"review\"",
    "\"comment\"",
    "\"edit\"",
    "\"close\"",
    "\"reopen\"",
    "\"issue\"",
    "\"release\"",
    "\"workflow\"",
    "\"run\"",
    "\"secret\"",
    "\"variable\"",
    "\"label\"",
    "\"gist\"",
    "\"extension\"",
    "\"alias\"",
    "\"codespace\"",
    "\"lock\"",
    "\"unlock\"",
    "\"update-branch\"",
    "\"graphql\"",
    "\"--approve\"",
    "\"--request-changes\"",
    "\"--comment\"",
    "\"--body\"",
    "\"-X\"",
    "\"--method\"",
    "\"-F\"",
    "\"--field\"",
    "\"--raw-field\"",
    "\"--exclude\"",
    "\"--allow-escape-sequences\"",
    "\"--jq\"",
    "\"--template\"",
    "\"-R\"",
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
/// `worktree`'s own `list`, `add` and `remove`), and of its `gh` list (§SC4.1, and phase 6 §X6.1's
/// `pr diff`). Quoted, they may appear in `host/scm/argv.rs` (git's) and `host/scm/gh.rs` (gh's) and
/// nowhere else in `host/scm`, so every command source control can run is one of those two files'
/// lists.
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
const SCM_GH_WORDS: [&str; 13] = [
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
    "\"diff\"",
];

/// The lines of `code` (from `file` in `host/scm`) that hold a subcommand word the file may not
/// write, comment lines skipped.
fn unlisted_words(file: &Path, code: &str) -> Vec<String> {
    let allowed: &[&str] = if file.ends_with("argv.rs") {
        &SCM_GIT_WORDS
    } else if file.ends_with("gh.rs") {
        &SCM_GH_WORDS
    } else {
        &[]
    };
    let mut words: Vec<&str> = SCM_GIT_WORDS.to_vec();
    words.extend(SCM_GH_WORDS.iter().filter(|w| !SCM_GIT_WORDS.contains(w)));
    let mut found = Vec::new();
    for (number, line) in code.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        for word in &words {
            if line.contains(word) && !allowed.contains(word) {
                found.push(format!("{}:{}: {word}", file.display(), number + 1));
            }
        }
    }
    found
}

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
        found.extend(unlisted_words(file, &shipped(file, &text)));
    }
    assert!(
        found.is_empty(),
        "subcommand words outside argv.rs and gh.rs in host/scm:\n{}",
        found.join("\n")
    );
}

/// What project scripts (`host/scripts`, phase 6 §X3) never write as an argument: a file a
/// repository could use to define what runs, another process's `environ` or `cmdline`, a privilege
/// change, or a flag that makes the user's shell a login or interactive one. A script's command
/// comes from `folders.json` only and reaches the shell as typed. Never shorten this.
const SCRIPTS_NEVER: [&str; 12] = [
    "\"package.json\"",
    "\"t3.json\"",
    "\"Makefile\"",
    "\"justfile\"",
    "\"tasks.json\"",
    "\"Procfile\"",
    "\"environ\"",
    "\"cmdline\"",
    "\"sudo\"",
    "\"-i\"",
    "\"-l\"",
    "\"--login\"",
];

/// Each word of [`SCRIPTS_NEVER`] quoted, and also as the last part of a path in a string
/// (`"/proc/{pid}/environ"`, `"{dir}/Makefile"`).
fn scripts_never_forms() -> Vec<String> {
    SCRIPTS_NEVER.iter().flat_map(|w| [(*w).to_owned(), w.replacen('"', "/", 1)]).collect()
}

/// The lines of `code` that start a process, comment lines skipped.
fn spawning_lines(file: &Path, code: &str) -> Vec<String> {
    code.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//") && line.contains("Command::new"))
        .map(|(number, _)| format!("{}:{}: Command::new", file.display(), number + 1))
        .collect()
}

/// `host/scripts`' shipped sources; empty while track H has not added it.
fn scripts_sources() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/host/scripts");
    let mut files = Vec::new();
    sources(&dir, &mut files);
    files
}

#[test]
fn scripts_never_read_a_command_from_the_folder_or_another_process() {
    let files = scripts_sources();
    if files.is_empty() {
        eprintln!(
            "host/scripts has no sources yet: the SCRIPTS_NEVER guard is skipped until track H adds them"
        );
        return;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/host/scripts");
    let forms = scripts_never_forms();
    let forms: Vec<&str> = forms.iter().map(String::as_str).collect();
    let found = banned_arguments(&dir, &forms, &[], 2, "host/scripts");
    assert!(found.is_empty(), "banned words in host/scripts:\n{}", found.join("\n"));
}

#[test]
fn scripts_spawn_in_one_file() {
    let files = scripts_sources();
    if files.is_empty() {
        eprintln!("host/scripts has no sources yet: the spawn guard is skipped until track H adds them");
        return;
    }
    assert!(files.len() >= 2, "the scan found only {} files in host/scripts; it moved", files.len());
    assert!(
        files.iter().any(|f| f.ends_with("spawn.rs")),
        "host/scripts/spawn.rs is missing; the guard's file moved"
    );
    let mut found = Vec::new();
    for file in files.iter().filter(|f| !f.ends_with("spawn.rs")) {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        found.extend(spawning_lines(file, &shipped(file, &text)));
    }
    assert!(found.is_empty(), "a process started outside host/scripts/spawn.rs:\n{}", found.join("\n"));
}

// ---- Each list catches its words and passes clean code ----

#[test]
fn forbidden_words_are_caught_and_plain_code_passes() {
    for word in FORBIDDEN {
        assert!(names(&format!("let s = \"{word}\";"), word), "{word}");
        assert!(names(&format!("// G7 never runs `{word}` here"), word), "{word} in a comment");
    }
    let clean = [
        "let argv = [\"pr\", \"view\", \"212\", \"--repo\", \"o/r\"];",
        "// GitHub's write commands are never run; `gh pr view` and `gh pr diff` only read.",
        "Text { text: qsTr(\"Ask %1 to review it\") }",
        "<string name=\"agents_pr_detail_comments\">Comments</string>",
        "let token = oauth token;",
    ];
    for line in clean {
        for word in FORBIDDEN {
            assert!(!names(line, word), "{word} in {line}");
        }
    }
}

#[test]
fn every_banned_argument_is_caught_and_plain_code_passes() {
    let lists: [(&str, &[&str]); 2] =
        [("src/host/git/run.rs", &GIT_NEVER), ("src/host/scm/push.rs", &SCM_NEVER)];
    for (file, words) in lists {
        for word in words {
            let line = format!("let v = words(&[{word}]);");
            let found = banned_lines(Path::new(file), &line, words, &[]);
            assert_eq!(found.len(), 1, "{file}: {word}: {found:?}");
            let commented = format!("// {word}");
            assert!(banned_lines(Path::new(file), &commented, words, &[]).is_empty(), "{word} in a comment");
        }
    }
    let reads = "let v = words(&[\"pr\", \"view\", \"--repo\", \"--json\", \"--color\", \"never\", \"--body-file\", \"-\"]);\n\
                 let w = [\"--head\", \"--base\", \"--title=-T\", \"--draft\", \"--match-head-commit\", \"--squash\"];\n";
    assert!(banned_lines(Path::new("src/host/scm/gh.rs"), reads, &SCM_NEVER, &SCM_EXEMPT).is_empty());
    for word in ["\"review\"", "\"comment\"", "\"--body\"", "\"-R\"", "\"--jq\"", "\"graphql\""] {
        assert!(SCM_NEVER.contains(&word), "{word} left SCM_NEVER");
    }
}

#[test]
fn subcommand_words_are_pinned_to_their_file() {
    let gh = "let v = words(&[\"pr\", \"diff\", \"--color\", \"never\"]);\n";
    assert!(unlisted_words(Path::new("src/host/scm/gh.rs"), gh).is_empty(), "gh.rs may write pr diff");
    assert!(unlisted_words(Path::new("src/host/scm/argv.rs"), "let v = [\"diff\", \"--patch\"];").is_empty());
    let elsewhere = unlisted_words(Path::new("src/host/scm/patch.rs"), gh);
    assert_eq!(elsewhere.len(), 2, "{elsewhere:?}");
    let view_in_argv = unlisted_words(Path::new("src/host/scm/argv.rs"), "let v = [\"view\"];");
    assert_eq!(view_in_argv.len(), 1, "a gh word in argv.rs: {view_in_argv:?}");
    let commit_in_gh = unlisted_words(Path::new("src/host/scm/gh.rs"), "let v = [\"commit\"];");
    assert_eq!(commit_in_gh.len(), 1, "a git word in gh.rs: {commit_in_gh:?}");
    assert!(unlisted_words(Path::new("src/host/scm/gh.rs"), "// \"commit\" in a comment").is_empty());
}

#[test]
fn scripts_words_are_caught_quoted_or_as_a_path_and_plain_code_passes() {
    let forms = scripts_never_forms();
    let forms: Vec<&str> = forms.iter().map(String::as_str).collect();
    let file = Path::new("src/host/scripts/spawn.rs");
    for word in SCRIPTS_NEVER {
        let bare = word.trim_matches('"');
        for line in [format!("cmd.arg({word});"), format!("let p = format!(\"/proc/{{pid}}/{bare}\");")] {
            assert!(!banned_lines(file, &line, &forms, &[]).is_empty(), "{line}");
        }
    }
    let clean = "let mut cmd = Command::new(&shell);\n\
                 cmd.arg(\"-c\").arg(&command).env_clear().envs(environment.iter());\n\
                 // never reads package.json or another process's environ\n\
                 let names = [\"CLAUDECODE\", \"GIT_DIR\", \"GIT_WORK_TREE\", \"GIT_INDEX_FILE\"];\n";
    assert!(banned_lines(file, clean, &forms, &[]).is_empty());
    assert!(spawning_lines(Path::new("src/host/scripts/mod.rs"), clean).len() == 1);
    assert!(
        spawning_lines(Path::new("src/host/scripts/mod.rs"), "// Command::new only in spawn.rs").is_empty()
    );
}
