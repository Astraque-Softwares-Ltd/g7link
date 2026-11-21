// SPDX-License-Identifier: GPL-3.0-or-later
//! Phase 2 of the Agents page, without I/O: the model control from the
//! agent's own list, the choices remembered per agent and folder, which key
//! steers and which queues, the queue, the dock that shows requests and
//! questions in turn, the answers being given, the `@` and `/` lists, drafts
//! from other devices, and the file a pasted image passes through.
//!
//! Everything here is data for the page; its words are the page's own
//! (`qsTr`), except the agent's names and the CLI's model and effort names.

use std::{
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use g7_agents::api::{
    self, AgentId, AgentInfo, Answer, Approval, Can, Command, CommandKind, Dispatch, Draft, FileHit, Limit,
    LimitState, Mode, ModelInfo, Origin, OriginKind, Question, ThreadInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ---- Preferences: the follow-up key and the choices per agent and folder ----

/// What Enter does with a message while the agent works; Alt+Enter does the other.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FollowUp {
    /// It reaches the running turn.
    #[default]
    Steer,
    /// It waits for the turn to end.
    Queue,
}

/// A model and effort the user picked for an agent in a folder; the next
/// thread there starts with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Choice {
    pub agent: AgentId,
    pub folder: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// `model` was typed, not picked from the list.
    #[serde(default)]
    pub custom: bool,
}

/// Choices kept; the oldest goes first.
pub(crate) const MAX_CHOICES: usize = 64;

/// The page's own settings, kept on this PC.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Prefs {
    #[serde(default)]
    pub follow_up: FollowUp,
    #[serde(default)]
    pub choices: Vec<Choice>,
}

impl Prefs {
    pub(crate) fn choice(&self, agent: AgentId, folder: &str) -> Option<&Choice> {
        self.choices.iter().find(|c| c.agent == agent && c.folder == folder)
    }

    /// Remembers `choice` as the newest; true when something changed.
    pub(crate) fn remember(&mut self, choice: Choice) -> bool {
        if self.choices.last() == Some(&choice) {
            return false;
        }
        self.choices.retain(|c| !(c.agent == choice.agent && c.folder == choice.folder));
        self.choices.push(choice);
        if self.choices.len() > MAX_CHOICES {
            let extra = self.choices.len() - MAX_CHOICES;
            self.choices.drain(..extra);
        }
        true
    }
}

/// How a message sent while the agent works travels: the user's default,
/// or the other one with Alt+Enter. An agent that can't be steered queues.
pub(crate) fn dispatch(follow_up: FollowUp, other: bool, can_steer: bool) -> Dispatch {
    let steer = (follow_up == FollowUp::Steer) != other;
    if steer && can_steer { Dispatch::Steer } else { Dispatch::Queue }
}

/// A model name the user typed can be passed on: no control character, no
/// whitespace, not starting with `-`, at most 64 bytes (contract §Q7).
pub(crate) fn valid_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= api::MAX_MODEL_ID
        && !id.starts_with('-')
        && !id.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// An effort as the page shows it: the CLI's word, its first letter capitalised.
pub(crate) fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// ---- The model control ----

/// What a thread (or the new-thread form) is set to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Setting {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<Mode>,
}

impl Setting {
    pub(crate) fn of_thread(info: &ThreadInfo) -> Self {
        Self { model: info.model.clone(), effort: info.effort.clone(), mode: Some(info.mode) }
    }
}

/// The model the setting stands for: the one it names, or the list's default.
fn chosen<'a>(agent: &'a AgentInfo, model: Option<&str>) -> Option<&'a ModelInfo> {
    match model {
        Some(id) => agent.model(id),
        None => agent.default_model(),
    }
}

/// The selected model reads images (true when the list doesn't say).
pub(crate) fn takes_images(agent: &AgentInfo, model: Option<&str>) -> bool {
    agent.can(Can::Images) && chosen(agent, model).is_none_or(|m| m.images)
}

/// The control's label: the model's name ("" for the CLI's default, which
/// the page calls "Default") and an effort set by hand ("" for none).
pub(crate) fn model_label(agent: &AgentInfo, setting: &Setting) -> (String, String) {
    let model = match setting.model.as_deref() {
        None => String::new(),
        Some(id) => agent.model(id).map_or_else(|| id.to_owned(), |m| m.name.clone()),
    };
    (model, setting.effort.as_deref().map(capitalized).unwrap_or_default())
}

fn model_row(m: &ModelInfo, setting: &Setting) -> Value {
    let checked = match setting.model.as_deref() {
        Some(id) => id == m.id,
        None => m.default,
    };
    json!({
        "id": m.id,
        "name": m.name,
        "note": m.note.clone().unwrap_or_default(),
        "isDefault": m.default,
        "checked": checked,
    })
}

/// The picker: the agent's models as its CLI listed them (short names
/// first, the rest under "More models"), the chosen model's efforts, and the
/// three modes with whether Plan is there. Before the CLI has listed any,
/// only "Default" and a typed name.
pub(crate) fn picker_json(agent: &AgentInfo, setting: &Setting) -> Value {
    let models: Vec<&ModelInfo> = agent.models.iter().collect();
    let known = !models.is_empty();
    let has_default = models.iter().any(|m| m.default);
    // The CLI's default stands for "no model set"; one the list doesn't
    // flag gets a row of its own.
    let mut short: Vec<Value> = Vec::new();
    if !has_default {
        short.push(json!({
            "id": "",
            "name": "",
            "note": "",
            "isDefault": true,
            "checked": setting.model.is_none(),
        }));
    }
    let short_models = models.iter().filter(|m| m.short || m.default);
    short.extend(short_models.map(|m| model_row(m, setting)));
    let more: Vec<Value> =
        models.iter().filter(|m| !m.short && !m.default).map(|m| model_row(m, setting)).collect();
    let custom = setting.model.as_deref().filter(|id| agent.model(id).is_none()).unwrap_or_default();
    let model = chosen(agent, setting.model.as_deref());
    let efforts: Vec<Value> = model
        .map(|m| {
            m.efforts
                .iter()
                .map(|e| {
                    let is_default = m.default_effort.as_deref() == Some(e.as_str());
                    let checked = match setting.effort.as_deref() {
                        Some(set) => set == e,
                        None => is_default,
                    };
                    json!({ "value": e, "label": capitalized(e), "isDefault": is_default, "checked": checked })
                })
                .collect()
        })
        .unwrap_or_default();
    let mode = setting.mode.unwrap_or(Mode::Supervised);
    let (label_model, label_effort) = model_label(agent, setting);
    json!({
        "agent": agent.id.as_str(),
        "agentName": agent.name,
        "known": known,
        "short": short,
        "more": more,
        "custom": custom,
        "efforts": efforts,
        "mode": api_word(&mode),
        "plan": agent.can(Can::Plan),
        "images": takes_images(agent, setting.model.as_deref()),
        "label": { "model": label_model, "effort": label_effort },
    })
}

fn api_word<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

/// The setting after picking model `id` from the list ("" or the default
/// model: the CLI's default). An effort the new model doesn't take is
/// dropped.
pub(crate) fn pick_model(agent: &AgentInfo, setting: &Setting, id: &str) -> Setting {
    let model = agent.model(id).filter(|m| !m.default).map(|m| m.id.clone());
    let effort = setting
        .effort
        .clone()
        .filter(|e| chosen(agent, model.as_deref()).is_some_and(|m| m.efforts.iter().any(|x| x == e)));
    Setting { model, effort, mode: setting.mode }
}

/// The setting after the user typed a model name; `None` when it can't be passed on.
pub(crate) fn type_model(setting: &Setting, id: &str) -> Option<Setting> {
    let id = id.trim();
    valid_model_id(id).then(|| Setting { model: Some(id.to_owned()), effort: None, mode: setting.mode })
}

/// The setting after picking an effort the chosen model lists.
pub(crate) fn pick_effort(agent: &AgentInfo, setting: &Setting, effort: &str) -> Option<Setting> {
    let model = chosen(agent, setting.model.as_deref())?;
    model
        .efforts
        .iter()
        .any(|e| e == effort)
        .then(|| Setting { effort: Some(effort.to_owned()), ..setting.clone() })
}

/// A mode the page offers: the three, Plan only when the agent can.
pub(crate) fn mode_of(word: &str, agent: &AgentInfo) -> Option<Mode> {
    match word {
        "supervised" => Some(Mode::Supervised),
        "auto_edit" => Some(Mode::AutoEdit),
        "plan" if agent.can(Can::Plan) => Some(Mode::Plan),
        _ => None,
    }
}

// ---- Queue, context, limits ----

/// A queued message's first line, for its one-line row.
fn first_line(text: &str) -> &str {
    text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim()
}

/// The messages waiting for the turn to end, and whether they're held.
pub(crate) fn queue_json(info: &ThreadInfo) -> Value {
    let items: Vec<Value> = info
        .queued
        .iter()
        .map(|q| {
            json!({
                "id": q.id,
                "text": first_line(&q.text),
                "full": q.text,
                "from": phone_name(&q.origin),
                "images": q.images.len(),
            })
        })
        .collect();
    json!({ "items": items, "held": info.queue_held })
}

fn phone_name(origin: &Origin) -> String {
    match origin.kind {
        OriginKind::Phone => origin.device_name.clone().unwrap_or_default(),
        OriginKind::Launcher | OriginKind::Cli => String::new(),
    }
}

/// A token count as the context line shows it: "950", "475K", "1M", "1.5M".
pub(crate) fn compact_tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{}K", (n + 500) / 1_000),
        _ => {
            let tenths = (n + 50_000) / 100_000;
            if tenths.is_multiple_of(10) {
                format!("{}M", tenths / 10)
            } else {
                format!("{}.{}M", tenths / 10, tenths % 10)
            }
        }
    }
}

/// "Context 12% of 1M" as data, once the CLI reported it.
pub(crate) fn context_json(info: &ThreadInfo) -> Value {
    match info.context {
        Some(c) if c.max > 0 => {
            let pct = (c.used.saturating_mul(100) / c.max).min(100);
            json!({ "pct": pct, "max": compact_tokens(c.max), "used": c.used })
        }
        _ => json!({}),
    }
}

fn limit_json(l: &Limit) -> Value {
    json!({
        "window": l.window,
        "usedPct": l.used_pct.map_or(-1, i32::from),
        "resetsMs": l.resets_ms.unwrap_or(0),
        "state": api_word(&l.state),
    })
}

/// Every usage window the agent reported, for setup.
pub(crate) fn limits_json(limits: &[Limit]) -> Value {
    Value::Array(limits.iter().take(api::MAX_LIMITS).map(limit_json).collect())
}

/// The one amber line above the composer: a window that's reached, else
/// one near its end (`{}` for none).
pub(crate) fn limit_line(agent: Option<&AgentInfo>) -> Value {
    let Some(agent) = agent else { return json!({}) };
    let pick = |state| agent.limits.iter().find(|l| l.state == state);
    pick(LimitState::Reached).or_else(|| pick(LimitState::Warn)).map_or_else(|| json!({}), limit_json)
}

// ---- The dock: requests and questions in turn ----

/// Something the agent waits on: a request, or a question.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Waiting<'a> {
    Approval(&'a Approval),
    Question(&'a Question),
}

impl Waiting<'_> {
    pub(crate) fn token(&self) -> &str {
        match self {
            Waiting::Approval(a) => &a.token,
            Waiting::Question(q) => &q.token,
        }
    }

    fn created_ms(&self) -> u64 {
        match self {
            Waiting::Approval(a) => a.created_ms,
            Waiting::Question(q) => q.created_ms,
        }
    }
}

/// Requests and questions, oldest first (a request before a question of
/// the same moment).
pub(crate) fn waiting<'a>(asks: &'a [Approval], questions: &'a [Question]) -> Vec<Waiting<'a>> {
    let mut all: Vec<Waiting<'a>> =
        asks.iter().map(Waiting::Approval).chain(questions.iter().map(Waiting::Question)).collect();
    all.sort_by_key(|w| (w.created_ms(), matches!(w, Waiting::Question(_))));
    all
}

/// The one the dock shows: the first the typing guard doesn't hold, or the
/// first when it holds them all.
pub(crate) fn shown(all: &[Waiting<'_>], held: impl Fn(&str) -> bool) -> Option<usize> {
    all.iter().position(|w| !held(w.token())).or((!all.is_empty()).then_some(0))
}

// ---- Answering a question ----

/// A secret answer: kept only until it's sent or given up, never printed,
/// and overwritten when dropped.
#[derive(Default, Clone, PartialEq, Eq)]
pub(crate) struct Secret(String);

impl Secret {
    /// Takes `text` over: it's overwritten when this is dropped.
    pub(crate) fn new(text: String) -> Self {
        Self(text)
    }

    pub(crate) fn set(&mut self, text: &str) {
        self.clear();
        self.0.push_str(text);
    }

    pub(crate) fn clear(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.fill(0);
        std::hint::black_box(&bytes);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn reveal(&self) -> &str {
        &self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.clear();
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret")
    }
}

/// One ask's answer so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AskAnswer {
    /// Options picked, by index.
    picks: Vec<usize>,
    /// "Something else" is chosen: the composer's text is the answer.
    other_on: bool,
    /// What the composer held for this ask.
    other: String,
    secret: Secret,
}

/// The answers being given to the question the dock shows. Its asks are
/// answered one at a time ("1 of 3"); nothing of it is saved anywhere.
#[derive(Debug, Default)]
pub(crate) struct Answering {
    token: String,
    at: usize,
    answers: Vec<AskAnswer>,
    /// Goes up whenever the page must empty the secret field.
    clears: u64,
}

impl Answering {
    /// Follows the question the dock shows: a different one (or none) starts
    /// over, with the composer's `text` as the first ask's own words. True
    /// when it started over.
    pub(crate) fn follow(&mut self, shown: Option<&Question>, text: &str) -> bool {
        let token = shown.map(|q| q.token.as_str()).unwrap_or_default();
        if token == self.token {
            return false;
        }
        self.reset();
        if let Some(q) = shown {
            self.token = q.token.clone();
            self.answers = vec![AskAnswer::default(); q.asks.len()];
            if let Some(first) = self.answers.first_mut() {
                first.other = text.to_owned();
            }
        }
        true
    }

    /// Forgets everything, the secrets first.
    pub(crate) fn reset(&mut self) {
        for a in &mut self.answers {
            a.secret.clear();
        }
        self.answers.clear();
        self.token.clear();
        self.at = 0;
        self.clears += 1;
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    pub(crate) fn at(&self) -> usize {
        self.at
    }

    #[cfg(test)]
    fn clears(&self) -> u64 {
        self.clears
    }

    fn current(&mut self) -> Option<&mut AskAnswer> {
        self.answers.get_mut(self.at)
    }

    /// Picks option `index` of the shown ask; the index just past the
    /// options is "Something else". One answer: the pick replaces the
    /// others; several: it toggles.
    pub(crate) fn pick(&mut self, q: &Question, index: usize) {
        let Some(ask) = q.asks.get(self.at) else { return };
        let options = ask.options.len();
        let multi = ask.multi;
        let other_allowed = ask.other && !ask.secret;
        let Some(answer) = self.current() else { return };
        if index == options {
            if !other_allowed {
                return;
            }
            if multi {
                answer.other_on = !answer.other_on;
            } else {
                answer.picks.clear();
                answer.other_on = true;
            }
        } else if index < options {
            if multi {
                match answer.picks.iter().position(|&p| p == index) {
                    Some(at) => {
                        answer.picks.remove(at);
                    }
                    None => answer.picks.push(index),
                }
            } else {
                answer.picks = vec![index];
                answer.other_on = false;
            }
        }
    }

    /// The composer's text changed while the ask allows its own words:
    /// typing chooses "Something else".
    pub(crate) fn type_other(&mut self, q: &Question, text: &str) {
        let Some(ask) = q.asks.get(self.at) else { return };
        if !ask.other || ask.secret {
            return;
        }
        let multi = ask.multi;
        let Some(answer) = self.current() else { return };
        let typed = !text.trim().is_empty() && text != answer.other;
        answer.other = api::cut(text, api::MAX_ANSWER_OTHER).to_owned();
        if typed {
            answer.other_on = true;
            if !multi {
                answer.picks.clear();
            }
        }
    }

    /// The masked field of a secret ask changed.
    pub(crate) fn type_secret(&mut self, q: &Question, text: &str) {
        if !q.asks.get(self.at).is_some_and(|a| a.secret) {
            return;
        }
        if let Some(answer) = self.current() {
            answer.secret.set(api::cut(text, api::MAX_ANSWER_OTHER));
        }
    }

    /// Whether ask `i` has an answer.
    fn answered(&self, q: &Question, i: usize) -> bool {
        let (Some(ask), Some(a)) = (q.asks.get(i), self.answers.get(i)) else { return false };
        if ask.secret {
            return !a.secret.is_empty();
        }
        !a.picks.is_empty() || (a.other_on && !a.other.trim().is_empty())
    }

    /// Goes to the next ask; the composer then holds that ask's words.
    pub(crate) fn next(&mut self, q: &Question) -> bool {
        if self.at + 1 >= q.asks.len() || !self.answered(q, self.at) {
            return false;
        }
        self.at += 1;
        self.clears += 1;
        true
    }

    pub(crate) fn back(&mut self) -> bool {
        if self.at == 0 {
            return false;
        }
        self.at -= 1;
        self.clears += 1;
        true
    }

    /// What the composer holds for the shown ask.
    pub(crate) fn other_text(&self) -> &str {
        self.answers.get(self.at).map(|a| a.other.as_str()).unwrap_or_default()
    }

    /// Every ask's answer, once each has one. A secret stays here until the
    /// question goes (answered, or gone elsewhere), so a reply that fails
    /// can be sent again.
    pub(crate) fn answers(&self, q: &Question) -> Option<Vec<Answer>> {
        if q.asks.is_empty() || !(0..q.asks.len()).all(|i| self.answered(q, i)) {
            return None;
        }
        let answers = q
            .asks
            .iter()
            .zip(&self.answers)
            .map(|(ask, a)| {
                let labels: Vec<String> =
                    a.picks.iter().filter_map(|&i| ask.options.get(i)).map(|o| o.label.clone()).collect();
                let other = if ask.secret {
                    Some(a.secret.reveal().to_owned())
                } else {
                    (a.other_on && !a.other.trim().is_empty()).then(|| a.other.trim().to_owned())
                };
                Answer { id: ask.id.clone(), labels, other }
            })
            .collect();
        Some(answers)
    }

    /// The shown ask as the dock draws it.
    pub(crate) fn json(&self, q: &Question) -> Value {
        let Some(ask) = q.asks.get(self.at) else { return json!({}) };
        let empty = AskAnswer::default();
        let a = self.answers.get(self.at).unwrap_or(&empty);
        let options: Vec<Value> = ask
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                json!({
                    "label": o.label,
                    "note": o.note.clone().unwrap_or_default(),
                    "checked": a.picks.contains(&i),
                })
            })
            .collect();
        let last = self.at + 1 >= q.asks.len();
        let ready =
            if last { (0..q.asks.len()).all(|i| self.answered(q, i)) } else { self.answered(q, self.at) };
        json!({
            "id": ask.id,
            "header": ask.header,
            "text": ask.text,
            "multi": ask.multi,
            "other": ask.other && !ask.secret,
            "secret": ask.secret,
            "options": options,
            "otherOn": a.other_on,
            "otherFilled": !a.other.trim().is_empty(),
            "secretFilled": !a.secret.is_empty(),
            "at": self.at,
            "count": q.asks.len(),
            "last": last,
            "ready": ready,
            "clears": self.clears,
        })
    }
}

// ---- `@` and `/` ----

pub(crate) use g7_agents_render::{Trigger, TriggerKind, apply, trigger};

/// The text without the trigger's run (for `/` its whole first word), and
/// the cursor where it was: a command G7 runs itself leaves nothing behind.
pub(crate) fn remove_run(text: &str, trigger: &Trigger, cursor: u32) -> (String, u32) {
    let units: Vec<u16> = text.encode_utf16().collect();
    let start = (trigger.start as usize).min(units.len());
    let mut end = (cursor as usize).clamp(start, units.len());
    if trigger.kind == TriggerKind::Slash {
        let space = |u: &u16| char::from_u32(u32::from(*u)).is_some_and(char::is_whitespace);
        end = end.max(units.iter().position(space).unwrap_or(units.len()));
    }
    // Its space goes with it.
    if units.get(end) == Some(&u16::from(b' ')) {
        end += 1;
    }
    let mut out = units[..start].to_vec();
    out.extend_from_slice(&units[end..]);
    (String::from_utf16_lossy(&out), start as u32)
}

/// The files `@` lists: up to 8.
pub(crate) const MENTION_ROWS: usize = 8;
/// The commands `/` lists.
const COMMAND_ROWS: usize = 50;

/// G7's own commands: they act at once and are never sent.
pub(crate) const OWN_COMMANDS: [&str; 3] = ["model", "effort", "plan"];

/// A file hit as the `@` list shows it.
pub(crate) fn file_rows(hits: &[FileHit]) -> Value {
    Value::Array(
        hits.iter()
            .take(MENTION_ROWS)
            .map(|h| {
                let path = h.path.trim_end_matches('/');
                let (parent, name) = path.rsplit_once('/').map_or(("", path), |(p, n)| (p, n));
                json!({
                    "path": h.path,
                    "dir": h.dir,
                    "name": if h.dir { format!("{name}/") } else { name.to_owned() },
                    "parent": if parent.is_empty() { String::new() } else { format!("{parent}/") },
                })
            })
            .collect(),
    )
}

/// One row of the `/` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandRow {
    pub name: String,
    pub note: String,
    pub hint: String,
    /// G7's own (`/model`, `/effort`, `/plan`).
    pub own: bool,
    pub skill: bool,
}

/// G7's three first, then the agent's; in each, names that start with the
/// query before names that contain it, in the agent's order.
pub(crate) fn command_rows(commands: &[Command], query: &str, plan: bool) -> Vec<CommandRow> {
    let query = query.to_lowercase();
    let own: Vec<CommandRow> = OWN_COMMANDS
        .iter()
        .filter(|n| plan || **n != "plan")
        .map(|name| CommandRow {
            name: (*name).to_owned(),
            note: String::new(),
            hint: String::new(),
            own: true,
            skill: false,
        })
        .collect();
    let theirs: Vec<CommandRow> = commands
        .iter()
        .filter(|c| !OWN_COMMANDS.contains(&c.name.as_str()))
        .map(|c| CommandRow {
            name: c.name.clone(),
            note: c.note.clone().unwrap_or_default(),
            hint: c.hint.clone().unwrap_or_default(),
            own: false,
            skill: c.kind == CommandKind::Skill,
        })
        .collect();
    let mut rows = Vec::new();
    for group in [own, theirs] {
        let lower = |r: &CommandRow| r.name.to_lowercase();
        rows.extend(group.iter().filter(|r| lower(r).starts_with(&query)).cloned());
        rows.extend(
            group.iter().filter(|r| !lower(r).starts_with(&query) && lower(r).contains(&query)).cloned(),
        );
    }
    rows.truncate(COMMAND_ROWS);
    rows
}

pub(crate) fn command_rows_json(rows: &[CommandRow]) -> Value {
    Value::Array(
        rows.iter()
            .map(
                |r| json!({ "name": r.name, "note": r.note, "hint": r.hint, "own": r.own, "skill": r.skill }),
            )
            .collect(),
    )
}

/// The argument hint shown after a command the composer holds alone
/// (`/name ` and nothing after it).
pub(crate) fn command_hint<'a>(text: &str, commands: &'a [Command]) -> Option<&'a str> {
    let name = text.strip_prefix('/')?.strip_suffix(' ')?;
    if name.is_empty() || name.contains(char::is_whitespace) {
        return None;
    }
    commands.iter().find(|c| c.name == name)?.hint.as_deref().filter(|h| !h.is_empty())
}

// ---- Drafts from other devices ----

/// What a newer draft from another device does to the composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DraftAction {
    /// The composer isn't in use: its text becomes the draft's.
    Replace(String),
    /// The user is typing: a line offers it ("A newer draft from …").
    Offer { by: String },
    /// It's what the composer holds already.
    Keep,
}

/// Replace when the composer isn't focused, offer when it is.
pub(crate) fn remote_draft(focused: bool, current: &str, draft: &Draft, this_pc: &str) -> DraftAction {
    if draft.text == current {
        return DraftAction::Keep;
    }
    if !focused {
        return DraftAction::Replace(draft.text.clone());
    }
    let by = match &draft.by {
        Some(o) if o.kind == OriginKind::Phone => o.device_name.clone().unwrap_or_default(),
        _ => this_pc.to_owned(),
    };
    DraftAction::Offer { by }
}

// ---- A pasted image ----

/// A pasted image on its way to the host: a file only this user can read,
/// removed when this is dropped.
#[derive(Debug)]
pub(crate) struct PastedImage {
    path: PathBuf,
}

impl PastedImage {
    /// Writes `bytes` to a new file in `dir` (made `0700` if it isn't
    /// there), readable by this user only.
    pub(crate) fn write(dir: &Path, bytes: &[u8]) -> io::Result<Self> {
        Self::write_as(dir, "agent-paste", "png", bytes)
    }

    /// As [`Self::write`], named `<stem>-<random>.<ext>` (a phone's photo
    /// keeps its type).
    pub(crate) fn write_as(dir: &Path, stem: &str, ext: &str, bytes: &[u8]) -> io::Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        let meta = std::fs::symlink_metadata(dir)?;
        if !meta.is_dir() || meta.uid() != g7_agents::client::euid() || meta.mode() & 0o077 != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the folder isn't private"));
        }
        let path = dir.join(format!("{stem}-{:016x}.{ext}", rand::random::<u64>()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        let pasted = Self { path };
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(pasted)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PastedImage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Where pasted images pass: a private folder of this app's in the runtime directory.
pub(crate) fn paste_dir() -> Option<PathBuf> {
    dirs::runtime_dir().filter(|p| p.is_absolute()).map(|d| d.join("g7link").join("agents"))
}

#[cfg(test)]
mod tests {
    use g7_agents::api::{Ask, AskOption, ContextUse, Queued};

    use super::*;

    fn model(id: &str, name: &str, efforts: &[&str], default: bool, short: bool) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            name: name.into(),
            note: None,
            default,
            efforts: efforts.iter().map(|e| (*e).to_owned()).collect(),
            default_effort: None,
            images: true,
            short,
        }
    }

    fn claude() -> AgentInfo {
        let mut a = AgentInfo::missing(AgentId::Claude);
        a.installed = true;
        a.signed_in = true;
        a.can = vec![Can::Plan, Can::Questions, Can::Steer, Can::Images, Can::Commands];
        a.models = vec![
            model("default", "Default", &["low", "medium", "high"], true, true),
            model("opus", "Opus", &["low", "medium", "high", "xhigh", "max"], false, true),
            model("sonnet", "Sonnet", &["low", "high"], false, true),
            model("claude-opus-4-7", "Opus 4.7", &[], false, false),
        ];
        a
    }

    fn codex() -> AgentInfo {
        let mut a = AgentInfo::missing(AgentId::Codex);
        a.can = vec![Can::Questions, Can::Steer, Can::Images];
        let mut first = model("gpt-x", "GPT X", &["low", "medium", "high"], true, false);
        first.default_effort = Some("medium".into());
        a.models = vec![first, model("gpt-y", "GPT Y", &["low"], false, false)];
        a
    }

    #[test]
    fn the_picker_shows_the_clis_own_list_short_names_first() {
        let agent = claude();
        let setting =
            Setting { model: Some("opus".into()), effort: Some("high".into()), mode: Some(Mode::Plan) };
        let p = picker_json(&agent, &setting);
        let ids = |key: &str| -> Vec<String> {
            p[key].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_owned()).collect()
        };
        assert_eq!(ids("short"), ["default", "opus", "sonnet"]);
        assert_eq!(ids("more"), ["claude-opus-4-7"]);
        assert_eq!(p["short"][1]["checked"], true);
        assert_eq!(p["short"][0]["checked"], false);
        let efforts: Vec<&str> =
            p["efforts"].as_array().unwrap().iter().map(|e| e["label"].as_str().unwrap()).collect();
        assert_eq!(efforts, ["Low", "Medium", "High", "Xhigh", "Max"], "named as the CLI does, capitalised");
        assert_eq!(p["label"]["model"], "Opus");
        assert_eq!(p["label"]["effort"], "High");
        assert_eq!((p["mode"].as_str(), p["plan"].as_bool()), (Some("plan"), Some(true)));

        // Nothing set: the CLI's default is the checked row, and the label says so.
        let p = picker_json(&agent, &Setting::default());
        assert_eq!(p["short"][0]["checked"], true);
        assert_eq!(p["label"]["model"], "");
        assert_eq!(p["efforts"].as_array().unwrap().len(), 3, "the default model's efforts");
    }

    #[test]
    fn codex_marks_its_default_and_has_no_plan_without_the_switch() {
        let agent = codex();
        let p = picker_json(&agent, &Setting::default());
        assert_eq!(p["short"][0]["id"], "gpt-x", "the default model comes first");
        assert_eq!(p["short"][0]["isDefault"], true);
        assert_eq!(p["more"][0]["id"], "gpt-y");
        let medium = &p["efforts"][1];
        assert_eq!((medium["isDefault"].as_bool(), medium["checked"].as_bool()), (Some(true), Some(true)));
        assert_eq!(p["plan"], false);
        assert_eq!(mode_of("plan", &agent), None);
        assert_eq!(mode_of("auto_edit", &agent), Some(Mode::AutoEdit));
        assert_eq!(mode_of("full_access", &agent), None, "no other mode exists");
    }

    #[test]
    fn before_a_list_there_is_default_and_a_typed_name() {
        let mut agent = claude();
        agent.models.clear();
        let p = picker_json(&agent, &Setting::default());
        assert_eq!(p["known"], false);
        assert_eq!(p["short"].as_array().unwrap().len(), 1);
        assert_eq!(
            (p["short"][0]["id"].as_str(), p["short"][0]["checked"].as_bool()),
            (Some(""), Some(true))
        );
        let typed = type_model(&Setting::default(), " my-model ").unwrap();
        assert_eq!(typed.model.as_deref(), Some("my-model"));
        let p = picker_json(&agent, &typed);
        assert_eq!(p["custom"], "my-model");
        assert_eq!(p["label"]["model"], "my-model");
        for bad in ["", "-rf", "two words", "a\u{7}", &"x".repeat(65)] {
            assert!(type_model(&Setting::default(), bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn a_choice_sticks_per_agent_and_folder_and_a_new_model_drops_an_effort_it_lacks() {
        let agent = claude();
        let set = Setting { model: Some("opus".into()), effort: Some("max".into()), mode: None };
        let sonnet = pick_model(&agent, &set, "sonnet");
        assert_eq!(
            (sonnet.model.as_deref(), sonnet.effort.as_deref()),
            (Some("sonnet"), None),
            "Sonnet has no max"
        );
        let back = pick_model(&agent, &set, "default");
        assert_eq!(back.model, None, "the default row is no model set");
        assert!(pick_effort(&agent, &sonnet, "max").is_none());
        assert_eq!(pick_effort(&agent, &sonnet, "high").unwrap().effort.as_deref(), Some("high"));

        let mut prefs = Prefs::default();
        let choice = |folder: &str, model: &str| Choice {
            agent: AgentId::Claude,
            folder: folder.into(),
            model: Some(model.into()),
            effort: None,
            custom: false,
        };
        assert!(prefs.remember(choice("f1", "opus")));
        assert!(!prefs.remember(choice("f1", "opus")));
        assert!(prefs.remember(choice("f2", "sonnet")));
        assert!(prefs.remember(choice("f1", "haiku")));
        assert_eq!(prefs.choice(AgentId::Claude, "f1").unwrap().model.as_deref(), Some("haiku"));
        assert_eq!(prefs.choice(AgentId::Codex, "f1"), None, "per agent");
        assert_eq!(prefs.choices.len(), 2);
        for i in 0..100 {
            prefs.remember(choice(&format!("g{i}"), "opus"));
        }
        assert_eq!(prefs.choices.len(), MAX_CHOICES);
        assert_eq!(prefs.choices.last().unwrap().folder, "g99", "the oldest go first");
        let saved: Prefs = serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
        assert_eq!(saved, prefs);
        let old: Prefs = serde_json::from_str("{}").unwrap();
        assert_eq!(old.follow_up, FollowUp::Steer, "steering is the default");
    }

    #[test]
    fn enter_does_the_default_follow_up_and_alt_enter_the_other() {
        assert_eq!(dispatch(FollowUp::Steer, false, true), Dispatch::Steer);
        assert_eq!(dispatch(FollowUp::Steer, true, true), Dispatch::Queue);
        assert_eq!(dispatch(FollowUp::Queue, false, true), Dispatch::Queue);
        assert_eq!(dispatch(FollowUp::Queue, true, true), Dispatch::Steer);
        assert_eq!(
            dispatch(FollowUp::Steer, false, false),
            Dispatch::Queue,
            "an agent that can't be steered"
        );
    }

    fn thread() -> ThreadInfo {
        ThreadInfo {
            id: "0123456789abcdef0123456789abcdef".into(),
            agent: AgentId::Codex,
            folder_id: "f0123456789ab".into(),
            folder_label: "g7linux".into(),
            title: "Check the build".into(),
            state: api::State::Working,
            mode: Mode::Supervised,
            origin: Origin::launcher(),
            created_ms: 1,
            updated_ms: 2,
            last_seq: 3,
            pending: 0,
            summary: None,
            error: None,
            native_id: None,
            step: None,
            plan: None,
            state_since_ms: 0,
            subdir: None,
            continued: false,
            model: None,
            effort: None,
            answered_model: None,
            queued: Vec::new(),
            queue_held: false,
            context: None,
            mcp: None,
            asking: 0,
            pinned: false,
            archived: false,
            unread: false,
            snoozed_until_ms: None,
            turns: 0,
            forked_from: None,
            found: false,
            readonly: None,
            undo: None,
            branch: None,
            worktree: None,
            changed: None,
            wrote: None,
        }
    }

    #[test]
    fn queued_messages_show_one_line_each_and_who_sent_them() {
        let mut info = thread();
        info.queued = vec![
            Queued {
                id: "0123456789abcdef".into(),
                text: "\n  Then bump versionCode to 115\nand tag it".into(),
                origin: Origin::launcher(),
                images: Vec::new(),
                created_ms: 5,
            },
            Queued {
                id: "fedcba9876543210".into(),
                text: "How big is the APK?".into(),
                origin: Origin::phone("d", "Pixel 8 Pro"),
                images: Vec::new(),
                created_ms: 6,
            },
        ];
        info.queue_held = true;
        let q = queue_json(&info);
        assert_eq!(q["items"][0]["text"], "Then bump versionCode to 115");
        assert_eq!(q["items"][0]["from"], "");
        assert_eq!(q["items"][1]["from"], "Pixel 8 Pro");
        assert_eq!(q["held"], true);
    }

    #[test]
    fn context_and_limits_are_shown_as_reported() {
        let mut info = thread();
        assert_eq!(context_json(&info), json!({}), "nothing until the CLI reports it");
        info.context = Some(ContextUse { used: 104_500, max: 475_000 });
        assert_eq!(context_json(&info)["pct"], 22);
        assert_eq!(context_json(&info)["max"], "475K");
        assert_eq!(compact_tokens(1_000_000), "1M");
        assert_eq!(compact_tokens(1_500_000), "1.5M");
        assert_eq!(compact_tokens(950), "950");
        let mut agent = codex();
        assert_eq!(limit_line(Some(&agent)), json!({}));
        agent.limits = vec![
            Limit { window: "10080".into(), used_pct: Some(12), resets_ms: Some(9), state: LimitState::Ok },
            Limit { window: "300".into(), used_pct: Some(91), resets_ms: Some(7), state: LimitState::Warn },
        ];
        let line = limit_line(Some(&agent));
        assert_eq!((line["window"].as_str(), line["usedPct"].as_i64()), (Some("300"), Some(91)));
        agent.limits[0].state = LimitState::Reached;
        assert_eq!(limit_line(Some(&agent))["window"], "10080", "a reached window comes first");
        assert_eq!(limits_json(&agent.limits).as_array().unwrap().len(), 2);
    }

    fn question(token: &str, created_ms: u64, asks: Vec<Ask>) -> Question {
        Question {
            token: token.into(),
            thread_id: "t".into(),
            item_id: format!("q{token}"),
            secret: asks.iter().any(|a| a.secret),
            asks,
            created_ms,
            expires_ms: created_ms + api::QUESTION_LIFETIME_MS,
        }
    }

    fn ask(id: &str, options: &[&str], multi: bool, other: bool) -> Ask {
        Ask {
            id: id.into(),
            header: "Test runner".into(),
            text: "Which test setup?".into(),
            multi,
            other,
            secret: false,
            options: options.iter().map(|l| AskOption { label: (*l).into(), note: None }).collect(),
        }
    }

    fn approval(token: &str, created_ms: u64) -> Approval {
        Approval {
            token: token.into(),
            thread_id: "t".into(),
            kind: api::ApprovalKind::Command,
            title: "Run".into(),
            command: Some("ls".into()),
            diff: None,
            path: None,
            reason: None,
            truncated: false,
            can_allow_thread: true,
            created_ms,
            expires_ms: created_ms + 1,
            item_id: None,
        }
    }

    #[test]
    fn the_dock_takes_requests_and_questions_in_the_order_they_came() {
        let asks = [approval("a", 30), approval("b", 10)];
        let questions = [question("q", 20, vec![ask("0", &["x"], false, true)])];
        let all = waiting(&asks, &questions);
        let order: Vec<&str> = all.iter().map(Waiting::token).collect();
        assert_eq!(order, ["b", "q", "a"]);
        assert_eq!(shown(&all, |_| false), Some(0));
        assert_eq!(shown(&all, |t| t == "b"), Some(1), "a held one gives way");
        assert_eq!(shown(&all, |_| true), Some(0));
        assert_eq!(shown(&[], |_| false), None);
    }

    #[test]
    fn one_answer_picks_several_toggle_and_typing_chooses_something_else() {
        let q = question(
            "q",
            1,
            vec![
                ask("0", &["cargo test", "nextest", "Both"], false, true),
                ask("1", &["A", "B", "C"], true, true),
            ],
        );
        let mut answering = Answering::default();
        assert!(answering.follow(Some(&q), "draft before"));
        assert_eq!(answering.other_text(), "draft before", "the composer's words stay as the first ask's");
        let shown = answering.json(&q);
        assert_eq!((shown["otherOn"].as_bool(), shown["ready"].as_bool()), (Some(false), Some(false)));
        assert!(!answering.next(&q), "no answer yet");

        answering.pick(&q, 1);
        answering.pick(&q, 2);
        assert_eq!(answering.json(&q)["options"][2]["checked"], true);
        assert_eq!(answering.json(&q)["options"][1]["checked"], false, "one answer: the pick replaces");

        // Typing chooses "Something else"; another option leaves the words, unused.
        answering.type_other(&q, "With a fake radio");
        let shown = answering.json(&q);
        assert_eq!(
            (shown["otherOn"].as_bool(), shown["options"][2]["checked"].as_bool()),
            (Some(true), Some(false))
        );
        answering.pick(&q, 0);
        assert_eq!(answering.other_text(), "With a fake radio");
        assert_eq!(answering.json(&q)["otherOn"], false);
        // "Something else" chosen with nothing typed isn't an answer.
        answering.type_other(&q, "");
        answering.pick(&q, 3);
        assert_eq!(answering.json(&q)["ready"], false);
        answering.type_other(&q, "With a fake radio");
        assert!(answering.next(&q));
        assert_eq!(answering.other_text(), "", "the next ask has words of its own");

        answering.pick(&q, 0);
        answering.pick(&q, 2);
        answering.pick(&q, 0);
        let shown = answering.json(&q);
        assert_eq!((shown["last"].as_bool(), shown["ready"].as_bool()), (Some(true), Some(true)));
        let answers = answering.answers(&q).unwrap();
        assert_eq!(answers[0].labels, Vec::<String>::new());
        assert_eq!(answers[0].other.as_deref(), Some("With a fake radio"));
        assert_eq!(answers[1].labels, ["C"]);
        assert_eq!(answers[1].other, None);
        assert!(answering.back());
        assert_eq!(answering.other_text(), "With a fake radio");
    }

    #[test]
    fn an_ask_without_its_own_answer_takes_no_words() {
        let q = question("q", 1, vec![ask("0", &["Release", "Debug"], false, false)]);
        let mut answering = Answering::default();
        answering.follow(Some(&q), "");
        answering.type_other(&q, "something");
        answering.pick(&q, 2);
        assert_eq!(answering.json(&q)["otherOn"], false);
        assert!(answering.answers(&q).is_none());
        answering.pick(&q, 0);
        assert_eq!(answering.answers(&q).unwrap()[0].labels, ["Release"]);
    }

    #[test]
    fn a_secret_answer_is_gone_after_the_reply_an_ask_change_and_closing() {
        let mut secret_ask = ask("key", &[], false, true);
        secret_ask.secret = true;
        let q = question("s", 1, vec![secret_ask, ask("1", &["Yes"], false, false)]);
        let mut answering = Answering::default();
        answering.follow(Some(&q), "");
        assert_eq!(answering.json(&q)["other"], false, "the composer is never the secret's field");
        answering.type_other(&q, "sk-not-here");
        assert_eq!(answering.json(&q)["secretFilled"], false);
        answering.type_secret(&q, "sk-test-123");
        assert_eq!(answering.json(&q)["secretFilled"], true);
        assert!(!format!("{answering:?}").contains("sk-test"), "never printed");

        // Changing ask asks the page to empty its field.
        let clears = answering.clears();
        assert!(answering.next(&q));
        assert!(answering.clears() > clears);
        answering.pick(&q, 0);
        let answers = answering.answers(&q).unwrap();
        assert_eq!(answers[0].other.as_deref(), Some("sk-test-123"));
        drop(answers);
        // Replied: the question goes, and its secret with it.
        let clears = answering.clears();
        answering.follow(None, "");
        assert!(answering.answers.is_empty() && answering.clears() > clears, "gone after the reply");

        // A question that goes away (answered elsewhere, the thread closed) takes its secret with it.
        answering.follow(Some(&q), "");
        answering.follow(None, "");
        answering.follow(Some(&q), "");
        answering.type_secret(&q, "sk-test-456");
        let clears = answering.clears();
        answering.reset();
        assert!(answering.answers.is_empty() && answering.clears() > clears);
    }

    #[test]
    fn g7s_own_command_leaves_nothing_behind() {
        let t = trigger("/pl", 3).unwrap();
        assert_eq!(remove_run("/pl", &t, 3), (String::new(), 0));
        let t = trigger("/plan the rest", 2).unwrap();
        assert_eq!(remove_run("/plan the rest", &t, 2), ("the rest".to_owned(), 0), "the whole first word");
        let t = trigger("look at @src", 12).unwrap();
        assert_eq!(remove_run("look at @src", &t, 12), ("look at ".to_owned(), 8));
    }

    #[test]
    fn g7s_commands_come_first_and_a_command_shows_its_hint() {
        let commands = vec![
            Command {
                name: "compact".into(),
                note: Some("Clear".into()),
                hint: Some("[instructions]".into()),
                kind: CommandKind::Command,
            },
            Command { name: "review".into(), note: None, hint: None, kind: CommandKind::Command },
            Command {
                name: "fix-ci".into(),
                note: None,
                hint: Some("[job]".into()),
                kind: CommandKind::Skill,
            },
            Command { name: "plan".into(), note: None, hint: None, kind: CommandKind::Command },
        ];
        let names = |rows: Vec<CommandRow>| rows.into_iter().map(|r| r.name).collect::<Vec<_>>();
        assert_eq!(
            names(command_rows(&commands, "", true)),
            ["model", "effort", "plan", "compact", "review", "fix-ci"]
        );
        assert_eq!(
            names(command_rows(&commands, "", false)),
            ["model", "effort", "compact", "review", "fix-ci"]
        );
        assert_eq!(
            names(command_rows(&commands, "c", true)),
            ["compact", "fix-ci"],
            "prefix, then substring"
        );
        assert_eq!(names(command_rows(&commands, "E", true)), ["effort", "model", "review"]);
        assert_eq!(command_hint("/compact ", &commands), Some("[instructions]"));
        assert_eq!(command_hint("/compact now", &commands), None);
        assert_eq!(command_hint("/review ", &commands), None);
        let files = file_rows(&[
            FileHit { path: "core/ble/src/reconnect.rs".into(), dir: false },
            FileHit { path: "docs".into(), dir: true },
        ]);
        assert_eq!(
            (files[0]["name"].as_str(), files[0]["parent"].as_str()),
            (Some("reconnect.rs"), Some("core/ble/src/"))
        );
        assert_eq!((files[1]["name"].as_str(), files[1]["parent"].as_str()), (Some("docs/"), Some("")));
    }

    #[test]
    fn a_draft_from_elsewhere_replaces_an_idle_composer_and_is_offered_to_a_busy_one() {
        let draft = Draft {
            key: "t".into(),
            text: "Make the log say which side".into(),
            images: Vec::new(),
            rev: 3,
            by: Some(Origin::phone("d", "Pixel 8 Pro")),
        };
        assert_eq!(remote_draft(false, "old", &draft, "guchu"), DraftAction::Replace(draft.text.clone()));
        assert_eq!(
            remote_draft(true, "old", &draft, "guchu"),
            DraftAction::Offer { by: "Pixel 8 Pro".into() }
        );
        assert_eq!(remote_draft(true, &draft.text, &draft, "guchu"), DraftAction::Keep);
        let from_launcher = Draft { by: Some(Origin::launcher()), ..draft };
        assert_eq!(
            remote_draft(true, "", &from_launcher, "guchu"),
            DraftAction::Offer { by: "guchu".into() }
        );
    }

    #[test]
    fn a_pasted_image_passes_through_a_private_file_that_is_removed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("g7link");
        let pasted = PastedImage::write(&folder, b"\x89PNG pixels").unwrap();
        let path = pasted.path().to_owned();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::read(&path).unwrap(), b"\x89PNG pixels");
        drop(pasted);
        assert!(!path.exists(), "removed once it's added (or not)");
        // A folder others can read isn't used.
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(PastedImage::write(&folder, b"x").is_err());
    }

    #[test]
    fn a_phones_photo_passes_through_a_private_file_of_its_own_type() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("g7link");
        let photo = PastedImage::write_as(&folder, "agent-photo", "jpg", b"\xff\xd8 jpeg").unwrap();
        let path = photo.path().to_owned();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("agent-photo-") && name.ends_with(".jpg"), "{name}");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        drop(photo);
        assert!(!path.exists());
    }
}
