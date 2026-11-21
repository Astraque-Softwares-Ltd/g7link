// SPDX-License-Identifier: MPL-2.0
//! The shared palette (`docs/design/tokens.json`, `themes.quiet.agents`)
//! colours every class this crate produces, in both themes.

use g7_agents_render::{TokClass, ToolClass};
use serde_json::Value;

/// Every token class. The match has no wildcard, so a new class fails to
/// compile here until it is listed, and then fails below until the palette
/// gives it a colour.
fn token_classes() -> Vec<TokClass> {
    use TokClass::*;
    let all = [
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
        Dir,
        File,
        Pos,
        Error,
        Warn,
        Ok,
    ];
    for class in all {
        match class {
            Plain | Comment | Keyword | String | Number | Constant | Type | Function | Variable
            | Operator | Punctuation | Tag | Attribute | Inserted | Deleted | Dir | File | Pos | Error
            | Warn | Ok => {}
        }
    }
    all.to_vec()
}

fn tool_classes() -> Vec<ToolClass> {
    use ToolClass::*;
    let all = [Command, Edit, Read, Search, Web, Mcp, Agent, Tasks, Other, Plan];
    for class in all {
        match class {
            Command | Edit | Read | Search | Web | Mcp | Agent | Tasks | Other | Plan => {}
        }
    }
    all.to_vec()
}

fn agents() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/tokens.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let tokens: Value = serde_json::from_str(&text).expect("tokens.json parses");
    tokens.pointer("/themes/quiet/agents").cloned().expect("an agents section")
}

fn word<T: serde::Serialize>(v: T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_owned)).expect("a word")
}

#[test]
fn every_token_and_tool_class_has_a_colour_in_both_themes() {
    let agents = agents();
    for theme in ["dark", "light"] {
        for class in token_classes() {
            let w = word(class);
            assert!(agents[theme]["syntax"][&w].is_string(), "{theme}.syntax.{w}");
        }
        let syntax = agents[theme]["syntax"].as_object().expect("syntax");
        assert_eq!(syntax.len(), token_classes().len(), "{theme}: a colour for a class the crate has not");
        for class in tool_classes() {
            let w = word(class);
            assert!(agents[theme]["tool"][&w].is_string(), "{theme}.tool.{w}");
        }
    }
    for class in agents["styles"].as_object().expect("styles").keys() {
        assert!(token_classes().iter().any(|c| word(*c) == *class), "styles.{class}");
    }
}
