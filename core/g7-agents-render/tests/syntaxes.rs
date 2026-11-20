// SPDX-License-Identifier: MPL-2.0
//! The grammar set (contract §Q2.1): Kotlin and TSX get their classes, every
//! pattern of every grammar compiles under the pure-Rust regex engine (one
//! that didn't would panic the first time a line reached it), and the
//! checked-in notices are the ones the set carries. Random input through
//! every grammar is in `torture.rs`.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use g7_agents_render::{TokClass, highlight, languages};
use syntect::parsing::Regex;
use syntect::parsing::syntax_definition::Pattern;

/// Each line's tokens as (text, class).
fn classes(lang: &str, code: &str) -> Vec<Vec<(String, TokClass)>> {
    highlight(lang, code)
        .into_iter()
        .map(|line| {
            let units: Vec<u16> = line.text.encode_utf16().collect();
            let mut at = 0usize;
            line.toks
                .iter()
                .map(|t| {
                    let end = at + t.len as usize;
                    let s = String::from_utf16_lossy(&units[at..end]);
                    at = end;
                    (s, t.class)
                })
                .collect()
        })
        .collect()
}

fn has(lines: &[Vec<(String, TokClass)>], line: usize, text: &str, class: TokClass) -> bool {
    lines[line].iter().any(|(s, c)| s.trim() == text && *c == class)
}

fn contains(lines: &[Vec<(String, TokClass)>], line: usize, text: &str, class: TokClass) -> bool {
    lines[line].iter().any(|(s, c)| s.contains(text) && *c == class)
}

const KOTLIN: &str = r#"// Greets someone
package app.greet

data class User(val name: String, val age: Int)

fun greet(user: User): String {
    val n = 42
    return "Hello, ${user.name}"
}
"#;

#[test]
fn kotlin_gets_keywords_strings_comments_and_types() {
    let lines = classes("kt", KOTLIN);
    assert_eq!(lines.len(), 9, "{lines:?}");
    assert!(contains(&lines, 0, "Greets someone", TokClass::Comment), "{:?}", lines[0]);
    assert!(has(&lines, 1, "package", TokClass::Keyword), "{:?}", lines[1]);
    assert!(has(&lines, 3, "class", TokClass::Keyword), "{:?}", lines[3]);
    assert!(has(&lines, 3, "User", TokClass::Type), "{:?}", lines[3]);
    // Built-in types are `storage.type`, which reads as a keyword (as `i32`
    // does in Rust).
    assert!(has(&lines, 3, "String", TokClass::Keyword), "{:?}", lines[3]);
    assert!(has(&lines, 5, "fun", TokClass::Keyword), "{:?}", lines[5]);
    assert!(has(&lines, 5, "greet", TokClass::Function), "{:?}", lines[5]);
    assert!(has(&lines, 6, "42", TokClass::Number), "{:?}", lines[6]);
    assert!(has(&lines, 7, "return", TokClass::Keyword), "{:?}", lines[7]);
    assert!(contains(&lines, 7, "Hello", TokClass::String), "{:?}", lines[7]);
    assert_eq!(classes("kts", KOTLIN), lines);
    assert_eq!(classes("Kotlin", KOTLIN), lines);
}

const TSX: &str = r#"// A button with a count
import React from "react";

interface Props { label: string; count: number }

export function Button({ label, count }: Props): JSX.Element {
  const shown: number = count + 1;
  return <button className="b">{label}</button>;
}
"#;

#[test]
fn tsx_gets_keywords_strings_comments_types_and_tags() {
    let lines = classes("tsx", TSX);
    assert_eq!(lines.len(), 9, "{lines:?}");
    assert!(contains(&lines, 0, "A button", TokClass::Comment), "{:?}", lines[0]);
    assert!(has(&lines, 1, "import", TokClass::Keyword), "{:?}", lines[1]);
    assert!(contains(&lines, 1, "react", TokClass::String), "{:?}", lines[1]);
    assert!(has(&lines, 3, "interface", TokClass::Keyword), "{:?}", lines[3]);
    assert!(has(&lines, 3, "Props", TokClass::Type), "{:?}", lines[3]);
    assert!(has(&lines, 3, "number", TokClass::Type), "{:?}", lines[3]);
    assert!(has(&lines, 5, "function", TokClass::Keyword), "{:?}", lines[5]);
    assert!(has(&lines, 5, "Button", TokClass::Function), "{:?}", lines[5]);
    assert!(has(&lines, 6, "const", TokClass::Keyword), "{:?}", lines[6]);
    assert!(has(&lines, 6, "1", TokClass::Number), "{:?}", lines[6]);
    assert!(has(&lines, 7, "button", TokClass::Tag), "{:?}", lines[7]);
    assert!(has(&lines, 7, "className", TokClass::Attribute), "{:?}", lines[7]);
}

#[test]
fn typescript_without_jsx_gets_its_own_grammar() {
    let code = "type Id = string;\nconst ids: Id[] = [];\nlet n = <number>x;\n";
    let lines = classes("ts", code);
    assert!(has(&lines, 0, "type", TokClass::Keyword), "{:?}", lines[0]);
    assert!(has(&lines, 0, "string", TokClass::Type), "{:?}", lines[0]);
    // A type assertion, not a tag: the TSX grammar would read a tag here.
    assert!(!lines[2].iter().any(|(_, c)| *c == TokClass::Tag), "{:?}", lines[2]);
    assert_eq!(classes("typescript", code), lines);
}

/// syntect's own substitution of `\1`…`\9` before such a pattern runs, with
/// a word in place of the captured text.
fn with_refs(regex: &str) -> String {
    let mut out = String::with_capacity(regex.len());
    let mut escaped = false;
    for c in regex.chars() {
        if escaped && c.is_ascii_digit() {
            out.push('x');
        } else if escaped {
            out.push('\\');
            out.push(c);
        } else if c != '\\' {
            out.push(c);
        }
        escaped = c == '\\' && !escaped;
    }
    if escaped {
        out.push('\\');
    }
    out
}

#[test]
fn every_pattern_of_every_grammar_compiles() {
    let builder = two_face::syntax::extra_newlines().into_builder();
    let mut sources: BTreeMap<String, &str> = BTreeMap::new();
    let mut patterns = 0usize;
    for syntax in builder.syntaxes() {
        for context in syntax.contexts.values() {
            for pattern in &context.patterns {
                let Pattern::Match(m) = pattern else { continue };
                let source = m.regex.regex_str();
                let source = if m.has_captures { with_refs(source) } else { source.to_owned() };
                patterns += 1;
                sources.entry(source).or_insert(syntax.name.as_str());
            }
        }
    }
    assert!(patterns > 10_000, "{patterns} patterns");
    // Compiling is slow unoptimised: the distinct patterns, over a few threads.
    let sources: Vec<(&String, &&str)> = sources.iter().collect();
    let chunk = sources.len().div_ceil(4).max(1);
    let failures: Vec<String> = std::thread::scope(|s| {
        let workers: Vec<_> = sources
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .filter_map(|(source, name)| {
                            Regex::try_compile(source).map(|e| format!("{name}: {source}: {e}"))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().expect("a compile worker panicked")).collect()
    });
    assert!(
        failures.is_empty(),
        "{} of {} distinct patterns do not compile:\n{}",
        failures.len(),
        sources.len(),
        failures.join("\n")
    );
}

// ── Notices ───────────────────────────────────────────────────────────────

const SUBLIME: &str = "\
Permission to copy, use, modify, sell and distribute this
software is granted. This software is provided \"as is\" without
express or implied warranty, and with no claim as to its
suitability for any purpose.";

/// A fence that no line of `text` can close.
fn fence(text: &str) -> String {
    let longest =
        text.lines().map(|l| l.trim_start().chars().take_while(|&c| c == '`').count()).max().unwrap_or(0);
    "`".repeat(longest.max(2) + 1)
}

fn block(out: &mut String, text: &str) {
    let text = text.trim_end();
    let f = fence(text);
    out.push_str(&format!("{f}text\n{text}\n{f}\n\n"));
}

/// The licence file of a grammar in bat's tree, by its directory there.
fn licence_of<'a>(
    listing: &'a two_face::acknowledgement::Acknowledgements,
    dir: &str,
) -> &'a two_face::acknowledgement::License {
    let prefix = format!("syntaxes/02_Extra/{dir}/");
    listing
        .for_syntaxes()
        .iter()
        .find(|l| l.rel_path.to_string_lossy().starts_with(&prefix))
        .unwrap_or_else(|| panic!("no licence under {prefix}"))
}

/// `THIRD-PARTY-SYNTAXES.md` as the grammar set gives it.
fn notices() -> String {
    let listing = two_face::acknowledgement::listing();
    let kotlin = licence_of(&listing, "Kotlin");
    let typescript = licence_of(&listing, "TypeScript");
    for l in [kotlin, typescript] {
        assert_eq!(l.ty, two_face::acknowledgement::LicenseType::Apache2, "{}", l.rel_path.display());
        assert!(
            l.text.contains("Apache License") && l.text.contains("Version 2.0"),
            "{}",
            l.rel_path.display()
        );
    }
    let count = languages().count();
    let mut out = String::new();
    out.push_str("# Syntax definitions built into this crate\n\n");
    out.push_str(&format!(
        "Code is highlighted with the syntax definitions the `bat` project curates, as the crate\n\
         [two-face](https://codeberg.org/CosmicHarper/two-face) (MIT OR Apache-2.0) bundles them for\n\
         syntect's pure-Rust regex engine (its `syntect-fancy` build): {count} syntaxes. No colour theme\n\
         is built in: clients colour token classes from their own palettes.\n\n\
         This file is written by the test `notices_are_current` (`tests/syntaxes.rs`); run it with\n\
         `G7_RENDER_BLESS=1` after changing the set.\n\n"
    ));
    out.push_str("## Kotlin\n\n");
    out.push_str(&format!(
        "The Kotlin syntax is from\n\
         [vkostyukov/kotlin-sublime-package](https://github.com/vkostyukov/kotlin-sublime-package),\n\
         Copyright 2012-2014 Vladimir Kostyukov, under the Apache License 2.0. Its full text is under\n\
         `{}` below.\n\n",
        kotlin.rel_path.display()
    ));
    out.push_str("## TypeScript and TypeScriptReact\n\n");
    out.push_str(&format!(
        "The TypeScript and TypeScriptReact syntaxes are from\n\
         [microsoft/TypeScript-Sublime-Plugin](https://github.com/microsoft/TypeScript-Sublime-Plugin),\n\
         under the Apache License 2.0. Its full text is under `{}` below.\n\n",
        typescript.rel_path.display()
    ));
    out.push_str("## The Sublime Text default packages\n\n");
    out.push_str(
        "The syntaxes from [sublimehq/Packages](https://github.com/sublimehq/Packages) are, unless a\n\
         file says otherwise, under this notice:\n\n",
    );
    block(&mut out, SUBLIME);
    out.push_str("## Every licence the set asks to reproduce\n\n");
    out.push_str(
        "As `two_face::acknowledgement::listing()` gives them, by the path of the licence file in\n\
         bat's tree.\n\n",
    );
    for l in listing.for_syntaxes() {
        out.push_str(&format!("### {}\n\n", l.rel_path.display()));
        block(&mut out, &l.text);
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

#[test]
fn notices_are_current() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("THIRD-PARTY-SYNTAXES.md");
    let want = notices();
    if common::blessing() {
        std::fs::write(&path, &want).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(
        have == want,
        "{} differs from the grammar set's notices; run this test with G7_RENDER_BLESS=1",
        path.display()
    );
}
