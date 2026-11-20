// SPDX-License-Identifier: MPL-2.0
//! Questions an agent asks the user (§Q4.4): Claude's `AskUserQuestion`
//! through the approval bridge and Codex's `item/tool/requestUserInput`, as
//! questions for the host's registry (`threads.rs`), and the answers back.
//!
//! An answer to a secret ask lives only in
//! [`SecretAnswers`](crate::host::agents::SecretAnswers) and in the
//! reply to the agent: it is never in an item, an event, the store or a log.

use serde_json::{Map, Value, json};

use crate::api::{self, Ask, AskOption, BridgeAskResult};
use crate::host::agents::{QuestionReply, QuestionSpec};

/// Claude's tool for asking; it reaches our approval bridge.
pub const ASK_TOOL: &str = "AskUserQuestion";
/// The notice when an MCP server asked for input mid-run (elicitation):
/// declined in phase 2.
pub const MCP_DECLINED: &str = "An MCP server asked for something G7 can't answer yet. Do it in a terminal.";
/// What the agent is told when nobody answered.
pub const NOT_ANSWERED: &str = "The user didn't answer. Carry on without it or stop.";

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn options_of(list: Option<&Value>) -> Vec<AskOption> {
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|o| {
            Some(AskOption {
                label: str_of(o, "label")?.to_owned(),
                note: str_of(o, "description").map(str::to_owned),
            })
        })
        .collect()
}

/// The asks of a question: each cleaned and cut, at most [`api::MAX_ASKS`];
/// `None` when none is usable.
fn make_spec(key: Option<String>, asks: impl Iterator<Item = Ask>) -> Option<QuestionSpec> {
    let asks: Vec<Ask> = asks.filter_map(|a| a.sanitized()).take(api::MAX_ASKS).collect();
    (!asks.is_empty()).then_some(QuestionSpec { key, asks })
}

// ---- Claude ----

/// An `AskUserQuestion` call's input as a question. Each ask's id is its
/// index; the user may always answer in their own words.
pub fn claude_spec(input: &Value, tool_use_id: &str) -> Option<QuestionSpec> {
    let questions = input.get("questions")?.as_array()?;
    let asks = questions.iter().enumerate().filter_map(|(i, q)| {
        Some(Ask {
            id: i.to_string(),
            header: str_of(q, "header").unwrap_or_default().to_owned(),
            text: str_of(q, "question")?.to_owned(),
            multi: q.get("multiSelect").and_then(Value::as_bool) == Some(true),
            other: true,
            secret: false,
            options: options_of(q.get("options")),
        })
    });
    let key = (!tool_use_id.is_empty()).then(|| tool_use_id.to_owned());
    make_spec(key, asks)
}

/// One answer as text: the labels picked, then the user's own words.
fn answer_text(answer: &api::Answer) -> String {
    let mut parts: Vec<&str> = answer.labels.iter().map(String::as_str).collect();
    if let Some(other) = answer.other.as_deref().filter(|o| !o.trim().is_empty()) {
        parts.push(other);
    }
    parts.join(", ")
}

/// The bridge's answer to `AskUserQuestion`: allowed, with the answers
/// added to the tool's input under each question's own text; or denied with
/// [`NOT_ANSWERED`].
pub fn claude_reply(input: &Value, reply: &QuestionReply) -> BridgeAskResult {
    let QuestionReply::Answered(answers) = reply else { return BridgeAskResult::deny(NOT_ANSWERED) };
    let questions = input.get("questions").and_then(Value::as_array);
    let mut map = Map::new();
    for (i, q) in questions.into_iter().flatten().enumerate() {
        if let (Some(text), Some(answer)) = (str_of(q, "question"), answers.get(&i.to_string())) {
            map.insert(text.to_owned(), Value::String(answer_text(answer)));
        }
    }
    let mut updated = input.as_object().cloned().unwrap_or_default();
    updated.insert("answers".to_owned(), Value::Object(map));
    BridgeAskResult { updated_input: Some(Value::Object(updated)), ..BridgeAskResult::allow() }
}

// ---- Codex ----

/// An `item/tool/requestUserInput` request's params as a question; one
/// option each, the user's own words when Codex allows them or offers no
/// options. Asks whose ids we would have to change are left out, so the
/// answers go back under Codex's own ids.
pub fn codex_spec(params: &Value) -> Option<QuestionSpec> {
    let questions = params.get("questions")?.as_array()?;
    let asks = questions.iter().filter_map(|q| {
        let id = str_of(q, "id")?;
        if api::clean_cut(id, api::MAX_ASK_ID).0 != id {
            return None;
        }
        Some(Ask {
            id: id.to_owned(),
            header: str_of(q, "header").unwrap_or_default().to_owned(),
            text: str_of(q, "question")?.to_owned(),
            multi: false,
            other: q.get("isOther").and_then(Value::as_bool) == Some(true),
            secret: q.get("isSecret").and_then(Value::as_bool) == Some(true),
            options: options_of(q.get("options")),
        })
    });
    make_spec(str_of(params, "itemId").map(str::to_owned), asks)
}

/// The result for a `requestUserInput` request: each ask's answer, or empty
/// lists when it wasn't answered.
pub fn codex_reply(spec: &QuestionSpec, reply: &QuestionReply) -> Value {
    let mut map = Map::new();
    for ask in &spec.asks {
        let answers: Vec<Value> = match reply {
            QuestionReply::Answered(answers) => answers
                .get(&ask.id)
                .map(|a| {
                    let mut list: Vec<Value> = a.labels.iter().cloned().map(Value::String).collect();
                    list.extend(a.other.iter().filter(|o| !o.trim().is_empty()).cloned().map(Value::String));
                    list
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        map.insert(ask.id.clone(), json!({"answers": answers}));
    }
    json!({"answers": map})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::agents::SecretAnswers;

    fn answer(id: &str, labels: &[&str], other: Option<&str>) -> api::Answer {
        api::Answer {
            id: id.into(),
            labels: labels.iter().map(|l| (*l).to_owned()).collect(),
            other: other.map(str::to_owned),
        }
    }

    fn claude_input() -> Value {
        json!({"questions": [
            {"question": "Which colour?", "header": "Colour", "multiSelect": false,
             "options": [{"label": "Red", "description": "Warm"}, {"label": "Blue", "description": "Cool"}]},
            {"question": "Which files?", "header": "Files", "multiSelect": true,
             "options": [{"label": "a.rs", "description": ""}, {"label": "b.rs", "description": "The other"}]}]})
    }

    #[test]
    fn claudes_question_and_its_answers() {
        let input = claude_input();
        let spec = claude_spec(&input, "toolu_q").unwrap();
        assert_eq!(spec.key.as_deref(), Some("toolu_q"));
        assert_eq!(spec.asks.len(), 2);
        assert_eq!((spec.asks[0].id.as_str(), spec.asks[0].multi, spec.asks[0].other), ("0", false, true));
        assert_eq!(spec.asks[1].id, "1");
        assert!(spec.asks[1].multi);
        assert_eq!(spec.asks[0].options[1].note.as_deref(), Some("Cool"));
        assert_eq!(spec.asks[1].options[0].note, None, "an empty note is none");
        // Single, multi and the user's own words.
        let answers = SecretAnswers::new(vec![
            answer("0", &["Blue"], None),
            answer("1", &["a.rs", "b.rs"], Some("and c.rs")),
        ]);
        let result = claude_reply(&input, &QuestionReply::Answered(answers));
        assert_eq!(result.behavior, api::BridgeBehavior::Allow);
        let updated = result.updated_input.unwrap();
        assert_eq!(
            updated["answers"],
            json!({"Which colour?": "Blue", "Which files?": "a.rs, b.rs, and c.rs"})
        );
        assert_eq!(updated["questions"], input["questions"], "the input is kept as it was");
        for reply in [QuestionReply::Dismissed, QuestionReply::Expired, QuestionReply::Cancelled] {
            let result = claude_reply(&input, &reply);
            assert_eq!(result.behavior, api::BridgeBehavior::Deny);
            assert_eq!(result.message.as_deref(), Some(NOT_ANSWERED));
        }
        assert!(claude_spec(&json!({"questions": []}), "t").is_none());
        assert!(claude_spec(&json!({}), "t").is_none());
    }

    #[test]
    fn codexs_question_and_its_answers() {
        let params = json!({"threadId": "th", "turnId": "tu", "itemId": "call_1", "isBlocking": true, "questions": [
            {"id": "colour", "header": "Colour", "question": "Red or blue?", "isOther": true, "isSecret": false,
             "options": [{"label": "Red", "description": "r"}, {"label": "Blue", "description": "b"}]},
            {"id": "token", "header": "Token", "question": "Your API token?", "isOther": false, "isSecret": true, "options": null},
            {"id": "bad\u{202e}id", "header": "x", "question": "Dropped", "options": []}]});
        let spec = codex_spec(&params).unwrap();
        assert_eq!(spec.key.as_deref(), Some("call_1"));
        let ids: Vec<&str> = spec.asks.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["colour", "token"]);
        assert!(spec.asks[1].secret && spec.asks[1].other, "no options: the user's own words");
        let answers =
            SecretAnswers::new(vec![answer("colour", &["Blue"], None), answer("token", &[], Some("s3cret"))]);
        let reply = codex_reply(&spec, &QuestionReply::Answered(answers));
        assert_eq!(
            reply,
            json!({"answers": {"colour": {"answers": ["Blue"]}, "token": {"answers": ["s3cret"]}}})
        );
        let dismissed = codex_reply(&spec, &QuestionReply::Dismissed);
        assert_eq!(dismissed, json!({"answers": {"colour": {"answers": []}, "token": {"answers": []}}}));
    }

    #[test]
    fn a_secret_answer_is_only_in_the_reply() {
        let params = json!({"itemId": "c", "questions": [{"id": "token", "header": "Token", "question": "Your token?",
            "isOther": true, "isSecret": true, "options": []}]});
        let spec = codex_spec(&params).unwrap();
        assert!(spec.asks[0].secret);
        let reply =
            QuestionReply::Answered(SecretAnswers::new(vec![answer("token", &[], Some("hunter2-secret"))]));
        assert!(!format!("{reply:?}").contains("hunter2"), "Debug prints none of it");
        assert!(!format!("{spec:?}").contains("hunter2"));
        assert_eq!(codex_reply(&spec, &reply)["answers"]["token"]["answers"][0], "hunter2-secret");
    }
}
