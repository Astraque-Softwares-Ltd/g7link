// SPDX-License-Identifier: MPL-2.0
//! Code into lines of classed tokens.
//!
//! The grammars are the Sublime Text syntaxes bat curates, from two-face's
//! pure-Rust build, run by syntect with its pure-Rust regex engine. No theme
//! is loaded: each token gets a [`TokClass`] from its scopes, and every
//! client colours the classes from its own palette.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::bounds::{
    HIGHLIGHT_STEPS, MAX_CODE_BYTES, MAX_CODE_LINE_UNITS, MAX_CODE_LINES, clean, units, units32,
};

/// One line of code with its tokens, which cover it in order.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeLine {
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub toks: Vec<Tok>,
}

impl std::fmt::Debug for CodeLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeLine").field("text_bytes", &self.text.len()).field("toks", &self.toks).finish()
    }
}

/// A run of `len` UTF-16 units of one class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tok {
    pub len: u32,
    pub class: TokClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokClass {
    #[default]
    Plain,
    Comment,
    Keyword,
    String,
    Number,
    Constant,
    Type,
    Function,
    Variable,
    Operator,
    Punctuation,
    Tag,
    Attribute,
    Inserted,
    Deleted,
}

/// The coloured lines of `code` in language `lang` (a fence's word); empty
/// when the language is unknown or the code is over a bound, so the client
/// shows the text uncoloured.
pub fn highlight(lang: &str, code: &str) -> Vec<CodeLine> {
    if code.len() > MAX_CODE_BYTES {
        return Vec::new();
    }
    let code = clean(code);
    let mut texts: Vec<&str> = code.split('\n').collect();
    if texts.len() > 1 && texts.last().is_some_and(|l| l.is_empty()) {
        texts.pop();
    }
    if texts.len() > MAX_CODE_LINES || texts.iter().any(|l| units(l) > MAX_CODE_LINE_UNITS) {
        return Vec::new();
    }
    let Some(syntax) = find_syntax(lang) else { return Vec::new() };
    let set = syntaxes();
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut steps: u64 = 0;
    let mut spent = false;
    let mut line_buf = String::new();
    let mut out = Vec::with_capacity(texts.len());
    for text in texts {
        let toks = if spent || steps >= HIGHLIGHT_STEPS {
            spent = true;
            plain(text)
        } else {
            line_buf.clear();
            line_buf.push_str(text);
            line_buf.push('\n');
            match state.parse_line(&line_buf, set) {
                Ok(ops) => {
                    steps = steps.saturating_add(line_steps(ops.len(), line_buf.len()));
                    match tokens(text, &ops, &mut stack) {
                        Some(toks) => toks,
                        None => {
                            spent = true;
                            plain(text)
                        }
                    }
                }
                Err(_) => {
                    spent = true;
                    plain(text)
                }
            }
        };
        out.push(CodeLine { text: text.to_owned(), toks });
    }
    out
}

/// Regex searches counted per byte of a line: at every position it reaches,
/// the parser tries each pattern of the current context. Measured on the
/// bundled grammars (release build, this project's reference laptop) at
/// 100 to 350 bytes per millisecond, so `HIGHLIGHT_STEPS` is about 12 KiB
/// of code, 40 to 120 ms.
const SEARCHES_PER_BYTE: u64 = 32;

/// The work one parsed line counts for: a search per pattern tried at each
/// byte, and one step per scope operation (token) it produced.
fn line_steps(ops: usize, bytes: usize) -> u64 {
    let ops = u64::try_from(ops).unwrap_or(u64::MAX);
    let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
    bytes.saturating_mul(SEARCHES_PER_BYTE).saturating_add(ops).saturating_add(1)
}

fn plain(text: &str) -> Vec<Tok> {
    let len = units32(text);
    if len == 0 { Vec::new() } else { vec![Tok { len, class: TokClass::Plain }] }
}

/// The line's tokens from the scope operations syntect returned for it.
/// `None` when the operations do not fit the line or the stack.
fn tokens(
    text: &str,
    ops: &[(usize, syntect::parsing::ScopeStackOp)],
    stack: &mut ScopeStack,
) -> Option<Vec<Tok>> {
    let mut toks: Vec<Tok> = Vec::new();
    let mut prev = 0usize;
    let push = |toks: &mut Vec<Tok>, piece: &str, class: TokClass| {
        let len = units32(piece);
        if len == 0 {
            return;
        }
        match toks.last_mut() {
            Some(last) if last.class == class => last.len = last.len.saturating_add(len),
            _ => toks.push(Tok { len, class }),
        }
    };
    for (at, op) in ops {
        let at = (*at).min(text.len());
        if at > prev {
            let piece = text.get(prev..at)?;
            push(&mut toks, piece, classify(stack));
            prev = at;
        }
        stack.apply(op).ok()?;
    }
    if prev < text.len() {
        let piece = text.get(prev..)?;
        push(&mut toks, piece, classify(stack));
    }
    Some(toks)
}

/// The class of a token from its scopes, innermost first; for each scope the
/// first rule whose prefix matches decides. `meta.function-call` counts only
/// as the innermost scope and not for a call's arguments, so the arguments
/// of a call are not coloured as its name.
fn classify(stack: &ScopeStack) -> TokClass {
    let (rules, call, arguments) = rules();
    for (depth, scope) in stack.as_slice().iter().rev().enumerate() {
        for (prefix, class) in rules {
            if !prefix.is_prefix_of(*scope) {
                continue;
            }
            if prefix == call && (depth > 0 || arguments.is_prefix_of(*scope)) {
                continue;
            }
            return *class;
        }
    }
    TokClass::Plain
}

type Rules = (Vec<(Scope, TokClass)>, Scope, Scope);

/// The scope rules in order, first match wins (contract §P2.4), with
/// `variable.function` added before `variable`: the bundled JavaScript,
/// Python and shell grammars name called functions that way.
fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        let rules = [
            ("comment", TokClass::Comment),
            ("string", TokClass::String),
            ("constant.numeric", TokClass::Number),
            ("constant", TokClass::Constant),
            ("keyword.operator", TokClass::Operator),
            ("keyword", TokClass::Keyword),
            ("storage", TokClass::Keyword),
            ("entity.name.function", TokClass::Function),
            ("support.function", TokClass::Function),
            ("meta.function-call", TokClass::Function),
            ("variable.function", TokClass::Function),
            ("entity.name.type", TokClass::Type),
            ("entity.name.class", TokClass::Type),
            ("support.type", TokClass::Type),
            ("support.class", TokClass::Type),
            ("storage.type", TokClass::Type),
            ("entity.name.tag", TokClass::Tag),
            ("entity.other.attribute-name", TokClass::Attribute),
            ("variable", TokClass::Variable),
            ("punctuation", TokClass::Punctuation),
            ("markup.inserted", TokClass::Inserted),
            ("markup.deleted", TokClass::Deleted),
        ]
        .into_iter()
        .filter_map(|(name, class)| Scope::new(name).ok().map(|s| (s, class)))
        .collect();
        let call = Scope::new("meta.function-call").unwrap_or_default();
        let arguments = Scope::new("meta.function-call.arguments").unwrap_or_default();
        (rules, call, arguments)
    })
}

/// The bundled syntaxes, loaded once.
pub(crate) fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

/// The name of every grammar in the set, hidden ones included; each is a
/// language word [`highlight`] accepts.
pub fn languages() -> impl Iterator<Item = &'static str> {
    syntaxes().syntaxes().iter().map(|s| s.name.as_str())
}

/// A fence's language word to the name or extension it is looked up by.
fn alias(lang: &str) -> &str {
    match lang {
        "sh" | "shell" | "zsh" => "bash",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "py" => "python",
        "rs" => "rust",
        "yml" => "yaml",
        "kt" | "kts" => "kotlin",
        "cpp" | "cc" | "hpp" | "cxx" => "c++",
        "md" => "markdown",
        "patch" => "diff",
        "docker" | "containerfile" => "dockerfile",
        other => other,
    }
}

/// The syntax for a fence's language word: by name, then by extension,
/// case-insensitively, after the aliases.
pub(crate) fn find_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let lang = lang.trim().to_lowercase();
    if lang.is_empty() {
        return None;
    }
    let name = alias(&lang);
    let set = syntaxes();
    set.syntaxes().iter().find(|s| s.name.eq_ignore_ascii_case(name)).or_else(|| {
        set.syntaxes().iter().find(|s| s.file_extensions.iter().any(|e| e.eq_ignore_ascii_case(name)))
    })
}

/// The language words the aliases accept, and a few names the set has, for
/// tests.
pub const ALIASES: &[&str] = &[
    "sh",
    "shell",
    "zsh",
    "bash",
    "js",
    "mjs",
    "cjs",
    "jsx",
    "javascript",
    "ts",
    "mts",
    "cts",
    "typescript",
    "tsx",
    "py",
    "python",
    "rs",
    "rust",
    "yml",
    "yaml",
    "kt",
    "kts",
    "kotlin",
    "cpp",
    "cc",
    "hpp",
    "cxx",
    "c++",
    "md",
    "markdown",
    "diff",
    "patch",
    "toml",
    "dockerfile",
    "docker",
    "containerfile",
    "swift",
    "dart",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_alias_finds_a_grammar() {
        for a in ALIASES {
            assert!(find_syntax(a).is_some(), "{a}");
        }
    }

    /// What each language word resolves to (contract §Q2.1).
    #[test]
    fn aliases_resolve_to_these_grammars() {
        let want = [
            ("sh", "Bourne Again Shell (bash)"),
            ("zsh", "Bourne Again Shell (bash)"),
            ("js", "JavaScript"),
            ("jsx", "JavaScript"),
            ("ts", "TypeScript"),
            ("typescript", "TypeScript"),
            ("mts", "TypeScript"),
            ("tsx", "TypeScriptReact"),
            ("kt", "Kotlin"),
            ("kts", "Kotlin"),
            ("kotlin", "Kotlin"),
            ("py", "Python"),
            ("rs", "Rust"),
            ("yml", "YAML"),
            ("cpp", "C++"),
            ("md", "Markdown"),
            ("patch", "Diff"),
            ("toml", "TOML"),
            ("dockerfile", "Dockerfile"),
            ("docker", "Dockerfile"),
            ("swift", "Swift"),
            ("dart", "Dart"),
            ("Kotlin", "Kotlin"),
            (" TSX ", "TypeScriptReact"),
        ];
        for (word, name) in want {
            assert_eq!(find_syntax(word).map(|s| s.name.as_str()), Some(name), "{word}");
        }
    }

    #[test]
    fn every_grammar_name_is_a_language_word() {
        let names: Vec<&str> = languages().collect();
        assert!(names.len() > 150, "{} grammars", names.len());
        for name in names {
            assert!(find_syntax(name).is_some(), "{name}");
        }
    }

    #[test]
    fn call_arguments_are_not_coloured_as_the_call() {
        let lines = classes("python", "os.path.join(a, 'x')");
        assert!(lines[0].contains(&("join".to_owned(), TokClass::Function)));
        assert!(!lines[0].iter().any(|(s, c)| s == "a" && *c == TokClass::Function));
    }

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

    #[test]
    fn rust_gets_keywords_strings_and_comments() {
        let lines = classes("rs", "fn main() {\n    let s = \"hi\"; // greet\n}");
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains(&("fn".to_owned(), TokClass::Keyword)));
        assert!(lines[1].iter().any(|(s, c)| s.contains("hi") && *c == TokClass::String));
        assert!(lines[1].iter().any(|(s, c)| s.contains("greet") && *c == TokClass::Comment));
    }

    #[test]
    fn tokens_cover_each_line() {
        for line in highlight("python", "def f(x):\n    return x + 1  # 😀\n") {
            let total: u32 = line.toks.iter().map(|t| t.len).sum();
            assert_eq!(total as usize, units(&line.text));
        }
    }

    #[test]
    fn unknown_language_and_bounds_give_nothing() {
        assert!(highlight("no-such-language", "x").is_empty());
        assert!(highlight("", "x").is_empty());
        let long_line = "a".repeat(MAX_CODE_LINE_UNITS + 1);
        assert!(highlight("rust", &long_line).is_empty());
    }
}
