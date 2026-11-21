// SPDX-License-Identifier: MPL-2.0
//! Bodies of the agents' control messages (`docs/protocol/agents.md` §5,
//! capability `agent.control`): models and effort, plan mode, steering and
//! the queue, questions, images, files, slash commands and drafts.
//!
//! Every text an agent or a CLI produced is cleaned like an item's text:
//! no control characters (newline and tab only where the field is
//! multi-line), no bidirectional controls, cut to its limit. An id, a path
//! or a label that names something is never cut: one over its limit is
//! left out instead. A word this build doesn't know reads as absent (or as
//! the type's default word), never as a reason to drop the message.

use std::ops::Not;

use serde::{Deserialize, Serialize};

use super::{
    agent, clean_agent_text, clean_line, clean_opt, cut_text_front, fits_opt, folder_name, is_agent_image_id,
    is_agent_item_id, is_agent_queued_id, is_agent_word, is_bidi_control, is_effort, is_model_id,
    is_valid_agent_id, is_valid_image_ids,
};

/// The `agent/image` and `agent/thumb` streams: one picture per stream (§5.3).
pub mod agent_image {
    pub const SERVICE: &str = "agent";
    /// Phone → PC: an image for a message.
    pub const OP_UPLOAD: &str = "image";
    /// PC → phone: a JPEG thumbnail of an image.
    pub const OP_THUMB: &str = "thumb";
    pub const VERSION: u32 = 1;
}

/// Whether `bytes` start like a JPEG.
pub fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8, 0xFF])
}

/// One line of `text`, cleaned, cut to `max`; `None` when nothing is left.
fn line_or_none(text: Option<String>, max: usize) -> Option<String> {
    let mut text = text;
    clean_opt(&mut text, false, max);
    text.filter(|t| !t.trim().is_empty())
}

/// Whether `text` is 1 to `max` bytes without control or bidi characters
/// (a label, an id or a name that must arrive whole).
fn is_whole_line(text: &str, max: usize) -> bool {
    (1..=max).contains(&text.len()) && !text.chars().any(|c| c.is_control() || is_bidi_control(c))
}

/// Whether `text` is at most `max` bytes without control characters other
/// than newline and tab, and without bidi controls.
fn is_clean_text(text: &str, max: usize) -> bool {
    text.len() <= max
        && !text.chars().any(|c| is_bidi_control(c) || (c.is_control() && !matches!(c, '\n' | '\t')))
}

// ---- The catalog ----

/// A model an agent's CLI reports on the PC (`AgentCatalogAgent.models`).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentModel {
    /// What the CLI takes (`--model <id>`): no white space, not starting with `-`.
    pub id: String,
    pub name: String,
    /// The CLI's description of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The CLI's default.
    #[serde(rename = "default", default, skip_serializing_if = "Not::not")]
    pub is_default: bool,
    /// Its effort levels, as the CLI names them; empty: no effort choice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    /// It takes images.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub images: bool,
    /// A short name that follows the newest model (`opus`): listed first.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub short: bool,
}

/// Never prints the description.
impl std::fmt::Debug for AgentModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentModel")
            .field("id", &self.id)
            .field("default", &self.is_default)
            .field("efforts", &self.efforts)
            .field("images", &self.images)
            .finish_non_exhaustive()
    }
}

impl AgentModel {
    pub fn is_valid(&self) -> bool {
        is_model_id(&self.id)
            && self.name.len() <= agent::MAX_MODEL_NAME_BYTES
            && fits_opt(&self.note, agent::MAX_MODEL_NOTE_BYTES)
            && self.efforts.len() <= agent::MAX_EFFORTS
            && self.efforts.iter().all(|e| is_effort(e))
            && self.default_effort.as_deref().is_none_or(is_effort)
    }

    /// The model within its limits: its name and note cleaned and cut, its
    /// efforts the well-formed ones; `None` when its id isn't one.
    pub fn sanitized(mut self) -> Option<Self> {
        if !is_model_id(&self.id) {
            return None;
        }
        clean_line(&mut self.name, agent::MAX_MODEL_NAME_BYTES);
        if self.name.trim().is_empty() {
            self.name = self.id.clone();
        }
        self.note = line_or_none(self.note, agent::MAX_MODEL_NOTE_BYTES);
        self.efforts.retain(|e| is_effort(e));
        self.efforts.truncate(agent::MAX_EFFORTS);
        if !self.default_effort.as_deref().is_none_or(is_effort) {
            self.default_effort = None;
        }
        Some(self)
    }
}

/// A usage window as the agent's CLI reported it (`AgentCatalogAgent.limits`).
/// Nothing else about the account crosses.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentLimit {
    /// As the CLI names it: `five_hour`, `seven_day`, or minutes (`300`).
    pub window: String,
    /// How much of it is used, 0 to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_pct: Option<u8>,
    /// When it resets, Unix ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets: Option<u64>,
    /// One of [`agent::limit_state`].
    pub state: String,
}

impl AgentLimit {
    pub fn is_valid(&self) -> bool {
        (1..=agent::MAX_LIMIT_WINDOW_BYTES).contains(&self.window.len())
            && !self.state.is_empty()
            && self.state.len() <= agent::MAX_WORD_BYTES
    }

    /// The window within its limits: its name cleaned and cut, `used_pct`
    /// at most 100, a state this build doesn't know as `ok`; `None` without
    /// a name.
    pub fn sanitized(mut self) -> Option<Self> {
        clean_line(&mut self.window, agent::MAX_LIMIT_WINDOW_BYTES);
        if self.window.trim().is_empty() {
            return None;
        }
        self.used_pct = self.used_pct.map(|p| p.min(100));
        if !agent::limit_state::ALL.contains(&self.state.as_str()) {
            self.state = agent::limit_state::OK.to_owned();
        }
        Some(self)
    }
}

// ---- Threads ----

/// An image of a message, a queued message or a draft. The bytes stay on
/// the PC; a phone fetches a thumbnail with `agent.image.thumb`.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentImage {
    /// 32 lowercase hex digits.
    pub id: String,
    /// A file name, never a path.
    pub name: String,
    /// Its size as the agent gets it.
    pub bytes: u32,
    pub w: u16,
    pub h: u16,
}

/// Never prints the name.
impl std::fmt::Debug for AgentImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentImage")
            .field("bytes", &self.bytes)
            .field("w", &self.w)
            .field("h", &self.h)
            .finish_non_exhaustive()
    }
}

/// `name` as an image's name: one line, its last path component, at most
/// [`agent::MAX_IMAGE_NAME_BYTES`] keeping its end (the extension).
pub fn clean_image_name(name: &str) -> String {
    let clean = clean_agent_text(name, false, name.len()).0;
    let last = folder_name(&clean);
    cut_text_front(last, agent::MAX_IMAGE_NAME_BYTES).0.to_owned()
}

impl AgentImage {
    pub fn is_valid(&self) -> bool {
        is_agent_image_id(&self.id) && self.name.len() <= agent::MAX_IMAGE_NAME_BYTES
    }

    /// The image with its name a file name; `None` when its id isn't one.
    pub fn sanitized(mut self) -> Option<Self> {
        if !is_agent_image_id(&self.id) {
            return None;
        }
        self.name = clean_image_name(&self.name);
        Some(self)
    }
}

/// A message waiting for the running turn to end (`AgentThread.queued`).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentQueued {
    /// 16 lowercase hex digits.
    pub id: String,
    pub text: String,
    /// The text was cut to fit this message: the whole of it is on the PC
    /// (in the thread's page and its events). Don't offer to edit it here.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub truncated: bool,
    /// The device it was sent from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<AgentImage>,
    /// Unix ms.
    pub created: u64,
}

/// Never prints the text or who sent it.
impl std::fmt::Debug for AgentQueued {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentQueued")
            .field("bytes", &self.text.len())
            .field("truncated", &self.truncated)
            .field("images", &self.images.len())
            .finish_non_exhaustive()
    }
}

impl AgentQueued {
    pub fn is_valid(&self) -> bool {
        is_agent_queued_id(&self.id)
            && self.text.len() <= agent::MAX_QUEUED_TEXT_BYTES
            && fits_opt(&self.by_name, agent::MAX_LABEL_BYTES)
            && self.images.len() <= agent::MAX_IMAGES
            && self.images.iter().all(AgentImage::is_valid)
    }

    /// The message within its limits (text cut is flagged `truncated`);
    /// `None` when its id isn't one.
    pub fn sanitized(mut self) -> Option<Self> {
        if !is_agent_queued_id(&self.id) {
            return None;
        }
        let (text, cut) = clean_agent_text(&self.text, true, agent::MAX_QUEUED_TEXT_BYTES);
        self.text = text;
        self.truncated |= cut;
        self.by_name = line_or_none(self.by_name, agent::MAX_LABEL_BYTES);
        self.images = self.images.into_iter().filter_map(AgentImage::sanitized).collect();
        self.images.truncate(agent::MAX_IMAGES);
        Some(self)
    }
}

/// How full a thread's context is, as the agent's CLI last reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentContext {
    /// Tokens in use.
    pub used: u64,
    /// Tokens the model takes.
    pub max: u64,
}

/// The user's own MCP servers in a thread (`AgentThread.mcp`): counts only;
/// their names never reach a phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentMcp {
    /// The folder's switch: the thread loads them.
    pub on: bool,
    /// How many the agent reported.
    #[serde(default)]
    pub servers: u16,
    pub connected: u16,
    /// How many need the user to sign in, in a terminal on the PC.
    #[serde(default)]
    pub need_login: u16,
}

/// A `question` item's state (`AgentItem.question`).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentItemQuestion {
    /// How many it asked, 1 to [`agent::MAX_QUESTION_ASKS`].
    pub count: u8,
    /// Some ask was secret: the item never has the answers.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub secret: bool,
    /// How it ended: one of [`agent::questioned`]; absent while it waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// The device that answered or dismissed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_name: Option<String>,
}

impl std::fmt::Debug for AgentItemQuestion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentItemQuestion")
            .field("count", &self.count)
            .field("secret", &self.secret)
            .field("outcome", &self.outcome)
            .finish_non_exhaustive()
    }
}

impl AgentItemQuestion {
    pub fn is_valid(&self) -> bool {
        fits_opt(&self.outcome, agent::MAX_WORD_BYTES) && fits_opt(&self.by_name, agent::MAX_LABEL_BYTES)
    }

    /// Within its limits: `count` at most [`agent::MAX_QUESTION_ASKS`], an
    /// outcome this build doesn't know read as absent (waiting).
    pub fn sanitized(mut self) -> Self {
        self.count = self.count.min(agent::MAX_QUESTION_ASKS as u8);
        if !self.outcome.as_deref().is_some_and(|o| agent::questioned::ALL.contains(&o)) {
            self.outcome = None;
        }
        self.by_name = line_or_none(self.by_name, agent::MAX_LABEL_BYTES);
        self
    }
}

// ---- Requests: configure, queue, implement ----

/// Body of `agent.configure`: phone to PC. Each field set changes it for
/// the thread's next turn; an empty `model` or `effort` goes back to the
/// CLI's default. Answered with `agent.thread`.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentConfigure {
    pub thread: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// [`agent::mode::SUPERVISED`] or [`agent::mode::PLAN`] from a phone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// `model` was typed by the user rather than picked from the catalog.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub custom_model: bool,
}

impl std::fmt::Debug for AgentConfigure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentConfigure")
            .field("thread", &self.thread)
            .field("model", &self.model)
            .field("effort", &self.effort)
            .field("mode", &self.mode)
            .field("custom_model", &self.custom_model)
            .finish()
    }
}

impl AgentConfigure {
    /// A thread id, at least one change, each well formed. Whether the mode
    /// may be used is the PC's call (`DENIED`).
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
            && (self.model.is_some() || self.effort.is_some() || self.mode.is_some())
            && self.model.as_deref().is_none_or(|m| m.is_empty() || is_model_id(m))
            && self.effort.as_deref().is_none_or(|e| e.is_empty() || is_effort(e))
            && self.mode.as_deref().is_none_or(is_agent_word)
            && (!self.custom_model || self.model.as_deref().is_some_and(|m| !m.is_empty()))
    }
}

/// Body of `agent.queue`: phone to PC, a change to one queued message.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentQueueChange {
    pub thread: String,
    /// The queued message's id.
    pub id: String,
    /// One of [`agent::queue_op`].
    pub op: String,
    /// [`agent::queue_op::EDIT`] only: the new text, 1 to [`agent::MAX_QUEUED_TEXT_BYTES`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Never prints the text.
impl std::fmt::Debug for AgentQueueChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentQueueChange")
            .field("thread", &self.thread)
            .field("op", &self.op)
            .field("bytes", &self.text.as_ref().map(String::len))
            .finish_non_exhaustive()
    }
}

impl AgentQueueChange {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
            && is_agent_queued_id(&self.id)
            && agent::queue_op::ALL.contains(&self.op.as_str())
            && match &self.text {
                Some(text) => {
                    self.op == agent::queue_op::EDIT
                        && (1..=agent::MAX_QUEUED_TEXT_BYTES).contains(&text.len())
                }
                None => self.op != agent::queue_op::EDIT,
            }
    }
}

/// Body of `agent.implement`: phone to PC. Implements the thread's newest
/// proposed plan (`item`), asking before edits and commands.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentImplement {
    pub thread: String,
    /// The `plan` item's id.
    pub item: String,
}

impl AgentImplement {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread) && is_agent_item_id(&self.item)
    }
}

// ---- Questions ----

/// One option of an ask.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentQuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl std::fmt::Debug for AgentQuestionOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentQuestionOption").finish_non_exhaustive()
    }
}

/// One ask of a question: the agent's words, and the options it offers.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentQuestionAsk {
    pub id: String,
    /// A short tag ("Auth method").
    #[serde(default)]
    pub header: String,
    pub text: String,
    /// More than one option may be picked.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub multi: bool,
    /// The user may answer in their own words.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub other: bool,
    /// Answered on the PC only (never reaches a phone).
    #[serde(default, skip_serializing_if = "Not::not")]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<AgentQuestionOption>,
}

/// Never prints the agent's words or the options.
impl std::fmt::Debug for AgentQuestionAsk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentQuestionAsk")
            .field("multi", &self.multi)
            .field("other", &self.other)
            .field("secret", &self.secret)
            .field("options", &self.options.len())
            .finish_non_exhaustive()
    }
}

impl AgentQuestionAsk {
    fn is_valid(&self) -> bool {
        is_whole_line(&self.id, agent::MAX_ASK_ID_BYTES)
            && self.header.len() <= agent::MAX_ASK_HEADER_BYTES
            && self.text.len() <= agent::MAX_ASK_TEXT_BYTES
            && self.options.len() <= agent::MAX_OPTIONS
            && self.options.iter().all(|o| {
                is_whole_line(&o.label, agent::MAX_OPTION_LABEL_BYTES)
                    && fits_opt(&o.note, agent::MAX_OPTION_NOTE_BYTES)
            })
    }

    /// The ask cleaned and cut; options whose label isn't a whole label
    /// left out (an answer names them); `None` when its id isn't one.
    fn sanitized(mut self) -> Option<Self> {
        if !is_whole_line(&self.id, agent::MAX_ASK_ID_BYTES) {
            return None;
        }
        clean_line(&mut self.header, agent::MAX_ASK_HEADER_BYTES);
        self.text = clean_agent_text(&self.text, true, agent::MAX_ASK_TEXT_BYTES).0;
        self.options.retain(|o| is_whole_line(&o.label, agent::MAX_OPTION_LABEL_BYTES));
        self.options.truncate(agent::MAX_OPTIONS);
        for option in &mut self.options {
            option.note = line_or_none(option.note.take(), agent::MAX_OPTION_NOTE_BYTES);
        }
        Some(self)
    }
}

/// Body of `agent.question`: PC to phone, an agent asks the user (§5.2).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentQuestion {
    /// Single use, like an ask's.
    pub token: String,
    pub thread: String,
    #[serde(default)]
    pub thread_title: String,
    /// The agent's id.
    pub agent: String,
    /// The `question` item it belongs to.
    pub item: String,
    /// Answered on the PC only: a phone gets no asks and may not answer it.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub secret: bool,
    /// 1 to [`agent::MAX_QUESTION_ASKS`]; none for a secret question on a phone.
    #[serde(default)]
    pub asks: Vec<AgentQuestionAsk>,
    /// Unix ms.
    pub created: u64,
    /// Unix ms.
    pub expires: u64,
}

/// Never prints the token or the agent's words.
impl std::fmt::Debug for AgentQuestion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentQuestion")
            .field("thread", &self.thread)
            .field("agent", &self.agent)
            .field("secret", &self.secret)
            .field("asks", &self.asks)
            .finish_non_exhaustive()
    }
}

impl AgentQuestion {
    pub fn is_valid(&self) -> bool {
        let fewest = usize::from(!self.secret);
        is_valid_agent_id(&self.token)
            && is_valid_agent_id(&self.thread)
            && is_valid_agent_id(&self.agent)
            && self.thread_title.len() <= agent::MAX_TITLE_BYTES
            && is_agent_item_id(&self.item)
            && (fewest..=agent::MAX_QUESTION_ASKS).contains(&self.asks.len())
            && self.asks.iter().all(AgentQuestionAsk::is_valid)
            && (self.secret || !self.asks.iter().any(|a| a.secret))
    }

    /// The question within its limits, secret when any ask is; `None` when
    /// an id is bad or no ask is left of one that isn't secret.
    pub fn sanitized(mut self) -> Option<Self> {
        if !is_valid_agent_id(&self.token)
            || !is_valid_agent_id(&self.thread)
            || !is_valid_agent_id(&self.agent)
            || !is_agent_item_id(&self.item)
        {
            return None;
        }
        clean_line(&mut self.thread_title, agent::MAX_TITLE_BYTES);
        self.secret |= self.asks.iter().any(|a| a.secret);
        self.asks = self.asks.into_iter().filter_map(AgentQuestionAsk::sanitized).collect();
        self.asks.truncate(agent::MAX_QUESTION_ASKS);
        if self.asks.is_empty() && !self.secret {
            return None;
        }
        Some(self)
    }

    /// The question as a phone gets it: a secret one without its asks.
    pub fn for_phone(mut self) -> Self {
        if self.secret {
            self.asks = Vec::new();
        }
        self
    }
}

/// Body of `agent.questioned`: PC to phone, a question ended.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentQuestioned {
    pub token: String,
    pub thread: String,
    /// One of [`agent::questioned`].
    pub outcome: String,
    /// The device that answered or dismissed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_name: Option<String>,
}

impl std::fmt::Debug for AgentQuestioned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentQuestioned")
            .field("thread", &self.thread)
            .field("outcome", &self.outcome)
            .finish_non_exhaustive()
    }
}

impl AgentQuestioned {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.token)
            && is_valid_agent_id(&self.thread)
            && agent::questioned::ALL.contains(&self.outcome.as_str())
            && fits_opt(&self.by_name, agent::MAX_LABEL_BYTES)
    }

    pub fn sanitized(mut self) -> Option<Self> {
        if !is_valid_agent_id(&self.token)
            || !is_valid_agent_id(&self.thread)
            || !agent::questioned::ALL.contains(&self.outcome.as_str())
        {
            return None;
        }
        self.by_name = line_or_none(self.by_name, agent::MAX_LABEL_BYTES);
        Some(self)
    }
}

/// The answer to one ask: the options picked (by their labels) and/or the
/// user's own words.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentQuestionAnswer {
    /// The ask's id.
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other: Option<String>,
}

/// Never prints the answer.
impl std::fmt::Debug for AgentQuestionAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentQuestionAnswer")
            .field("labels", &self.labels.len())
            .field("other", &self.other.is_some())
            .finish_non_exhaustive()
    }
}

impl AgentQuestionAnswer {
    fn is_valid(&self) -> bool {
        is_whole_line(&self.id, agent::MAX_ASK_ID_BYTES)
            && self.labels.len() <= agent::MAX_OPTIONS
            && self.labels.iter().all(|l| is_whole_line(l, agent::MAX_OPTION_LABEL_BYTES))
            && self
                .other
                .as_deref()
                .is_none_or(|o| !o.trim().is_empty() && is_clean_text(o, agent::MAX_OTHER_BYTES))
            && (!self.labels.is_empty() || self.other.is_some())
    }
}

/// Body of `agent.reply`: phone to PC, the answers to a question (one per
/// ask), or `dismiss`. A secret question is answered on the PC only.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentQuestionReply {
    pub token: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<AgentQuestionAnswer>,
    /// The user won't answer: the agent is told so.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub dismiss: bool,
}

/// Never prints the token or the answers.
impl std::fmt::Debug for AgentQuestionReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentQuestionReply")
            .field("answers", &self.answers)
            .field("dismiss", &self.dismiss)
            .finish_non_exhaustive()
    }
}

impl AgentQuestionReply {
    /// A token, and either `dismiss` or 1 to 4 answers to distinct asks.
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.token)
            && if self.dismiss {
                self.answers.is_empty()
            } else {
                (1..=agent::MAX_QUESTION_ASKS).contains(&self.answers.len())
                    && self.answers.iter().all(AgentQuestionAnswer::is_valid)
                    && self
                        .answers
                        .iter()
                        .enumerate()
                        .all(|(i, a)| !self.answers[..i].iter().any(|b| b.id == a.id))
            }
    }
}

// ---- Files and slash commands ----

/// Body of `agent.files`: phone to PC, files of a folder for `@`. Exactly
/// one of `thread` (its folder) and `folder`.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentFilesRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// What follows the `@`; empty: the folder's top level and recent files.
    #[serde(default)]
    pub query: String,
    /// Default [`agent::DEFAULT_FILE_HITS`], at most [`agent::MAX_FILE_HITS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Never prints the query.
impl std::fmt::Debug for AgentFilesRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentFilesRequest")
            .field("thread", &self.thread)
            .field("folder", &self.folder)
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

impl AgentFilesRequest {
    pub fn is_valid(&self) -> bool {
        let one = match (&self.thread, &self.folder) {
            (Some(id), None) | (None, Some(id)) => is_valid_agent_id(id),
            _ => false,
        };
        one && self.query.len() <= agent::MAX_FILE_QUERY_BYTES
            && !self.query.chars().any(|c| c.is_control() || is_bidi_control(c))
            && self.limit.is_none_or(|l| (1..=agent::MAX_FILE_HITS as u32).contains(&l))
    }

    /// How many hits to ask for.
    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(agent::DEFAULT_FILE_HITS).clamp(1, agent::MAX_FILE_HITS as u32)
    }
}

/// A file or directory of a folder, by its path relative to the folder.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentFileHit {
    /// Relative to the folder, `/` between components.
    pub path: String,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub dir: bool,
}

/// Never prints the path.
impl std::fmt::Debug for AgentFileHit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentFileHit").field("dir", &self.dir).finish_non_exhaustive()
    }
}

/// Whether `path` is relative and stays inside its folder: no leading `/`,
/// `\` or `~`, no drive letter, no `..`, `.` or empty component (a
/// directory may end with `/`), no control or bidi characters.
pub fn is_relative_path(path: &str) -> bool {
    let trimmed = path.strip_suffix('/').unwrap_or(path);
    let bytes = trimmed.as_bytes();
    (1..=agent::MAX_FILE_PATH_BYTES).contains(&path.len())
        && !trimmed.is_empty()
        && !trimmed.starts_with(['/', '\\', '~'])
        && !(bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        && !trimmed.contains('\\')
        && trimmed.split('/').all(|part| !part.is_empty() && part != ".." && part != ".")
        && !path.chars().any(|c| c.is_control() || is_bidi_control(c))
}

impl AgentFileHit {
    pub fn is_valid(&self) -> bool {
        is_relative_path(&self.path)
    }
}

/// Body of the `agent.files` reply.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentFiles {
    #[serde(default)]
    pub files: Vec<AgentFileHit>,
    /// There are more than these (or the folder was too large to list whole).
    #[serde(default)]
    pub more: bool,
}

impl AgentFiles {
    pub fn is_valid(&self) -> bool {
        self.files.len() <= agent::MAX_FILE_HITS && self.files.iter().all(AgentFileHit::is_valid)
    }

    /// The hits that stay inside the folder, at most [`agent::MAX_FILE_HITS`]
    /// (`more` set when some were left out for that). A hit is never cut:
    /// one that isn't a relative path is left out.
    pub fn sanitized(mut self) -> Self {
        self.files.retain(AgentFileHit::is_valid);
        if self.files.len() > agent::MAX_FILE_HITS {
            self.files.truncate(agent::MAX_FILE_HITS);
            self.more = true;
        }
        self
    }
}

/// Body of `agent.commands`: phone to PC, the slash commands an agent offers in a folder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentCommandsRequest {
    pub agent: String,
    pub folder: String,
}

impl AgentCommandsRequest {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.agent) && is_valid_agent_id(&self.folder)
    }
}

/// A slash command or skill an agent offers.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentCommand {
    /// Without the `/`; one word.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Its arguments, shown after it as placeholder text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// One of [`agent::command_kind`].
    pub kind: String,
}

impl std::fmt::Debug for AgentCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentCommand").field("kind", &self.kind).finish_non_exhaustive()
    }
}

fn is_command_name(name: &str) -> bool {
    is_whole_line(name, agent::MAX_COMMAND_NAME_BYTES)
        && !name.starts_with('/')
        && !name.chars().any(char::is_whitespace)
}

impl AgentCommand {
    pub fn is_valid(&self) -> bool {
        is_command_name(&self.name)
            && fits_opt(&self.note, agent::MAX_COMMAND_NOTE_BYTES)
            && fits_opt(&self.hint, agent::MAX_COMMAND_HINT_BYTES)
            && !self.kind.is_empty()
            && self.kind.len() <= agent::MAX_WORD_BYTES
    }

    /// The command within its limits (a leading `/` dropped, a kind this
    /// build doesn't know as `command`); `None` when its name isn't one word.
    pub fn sanitized(mut self) -> Option<Self> {
        if let Some(name) = self.name.strip_prefix('/') {
            self.name = name.to_owned();
        }
        if !is_command_name(&self.name) {
            return None;
        }
        self.note = line_or_none(self.note, agent::MAX_COMMAND_NOTE_BYTES);
        self.hint = line_or_none(self.hint, agent::MAX_COMMAND_HINT_BYTES);
        if !agent::command_kind::ALL.contains(&self.kind.as_str()) {
            self.kind = agent::command_kind::COMMAND.to_owned();
        }
        Some(self)
    }
}

/// Body of the `agent.commands` reply.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentCommands {
    #[serde(default)]
    pub commands: Vec<AgentCommand>,
}

impl AgentCommands {
    pub fn is_valid(&self) -> bool {
        self.commands.len() <= agent::MAX_COMMANDS && self.commands.iter().all(AgentCommand::is_valid)
    }

    pub fn sanitized(mut self) -> Self {
        self.commands = self.commands.into_iter().filter_map(AgentCommand::sanitized).collect();
        self.commands.truncate(agent::MAX_COMMANDS);
        self
    }
}

// ---- Drafts ----

/// Whether `key` names a draft: a thread id, or `new:` and a folder id.
pub fn is_draft_key(key: &str) -> bool {
    key.len() <= agent::MAX_DRAFT_KEY_BYTES
        && match key.strip_prefix(agent::DRAFT_NEW_PREFIX) {
            Some(folder) => is_valid_agent_id(folder),
            None => is_valid_agent_id(key),
        }
}

/// A message being written, kept on the PC so any device can carry on with it.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentDraft {
    /// A thread id, or `new:` and a folder id.
    pub key: String,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<AgentImage>,
    /// Grows with every change.
    pub rev: u64,
    /// The device that wrote it last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_name: Option<String>,
}

/// Never prints the text.
impl std::fmt::Debug for AgentDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentDraft")
            .field("rev", &self.rev)
            .field("bytes", &self.text.len())
            .field("images", &self.images.len())
            .finish_non_exhaustive()
    }
}

impl AgentDraft {
    pub fn is_valid(&self) -> bool {
        is_draft_key(&self.key)
            && self.text.len() <= agent::MAX_DRAFT_TEXT_BYTES
            && self.images.len() <= agent::MAX_IMAGES
            && self.images.iter().all(AgentImage::is_valid)
            && fits_opt(&self.by_name, agent::MAX_LABEL_BYTES)
    }

    pub fn sanitized(mut self) -> Option<Self> {
        if !is_draft_key(&self.key) {
            return None;
        }
        self.text = clean_agent_text(&self.text, true, agent::MAX_DRAFT_TEXT_BYTES).0;
        self.images = self.images.into_iter().filter_map(AgentImage::sanitized).collect();
        self.images.truncate(agent::MAX_IMAGES);
        self.by_name = line_or_none(self.by_name, agent::MAX_LABEL_BYTES);
        Some(self)
    }
}

/// Body of `agent.draft.get`: phone to PC.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentDraftRequest {
    pub key: String,
}

impl AgentDraftRequest {
    pub fn is_valid(&self) -> bool {
        is_draft_key(&self.key)
    }
}

/// Body of the `agent.draft.get` reply: the draft, if there is one.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentDraftReply {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<AgentDraft>,
}

/// Body of `agent.draft.set`: phone to PC. `rev` is the draft's `rev` as
/// this phone last saw it (0 for a new one).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentDraftSet {
    pub key: String,
    #[serde(default)]
    pub text: String,
    /// Image ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    pub rev: u64,
}

/// Never prints the text.
impl std::fmt::Debug for AgentDraftSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentDraftSet")
            .field("rev", &self.rev)
            .field("bytes", &self.text.len())
            .field("images", &self.images.len())
            .finish_non_exhaustive()
    }
}

impl AgentDraftSet {
    pub fn is_valid(&self) -> bool {
        is_draft_key(&self.key)
            && self.text.len() <= agent::MAX_DRAFT_TEXT_BYTES
            && is_valid_image_ids(&self.images)
    }
}

/// Body of the `agent.draft.set` reply: the draft's new `rev`; or, when
/// another device changed it since `rev`, nothing stored and the draft as
/// it is (`conflict`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentDraftSaved {
    pub rev: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<AgentDraft>,
}

impl AgentDraftSaved {
    pub fn is_valid(&self) -> bool {
        self.conflict.as_ref().is_none_or(AgentDraft::is_valid)
    }

    pub fn sanitized(mut self) -> Self {
        self.conflict = self.conflict.and_then(AgentDraft::sanitized);
        self
    }
}

/// Body of `agent.draft`: PC to phone, a draft changed on another device
/// (read it with `agent.draft.get`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentDraftChanged {
    pub key: String,
    pub rev: u64,
}

impl AgentDraftChanged {
    pub fn is_valid(&self) -> bool {
        is_draft_key(&self.key)
    }
}

// ---- Images ----

/// Body of `agent.image.meta`, first on an `agent/image` stream: what
/// follows (§5.3).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentImageMeta {
    /// The picture's file name on the phone (the PC keeps its last component).
    pub name: String,
    /// The bytes that follow, 1 to [`agent::MAX_IMAGE_BYTES`].
    pub bytes: u64,
}

impl std::fmt::Debug for AgentImageMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentImageMeta").field("bytes", &self.bytes).finish_non_exhaustive()
    }
}

impl AgentImageMeta {
    pub fn is_valid(&self) -> bool {
        self.name.len() <= agent::MAX_IMAGE_NAME_BYTES
            && (1..=agent::MAX_IMAGE_BYTES as u64).contains(&self.bytes)
    }
}

/// Body of `agent.image.thumb`, first on an `agent/thumb` stream.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentImageThumb {
    pub id: String,
    /// Its longest side, [`agent::MIN_THUMB_PX`] to [`agent::MAX_THUMB_PX`];
    /// default [`agent::THUMB_DEFAULT_PX`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
}

impl AgentImageThumb {
    pub fn is_valid(&self) -> bool {
        is_agent_image_id(&self.id)
            && self.max.is_none_or(|m| (agent::MIN_THUMB_PX..=agent::MAX_THUMB_PX).contains(&m))
    }

    pub fn max(&self) -> u32 {
        self.max.unwrap_or(agent::THUMB_DEFAULT_PX).clamp(agent::MIN_THUMB_PX, agent::MAX_THUMB_PX)
    }
}

/// Body of `agent.image.thumb.meta`: how many bytes of JPEG follow.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentImageThumbMeta {
    pub length: u64,
}

impl AgentImageThumbMeta {
    pub fn is_valid(&self) -> bool {
        (1..=agent::MAX_THUMB_BYTES as u64).contains(&self.length)
    }
}

#[cfg(test)]
mod tests {
    use serde::de::DeserializeOwned;

    use super::*;
    use crate::{
        Envelope,
        messages::{
            AgentCatalog, AgentCatalogAgent, AgentItem, AgentSend, AgentStart, AgentThread, AgentThreads,
            AgentTool, types,
        },
    };

    const THREAD: &str = "0123456789abcdef0123456789abcdef";
    const IMAGE: &str = "aaaabbbbccccddddeeeeffff00001111";
    const QUEUED: &str = "0123456789abcdef";

    fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(t: &str, body: &T) {
        let env = Envelope::new(t, body).unwrap();
        let back = Envelope::from_cbor(&env.to_cbor()).unwrap();
        assert_eq!(&back.body::<T>().unwrap(), body);
    }

    /// The keys of `body` as it goes on the wire.
    fn keys<T: Serialize>(body: &T) -> Vec<String> {
        let mut bytes = Vec::new();
        ciborium::into_writer(body, &mut bytes).unwrap();
        let value: ciborium::Value = ciborium::from_reader(bytes.as_slice()).unwrap();
        value.as_map().unwrap().iter().map(|(k, _)| k.as_text().unwrap().to_owned()).collect()
    }

    fn image(name: &str) -> AgentImage {
        AgentImage { id: IMAGE.into(), name: name.into(), bytes: 120_000, w: 1024, h: 768 }
    }

    fn model(id: &str) -> AgentModel {
        AgentModel {
            id: id.into(),
            name: "Opus".into(),
            note: Some("The most capable model.".into()),
            is_default: false,
            efforts: vec!["low".into(), "high".into(), "xhigh".into()],
            default_effort: Some("high".into()),
            images: true,
            short: true,
        }
    }

    fn queued(text: &str) -> AgentQueued {
        AgentQueued {
            id: QUEUED.into(),
            text: text.into(),
            truncated: false,
            by_name: Some("Pixel".into()),
            images: vec![image("shot.png")],
            created: 1_795_000_000_000,
        }
    }

    fn full_thread() -> AgentThread {
        AgentThread {
            id: THREAD.into(),
            agent: "claude".into(),
            folder: "g7linux".into(),
            title: "Plan the login fix".into(),
            state: agent::state::WORKING.into(),
            mode: agent::mode::PLAN.into(),
            updated: 1_795_000_000_000,
            model: Some("claude-opus-4-5".into()),
            effort: Some("high".into()),
            answered_model: Some("claude-sonnet-4-5".into()),
            queued: vec![queued("Then run the tests.")],
            queue_held: true,
            context: Some(AgentContext { used: 120_000, max: 1_000_000 }),
            mcp: Some(AgentMcp { on: true, servers: 3, connected: 2, need_login: 1 }),
            asking: 1,
            ..Default::default()
        }
    }

    fn question(secret: bool) -> AgentQuestion {
        AgentQuestion {
            token: "ffff0000ffff0000ffff0000ffff0000".into(),
            thread: THREAD.into(),
            thread_title: "Plan the login fix".into(),
            agent: "codex".into(),
            item: "q:call_1".into(),
            secret,
            asks: vec![AgentQuestionAsk {
                id: "0".into(),
                header: "Auth method".into(),
                text: "Which sign-in should the app use?".into(),
                multi: false,
                other: true,
                secret,
                options: vec![
                    AgentQuestionOption { label: "OAuth".into(), note: Some("Recommended".into()) },
                    AgentQuestionOption { label: "Password".into(), note: None },
                ],
            }],
            created: 1_795_000_000_000,
            expires: 1_795_007_200_000,
        }
    }

    #[test]
    fn control_bodies_round_trip() {
        let catalog = AgentCatalog {
            agents: vec![AgentCatalogAgent {
                id: "claude".into(),
                name: "Claude".into(),
                installed: true,
                signed_in: true,
                login_hint: "claude auth login".into(),
                models: vec![model("opus"), AgentModel { is_default: true, ..model("default") }],
                can: agent::can::ALL.iter().map(|c| (*c).to_owned()).collect(),
                limits: vec![AgentLimit {
                    window: "five_hour".into(),
                    used_pct: Some(41),
                    resets: Some(1_795_010_000_000),
                    state: agent::limit_state::OK.into(),
                }],
                ..Default::default()
            }],
            folders: Vec::new(),
            modes: vec![agent::mode::SUPERVISED.into(), agent::mode::PLAN.into()],
        };
        assert!(catalog.is_valid());
        assert_eq!(catalog.clone().sanitized(), catalog, "nothing to clean");
        round_trip(types::AGENT_CATALOG, &catalog);
        assert!(keys(&catalog.agents[0].models[1]).contains(&"default".to_owned()));

        let thread = full_thread();
        assert!(thread.is_valid());
        assert_eq!(thread.clone().sanitized(), Some(thread.clone()));
        round_trip(types::AGENT_THREAD, &thread);

        let mut user = AgentItem {
            seq: 4,
            id: "u:4".into(),
            kind: agent::item::USER.into(),
            text: Some("Also check the logout.".into()),
            via: Some(agent::via::STEER.into()),
            images: vec![image("error.png")],
            ..Default::default()
        };
        let plan = AgentItem {
            seq: 5,
            id: "p:5".into(),
            kind: agent::item::PLAN.into(),
            text: Some("1. Read the code\n2. Fix it".into()),
            plan_state: Some(agent::plan_state::PROPOSED.into()),
            ..Default::default()
        };
        for item in [&user, &plan] {
            assert!(item.is_valid());
            assert_eq!(item.clone().sanitized().as_ref(), Some(item));
            round_trip(types::AGENT_EVENT, item);
        }
        user.via = Some(agent::via::QUEUE.into());
        round_trip(types::AGENT_EVENT, &user);

        let start = AgentStart {
            agent: "claude".into(),
            folder: "f0123456789ab".into(),
            prompt: "Plan the login fix".into(),
            mode: agent::mode::PLAN.into(),
            model: Some("my-own-model".into()),
            effort: Some("high".into()),
            images: vec![IMAGE.into()],
            custom_model: true,
            key: None,
        };
        assert!(start.is_valid() && start.uses_control());
        round_trip(types::AGENT_START, &start);
        let send = AgentSend {
            thread: THREAD.into(),
            text: String::new(),
            dispatch: Some(agent::dispatch::STEER.into()),
            images: vec![IMAGE.into()],
            key: None,
        };
        assert!(send.is_valid() && send.uses_control(), "an image needs no words");
        round_trip(types::AGENT_SEND, &send);

        let configure = AgentConfigure {
            thread: THREAD.into(),
            model: Some(String::new()),
            effort: Some("low".into()),
            mode: Some(agent::mode::PLAN.into()),
            custom_model: false,
        };
        assert!(configure.is_valid());
        round_trip(types::AGENT_CONFIGURE, &configure);
        let edit = AgentQueueChange {
            thread: THREAD.into(),
            id: QUEUED.into(),
            op: agent::queue_op::EDIT.into(),
            text: Some("Then run all the tests.".into()),
        };
        assert!(edit.is_valid());
        round_trip(types::AGENT_QUEUE, &edit);
        let implement = AgentImplement { thread: THREAD.into(), item: "p:5".into() };
        assert!(implement.is_valid());
        round_trip(types::AGENT_IMPLEMENT, &implement);

        let question = question(false);
        assert!(question.is_valid());
        assert_eq!(question.clone().sanitized(), Some(question.clone()));
        round_trip(types::AGENT_QUESTION, &question);
        let questioned = AgentQuestioned {
            token: question.token.clone(),
            thread: THREAD.into(),
            outcome: agent::questioned::ANSWERED.into(),
            by_name: Some("Pixel".into()),
        };
        assert!(questioned.is_valid());
        round_trip(types::AGENT_QUESTIONED, &questioned);
        let reply = AgentQuestionReply {
            token: question.token.clone(),
            answers: vec![AgentQuestionAnswer {
                id: "0".into(),
                labels: vec!["OAuth".into()],
                other: Some("With the device code flow".into()),
            }],
            dismiss: false,
        };
        assert!(reply.is_valid());
        round_trip(types::AGENT_REPLY, &reply);
        let dismiss =
            AgentQuestionReply { token: question.token.clone(), answers: Vec::new(), dismiss: true };
        assert!(dismiss.is_valid());
        round_trip(types::AGENT_REPLY, &dismiss);

        let files = AgentFilesRequest {
            thread: Some(THREAD.into()),
            folder: None,
            query: "src/ma".into(),
            limit: Some(8),
        };
        assert!(files.is_valid() && files.limit() == 8);
        round_trip(types::AGENT_FILES, &files);
        let hits = AgentFiles {
            files: vec![
                AgentFileHit { path: "src/main.rs".into(), dir: false },
                AgentFileHit { path: "src/".into(), dir: true },
            ],
            more: true,
        };
        assert!(hits.is_valid());
        assert_eq!(hits.clone().sanitized(), hits);
        round_trip(types::AGENT_FILES, &hits);

        let commands = AgentCommands {
            commands: vec![AgentCommand {
                name: "review".into(),
                note: Some("Review a pull request".into()),
                hint: Some("[pr]".into()),
                kind: agent::command_kind::COMMAND.into(),
            }],
        };
        assert!(commands.is_valid());
        assert_eq!(commands.clone().sanitized(), commands);
        round_trip(types::AGENT_COMMANDS, &commands);
        round_trip(
            types::AGENT_COMMANDS,
            &AgentCommandsRequest { agent: "codex".into(), folder: "f0123456789ab".into() },
        );

        let draft = AgentDraft {
            key: format!("{}f0123456789ab", agent::DRAFT_NEW_PREFIX),
            text: "Half a thought".into(),
            images: vec![image("a.jpg")],
            rev: 3,
            by_name: Some("guchu".into()),
        };
        assert!(draft.is_valid());
        round_trip(types::AGENT_DRAFT_GET, &AgentDraftRequest { key: draft.key.clone() });
        round_trip(types::AGENT_DRAFT_GET, &AgentDraftReply { draft: Some(draft.clone()) });
        let set =
            AgentDraftSet { key: THREAD.into(), text: "Half".into(), images: vec![IMAGE.into()], rev: 3 };
        assert!(set.is_valid());
        round_trip(types::AGENT_DRAFT_SET, &set);
        round_trip(types::AGENT_DRAFT_SET, &AgentDraftSaved { rev: 4, conflict: Some(draft.clone()) });
        round_trip(types::AGENT_DRAFT, &AgentDraftChanged { key: THREAD.into(), rev: 4 });

        let meta = AgentImageMeta { name: "PXL_0001.jpg".into(), bytes: 3_000_000 };
        assert!(meta.is_valid());
        round_trip(types::AGENT_IMAGE_META, &meta);
        round_trip(types::AGENT_IMAGE, &image("PXL_0001.jpg"));
        let thumb = AgentImageThumb { id: IMAGE.into(), max: None };
        assert!(thumb.is_valid() && thumb.max() == agent::THUMB_DEFAULT_PX);
        round_trip(types::AGENT_IMAGE_THUMB, &thumb);
        round_trip(types::AGENT_IMAGE_THUMB_META, &AgentImageThumbMeta { length: 9_000 });
    }

    #[test]
    fn a_model_is_one_argument_and_never_a_flag() {
        for good in ["opus", "claude-opus-4-5", "gpt-5.1-codex", "default", "o3[1m]"] {
            assert!(crate::messages::is_model_id(good), "{good}");
        }
        for bad in ["", "-p", "--dangerously-skip-permissions", "opus 4", "a\tb", "a\nb", "x\u{202E}"] {
            assert!(!crate::messages::is_model_id(bad), "{bad:?}");
        }
        assert!(!crate::messages::is_model_id(&"m".repeat(agent::MAX_MODEL_ID_BYTES + 1)));
        for bad in ["High", "x-high", ""] {
            assert!(!crate::messages::is_effort(bad), "{bad}");
        }
        assert!(crate::messages::is_effort("sixteen_letters_"));
        assert!(!crate::messages::is_effort("seventeen_letters"));

        let start = |model: &str| AgentStart {
            agent: "claude".into(),
            folder: "f1".into(),
            prompt: "hi".into(),
            mode: agent::mode::SUPERVISED.into(),
            model: Some(model.into()),
            ..Default::default()
        };
        assert!(start("opus").is_valid());
        assert!(!start("--model=x").is_valid());
        assert!(!start("two words").is_valid());
        let typed_without_model = AgentStart { custom_model: true, model: None, ..start("x") };
        assert!(!typed_without_model.is_valid());
        let configure = |model: Option<&str>, effort: Option<&str>| AgentConfigure {
            thread: THREAD.into(),
            model: model.map(Into::into),
            effort: effort.map(Into::into),
            ..Default::default()
        };
        assert!(!configure(None, None).is_valid(), "nothing to change");
        assert!(configure(Some(""), None).is_valid(), "back to the default");
        assert!(!configure(Some("-x"), None).is_valid());
        assert!(!configure(None, Some("HIGH")).is_valid());
        assert!(!AgentConfigure { custom_model: true, ..configure(Some(""), None) }.is_valid());
    }

    #[test]
    fn the_catalog_keeps_its_limits() {
        let mut agent = AgentCatalogAgent {
            id: "claude".into(),
            name: "Claude".into(),
            models: (0..45).map(|i| model(&format!("m{i}"))).collect(),
            can: vec!["plan".into(), "teleport".into(), "plan".into(), "steer".into()],
            limits: vec![
                AgentLimit {
                    window: "five_hour".into(),
                    used_pct: Some(150),
                    resets: None,
                    state: "rejected".into(),
                },
                AgentLimit { window: " \u{1b}".into(), used_pct: None, resets: None, state: "ok".into() },
            ],
            ..Default::default()
        };
        agent.models[0].note = Some(format!("{}\u{202E}", "n".repeat(200)));
        agent.models[1].id = "--bare".into();
        agent.models[2].efforts = vec!["low".into(), "Max!".into()];
        agent.models[3].name = "\u{1b}".into();
        let catalog = AgentCatalog { agents: vec![agent], ..Default::default() };
        assert!(!catalog.is_valid());
        let agent = catalog.sanitized().agents.remove(0);
        assert_eq!(agent.models.len(), agent::MAX_MODELS);
        assert_eq!(agent.models[0].note.as_ref().unwrap().len(), agent::MAX_MODEL_NOTE_BYTES);
        assert!(agent.models.iter().all(|m| m.id != "--bare"), "a flag isn't a model");
        assert_eq!(agent.models[1].efforts, ["low"]);
        assert_eq!(agent.models[2].name, "m3", "a model without a name shows its id");
        assert_eq!(agent.can, ["plan", "steer"], "known words, once each");
        assert_eq!(agent.limits.len(), 1, "a window needs a name");
        assert_eq!(agent.limits[0].used_pct, Some(100));
        assert_eq!(agent.limits[0].state, agent::limit_state::OK, "a word this build doesn't know is ok");
        assert!(agent.can(agent::can::STEER) && !agent.can(agent::can::IMAGES));
    }

    #[test]
    fn a_thread_list_keeps_its_queued_text_whole_and_comes_in_parts() {
        use crate::lanes::{Paged, RPC_PART_TARGET};
        let long = "q".repeat(agent::MAX_QUEUED_TEXT_BYTES);
        let mut thread = full_thread();
        thread.queued = (0..agent::MAX_QUEUED)
            .map(|i| AgentQueued { id: format!("{i:016x}"), ..queued(&long) })
            .collect();
        assert!(thread.is_valid());
        let threads: Vec<AgentThread> = (0..agent::MAX_THREADS)
            .map(|i| AgentThread { id: format!("{i:032x}"), ..thread.clone() })
            .collect();
        let list = AgentThreads { threads }.sanitized();
        assert!(list.is_valid());
        assert!(
            list.threads.iter().flat_map(|t| &t.queued).all(|q| !q.truncated && q.text.len() == long.len())
        );
        // More than a frame holds: it goes in parts that each fit one.
        assert!(Envelope::new(types::OK, &list).unwrap().to_cbor().len() > crate::MAX_FRAME_LEN);
        let parts = list.clone().split(RPC_PART_TARGET);
        assert!(parts.len() > 1);
        for part in &parts {
            assert!(part.is_valid());
            assert!(Envelope::new(types::PART, part).unwrap().to_cbor().len() <= RPC_PART_TARGET);
        }
        let mut parts = parts.into_iter();
        let mut whole = parts.next().unwrap();
        parts.for_each(|part| whole.append(part));
        assert_eq!(whole, list);
        // One thread's page or event carries its queue whole.
        assert!(thread.clone().sanitized().unwrap().queued.iter().all(|q| !q.truncated));
        let mut six = thread.clone();
        six.queued.push(AgentQueued { id: "00000000000000ff".into(), ..queued("x") });
        assert!(!six.is_valid());
        assert_eq!(six.sanitized().unwrap().queued.len(), agent::MAX_QUEUED);
    }

    #[test]
    fn words_a_build_doesnt_know_read_as_absent() {
        let mut user = AgentItem {
            seq: 1,
            id: "u:1".into(),
            kind: agent::item::USER.into(),
            text: Some("hi".into()),
            via: Some("teleport".into()),
            images: vec![image("/home/me/Pictures/shot.png"), AgentImage { id: "nope".into(), ..image("x") }],
            plan_state: Some(agent::plan_state::PROPOSED.into()),
            ..Default::default()
        };
        assert!(!user.is_valid(), "an image id that isn't one");
        let clean = user.clone().sanitized().unwrap();
        assert_eq!(clean.via, None);
        assert_eq!(clean.plan_state, None, "a plan state goes with a plan");
        assert_eq!(clean.images.len(), 1, "a bad id is left out");
        assert_eq!(clean.images[0].name, "shot.png", "a name, never a path");
        user.kind = agent::item::ASSISTANT.into();
        user.via = Some(agent::via::STEER.into());
        let clean = user.sanitized().unwrap();
        assert_eq!((clean.via, clean.images.len()), (None, 0), "via and images go with a user item");
        let asked = AgentItem {
            seq: 3,
            id: "q:3".into(),
            kind: agent::item::QUESTION.into(),
            question: Some(AgentItemQuestion {
                count: 9,
                secret: true,
                outcome: Some("forgotten".into()),
                by_name: Some("Pixel\u{1b}".into()),
            }),
            ..Default::default()
        };
        assert!(asked.is_valid());
        let question = asked.clone().sanitized().unwrap().question.unwrap();
        assert_eq!((question.count, question.outcome, question.by_name.as_deref()), (4, None, Some("Pixel")));
        let elsewhere = AgentItem { kind: agent::item::ASSISTANT.into(), ..asked };
        assert_eq!(elsewhere.sanitized().unwrap().question, None, "question state goes with a question");
        let plan = AgentItem {
            seq: 2,
            id: "p:2".into(),
            kind: agent::item::PLAN.into(),
            plan_state: Some("superseded".into()),
            ..Default::default()
        };
        assert!(plan.is_valid());
        assert_eq!(plan.sanitized().unwrap().plan_state, None);
        let long_word =
            AgentItem { via: Some("v".repeat(agent::MAX_WORD_BYTES + 1)), ..AgentItem::default() };
        assert!(!AgentItem { seq: 1, id: "a".into(), kind: "user".into(), ..long_word }.is_valid());
    }

    #[test]
    fn a_phone_without_control_gets_what_it_knows() {
        let legacy = full_thread().for_legacy();
        assert_eq!(legacy.mode, agent::mode::SUPERVISED, "plan mode reads as supervised");
        let shown = keys(&legacy);
        for added in ["model", "effort", "answered_model", "queued", "queue_held", "context", "mcp", "asking"]
        {
            assert!(!shown.contains(&added.to_owned()), "{added} in {shown:?}");
        }
        let auto = AgentThread { mode: agent::mode::AUTO_EDIT.into(), ..full_thread() }.for_legacy();
        assert_eq!(auto.mode, agent::mode::AUTO_EDIT);

        let catalog = AgentCatalog {
            agents: vec![AgentCatalogAgent {
                id: "claude".into(),
                models: vec![model("opus")],
                can: vec!["plan".into()],
                ..Default::default()
            }],
            folders: Vec::new(),
            modes: vec![agent::mode::SUPERVISED.into(), agent::mode::PLAN.into()],
        }
        .for_legacy();
        assert_eq!(catalog.modes, ["supervised"]);
        assert_eq!(keys(&catalog.agents[0]), ["id", "name", "installed", "signed_in", "login_hint"]);

        let plan = AgentItem {
            seq: 5,
            id: "p:5".into(),
            kind: agent::item::PLAN.into(),
            text: Some("1. Fix it".into()),
            plan_state: Some(agent::plan_state::PROPOSED.into()),
            ..Default::default()
        };
        let old = plan.clone().for_phone(true, false);
        assert_eq!(old.kind, agent::item::ASSISTANT);
        assert_eq!((old.text.as_deref(), old.plan_state), (Some("1. Fix it"), None));
        assert_eq!(plan.clone().for_phone(true, true), plan, "a phone with control gets it as it is");

        let mut asked = AgentItem {
            seq: 6,
            id: "q:6".into(),
            kind: agent::item::QUESTION.into(),
            text: Some("Auth method: OAuth".into()),
            ..Default::default()
        };
        let old = asked.clone().for_phone(true, false);
        let tool = old.tool.unwrap();
        assert_eq!((old.kind.as_str(), old.text), (agent::item::TOOL, None));
        assert_eq!((tool.name.as_str(), tool.title.as_str()), ("question", "Question"));
        assert_eq!(tool.status, agent::tool::RUNNING);
        asked.end = Some(1_795_000_000_100);
        assert_eq!(asked.clone().for_phone(false, false).tool.unwrap().status, agent::tool::OK);
        assert_eq!(asked.clone().for_phone(true, false).seq, 6, "its seq stays");
        asked.end = None;
        asked.question = Some(AgentItemQuestion {
            count: 2,
            secret: false,
            outcome: Some(agent::questioned::ANSWERED.into()),
            by_name: Some("Pixel".into()),
        });
        assert_eq!(asked.clone().for_phone(true, true), asked, "a phone with control gets it as it is");
        let old = asked.for_phone(true, false);
        assert_eq!((old.tool.unwrap().status.as_str(), old.question), (agent::tool::OK, None), "ended");

        let steered = AgentItem {
            seq: 7,
            id: "u:7".into(),
            kind: agent::item::USER.into(),
            text: Some("and the logout".into()),
            via: Some(agent::via::STEER.into()),
            images: vec![image("a.png")],
            ..Default::default()
        };
        let old = steered.for_phone(true, false);
        assert_eq!(keys(&old), ["seq", "id", "kind", "when", "text"]);
    }

    #[test]
    fn the_plan_file_never_reaches_a_phone() {
        let write = AgentItem {
            seq: 3,
            id: "toolu_01Write".into(),
            kind: agent::item::TOOL.into(),
            tool: Some(AgentTool {
                name: "Write".into(),
                title: "Write /home/me/.claude/plans/fix-login.md".into(),
                detail: Some("# The plan\nsecret steps".into()),
                status: agent::tool::OK.into(),
                output: Some("written".into()),
                class: Some(agent::tool_class::PLAN.into()),
                path: Some("fix-login.md".into()),
                by_rule: false,
            }),
            ..Default::default()
        };
        for (items2, control) in [(true, true), (true, false), (false, false)] {
            let sent = write.clone().for_phone(items2, control);
            let tool = sent.tool.unwrap();
            assert_eq!(sent.seq, 3);
            assert_eq!((tool.name.as_str(), tool.title.as_str()), ("plan", "Plan"));
            assert_eq!((tool.detail, tool.output, tool.path), (None, None, None));
            assert_eq!(tool.class.as_deref(), control.then_some(agent::tool_class::PLAN));
            assert_eq!(tool.status, agent::tool::OK);
        }
    }

    #[test]
    fn an_older_decoder_ignores_what_it_doesnt_know() {
        // The fields of a 0.1.16 phone's thread and item, as it decodes them.
        #[derive(Debug, serde::Deserialize)]
        #[allow(dead_code)]
        struct Phase1Thread {
            id: String,
            agent: String,
            folder: String,
            title: String,
            state: String,
            mode: String,
            #[serde(default)]
            from_phone: bool,
            updated: u64,
            #[serde(default)]
            asks: u32,
            #[serde(default)]
            plan: Option<Vec<serde::de::IgnoredAny>>,
        }
        #[derive(Debug, serde::Deserialize)]
        #[allow(dead_code)]
        struct Phase1Item {
            seq: u64,
            id: String,
            kind: String,
            when: u64,
            #[serde(default)]
            text: Option<String>,
        }
        let env = Envelope::new(types::AGENT_THREAD, &full_thread()).unwrap();
        let old: Phase1Thread = Envelope::from_cbor(&env.to_cbor()).unwrap().body().unwrap();
        assert_eq!(old.mode, "plan", "which is why a PC sends such a phone the legacy form");
        let item = AgentItem {
            seq: 1,
            id: "u:1".into(),
            kind: agent::item::USER.into(),
            via: Some(agent::via::QUEUE.into()),
            images: vec![image("a.png")],
            ..Default::default()
        };
        let env = Envelope::new(types::AGENT_EVENT, &item).unwrap();
        let old: Phase1Item = Envelope::from_cbor(&env.to_cbor()).unwrap().body().unwrap();
        assert_eq!(old.kind, "user");
    }

    #[test]
    fn a_page_fits_the_phone_it_goes_to() {
        use crate::messages::AgentItems;
        let page = AgentItems {
            thread: full_thread(),
            items: vec![AgentItem {
                seq: 1,
                id: "p:1".into(),
                kind: agent::item::PLAN.into(),
                text: Some("1. Fix it".into()),
                ..Default::default()
            }],
            questions: vec![question(false), AgentQuestion { token: "e".repeat(32), ..question(true) }],
            ..Default::default()
        };
        assert!(page.is_valid());
        let page = page.sanitized().unwrap();
        let with = page.clone().for_phone(true, true);
        assert_eq!(with.questions[0].asks.len(), 1);
        assert!(with.questions[1].secret && with.questions[1].asks.is_empty(), "never a secret's asks");
        assert_eq!((with.thread.mode.as_str(), with.items[0].kind.as_str()), ("plan", "plan"));
        let without = page.for_phone(true, false);
        assert!(without.questions.is_empty());
        assert_eq!(
            (without.thread.mode.as_str(), without.items[0].kind.as_str()),
            ("supervised", "assistant")
        );
        assert!(without.thread.queued.is_empty());
    }

    #[test]
    fn a_question_keeps_its_limits_and_its_secret() {
        let mut long = question(false);
        long.asks[0].text = format!("{}\u{202E}", "w".repeat(600));
        long.asks[0].header = "h".repeat(60);
        long.asks[0].options.push(AgentQuestionOption { label: "l".repeat(81), note: None });
        long.asks[0].options.push(AgentQuestionOption { label: "Fine".into(), note: Some("n".repeat(300)) });
        assert!(!long.is_valid());
        let clean = long.sanitized().unwrap();
        let ask = &clean.asks[0];
        assert_eq!(
            (ask.text.len(), ask.header.len()),
            (agent::MAX_ASK_TEXT_BYTES, agent::MAX_ASK_HEADER_BYTES)
        );
        assert_eq!(ask.options.len(), 3, "a label too long to answer with is left out");
        assert_eq!(ask.options[2].note.as_ref().unwrap().len(), agent::MAX_OPTION_NOTE_BYTES);

        let mut hidden = question(false);
        hidden.asks[0].secret = true;
        assert!(!hidden.is_valid(), "a secret ask makes the question secret");
        let hidden = hidden.sanitized().unwrap();
        assert!(hidden.secret);
        let sent = hidden.for_phone();
        assert!(sent.asks.is_empty() && sent.is_valid(), "a phone gets no asks of a secret question");
        assert_eq!(question(false).for_phone().asks.len(), 1);

        let mut none = question(false);
        none.asks[0].id = "a\u{0}b".into();
        assert!(none.sanitized().is_none(), "nothing left to ask");
        let five = AgentQuestion { asks: vec![question(false).asks[0].clone(); 5], ..question(false) };
        assert!(!five.is_valid());
        assert_eq!(five.sanitized().unwrap().asks.len(), agent::MAX_QUESTION_ASKS);
        let shown = format!("{:?}", question(false));
        assert!(
            !shown.contains("sign-in") && !shown.contains("OAuth") && !shown.contains("ffff0000"),
            "{shown}"
        );
    }

    #[test]
    fn a_reply_answers_or_dismisses() {
        let answer = |id: &str, labels: &[&str], other: Option<&str>| AgentQuestionAnswer {
            id: id.into(),
            labels: labels.iter().map(|l| (*l).to_owned()).collect(),
            other: other.map(Into::into),
        };
        let reply = |answers: Vec<AgentQuestionAnswer>, dismiss: bool| AgentQuestionReply {
            token: "ffff0000ffff0000ffff0000ffff0000".into(),
            answers,
            dismiss,
        };
        assert!(reply(vec![answer("0", &["OAuth"], None)], false).is_valid());
        assert!(reply(vec![answer("0", &[], Some("Neither"))], false).is_valid());
        assert!(!reply(vec![answer("0", &[], None)], false).is_valid(), "an answer says something");
        assert!(!reply(vec![answer("0", &["OAuth"], None)], true).is_valid(), "one or the other");
        assert!(!reply(Vec::new(), false).is_valid());
        assert!(!reply(vec![answer("0", &["a"], None), answer("0", &["b"], None)], false).is_valid());
        assert!(!reply((0..5).map(|i| answer(&i.to_string(), &["a"], None)).collect(), false).is_valid());
        assert!(!reply(vec![answer("0", &["a\u{1b}"], None)], false).is_valid());
        assert!(!reply(vec![answer("0", &[], Some("  "))], false).is_valid());
        assert!(!reply(vec![answer("0", &[], Some(&"o".repeat(501)))], false).is_valid());
        assert!(reply(vec![answer("0", &[], Some("two\nlines"))], false).is_valid());
        let labels: Vec<&str> = vec!["a"; 7];
        assert!(!reply(vec![answer("0", &labels, None)], false).is_valid());
        let shown = format!("{:?}", reply(vec![answer("0", &["OAuth"], Some("my secret"))], false));
        assert!(!shown.contains("OAuth") && !shown.contains("secret") && !shown.contains("ffff"), "{shown}");
    }

    #[test]
    fn file_hits_never_leave_the_folder() {
        for good in ["src/main.rs", "README.md", ".github/workflows/ci.yml", "src/", "a b/c d.txt"] {
            assert!(is_relative_path(good), "{good}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "\\server\\share",
            "~/x",
            "C:/x",
            "../x",
            "a/../../x",
            "a//b",
            "./a",
            "a\\b",
            "a\nb",
            "a\u{202E}b",
            "/",
        ] {
            assert!(!is_relative_path(bad), "{bad:?}");
        }
        assert!(!is_relative_path(&"p".repeat(agent::MAX_FILE_PATH_BYTES + 1)));
        let mut files: Vec<AgentFileHit> =
            (0..60).map(|i| AgentFileHit { path: format!("f{i}.rs"), dir: false }).collect();
        files.insert(0, AgentFileHit { path: "/home/me/.ssh/id_ed25519".into(), dir: false });
        files.insert(1, AgentFileHit { path: "../outside.txt".into(), dir: false });
        let hits = AgentFiles { files, more: false };
        assert!(!hits.is_valid());
        let hits = hits.sanitized();
        assert_eq!(hits.files.len(), agent::MAX_FILE_HITS);
        assert!(hits.more && hits.files[0].path == "f0.rs");
        let request = |thread: Option<&str>, folder: Option<&str>, query: &str| AgentFilesRequest {
            thread: thread.map(Into::into),
            folder: folder.map(Into::into),
            query: query.into(),
            limit: None,
        };
        assert!(request(None, Some("f1"), "").is_valid());
        assert!(!request(Some("a"), Some("f1"), "").is_valid(), "one place");
        assert!(!request(None, None, "x").is_valid());
        assert!(!request(None, Some("f1"), &"q".repeat(129)).is_valid());
        assert!(!request(None, Some("f1"), "a\u{0}").is_valid());
        assert!(request(None, Some("f1"), "../../etc").is_valid(), "a query is only a query");
        assert_eq!(AgentFilesRequest { limit: Some(500), ..request(None, Some("f1"), "") }.limit(), 50);
        assert!(!AgentFilesRequest { limit: Some(0), ..request(None, Some("f1"), "") }.is_valid());
    }

    #[test]
    fn a_command_is_one_word() {
        let command = |name: &str, kind: &str| AgentCommand {
            name: name.into(),
            note: Some(format!("{}\u{1b}", "n".repeat(250))),
            hint: Some("h".repeat(100)),
            kind: kind.into(),
        };
        let clean = command("/review", "macro").sanitized().unwrap();
        assert_eq!((clean.name.as_str(), clean.kind.as_str()), ("review", "command"));
        assert_eq!(clean.note.unwrap().len(), agent::MAX_COMMAND_NOTE_BYTES);
        assert_eq!(clean.hint.unwrap().len(), agent::MAX_COMMAND_HINT_BYTES);
        assert_eq!(command("pdf", "skill").sanitized().unwrap().kind, "skill");
        for bad in ["two words", "", "/", "a\u{1b}", &"c".repeat(65)] {
            assert!(command(bad, "command").sanitized().is_none(), "{bad:?}");
        }
        let many = AgentCommands { commands: vec![command("x", "command"); 301] };
        assert!(!many.is_valid());
        assert_eq!(many.sanitized().commands.len(), agent::MAX_COMMANDS);
    }

    #[test]
    fn drafts_queues_and_images_keep_their_limits() {
        for good in [THREAD.to_owned(), "new:f0123456789ab".to_owned()] {
            assert!(is_draft_key(&good), "{good}");
        }
        for bad in ["", "new:", "NEW:f1", "new:../x", "old:f1", &format!("new:{}", "a".repeat(61))] {
            assert!(!is_draft_key(bad), "{bad}");
        }
        let set =
            |text: String, images: Vec<String>| AgentDraftSet { key: THREAD.into(), text, images, rev: 0 };
        assert!(set(String::new(), Vec::new()).is_valid(), "an empty draft clears it");
        assert!(!set("d".repeat(agent::MAX_DRAFT_TEXT_BYTES + 1), Vec::new()).is_valid());
        assert!(!set(String::new(), vec![IMAGE.into(); 2]).is_valid(), "an image once");
        assert!(!set(String::new(), (0..5).map(|i| format!("{i:032x}")).collect()).is_valid());
        assert!(!set(String::new(), vec!["AAAABBBBCCCCDDDDEEEEFFFF00001111".into()]).is_valid());

        let change = |op: &str, text: Option<&str>| AgentQueueChange {
            thread: THREAD.into(),
            id: QUEUED.into(),
            op: op.into(),
            text: text.map(Into::into),
        };
        assert!(change("remove", None).is_valid() && change("send_now", None).is_valid());
        assert!(!change("edit", None).is_valid(), "an edit has the new text");
        assert!(!change("remove", Some("x")).is_valid());
        assert!(!change("edit", Some("")).is_valid());
        assert!(!change("reorder", None).is_valid());
        assert!(!AgentQueueChange { id: "123".into(), ..change("remove", None) }.is_valid());

        assert!(!AgentImageMeta { name: "a.png".into(), bytes: 0 }.is_valid());
        assert!(
            !AgentImageMeta { name: "a.png".into(), bytes: agent::MAX_IMAGE_BYTES as u64 + 1 }.is_valid()
        );
        assert!(!AgentImageThumb { id: IMAGE.into(), max: Some(4096) }.is_valid());
        assert!(!AgentImageThumbMeta { length: agent::MAX_THUMB_BYTES as u64 + 1 }.is_valid());
        assert_eq!(clean_image_name("C:\\Users\\me\\IMG\u{202E}gnp.jpg"), "IMGgnp.jpg");
        let long = format!("{}.jpeg", "n".repeat(200));
        let cut = clean_image_name(&long);
        assert!(cut.len() <= agent::MAX_IMAGE_NAME_BYTES && cut.ends_with(".jpeg"));
        assert!(is_jpeg(&[0xFF, 0xD8, 0xFF, 0xDB]) && !is_jpeg(b"\x89PNG"));

        let send = AgentSend { thread: THREAD.into(), text: String::new(), ..Default::default() };
        assert!(!send.is_valid(), "no words and no image");
        assert!(!AgentSend { dispatch: Some("now".into()), text: "x".into(), ..send.clone() }.is_valid());
        let shown = format!(
            "{:?} {:?} {:?} {:?}",
            queued("my secret plan"),
            set("my secret plan".into(), Vec::new()),
            image("secret.png"),
            change("edit", Some("my secret plan")),
        );
        assert!(!shown.contains("secret") && !shown.contains("Pixel"), "{shown}");
    }
}
