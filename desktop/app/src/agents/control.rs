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
    collections::HashMap,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use g7_agents::api::{
    self, AgentId, AgentInfo, Answer, Approval, Can, Command, CommandKind, Dispatch, Draft, FileHit, Limit,
    LimitState, Mode, ModelInfo, Origin, OriginKind, Question, ThreadInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::i18n::tr;

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

/// A model's name as the page shows it: the exact model it stands for when
/// the CLI says ("Opus 5"), else its listed name ("Opus"). `None` for the
/// CLI's default when the CLI doesn't say which model that is: the page then
/// writes "<Agent>'s default" rather than guess a version.
pub(crate) fn shown_name(m: &ModelInfo) -> Option<String> {
    match &m.exact {
        Some(exact) => Some(exact.clone()),
        None if m.default => None,
        None => Some(m.name.clone()),
    }
}

/// The shown name of what a setting's `model` runs: the listed model's, the
/// id itself for one the list doesn't hold, and for no model set the model
/// that answered there (`answered`, when the list has it), else the
/// default's. `None`: "<Agent>'s default", version unknown.
pub(crate) fn setting_name(agent: &AgentInfo, model: Option<&str>, answered: Option<&str>) -> Option<String> {
    match model {
        Some(id) => agent.model(id).map_or_else(|| Some(id.to_owned()), shown_name),
        None => {
            answered.and_then(|id| agent.model(id)).or_else(|| agent.default_model()).and_then(shown_name)
        }
    }
}

/// The control's label: the model's shown name ("" when no version is
/// known for the CLI's default, which the page calls "Default") and an
/// effort set by hand ("" for none). `answered`: the model that answered
/// a thread with no model set.
pub(crate) fn model_label(agent: &AgentInfo, setting: &Setting, answered: Option<&str>) -> (String, String) {
    let model = setting_name(agent, setting.model.as_deref(), answered).unwrap_or_default();
    (model, setting.effort.as_deref().map(capitalized).unwrap_or_default())
}

fn model_row(m: &ModelInfo, setting: &Setting, note: Option<&str>) -> Value {
    let checked = match setting.model.as_deref() {
        Some(id) => id == m.id,
        None => m.default,
    };
    json!({
        "id": m.id,
        "name": shown_name(m).unwrap_or_default(),
        "note": note.map_or_else(|| m.note.clone().unwrap_or_default(), str::to_owned),
        "isDefault": m.default,
        "checked": checked,
    })
}

/// Model chips the new-thread form shows before "More…".
pub(crate) const MAX_CHIPS: usize = 4;

/// The agent's models as rows: the short ones (the default first, then the
/// CLI's short names, each shown name once) and the rest. A short name that
/// reads like one already listed ("Opus 5" for both `default` and `opus`)
/// moves to the rest; there, rows that share a name carry their id as the
/// note.
fn model_rows(agent: &AgentInfo, setting: &Setting) -> (Vec<Value>, Vec<Value>) {
    let name = |m: &ModelInfo| shown_name(m).unwrap_or_default();
    let mut short_models: Vec<&ModelInfo> = agent.models.iter().filter(|m| m.default).collect();
    short_models.extend(agent.models.iter().filter(|m| m.short && !m.default));
    let mut seen: Vec<String> = Vec::new();
    let mut short = Vec::new();
    let mut rest: Vec<&ModelInfo> = Vec::new();
    for m in short_models {
        let n = name(m);
        if seen.contains(&n) {
            rest.push(m);
        } else {
            seen.push(n);
            short.push(model_row(m, setting, None));
        }
    }
    rest.extend(agent.models.iter().filter(|m| !m.short && !m.default));
    let shared = |m: &ModelInfo| agent.models.iter().filter(|o| name(o) == name(m)).count() > 1;
    let more = rest.into_iter().map(|m| model_row(m, setting, shared(m).then_some(m.id.as_str()))).collect();
    (short, more)
}

/// The picker: the agent's models as its CLI listed them (short names
/// first, the rest under "More models"), the chosen model's efforts, and the
/// three modes with whether Plan is there. Before the CLI has listed any,
/// only "Default" and a typed name. The new-thread form also reads `chips`
/// (the first rows, and the chosen model when it isn't among them), `menu`
/// (the rows behind "More…"), `effortAuto` ("Auto" comes first: the model
/// says no default effort) and `effortWord` ("thinking" or "reasoning").
pub(crate) fn picker_json(agent: &AgentInfo, setting: &Setting, answered: Option<&str>) -> Value {
    let known = !agent.models.is_empty();
    let has_default = agent.models.iter().any(|m| m.default);
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
    let (listed_short, more) = model_rows(agent, setting);
    short.extend(listed_short);
    let custom = setting.model.as_deref().filter(|id| agent.model(id).is_none()).unwrap_or_default();
    // The new form's chips: the first short rows, and the chosen model in
    // the last place when it isn't among them; the rest go behind "More…".
    let rows: Vec<&Value> = short.iter().chain(&more).collect();
    let chosen_at = rows.iter().position(|r| r["checked"] == true && custom.is_empty());
    let mut shown = MAX_CHIPS.min(short.len());
    let hidden_choice = !custom.is_empty() || chosen_at.is_some_and(|i| i >= shown);
    if hidden_choice {
        shown = shown.min(MAX_CHIPS - 1);
    }
    let mut chips: Vec<Value> = rows[..shown].iter().map(|r| (*r).clone()).collect();
    let menu: Vec<Value> = rows
        .iter()
        .enumerate()
        .skip(shown)
        .filter(|(i, _)| Some(*i) != chosen_at)
        .map(|(_, r)| (*r).clone())
        .collect();
    match chosen_at.filter(|&i| i >= shown) {
        Some(i) => chips.push(rows[i].clone()),
        None if !custom.is_empty() => chips.push(json!({
            "id": custom, "name": custom, "note": "", "isDefault": false, "checked": true, "custom": true,
        })),
        None => {}
    }
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
    let effort_auto = model.is_some_and(|m| !m.efforts.is_empty() && m.default_effort.is_none());
    let mode = setting.mode.unwrap_or(Mode::Ask);
    let (mode_name, mode_line) = mode_text(agent.id, mode);
    let (label_model, label_effort) = model_label(agent, setting, answered);
    json!({
        "agent": agent.id.as_str(),
        "agentName": agent.name,
        "known": known,
        "short": short,
        "more": more,
        "chips": chips,
        "menu": menu,
        "custom": custom,
        "efforts": efforts,
        "effortAuto": effort_auto,
        "effortWord": effort_word(agent.id),
        "mode": api_word(&mode),
        "modeName": mode_name,
        "modeLine": mode_line,
        "modeWarn": mode == Mode::Full,
        "modes": modes_json(agent, setting, mode, true),
        "plan": agent.can(Can::Plan),
        "images": takes_images(agent, setting.model.as_deref()),
        "label": { "model": label_model, "effort": label_effort },
    })
}

/// What the agent calls its effort: Claude thinks, Codex reasons.
pub(crate) fn effort_word(agent: AgentId) -> &'static str {
    match agent {
        AgentId::Claude => "thinking",
        AgentId::Codex => "reasoning",
    }
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

/// The setting after picking an effort the chosen model lists, or "" (the
/// new-thread form's "Auto": the model's own).
pub(crate) fn pick_effort(agent: &AgentInfo, setting: &Setting, effort: &str) -> Option<Setting> {
    if effort.is_empty() {
        return Some(Setting { effort: None, ..setting.clone() });
    }
    let model = chosen(agent, setting.model.as_deref())?;
    model
        .efforts
        .iter()
        .any(|e| e == effort)
        .then(|| Setting { effort: Some(effort.to_owned()), ..setting.clone() })
}

// ---- Modes (plan modes-approvals-composer §4.2) ----

/// A mode's word (`ask`, `accept_edits`, `auto`, `plan`, `full`), as the
/// page and phones send it, or one of the words before 0.1.31
/// (`supervised`, `auto_edit`); `None` for any other.
pub(crate) fn parse_mode(word: &str) -> Option<Mode> {
    serde_json::from_value(Value::String(word.to_owned())).ok()
}

/// A mode as the page sends it back.
pub(crate) fn mode_word(mode: Mode) -> String {
    api_word(&mode)
}

/// A mode the page offers for `agent`: any word of the ones it lists.
pub(crate) fn mode_of(word: &str, agent: &AgentInfo) -> Option<Mode> {
    parse_mode(word).filter(|mode| agent.offers(*mode))
}

/// The mode a new thread of `agent` starts in (plan §2.2): the one `folder`
/// remembers for it while the agent still offers it, else `ask`; never
/// `plan`. Claude's Auto only while `model` (the CLI's default when `None`)
/// can run it.
pub(crate) fn remembered_mode(folder: Option<&api::Folder>, agent: &AgentInfo, model: Option<&str>) -> Mode {
    folder
        .and_then(|f| f.last_mode(agent.id))
        .filter(|mode| mode.remembered() && agent.offers(*mode) && auto_runs(agent, *mode, model))
        .unwrap_or(Mode::ask_first())
}

/// Claude's Auto runs with `model` unless the list says it can't; every
/// other mode does, and Codex's Approve for me with any model.
fn auto_runs(agent: &AgentInfo, mode: Mode, model: Option<&str>) -> bool {
    mode != Mode::Auto || agent.id != AgentId::Claude || chosen(agent, model).is_none_or(|m| m.auto)
}

/// A mode's name and its one line in the agent's own words (plan §4.2;
/// "Claude" and "Codex" stay names).
pub(crate) fn mode_text(agent: AgentId, mode: Mode) -> (String, String) {
    match (agent, mode) {
        (AgentId::Claude, Mode::Ask) => {
            (tr!("Agents", "Manual"), tr!("Agents", "Always ask before making changes"))
        }
        (AgentId::Claude, Mode::AcceptEdits) => {
            (tr!("Agents", "Accept edits"), tr!("Agents", "Automatically accept all file edits"))
        }
        (AgentId::Claude, Mode::Auto) => {
            (tr!("Agents", "Auto"), tr!("Agents", "Claude handles permission decisions"))
        }
        (AgentId::Claude, Mode::Plan) => {
            (tr!("Agents", "Plan"), tr!("Agents", "Create a plan before making changes"))
        }
        (AgentId::Claude, Mode::Full) => {
            (tr!("Agents", "Bypass permissions"), tr!("Agents", "Accepts all permissions"))
        }
        (AgentId::Codex, Mode::Ask) => (
            tr!("Agents", "Ask for approval"),
            tr!("Agents", "Asks before working outside the folder or using the internet"),
        ),
        // Codex has no such mode; a thread from elsewhere still says what it is.
        (AgentId::Codex, Mode::AcceptEdits) => {
            (tr!("Agents", "Accept edits"), tr!("Agents", "Automatically accept all file edits"))
        }
        (AgentId::Codex, Mode::Auto) => (
            tr!("Agents", "Approve for me"),
            tr!("Agents", "Only ask for actions detected as potentially unsafe"),
        ),
        (AgentId::Codex, Mode::Plan) => {
            (tr!("Agents", "Plan"), tr!("Agents", "Changes nothing until you say so"))
        }
        (AgentId::Codex, Mode::Full) => (
            tr!("Agents", "Full access"),
            tr!("Agents", "Unrestricted access to the internet and any file on your computer"),
        ),
    }
}

/// The modes the page offers for `agent` with `setting` (its model), in its
/// CLI's order, each with its name and line, and why one is off (Claude's
/// Auto with a model that can't run it; Plan with a Codex that can't);
/// `current` is checked, and listed last (off) when the agent no longer
/// offers it. Without `plan` for Implement.
pub(crate) fn modes_json(agent: &AgentInfo, setting: &Setting, current: Mode, plan: bool) -> Vec<Value> {
    // A mode the agent doesn't offer comes with why.
    let mut modes: Vec<(Mode, Option<String>)> =
        agent.modes.iter().filter(|m| plan || **m != Mode::Plan).map(|m| (*m, None)).collect();
    if plan && agent.id == AgentId::Codex && !agent.modes.is_empty() && !agent.offers(Mode::Plan) {
        modes.push((Mode::Plan, Some(tr!("Agents", "Plan isn't available with this version of Codex."))));
    }
    if !modes.iter().any(|(m, _)| *m == current) && (plan || current != Mode::Plan) {
        modes.push((current, Some(tr!("Agents", "Not available now"))));
    }
    modes
        .into_iter()
        .map(|(mode, not_offered)| {
            let (name, line) = mode_text(agent.id, mode);
            let runs = auto_runs(agent, mode, setting.model.as_deref());
            let on = not_offered.is_none() && runs;
            let why = match not_offered {
                Some(why) => why,
                None if !runs => tr!("Agents", "Not with this model"),
                None => String::new(),
            };
            json!({
                "id": mode_word(mode),
                "name": name,
                "line": line,
                "on": on,
                "why": why,
                "checked": mode == current,
                // Nothing is asked: shown in the warning tint.
                "warn": mode == Mode::Full,
            })
        })
        .collect()
}

// ---- The new thread: what each folder last ran, and quick picks ----

/// What a folder's newest thread ran with, and what it was about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FolderLast {
    pub agent: AgentId,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The model that answered, for a thread with no model set.
    pub answered: Option<String>,
    pub title: String,
    /// The folder's branch as the thread last read it; never a worktree's.
    pub branch: Option<String>,
    pub used_ms: u64,
}

impl FolderLast {
    fn of(t: &ThreadInfo) -> Self {
        Self {
            agent: t.agent,
            model: t.model.clone(),
            effort: t.effort.clone(),
            answered: t.answered_model.clone(),
            title: t.title.clone(),
            branch: t.branch.clone().filter(|_| t.worktree.is_none()),
            used_ms: t.updated_ms,
        }
    }
}

/// Which of two threads is the newer (ties settled by start, then id).
fn newer(t: &ThreadInfo) -> (u64, u64, &str) {
    (t.updated_ms, t.created_ms, t.id.as_str())
}

/// The newest thread in `folder_id`, as [`FolderLast`].
pub(crate) fn folder_last<'a>(
    threads: impl IntoIterator<Item = &'a ThreadInfo>,
    folder_id: &str,
) -> Option<FolderLast> {
    threads
        .into_iter()
        .filter(|t| t.folder_id == folder_id)
        .max_by(|a, b| newer(a).cmp(&newer(b)))
        .map(FolderLast::of)
}

/// [`folder_last`] for every folder at once, by folder id.
pub(crate) fn lasts<'a>(threads: impl IntoIterator<Item = &'a ThreadInfo>) -> HashMap<String, FolderLast> {
    let mut newest: HashMap<&str, &ThreadInfo> = HashMap::new();
    for t in threads {
        let slot = newest.entry(t.folder_id.as_str()).or_insert(t);
        if newer(t) > newer(slot) {
            *slot = t;
        }
    }
    newest.into_iter().map(|(id, t)| (id.to_owned(), FolderLast::of(t))).collect()
}

/// A folder as quick picks read it.
#[derive(Debug, Clone)]
pub(crate) struct FolderUse<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub used_ms: u64,
    pub last: Option<FolderLast>,
}

/// Where a quick pick comes from: its sub-label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickFrom {
    /// "Last used here".
    Here,
    /// "Used in <folder>".
    Folder(String),
    /// "<Agent>'s default".
    Default,
    /// "Your choice": picked under "Other…".
    Yours,
}

/// What a new thread runs with: exactly what `thread.start` takes, without
/// the mode (that's chosen per task).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuickPick {
    pub agent: AgentId,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// For no model set: the model that answered there, to name it.
    pub answered: Option<String>,
    pub from: PickFrom,
}

/// Quick picks shown at most.
pub(crate) const MAX_PICKS: usize = 3;

/// The agent can start a thread: installed, signed in, nothing wrong.
pub(crate) fn usable(agent: &AgentInfo) -> bool {
    agent.ready() && agent.problem.as_deref().is_none_or(str::is_empty)
}

/// A model and effort as a start sends them: the list's default model is no
/// model set, and an effort the (listed) model doesn't take now is dropped.
/// A model the list doesn't hold keeps its effort: nothing says it can't.
pub(crate) fn normalized(
    agent: &AgentInfo,
    model: Option<&str>,
    effort: Option<&str>,
) -> (Option<String>, Option<String>) {
    let model = model.filter(|id| !agent.model(id).is_some_and(|m| m.default)).map(str::to_owned);
    let listed = match model.as_deref() {
        Some(id) => agent.model(id),
        None => agent.default_model(),
    };
    let effort =
        effort.filter(|e| listed.is_none_or(|m| m.efforts.iter().any(|x| x == e))).map(str::to_owned);
    (model, effort)
}

/// The quick picks for folder `folder_id`, distinct by agent, model and
/// effort, at most [`MAX_PICKS`]: what its newest thread ran, then what
/// other folders' newest threads ran (the most recently used folder first),
/// then each ready agent's default not yet among them. Agents that can't
/// start a thread are left out.
pub(crate) fn quick_picks(
    folders: &[FolderUse<'_>],
    agents: &[AgentInfo],
    folder_id: &str,
) -> Vec<QuickPick> {
    let mut others: Vec<&FolderUse<'_>> = folders.iter().filter(|f| f.id != folder_id).collect();
    others.sort_by(|a, b| b.used_ms.cmp(&a.used_ms).then_with(|| a.id.cmp(b.id)));
    let here = folders.iter().filter(|f| f.id == folder_id).map(|f| (f, PickFrom::Here));
    let elsewhere = others.into_iter().map(|f| (f, PickFrom::Folder(f.label.to_owned())));
    let history = here.chain(elsewhere).filter_map(|(f, from)| {
        let last = f.last.as_ref()?;
        let agent = agents.iter().find(|a| a.id == last.agent).filter(|a| usable(a))?;
        let (model, effort) = normalized(agent, last.model.as_deref(), last.effort.as_deref());
        let answered = model.is_none().then(|| last.answered.clone()).flatten();
        Some(QuickPick { agent: agent.id, model, effort, answered, from })
    });
    let mut picks: Vec<QuickPick> = Vec::with_capacity(MAX_PICKS);
    let same =
        |a: &QuickPick, b: &QuickPick| a.agent == b.agent && a.model == b.model && a.effort == b.effort;
    for pick in history {
        if picks.len() == MAX_PICKS {
            return picks;
        }
        if !picks.iter().any(|p| same(p, &pick)) {
            picks.push(pick);
        }
    }
    for agent in agents.iter().filter(|a| usable(a)) {
        if picks.len() == MAX_PICKS {
            break;
        }
        if !picks.iter().any(|p| p.agent == agent.id) {
            let pick = QuickPick {
                agent: agent.id,
                model: None,
                effort: None,
                answered: None,
                from: PickFrom::Default,
            };
            picks.push(pick);
        }
    }
    picks
}

/// Agents whose pick would be among the folder's history but can't start a
/// thread now: a line under Run with says why.
pub(crate) fn left_out(folders: &[FolderUse<'_>], agents: &[AgentInfo]) -> Vec<AgentId> {
    AgentId::ALL
        .into_iter()
        .filter(|id| folders.iter().any(|f| f.last.as_ref().is_some_and(|l| l.agent == *id)))
        .filter(|id| agents.iter().find(|a| a.id == *id).is_none_or(|a| !usable(a)))
        .collect()
}

/// The effort a pick shows: its own, else its model's default.
pub(crate) fn shown_effort(agent: &AgentInfo, model: Option<&str>, effort: Option<&str>) -> String {
    let default = || chosen(agent, model).and_then(|m| m.default_effort.clone());
    effort.map(str::to_owned).or_else(default).as_deref().map(capitalized).unwrap_or_default()
}

/// A pick as Run with shows it: `name` is the model's shown name ("" when
/// no version is known for the CLI's default), `delisted` a model the list
/// doesn't hold now, `checked` the one the form is set to.
fn pick_json(agent: &AgentInfo, pick: &QuickPick, index: usize, checked: bool) -> Value {
    let (from, folder) = match &pick.from {
        PickFrom::Here => ("here", ""),
        PickFrom::Folder(label) => ("folder", label.as_str()),
        PickFrom::Default => ("default", ""),
        PickFrom::Yours => ("yours", ""),
    };
    let model = pick.model.as_deref();
    json!({
        "index": index,
        "agent": agent.id.as_str(),
        "agentName": agent.name,
        "model": model.unwrap_or_default(),
        "name": setting_name(agent, model, pick.answered.as_deref()).unwrap_or_default(),
        "effort": shown_effort(agent, model, pick.effort.as_deref()),
        "from": from,
        "folder": folder,
        "delisted": !agent.models.is_empty() && model.is_some_and(|id| agent.model(id).is_none()),
        "checked": checked,
    })
}

/// Run with's pills: the quick picks, and the form's own setting as "Your
/// choice" when it is none of them (`current`: its agent and setting).
pub(crate) fn picks_json(
    picks: &[QuickPick],
    agents: &[AgentInfo],
    current: Option<(AgentId, &Setting)>,
) -> Value {
    let wanted = current.and_then(|(id, setting)| {
        let agent = agents.iter().find(|a| a.id == id)?;
        let (model, effort) = normalized(agent, setting.model.as_deref(), setting.effort.as_deref());
        Some((id, model, effort))
    });
    let is_wanted = |p: &QuickPick| {
        wanted.as_ref().is_some_and(|(id, m, e)| p.agent == *id && p.model == *m && p.effort == *e)
    };
    let mut out: Vec<Value> = picks
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            agents.iter().find(|a| a.id == p.agent).map(|a| pick_json(a, p, i, is_wanted(p)))
        })
        .collect();
    if let Some((id, model, effort)) = wanted.as_ref().filter(|_| !picks.iter().any(is_wanted))
        && let Some(agent) = agents.iter().find(|a| a.id == *id)
    {
        let yours = QuickPick {
            agent: *id,
            model: model.clone(),
            effort: effort.clone(),
            answered: None,
            from: PickFrom::Yours,
        };
        out.push(pick_json(agent, &yours, picks.len(), true));
    }
    Value::Array(out)
}

/// Every agent and whether it can start a thread, for "Other…": "ready",
/// "signed_out", "missing" or "problem".
pub(crate) fn agents_json(agents: &[AgentInfo]) -> Value {
    Value::Array(
        agents
            .iter()
            .map(|a| {
                let state = if !a.installed {
                    "missing"
                } else if !a.signed_in {
                    "signed_out"
                } else if !usable(a) {
                    "problem"
                } else {
                    "ready"
                };
                json!({ "id": a.id.as_str(), "name": a.name, "state": state, "loginHint": a.login_hint })
            })
            .collect(),
    )
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

/// Replace when the composer isn't focused, offer when it is; an emptied
/// draft (its text was sent or cleared elsewhere) is never offered.
pub(crate) fn remote_draft(focused: bool, current: &str, draft: &Draft, this_pc: &str) -> DraftAction {
    if draft.text == current {
        return DraftAction::Keep;
    }
    if !focused {
        return DraftAction::Replace(draft.text.clone());
    }
    if draft.text.is_empty() && draft.images.is_empty() {
        return DraftAction::Keep;
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
            exact: None,
            auto: false,
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
        a.modes = vec![Mode::Ask, Mode::AcceptEdits, Mode::Plan, Mode::Full];
        a
    }

    fn codex() -> AgentInfo {
        let mut a = AgentInfo::missing(AgentId::Codex);
        a.can = vec![Can::Questions, Can::Steer, Can::Images];
        let mut first = model("gpt-x", "GPT X", &["low", "medium", "high"], true, false);
        first.default_effort = Some("medium".into());
        a.models = vec![first, model("gpt-y", "GPT Y", &["low"], false, false)];
        // An older Codex: no plan.
        a.modes = vec![Mode::Ask, Mode::Auto, Mode::Full];
        a
    }

    /// Claude with a model that runs Auto (the default) and one that doesn't.
    fn claude_auto() -> AgentInfo {
        let mut a = claude();
        a.models[0].auto = true;
        a.modes = vec![Mode::Auto, Mode::Ask, Mode::AcceptEdits, Mode::Plan, Mode::Full];
        a
    }

    fn folder_with(modes: &[(AgentId, Mode)]) -> api::Folder {
        api::Folder {
            id: "f000000000001".into(),
            label: "app".into(),
            path: "/home/me/app".into(),
            exists: true,
            added_ms: 1,
            used_ms: 2,
            repo: false,
            mcp: true,
            mcp_seen: Vec::new(),
            checkpoints: true,
            follow_remote: false,
            scripts: Vec::new(),
            phone_scripts: false,
            last_modes: modes
                .iter()
                .map(|(agent, mode)| api::FolderMode { agent: *agent, mode: *mode })
                .collect(),
        }
    }

    #[test]
    fn modes_are_read_in_the_new_words_and_the_old_ones() {
        assert_eq!(parse_mode("ask"), Some(Mode::Ask));
        assert_eq!(parse_mode("supervised"), Some(Mode::Ask), "a stored word from before 0.1.31");
        assert_eq!(parse_mode("auto_edit"), Some(Mode::AcceptEdits));
        assert_eq!(parse_mode("accept_edits"), Some(Mode::AcceptEdits));
        assert_eq!(
            (parse_mode("auto"), parse_mode("plan"), parse_mode("full")),
            (Some(Mode::Auto), Some(Mode::Plan), Some(Mode::Full))
        );
        assert_eq!(parse_mode("bypass"), None);
        assert_eq!(parse_mode(""), None);
        for mode in Mode::ALL {
            assert_eq!(parse_mode(&mode_word(mode)), Some(mode), "{mode:?} goes there and back");
        }
    }

    #[test]
    fn the_mode_menu_lists_the_agents_own_modes_in_its_words_and_order() {
        let agent = claude_auto();
        let p = picker_json(&agent, &Setting { mode: Some(Mode::Full), ..Setting::default() }, None);
        let rows = p["modes"].as_array().unwrap();
        let shown: Vec<(&str, &str)> =
            rows.iter().map(|m| (m["id"].as_str().unwrap(), m["name"].as_str().unwrap())).collect();
        assert_eq!(
            shown,
            [
                ("auto", "Auto"),
                ("ask", "Manual"),
                ("accept_edits", "Accept edits"),
                ("plan", "Plan"),
                ("full", "Bypass permissions")
            ]
        );
        assert_eq!(rows[0]["line"], "Claude handles permission decisions");
        assert!(rows.iter().all(|m| m["on"] == true && m["why"] == ""));
        let checked: Vec<&str> =
            rows.iter().filter(|m| m["checked"] == true).map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(checked, ["full"]);
        let warned: Vec<&str> =
            rows.iter().filter(|m| m["warn"] == true).map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(warned, ["full"], "only nothing-asked is in the warning tint");
        assert_eq!(
            (p["modeName"].as_str(), p["modeWarn"].as_bool()),
            (Some("Bypass permissions"), Some(true))
        );
        assert_eq!(p["modeLine"], "Accepts all permissions");

        // Claude's Auto is per model: off, saying why, with one that can't.
        let p = picker_json(&agent, &Setting { model: Some("sonnet".into()), ..Setting::default() }, None);
        let auto = &p["modes"][0];
        assert_eq!((auto["id"].as_str(), auto["on"].as_bool()), (Some("auto"), Some(false)));
        assert_eq!(auto["why"], "Not with this model");
        assert_eq!((p["mode"].as_str(), p["modeName"].as_str()), (Some("ask"), Some("Manual")));

        let codex = codex();
        let p = picker_json(&codex, &Setting { mode: Some(Mode::Auto), ..Setting::default() }, None);
        let shown: Vec<(&str, &str)> = p["modes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| (m["id"].as_str().unwrap(), m["name"].as_str().unwrap()))
            .collect();
        assert_eq!(
            shown,
            [
                ("ask", "Ask for approval"),
                ("auto", "Approve for me"),
                ("full", "Full access"),
                ("plan", "Plan")
            ]
        );
        let plan = &p["modes"][3];
        assert_eq!(plan["on"], false, "this Codex can't plan, and says so");
        assert_eq!(plan["why"], "Plan isn't available with this version of Codex.");
        assert_eq!(p["modes"][0]["line"], "Asks before working outside the folder or using the internet");
        assert_eq!(p["modeName"], "Approve for me");
    }

    #[test]
    fn a_mode_the_agent_no_longer_offers_still_shows_checked_but_off() {
        // Claude in Accept edits, then the host stopped offering it.
        let mut no_edits = claude();
        no_edits.modes.retain(|m| *m != Mode::AcceptEdits);
        let p =
            picker_json(&no_edits, &Setting { mode: Some(Mode::AcceptEdits), ..Setting::default() }, None);
        let last = p["modes"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(
            (last["id"].as_str(), last["on"].as_bool(), last["checked"].as_bool()),
            (Some("accept_edits"), Some(false), Some(true))
        );
        assert_eq!(last["why"], "Not available now");
        // A Codex thread planned, then Codex lost its experimental switch: one row.
        let p = picker_json(&codex(), &Setting { mode: Some(Mode::Plan), ..Setting::default() }, None);
        let plans: Vec<&Value> =
            p["modes"].as_array().unwrap().iter().filter(|m| m["id"] == "plan").collect();
        assert_eq!(plans.len(), 1);
        assert_eq!((plans[0]["checked"].as_bool(), plans[0]["on"].as_bool()), (Some(true), Some(false)));
        // Implement never offers Plan.
        let implement = modes_json(&claude(), &Setting::default(), Mode::Ask, false);
        let ids: Vec<&str> = implement.iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["ask", "accept_edits", "full"]);
    }

    #[test]
    fn a_new_thread_starts_in_the_mode_its_folder_remembers_for_that_agent() {
        let claude = claude_auto();
        let codex = codex();
        let folder = folder_with(&[(AgentId::Claude, Mode::Full), (AgentId::Codex, Mode::Auto)]);
        assert_eq!(remembered_mode(Some(&folder), &claude, None), Mode::Full);
        assert_eq!(remembered_mode(Some(&folder), &codex, None), Mode::Auto, "per agent");
        assert_eq!(remembered_mode(None, &claude, None), Mode::Ask, "a new folder starts ask-first");
        let only_claude = folder_with(&[(AgentId::Claude, Mode::Auto)]);
        assert_eq!(
            remembered_mode(Some(&only_claude), &codex, None),
            Mode::Ask,
            "Claude's mode isn't Codex's"
        );
        assert_eq!(remembered_mode(Some(&only_claude), &claude, None), Mode::Auto);
        assert_eq!(
            remembered_mode(Some(&only_claude), &claude, Some("sonnet")),
            Mode::Ask,
            "not with this model"
        );
        let planned = folder_with(&[(AgentId::Claude, Mode::Plan)]);
        assert_eq!(remembered_mode(Some(&planned), &claude, None), Mode::Ask, "never Plan");
        let edits = folder_with(&[(AgentId::Codex, Mode::AcceptEdits)]);
        assert_eq!(remembered_mode(Some(&edits), &codex, None), Mode::Ask, "only while the agent offers it");
    }

    #[test]
    fn the_picker_shows_the_clis_own_list_short_names_first() {
        let agent = claude();
        let setting =
            Setting { model: Some("opus".into()), effort: Some("high".into()), mode: Some(Mode::Plan) };
        let p = picker_json(&agent, &setting, None);
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
        let p = picker_json(&agent, &Setting::default(), None);
        assert_eq!(p["short"][0]["checked"], true);
        assert_eq!(p["label"]["model"], "");
        assert_eq!(p["efforts"].as_array().unwrap().len(), 3, "the default model's efforts");
    }

    #[test]
    fn codex_marks_its_default_and_has_no_plan_without_the_switch() {
        let agent = codex();
        let p = picker_json(&agent, &Setting::default(), None);
        assert_eq!(p["short"][0]["id"], "gpt-x", "the default model comes first");
        assert_eq!(p["short"][0]["isDefault"], true);
        assert_eq!(p["more"][0]["id"], "gpt-y");
        let medium = &p["efforts"][1];
        assert_eq!((medium["isDefault"].as_bool(), medium["checked"].as_bool()), (Some(true), Some(true)));
        assert_eq!(p["plan"], false);
        assert_eq!(mode_of("plan", &agent), None);
        assert_eq!(mode_of("auto_edit", &agent), None, "Codex has no Accept edits");
        assert_eq!(mode_of("supervised", &agent), Some(Mode::Ask));
        assert_eq!(mode_of("full", &agent), Some(Mode::Full));
        assert_eq!(mode_of("full_access", &agent), None, "no other mode exists");
    }

    #[test]
    fn before_a_list_there_is_default_and_a_typed_name() {
        let mut agent = claude();
        agent.models.clear();
        let p = picker_json(&agent, &Setting::default(), None);
        assert_eq!(p["known"], false);
        assert_eq!(p["short"].as_array().unwrap().len(), 1);
        assert_eq!(
            (p["short"][0]["id"].as_str(), p["short"][0]["checked"].as_bool()),
            (Some(""), Some(true))
        );
        let typed = type_model(&Setting::default(), " my-model ").unwrap();
        assert_eq!(typed.model.as_deref(), Some("my-model"));
        let p = picker_json(&agent, &typed, None);
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

    fn exact(mut m: ModelInfo, name: &str) -> ModelInfo {
        m.exact = Some(name.into());
        m
    }

    /// Claude as a CLI that says which model each alias runs.
    fn claude_exact() -> AgentInfo {
        let mut a = claude();
        a.models = vec![
            exact(
                model("default", "Default (recommended)", &["low", "medium", "high"], true, true),
                "Opus 5",
            ),
            exact(model("opus", "Opus", &["low", "medium", "high", "max"], false, true), "Opus 5"),
            exact(model("sonnet", "Sonnet", &["low", "high"], false, true), "Sonnet 4.6"),
            exact(model("haiku", "Haiku", &[], false, true), "Haiku 4.5"),
            exact(model("fable", "Fable", &[], false, true), "Fable 1"),
            exact(model("claude-opus-5", "Opus 5", &["low", "high"], false, false), "Opus 5"),
            exact(model("claude-opus-4-7", "Opus 4.7", &[], false, false), "Opus 4.7"),
        ];
        a
    }

    fn names(rows: &Value) -> Vec<String> {
        rows.as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap().to_owned()).collect()
    }

    #[test]
    fn models_show_their_exact_names_and_say_plainly_when_none_is_known() {
        let opus = model("opus", "Opus", &[], false, true);
        assert_eq!(shown_name(&opus), Some("Opus".into()), "no version reported: the listed name");
        assert_eq!(shown_name(&exact(opus, "Opus 5")), Some("Opus 5".into()));
        let default = model("default", "Default (recommended)", &[], true, true);
        assert_eq!(
            shown_name(&default),
            None,
            "never 'Default (recommended)': the page says '<Agent>'s default'"
        );
        assert_eq!(shown_name(&exact(default, "Opus 5")), Some("Opus 5".into()));

        let agent = claude_exact();
        assert_eq!(
            setting_name(&agent, None, None).as_deref(),
            Some("Opus 5"),
            "no model set: the default's"
        );
        assert_eq!(
            setting_name(&agent, None, Some("claude-opus-4-7")).as_deref(),
            Some("Opus 4.7"),
            "what answered"
        );
        assert_eq!(setting_name(&agent, None, Some("gone")).as_deref(), Some("Opus 5"));
        assert_eq!(setting_name(&agent, Some("sonnet"), None).as_deref(), Some("Sonnet 4.6"));
        assert_eq!(
            setting_name(&agent, Some("my-model"), None).as_deref(),
            Some("my-model"),
            "typed: as typed"
        );
        assert_eq!(setting_name(&claude(), None, None), None);
        let (label, _) = model_label(&agent, &Setting::default(), None);
        assert_eq!(label, "Opus 5", "the composer's chip names the version too");
    }

    #[test]
    fn a_name_two_rows_share_shows_once_up_front_and_with_its_id_in_the_full_list() {
        let agent = claude_exact();
        let p = picker_json(&agent, &Setting::default(), None);
        assert_eq!(
            names(&p["short"]),
            ["Opus 5", "Sonnet 4.6", "Haiku 4.5", "Fable 1"],
            "the opus alias reads like the default"
        );
        assert_eq!(p["short"][0]["isDefault"], true);
        let more = p["more"].as_array().unwrap();
        let ids: Vec<&str> = more.iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["opus", "claude-opus-5", "claude-opus-4-7"]);
        assert_eq!(
            (more[0]["note"].as_str(), more[1]["note"].as_str()),
            (Some("opus"), Some("claude-opus-5"))
        );
        assert_eq!(more[2]["note"], "", "a name of its own needs no id");
        // Picking the alias still sends the alias.
        assert_eq!(pick_model(&agent, &Setting::default(), "opus").model.as_deref(), Some("opus"));
    }

    #[test]
    fn the_form_shows_four_chips_and_the_chosen_model_always_among_them() {
        let agent = claude_exact();
        let p = picker_json(&agent, &Setting::default(), None);
        assert_eq!(names(&p["chips"]), ["Opus 5", "Sonnet 4.6", "Haiku 4.5", "Fable 1"]);
        assert_eq!(p["menu"].as_array().unwrap().len(), 3);

        let set = Setting { model: Some("claude-opus-4-7".into()), ..Setting::default() };
        let p = picker_json(&agent, &set, None);
        assert_eq!(names(&p["chips"]), ["Opus 5", "Sonnet 4.6", "Haiku 4.5", "Opus 4.7"]);
        assert_eq!(p["chips"][3]["checked"], true);
        let menu: Vec<&str> =
            p["menu"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(menu, ["fable", "opus", "claude-opus-5"], "nothing lost, nothing twice");

        let typed = type_model(&Setting::default(), "my-model").unwrap();
        let p = picker_json(&agent, &typed, None);
        assert_eq!(names(&p["chips"]), ["Opus 5", "Sonnet 4.6", "Haiku 4.5", "my-model"]);
        assert_eq!(p["chips"][3]["custom"], true);
        assert_eq!(p["chips"][0]["checked"], false);

        let few = codex();
        let p = picker_json(&few, &Setting { model: Some("gpt-y".into()), ..Setting::default() }, None);
        let chips: Vec<&str> =
            p["chips"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
        assert_eq!(chips, ["gpt-x", "gpt-y"]);
        assert!(p["menu"].as_array().unwrap().is_empty());
    }

    #[test]
    fn claude_thinks_with_auto_first_and_codex_reasons_from_its_default() {
        let agent = claude_exact();
        let p = picker_json(&agent, &Setting::default(), None);
        assert_eq!((p["effortWord"].as_str(), p["effortAuto"].as_bool()), (Some("thinking"), Some(true)));
        let p = picker_json(&codex(), &Setting::default(), None);
        assert_eq!((p["effortWord"].as_str(), p["effortAuto"].as_bool()), (Some("reasoning"), Some(false)));
        let haiku = Setting { model: Some("haiku".into()), ..Setting::default() };
        assert_eq!(picker_json(&agent, &haiku, None)["effortAuto"], false, "no efforts: no Auto either");

        let high = pick_effort(&agent, &Setting::default(), "high").unwrap();
        assert_eq!(high.effort.as_deref(), Some("high"));
        assert_eq!(pick_effort(&agent, &high, "").unwrap().effort, None, "Auto: the model's own");
    }

    fn at(
        folder: &str,
        updated_ms: u64,
        agent: AgentId,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> ThreadInfo {
        let mut t = thread();
        t.id = format!("{folder}{updated_ms:024}");
        t.folder_id = folder.into();
        t.updated_ms = updated_ms;
        t.agent = agent;
        t.model = model.map(str::to_owned);
        t.effort = effort.map(str::to_owned);
        t.title = format!("Thread at {updated_ms}");
        t.branch = Some("main".into());
        t
    }

    #[test]
    fn a_folders_last_is_its_newest_thread_and_a_worktree_gives_no_branch() {
        let mut worktree = at("f1", 30, AgentId::Claude, Some("sonnet"), Some("high"));
        worktree.worktree = Some(api::WorktreeTag {
            id: "0123456789ab".into(),
            branch: "g7/fix".into(),
            label: "fix".into(),
            missing: false,
        });
        worktree.branch = Some("g7/fix".into());
        worktree.answered_model = Some("claude-sonnet-4-6".into());
        let threads =
            [at("f1", 10, AgentId::Codex, None, None), worktree, at("f2", 50, AgentId::Codex, None, None)];
        let last = folder_last(&threads, "f1").unwrap();
        assert_eq!(
            (last.agent, last.model.as_deref(), last.effort.as_deref()),
            (AgentId::Claude, Some("sonnet"), Some("high"))
        );
        assert_eq!(last.title, "Thread at 30");
        assert_eq!(last.branch, None, "a worktree's branch isn't the folder's");
        assert_eq!((last.used_ms, last.answered.as_deref()), (30, Some("claude-sonnet-4-6")));
        assert_eq!(folder_last(&threads, "f2").unwrap().branch.as_deref(), Some("main"));
        assert_eq!(folder_last(&threads, "f3"), None);
        let all = lasts(&threads);
        assert_eq!(all.len(), 2);
        assert_eq!(all["f1"], last, "the same answer for every folder at once");
    }

    fn uses<'a>(folders: &'a [(&'a str, u64)], threads: &[ThreadInfo]) -> Vec<FolderUse<'a>> {
        let lasts = lasts(threads);
        folders
            .iter()
            .map(|(id, used)| FolderUse { id, label: id, used_ms: *used, last: lasts.get(*id).cloned() })
            .collect()
    }

    fn summary(picks: &[QuickPick]) -> Vec<(AgentId, Option<&str>, Option<&str>, PickFrom)> {
        picks.iter().map(|p| (p.agent, p.model.as_deref(), p.effort.as_deref(), p.from.clone())).collect()
    }

    fn ready_codex() -> AgentInfo {
        let mut a = codex();
        a.installed = true;
        a.signed_in = true;
        a
    }

    #[test]
    fn quick_picks_are_this_folders_last_then_other_folders_then_defaults() {
        let agents = [claude_exact(), ready_codex()];
        let threads = [
            at("here", 10, AgentId::Claude, Some("sonnet"), Some("high")),
            at("b", 40, AgentId::Codex, Some("gpt-y"), Some("low")),
            at("c", 30, AgentId::Claude, Some("sonnet"), Some("high")),
            at("d", 20, AgentId::Claude, Some("opus"), Some("max")),
        ];
        let folders = [("here", 10), ("c", 90), ("b", 80), ("d", 70), ("never", 100)];
        let f = uses(&folders, &threads);
        let picks = quick_picks(&f, &agents, "here");
        assert_eq!(
            summary(&picks),
            [
                (AgentId::Claude, Some("sonnet"), Some("high"), PickFrom::Here),
                (AgentId::Codex, Some("gpt-y"), Some("low"), PickFrom::Folder("b".into())),
                (AgentId::Claude, Some("opus"), Some("max"), PickFrom::Folder("d".into())),
            ],
            "c ran the same as here: it's shown once; at most three"
        );

        // No history at all: each ready agent's default, in the agents' order.
        let picks = quick_picks(&uses(&[("here", 1)], &[]), &agents, "here");
        assert_eq!(
            summary(&picks),
            [
                (AgentId::Claude, None, None, PickFrom::Default),
                (AgentId::Codex, None, None, PickFrom::Default)
            ]
        );
        // A folder never used: what others ran, then the defaults of agents not among them.
        let threads = [at("b", 40, AgentId::Codex, Some("gpt-y"), None)];
        let picks = quick_picks(&uses(&[("new", 1), ("b", 40)], &threads), &agents, "new");
        assert_eq!(
            summary(&picks),
            [
                (AgentId::Codex, Some("gpt-y"), None, PickFrom::Folder("b".into())),
                (AgentId::Claude, None, None, PickFrom::Default)
            ]
        );
    }

    #[test]
    fn an_agent_that_cant_start_is_left_out_and_said_so() {
        let mut signed_out = claude_exact();
        signed_out.signed_in = false;
        let agents = [signed_out, ready_codex()];
        let threads = [at("here", 10, AgentId::Claude, Some("sonnet"), None)];
        let f = uses(&[("here", 10)], &threads);
        let picks = quick_picks(&f, &agents, "here");
        assert_eq!(summary(&picks), [(AgentId::Codex, None, None, PickFrom::Default)]);
        assert_eq!(left_out(&f, &agents), [AgentId::Claude]);
        let mut troubled = ready_codex();
        troubled.problem = Some("It crashed".into());
        assert!(!usable(&troubled));
        assert_eq!(
            left_out(&uses(&[("here", 10)], &[]), &agents),
            Vec::<AgentId>::new(),
            "no history, nothing to say"
        );
        assert_eq!(agents_json(&agents)[0]["state"], "signed_out");
        assert_eq!(agents_json(&[troubled, AgentInfo::missing(AgentId::Claude)])[0]["state"], "problem");
        assert_eq!(agents_json(&[AgentInfo::missing(AgentId::Claude)])[0]["state"], "missing");
    }

    #[test]
    fn a_pick_drops_an_effort_its_model_lost_and_keeps_a_model_the_list_lost() {
        let agents = [claude_exact()];
        let threads = [
            at("here", 30, AgentId::Claude, Some("sonnet"), Some("max")),
            at("b", 20, AgentId::Claude, Some("claude-opus-4-1"), Some("high")),
            at("c", 10, AgentId::Claude, Some("default"), Some("medium")),
        ];
        let f = uses(&[("here", 30), ("b", 20), ("c", 10)], &threads);
        let picks = quick_picks(&f, &agents, "here");
        assert_eq!(
            summary(&picks),
            [
                (AgentId::Claude, Some("sonnet"), None, PickFrom::Here),
                (AgentId::Claude, Some("claude-opus-4-1"), Some("high"), PickFrom::Folder("b".into())),
                (AgentId::Claude, None, Some("medium"), PickFrom::Folder("c".into())),
            ],
            "Sonnet takes no max now; the default row is no model set"
        );
        let json = picks_json(&picks, &agents, Some((AgentId::Claude, &Setting::default())));
        assert_eq!(
            (json[1]["name"].as_str(), json[1]["delisted"].as_bool()),
            (Some("claude-opus-4-1"), Some(true))
        );
        assert_eq!((json[2]["name"].as_str(), json[2]["effort"].as_str()), (Some("Opus 5"), Some("Medium")));
        assert_eq!(json[0]["from"], "here");
        assert_eq!(json[1]["folder"], "b");
        assert_eq!(
            json.as_array().unwrap().len(),
            4,
            "the form's own setting is none of them: 'Your choice'"
        );
        assert_eq!((json[3]["from"].as_str(), json[3]["checked"].as_bool()), (Some("yours"), Some(true)));

        let set = Setting { model: Some("sonnet".into()), effort: None, mode: Some(Mode::Plan) };
        let json = picks_json(&picks, &agents, Some((AgentId::Claude, &set)));
        assert_eq!(json.as_array().unwrap().len(), 3);
        assert_eq!((json[0]["checked"].as_bool(), json[1]["checked"].as_bool()), (Some(true), Some(false)));
    }

    #[test]
    fn a_pick_names_what_answered_and_its_models_default_effort() {
        let mut codex = ready_codex();
        codex.models[0].exact = Some("GPT-X 5".into());
        let agents = [codex];
        let mut t = at("here", 10, AgentId::Codex, None, None);
        t.answered_model = Some("gpt-y".into());
        let f = uses(&[("here", 10)], &[t]);
        let picks = quick_picks(&f, &agents, "here");
        let json = picks_json(&picks, &agents, None);
        assert_eq!((json[0]["name"].as_str(), json[0]["effort"].as_str()), (Some("GPT Y"), Some("Medium")));
        let json = picks_json(&quick_picks(&uses(&[("here", 1)], &[]), &agents, "here"), &agents, None);
        assert_eq!((json[0]["name"].as_str(), json[0]["from"].as_str()), (Some("GPT-X 5"), Some("default")));
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
            mode: Mode::Ask,
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
            ports: Vec::new(),
            runs: Vec::new(),
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
            deny_stops: false,
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
        let from_launcher = Draft { by: Some(Origin::launcher()), ..draft.clone() };
        assert_eq!(
            remote_draft(true, "", &from_launcher, "guchu"),
            DraftAction::Offer { by: "guchu".into() }
        );
        // Sent from the phone: the draft is emptied, and nothing is offered.
        let sent = Draft { text: String::new(), rev: 4, ..draft };
        assert_eq!(remote_draft(true, "mine", &sent, "guchu"), DraftAction::Keep);
        assert_eq!(remote_draft(false, "old", &sent, "guchu"), DraftAction::Replace(String::new()));
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
