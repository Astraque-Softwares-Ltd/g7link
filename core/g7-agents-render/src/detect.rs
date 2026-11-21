// SPDX-License-Identifier: MPL-2.0
//! The language of code that names none: a fence without a word, or a
//! command's output.
//!
//! The first line decides when it can (a shebang, `<?xml`, `<?php`, an HTML
//! doctype). Otherwise each language scores literal markers on the first
//! lines: no regular expressions and no grammar, so the launcher's C++ port
//! gives the same word. The best score must reach `THRESHOLD` and beat the
//! next by `MARGIN`; a file extension seen nearby only breaks a tie or lifts
//! a near miss, and never overrides the first line.

use crate::bounds::{MAX_DETECT_BYTES, MAX_DETECT_LINES, cut_bytes};

/// The languages the markers score, by index. Each word is one
/// `highlight` takes.
const WORDS: [&str; 16] = [
    "diff",
    "json",
    "bash",
    "rust",
    "python",
    "typescript",
    "javascript",
    "kotlin",
    "c++",
    "go",
    "html",
    "css",
    "yaml",
    "toml",
    "sql",
    "qml",
];

const DIFF: usize = 0;
const JSON: usize = 1;
const SHELL: usize = 2;
const RUST: usize = 3;
const PYTHON: usize = 4;
const TS: usize = 5;
const JS: usize = 6;
const KOTLIN: usize = 7;
const CPP: usize = 8;
const GO: usize = 9;
const HTML: usize = 10;
const CSS: usize = 11;
const YAML: usize = 12;
const TOML: usize = 13;
const SQL: usize = 14;
const QML: usize = 15;

/// The best score must reach this,
const THRESHOLD: u32 = 4;
/// and beat the runner-up by this.
const MARGIN: u32 = 2;
/// A hinted language needs at least this, and to be within one of the best.
const HINT_FLOOR: u32 = 2;
/// One marker counts on at most this many lines.
const RULE_CAP: u32 = 3;

/// A test on one line, with its leading and trailing whitespace trimmed.
#[derive(Clone, Copy)]
enum M {
    Starts(&'static str),
    Has(&'static str),
    Ends(&'static str),
    Is(&'static str),
    /// Starts with the first and contains the second after it.
    StartsHas(&'static str, &'static str),
    /// Starts with the first and ends with the second.
    StartsEnds(&'static str, &'static str),
}

impl M {
    fn test(self, line: &str) -> bool {
        match self {
            M::Starts(a) => line.starts_with(a),
            M::Has(a) => line.contains(a),
            M::Ends(a) => line.ends_with(a),
            M::Is(a) => line == a,
            M::StartsHas(a, b) => line.strip_prefix(a).is_some_and(|rest| rest.contains(b)),
            M::StartsEnds(a, b) => {
                line.len() >= a.len() + b.len() && line.starts_with(a) && line.ends_with(b)
            }
        }
    }
}

/// `(language, test, points)`. TypeScript's own markers are in `TS_RULES`.
const RULES: &[(usize, M, u32)] = &[
    (DIFF, M::Starts("@@ -"), 4),
    (DIFF, M::Starts("diff --git "), 3),
    (DIFF, M::Starts("--- a/"), 2),
    (DIFF, M::Starts("+++ b/"), 2),
    (SHELL, M::Starts("$ "), 3),
    (SHELL, M::Has(" | "), 1),
    (SHELL, M::Has(" && "), 1),
    (SHELL, M::Ends(" \\"), 2),
    (SHELL, M::Has("$("), 1),
    (SHELL, M::Has(" 2>&1"), 2),
    (SHELL, M::Has(" > /dev/null"), 2),
    (SHELL, M::Starts("if [ "), 3),
    (SHELL, M::Starts("if [[ "), 3),
    (SHELL, M::Is("fi"), 2),
    (SHELL, M::Is("esac"), 3),
    (SHELL, M::StartsHas("for ", "; do"), 3),
    (RUST, M::Starts("fn "), 3),
    (RUST, M::Starts("pub fn "), 3),
    (RUST, M::Starts("pub(crate) fn "), 3),
    (RUST, M::Starts("async fn "), 2),
    (RUST, M::Starts("impl "), 2),
    (RUST, M::Starts("impl<"), 2),
    (RUST, M::Starts("let mut "), 2),
    (RUST, M::Starts("use std::"), 3),
    (RUST, M::Starts("use crate::"), 3),
    (RUST, M::Starts("use super::"), 3),
    (RUST, M::Starts("#[derive("), 3),
    (RUST, M::Starts("#[test]"), 3),
    (RUST, M::Starts("pub struct "), 2),
    (RUST, M::Starts("pub enum "), 2),
    (RUST, M::Has("println!("), 2),
    (RUST, M::Has("format!("), 2),
    (RUST, M::Has("vec!["), 2),
    (RUST, M::Has(".unwrap()"), 2),
    (RUST, M::Has("-> Result<"), 2),
    (RUST, M::Has("&mut "), 1),
    (RUST, M::Has("Some("), 1),
    (PYTHON, M::StartsEnds("def ", ":"), 3),
    (PYTHON, M::StartsHas("from ", " import "), 3),
    (PYTHON, M::Starts("import "), 1),
    (PYTHON, M::Starts("elif "), 3),
    (PYTHON, M::StartsEnds("class ", ":"), 3),
    (PYTHON, M::StartsEnds("if ", ":"), 1),
    (PYTHON, M::StartsEnds("for ", ":"), 1),
    (PYTHON, M::Has("self."), 1),
    (PYTHON, M::Starts("print("), 1),
    (PYTHON, M::Has("__name__"), 2),
    (PYTHON, M::Has(" is not "), 1),
    (JS, M::Starts("const "), 1),
    (JS, M::Starts("function "), 2),
    (JS, M::Has(" => "), 1),
    (JS, M::Has("require("), 2),
    (JS, M::Has("module.exports"), 3),
    (JS, M::Has("console.log("), 2),
    (JS, M::StartsHas("import ", " from '"), 2),
    (JS, M::StartsHas("import ", " from \""), 2),
    (JS, M::Starts("export default "), 2),
    (JS, M::Starts("export const "), 2),
    (JS, M::Starts("export function "), 2),
    (JS, M::Has("document."), 2),
    (JS, M::Has(" === "), 2),
    (JS, M::Has(" !== "), 2),
    (JS, M::Starts("var "), 1),
    (KOTLIN, M::Starts("fun "), 3),
    (KOTLIN, M::Starts("private fun "), 3),
    (KOTLIN, M::Starts("override fun "), 3),
    (KOTLIN, M::Starts("suspend fun "), 3),
    (KOTLIN, M::Starts("internal fun "), 3),
    (KOTLIN, M::Starts("val "), 1),
    (KOTLIN, M::Starts("data class "), 3),
    (KOTLIN, M::Has("companion object"), 3),
    (KOTLIN, M::Has("?.let"), 2),
    (KOTLIN, M::Has(": Unit"), 2),
    (KOTLIN, M::Starts("when ("), 2),
    (KOTLIN, M::Starts("println("), 1),
    (CPP, M::Starts("#include <"), 3),
    (CPP, M::Starts("#include \""), 3),
    (CPP, M::Has("std::"), 2),
    (CPP, M::Starts("int main("), 2),
    (CPP, M::Starts("namespace "), 2),
    (CPP, M::Starts("template<"), 3),
    (CPP, M::Starts("template <"), 3),
    (CPP, M::Has("nullptr"), 2),
    (CPP, M::Starts("#pragma once"), 3),
    (CPP, M::Has("cout <<"), 2),
    (CPP, M::Is("public:"), 2),
    (CPP, M::Is("private:"), 2),
    (CPP, M::Starts("using namespace "), 3),
    (CPP, M::Has("const auto&"), 2),
    (GO, M::Starts("func "), 3),
    (GO, M::Has(" := "), 2),
    (GO, M::Starts("import ("), 2),
    (GO, M::Has("fmt."), 2),
    (GO, M::Has("err != nil"), 3),
    (GO, M::Starts("go func"), 2),
    (GO, M::Starts("package main"), 3),
    (GO, M::Starts("defer "), 2),
    (HTML, M::Starts("<html"), 3),
    (HTML, M::Starts("<head"), 3),
    (HTML, M::Starts("<body"), 3),
    (HTML, M::Starts("<div"), 2),
    (HTML, M::Has("</div>"), 1),
    (HTML, M::Starts("<p>"), 1),
    (HTML, M::Has("</p>"), 1),
    (HTML, M::Starts("<script"), 2),
    (HTML, M::Has("<a href="), 2),
    (HTML, M::Starts("<span"), 1),
    (HTML, M::Starts("<ul"), 1),
    (HTML, M::Starts("<li>"), 1),
    (HTML, M::Starts("<meta "), 2),
    (HTML, M::Starts("<link "), 2),
    (CSS, M::Starts("@media"), 3),
    (CSS, M::Starts("@import"), 2),
    (CSS, M::Starts(":root"), 3),
    (CSS, M::Has("!important"), 2),
    (CSS, M::Ends("px;"), 1),
    (CSS, M::Starts("@keyframes"), 3),
    (CSS, M::Starts("@font-face"), 3),
    (CSS, M::StartsEnds(".", "{"), 1),
    (CSS, M::StartsEnds("#", "{"), 1),
    (CSS, M::StartsHas(".", " { "), 2),
    (CSS, M::StartsHas("#", " { "), 2),
    (YAML, M::Is("---"), 2),
    (YAML, M::Starts("apiVersion:"), 3),
    (YAML, M::Starts("runs-on:"), 3),
    (YAML, M::Starts("steps:"), 2),
    (YAML, M::Starts("jobs:"), 2),
    (YAML, M::Starts("- name:"), 2),
    (YAML, M::Starts("- uses:"), 3),
    (TOML, M::StartsEnds("[[", "]]"), 3),
    (TOML, M::Is("[package]"), 2),
    (TOML, M::Is("[dependencies]"), 2),
    (SQL, M::Starts("SELECT "), 3),
    (SQL, M::Starts("select "), 3),
    (SQL, M::Has(" FROM "), 2),
    (SQL, M::Starts("FROM "), 2),
    (SQL, M::Starts("WHERE "), 2),
    (SQL, M::Starts("INSERT INTO "), 3),
    (SQL, M::Starts("insert into "), 3),
    (SQL, M::Starts("CREATE TABLE "), 3),
    (SQL, M::Starts("create table "), 3),
    (SQL, M::Starts("ALTER TABLE "), 3),
    (SQL, M::Starts("DELETE FROM "), 3),
    (SQL, M::Has(" JOIN "), 2),
    (SQL, M::Has(" PRIMARY KEY"), 2),
    (SQL, M::Has(" NOT NULL"), 1),
    (SQL, M::Starts("GROUP BY "), 2),
    (SQL, M::Starts("ORDER BY "), 2),
    (QML, M::Starts("import QtQuick"), 4),
    (QML, M::Starts("anchors."), 3),
    (QML, M::Starts("property "), 2),
    (QML, M::Starts("signal "), 2),
    (QML, M::Starts("id: "), 1),
    (QML, M::Starts("onClicked:"), 3),
];

/// TypeScript's own markers. TypeScript scores JavaScript's points plus
/// these when any of these match, and nothing otherwise.
const TS_RULES: &[(M, u32)] = &[
    (M::Starts("interface "), 2),
    (M::Starts("export interface "), 3),
    (M::Has(": string"), 2),
    (M::Has(": number"), 2),
    (M::Has(": boolean"), 2),
    (M::StartsHas("type ", " = "), 2),
    (M::Starts("export type "), 2),
    (M::Has(" as const"), 2),
    (M::Has("): Promise<"), 2),
    (M::Has(": void"), 2),
];

/// First words that make a line a shell command.
const COMMANDS: &[&str] = &[
    "apt",
    "apt-get",
    "awk",
    "bash",
    "brew",
    "cargo",
    "cat",
    "cd",
    "chmod",
    "chown",
    "cmake",
    "cp",
    "curl",
    "dnf",
    "docker",
    "echo",
    "find",
    "flatpak",
    "gh",
    "git",
    "go",
    "grep",
    "journalctl",
    "kubectl",
    "ln",
    "ls",
    "make",
    "mkdir",
    "mv",
    "nix",
    "node",
    "npm",
    "npx",
    "pacman",
    "pip",
    "pip3",
    "pnpm",
    "python",
    "python3",
    "rg",
    "rm",
    "rustup",
    "scp",
    "sed",
    "sh",
    "ssh",
    "sudo",
    "systemctl",
    "tar",
    "touch",
    "unzip",
    "wget",
    "yarn",
];

/// File extensions to the language word a hint stands for.
const EXTENSIONS: &[(&str, &str)] = &[
    ("rs", "rust"),
    ("py", "python"),
    ("pyi", "python"),
    ("ts", "typescript"),
    ("tsx", "typescript"),
    ("mts", "typescript"),
    ("cts", "typescript"),
    ("js", "javascript"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("jsx", "javascript"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("cpp", "c++"),
    ("cc", "c++"),
    ("cxx", "c++"),
    ("hpp", "c++"),
    ("hh", "c++"),
    ("h", "c++"),
    ("go", "go"),
    ("html", "html"),
    ("htm", "html"),
    ("css", "css"),
    ("yml", "yaml"),
    ("yaml", "yaml"),
    ("toml", "toml"),
    ("sql", "sql"),
    ("qml", "qml"),
    ("json", "json"),
    ("sh", "bash"),
    ("bash", "bash"),
    ("zsh", "bash"),
    ("diff", "diff"),
    ("patch", "diff"),
];

/// The language word for `code`, or `None` when nothing is clear enough.
/// `hint_ext` is the extension of a file the same turn edited or read,
/// without its dot, any case.
pub fn guess(code: &str, hint_ext: Option<&str>) -> Option<&'static str> {
    let code = cut_bytes(code, MAX_DETECT_BYTES);
    let lines: Vec<&str> =
        code.split('\n').map(str::trim).filter(|l| !l.is_empty()).take(MAX_DETECT_LINES).collect();
    let first = lines.first()?;
    if let Some(word) = first_line(first) {
        return Some(word);
    }
    let scores = scores(code.trim_start(), &lines);
    let mut order: Vec<usize> = (0..WORDS.len()).collect();
    // Highest first; equal scores keep the table's order.
    order.sort_by(|&a, &b| scores[b].cmp(&scores[a]));
    let (best, second) = (scores[order[0]], scores[order[1]]);
    if best >= THRESHOLD && best - second >= MARGIN {
        return Some(WORDS[order[0]]);
    }
    let hinted = hint_ext.and_then(hint_word)?;
    let k = WORDS.iter().position(|w| *w == hinted)?;
    (scores[k] >= HINT_FLOOR && scores[k] + 1 >= best).then_some(WORDS[k])
}

/// The language word a file extension stands for, any case, with or
/// without its dot.
pub(crate) fn hint_word(ext: &str) -> Option<&'static str> {
    let ext = ext.strip_prefix('.').unwrap_or(ext);
    EXTENSIONS.iter().find(|(e, _)| e.eq_ignore_ascii_case(ext)).map(|(_, w)| *w)
}

/// What the first line alone says: a shebang's interpreter, an XML or PHP
/// opening, an HTML doctype or root.
fn first_line(line: &str) -> Option<&'static str> {
    if let Some(rest) = line.strip_prefix("#!") {
        let mut words = rest.split_whitespace();
        let mut name = words.next()?.rsplit('/').next().unwrap_or_default();
        if name == "env" {
            name = words.find(|w| !w.starts_with('-'))?;
        }
        return interpreter(name);
    }
    let lower = line.to_ascii_lowercase();
    if lower.starts_with("<?xml") {
        Some("xml")
    } else if lower.starts_with("<?php") {
        Some("php")
    } else if lower.starts_with("<!doctype html") || lower.starts_with("<html") {
        Some("html")
    } else {
        None
    }
}

fn interpreter(name: &str) -> Option<&'static str> {
    match name {
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" => Some("bash"),
        "node" | "nodejs" | "deno" | "bun" => Some("javascript"),
        "ts-node" | "tsx" => Some("typescript"),
        "fish" => Some("fish"),
        "ruby" => Some("ruby"),
        "perl" => Some("perl"),
        "php" => Some("php"),
        "lua" => Some("lua"),
        n if n.starts_with("python") => Some("python"),
        _ => None,
    }
}

/// Each language's score over `lines` (trimmed, non-empty); `text` is the
/// code without its leading whitespace.
fn scores(text: &str, lines: &[&str]) -> [u32; WORDS.len()] {
    let mut scores = [0u32; WORDS.len()];
    let count = |test: &dyn Fn(&str) -> bool| -> u32 {
        u32::try_from(lines.iter().filter(|l| test(l)).count()).unwrap_or(u32::MAX).min(RULE_CAP)
    };
    for &(lang, m, points) in RULES {
        scores[lang] += points * count(&|l| m.test(l));
    }
    scores[SHELL] += 2 * count(&shell_command);
    scores[SHELL] += 2 * count(&shell_assignment);
    if json_opens(text) {
        scores[JSON] += 3 + count(&|l| (l.starts_with('"') || l.starts_with("{\"")) && l.contains("\":"));
    }
    scores[YAML] += 2 * count(&yaml_key);
    scores[TOML] += 2 * count(&toml_section) + count(&toml_key);
    scores[CSS] += count(&css_declaration);
    scores[QML] += 2 * count(&qml_object);
    let ts: u32 = TS_RULES.iter().map(|&(m, points)| points * count(&|l| m.test(l))).sum();
    if ts > 0 {
        scores[TS] = scores[JS] + ts;
    }
    scores
}

/// The line's first word (after a `$ ` prompt or `sudo`) is a command, or
/// runs a script in this folder.
fn shell_command(line: &str) -> bool {
    let line = line.strip_prefix("$ ").unwrap_or(line);
    let mut words = line.split_whitespace();
    let mut first = words.next().unwrap_or_default();
    if first == "sudo" {
        first = words.next().unwrap_or_default();
    }
    first.starts_with("./") || COMMANDS.contains(&first)
}

/// `NAME=value`, maybe after `export`: an upper-case name glued to `=`.
fn shell_assignment(line: &str) -> bool {
    let line = line.strip_prefix("export ").unwrap_or(line);
    let Some((name, _)) = line.split_once('=') else { return false };
    !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_uppercase() || c == '_')
        && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Text that opens like JSON: `{` or `[` followed by what a JSON value can
/// start with (so a TOML `[section]` doesn't).
fn json_opens(text: &str) -> bool {
    let mut chars = text.chars().filter(|c| !c.is_whitespace());
    matches!(chars.next(), Some('{' | '['))
        && chars.next().is_some_and(|c| {
            matches!(c, '{' | '[' | '"' | ']' | '}' | '-' | 't' | 'f' | 'n') || c.is_ascii_digit()
        })
}

/// `key:` or `key: value`, `key` a plain name, not ending like code.
fn yaml_key(line: &str) -> bool {
    let line = line.strip_prefix("- ").unwrap_or(line);
    let Some((key, rest)) = line.split_once(':') else { return false };
    let plain_key = key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    plain_key
        && (rest.is_empty() || rest.starts_with(' '))
        && !rest.ends_with([';', '{', ',', '('])
        && !line.contains(['(', '=', '"'])
}

/// `[section]` or `[a.b]`.
fn toml_section(line: &str) -> bool {
    line.strip_prefix('[').and_then(|l| l.strip_suffix(']')).is_some_and(|inner| {
        !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    })
}

/// `key = value`, with a value TOML would write: a string, number, array,
/// table or boolean.
fn toml_key(line: &str) -> bool {
    let Some((key, value)) = line.split_once(" = ") else { return false };
    !key.is_empty()
        && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && (value.starts_with(['"', '\'', '[', '{'])
            || value.starts_with(|c: char| c.is_ascii_digit())
            || value == "true"
            || value == "false")
}

/// `property: value;` with a lower-case, dashed property.
fn css_declaration(line: &str) -> bool {
    let Some((prop, value)) = line.split_once(':') else { return false };
    !prop.is_empty()
        && prop.chars().all(|c| c.is_ascii_lowercase() || c == '-')
        && value.starts_with(' ')
        && value.ends_with(';')
}

/// `Type {`: a capitalised (maybe dotted) name opening an object.
fn qml_object(line: &str) -> bool {
    line.strip_suffix(" {").is_some_and(|name| {
        name.starts_with(|c: char| c.is_ascii_uppercase())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight::find_syntax;

    #[test]
    fn every_word_has_a_grammar() {
        for word in WORDS.iter().chain(&["xml", "php", "ruby", "perl", "lua", "fish"]) {
            assert!(find_syntax(word).is_some(), "{word}");
        }
        for (_, word) in EXTENSIONS {
            assert!(WORDS.contains(word), "{word}");
        }
    }

    #[test]
    fn clear_cases() {
        assert_eq!(guess("fn main() {\n    let mut x = 1;\n}", None), Some("rust"));
        assert_eq!(guess("#!/usr/bin/env python3\nprint(1)", Some("rs")), Some("python"));
        assert_eq!(guess("Just a sentence about code.", Some("rs")), None);
        assert_eq!(guess("", None), None);
    }
}
