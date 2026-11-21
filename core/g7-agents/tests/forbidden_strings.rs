// SPDX-License-Identifier: MPL-2.0
//! Strings that name something G7 must never send, call or open (phase 2 contract §Q7). They may
//! only appear in tests that check they are absent, so this looks at shipped code only: Rust before
//! its first `#[cfg(test)]`, and the desktop and Android sources outside their test folders.

use std::path::{Path, PathBuf};

const FORBIDDEN: [&str; 6] = [
    "apply_flag_settings",
    "thinking-display",
    "rateLimitResetCredit",
    "account/usage/read",
    ".mcp.json",
    "claude mcp",
];

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
                if line.contains(word) {
                    found.push(format!("{}:{}: {word}", file.display(), number + 1));
                }
            }
        }
    }
    assert!(found.is_empty(), "forbidden strings in shipped code:\n{}", found.join("\n"));
}

/// Git subcommands and flags that rewrite the user's work tree, index or branches (phase 3 contract
/// §U5.5). Restore points only read the user's repository and write G7's own, so none of these may
/// appear as an argument in `host/git`.
const GIT_NEVER: [&str; 7] =
    ["\"clean\"", "\"reset\"", "\"checkout\"", "\"stash\"", "\"--staged\"", "\"--force\"", "\"-f\""];

#[test]
fn restore_points_never_run_a_destructive_git_command() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/host/git");
    let mut files = Vec::new();
    sources(&dir, &mut files);
    assert!(files.len() >= 4, "the scan found only {} files in host/git; it moved", files.len());

    let mut found = Vec::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        let code = shipped(file, &text);
        for (number, line) in code.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for word in GIT_NEVER {
                if line.contains(word) {
                    found.push(format!("{}:{}: {word}", file.display(), number + 1));
                }
            }
        }
    }
    assert!(found.is_empty(), "destructive git arguments in host/git:\n{}", found.join("\n"));
}
