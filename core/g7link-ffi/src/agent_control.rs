// SPDX-License-Identifier: MPL-2.0
//! What a phone controls of a PC's agent threads (`docs/protocol/agents.md`
//! §5, `agent.control`): models and effort, plan mode, steering and the
//! queue, the agent's questions, images, `@` files, `/` commands and drafts.
//!
//! Each method needs `agent.control` on both sides: a PC that doesn't offer
//! it (`AgentCatalog.can_control` false) answers `Unsupported`, and nothing
//! is sent. No `Debug` here prints text, answers, paths or names (protocol
//! v1 §11).

use g7link_core as core;

use crate::{AgentMode, AgentThread, G7LinkError, G7LinkNode, Result, ms, parse_id};

/// `Debug` for a record that prints only the fields named.
macro_rules! quiet_debug {
    ($ty:ident { $($field:ident),* }) => {
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($ty))$(.field(stringify!($field), &self.$field))*.finish_non_exhaustive()
            }
        }
    };
}

// ---- The catalog ----

/// What an agent can do on a PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentCan {
    /// Plan mode.
    Plan,
    /// It asks the user questions.
    Questions,
    /// A message sent while it works reaches the running turn.
    Steer,
    /// It takes images.
    Images,
    /// The PC's "Use my MCP servers here" switch works for it.
    McpSwitch,
    /// It has `/` commands.
    Commands,
}

impl AgentCan {
    pub(crate) fn from_wire(word: &str) -> Option<Self> {
        use core::agent::can;
        Some(match word {
            can::PLAN => Self::Plan,
            can::QUESTIONS => Self::Questions,
            can::STEER => Self::Steer,
            can::IMAGES => Self::Images,
            can::MCP_SWITCH => Self::McpSwitch,
            can::COMMANDS => Self::Commands,
            _ => return None,
        })
    }
}

/// A model an agent's CLI reports on the PC. List `short` ones first, the
/// rest under "More models", then "Other…" (a typed id, sent with
/// `custom_model`).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentModel {
    /// What the CLI takes; send it back as is.
    pub id: String,
    pub name: String,
    /// The CLI's description.
    pub note: Option<String>,
    /// The CLI's default ("Default").
    pub is_default: bool,
    /// Its effort levels; empty: no effort choice.
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
    /// It takes images: hide "Add image" otherwise.
    pub images: bool,
    /// A short name that follows the newest model (`opus`).
    pub short: bool,
}

quiet_debug!(AgentModel { id, is_default });

impl From<core::AgentModel> for AgentModel {
    fn from(m: core::AgentModel) -> Self {
        AgentModel {
            id: m.id,
            name: m.name,
            note: m.note,
            is_default: m.is_default,
            efforts: m.efforts,
            default_effort: m.default_effort,
            images: m.images,
            short: m.short,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentLimitState {
    Ok,
    /// Nearly used: one amber line in a thread.
    Warn,
    Reached,
}

impl AgentLimitState {
    fn from_wire(word: &str) -> Self {
        use core::agent::limit_state;
        match word {
            limit_state::WARN => Self::Warn,
            limit_state::REACHED => Self::Reached,
            _ => Self::Ok,
        }
    }
}

/// A usage window as the agent's CLI reported it on the PC. Word `window`
/// yourself: `"300"` → "5-hour", `"10080"` → "weekly", `"five_hour"`,
/// `"seven_day"`; anything else as given.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentLimit {
    pub window: String,
    /// 0 to 100.
    pub used_pct: Option<u8>,
    /// When it resets, Unix milliseconds.
    pub resets_ms: Option<i64>,
    pub state: AgentLimitState,
}

impl From<core::AgentLimit> for AgentLimit {
    fn from(l: core::AgentLimit) -> Self {
        AgentLimit {
            state: AgentLimitState::from_wire(&l.state),
            window: l.window,
            used_pct: l.used_pct,
            resets_ms: l.resets.map(ms),
        }
    }
}

// ---- Threads ----

/// An image of a message, a queued message or a draft. Its bytes stay on
/// the PC: show it with `agent_image_thumb`.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentImage {
    pub id: String,
    /// A file name, never a path.
    pub name: String,
    pub bytes: u32,
    pub width: u16,
    pub height: u16,
}

quiet_debug!(AgentImage { bytes, width, height });

impl From<core::AgentImage> for AgentImage {
    fn from(i: core::AgentImage) -> Self {
        AgentImage { id: i.id, name: i.name, bytes: i.bytes, width: i.w, height: i.h }
    }
}

/// A message waiting for the running turn to end: Send now, edit, remove.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentQueued {
    pub id: String,
    pub text: String,
    /// Cut to fit a thread list: the whole of it comes with the thread's
    /// page and its events. Don't offer to edit it until then.
    pub truncated: bool,
    /// The device it came from.
    pub by_name: Option<String>,
    pub images: Vec<AgentImage>,
    pub created_ms: i64,
}

quiet_debug!(AgentQueued { truncated, created_ms });

impl From<core::AgentQueued> for AgentQueued {
    fn from(q: core::AgentQueued) -> Self {
        AgentQueued {
            id: q.id,
            text: q.text,
            truncated: q.truncated,
            by_name: q.by_name,
            images: q.images.into_iter().map(Into::into).collect(),
            created_ms: ms(q.created),
        }
    }
}

/// How full a thread's context is: "Context 12% of 1M". Never a cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AgentContext {
    pub used: u64,
    pub max: u64,
}

/// The PC user's own MCP servers in a thread: counts only ("3 MCP servers,
/// 1 needs sign-in on <PC>").
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AgentMcp {
    /// The folder's switch is on.
    pub on: bool,
    pub servers: u16,
    pub connected: u16,
    pub need_login: u16,
}

/// How a message reached a running thread: "Sent while it worked".
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentVia {
    /// Read by the agent at its next step.
    Steer,
    /// Sent when the turn before it ended.
    Queue,
}

impl AgentVia {
    pub(crate) fn from_wire(word: &str) -> Option<Self> {
        match word {
            core::agent::via::STEER => Some(Self::Steer),
            core::agent::via::QUEUE => Some(Self::Queue),
            _ => None,
        }
    }
}

/// Where a plan stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentPlanState {
    /// Waiting for "Implement" or "Change the plan".
    Proposed,
    Implemented,
    /// "Replaced by a newer plan".
    Replaced,
}

impl AgentPlanState {
    pub(crate) fn from_wire(word: &str) -> Option<Self> {
        use core::agent::plan_state;
        Some(match word {
            plan_state::PROPOSED => Self::Proposed,
            plan_state::IMPLEMENTED => Self::Implemented,
            plan_state::REPLACED => Self::Replaced,
            _ => return None,
        })
    }
}

/// What a message sent while a turn runs does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentDispatch {
    /// The PC's choice: it waits its turn.
    Auto,
    /// The agent reads it at its next step (queued when it can't be steered).
    Steer,
    /// It waits for the turn to end.
    Queue,
}

impl AgentDispatch {
    fn wire(self) -> &'static str {
        use core::agent::dispatch;
        match self {
            Self::Auto => dispatch::AUTO,
            Self::Steer => dispatch::STEER,
            Self::Queue => dispatch::QUEUE,
        }
    }
}

/// What to do to a queued message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentQueueOp {
    /// Replace its text (`text`, 1 to 16384 bytes).
    Edit,
    Remove,
    /// Steer with it now while the thread works (when the agent can), or
    /// start a turn with it.
    SendNow,
}

impl AgentQueueOp {
    fn wire(self) -> &'static str {
        use core::agent::queue_op;
        match self {
            Self::Edit => queue_op::EDIT,
            Self::Remove => queue_op::REMOVE,
            Self::SendNow => queue_op::SEND_NOW,
        }
    }
}

// ---- Questions ----

/// One option of a question's ask.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentQuestionOption {
    pub label: String,
    pub note: Option<String>,
}

quiet_debug!(AgentQuestionOption {});

/// One ask of a question: the agent's words and the options it offers.
/// Show them as the agent's, apart from the app's own wording.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentQuestionAsk {
    pub id: String,
    /// A short tag ("Auth method").
    pub header: String,
    pub text: String,
    /// More than one option may be picked.
    pub multi: bool,
    /// "Something else": the user may answer in their own words.
    pub other: bool,
    pub secret: bool,
    pub options: Vec<AgentQuestionOption>,
}

quiet_debug!(AgentQuestionAsk { multi, other, secret });

impl From<core::AgentQuestionAsk> for AgentQuestionAsk {
    fn from(a: core::AgentQuestionAsk) -> Self {
        AgentQuestionAsk {
            id: a.id,
            header: a.header,
            text: a.text,
            multi: a.multi,
            other: a.other,
            secret: a.secret,
            options: a
                .options
                .into_iter()
                .map(|o| AgentQuestionOption { label: o.label, note: o.note })
                .collect(),
        }
    }
}

/// An agent on a PC asks the user. A `secret` one has no asks here: it's
/// answered on the PC ("Answer it on <PC>.").
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentQuestion {
    /// Answer with it (`agent_reply`, `agent_dismiss`).
    pub token: String,
    pub thread: String,
    pub thread_title: String,
    /// The agent's id: its name is in the catalog ("%1 asks").
    pub agent: String,
    /// The `Question` item it belongs to.
    pub item: String,
    pub secret: bool,
    pub asks: Vec<AgentQuestionAsk>,
    pub created_ms: i64,
    pub expires_ms: i64,
}

quiet_debug!(AgentQuestion { thread, agent, secret, asks });

impl From<core::AgentQuestion> for AgentQuestion {
    fn from(q: core::AgentQuestion) -> Self {
        let secret = q.secret;
        AgentQuestion {
            token: q.token,
            thread: q.thread,
            thread_title: q.thread_title,
            agent: q.agent,
            item: q.item,
            secret,
            asks: if secret { Vec::new() } else { q.asks.into_iter().map(Into::into).collect() },
            created_ms: ms(q.created),
            expires_ms: ms(q.expires),
        }
    }
}

/// How a question ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentQuestionOutcome {
    /// "You answered" / "Answered on %1".
    Answered,
    Dismissed,
    /// "Not answered".
    Expired,
    /// Its turn ended first.
    Cancelled,
}

impl AgentQuestionOutcome {
    pub(crate) fn from_wire(word: &str) -> Option<Self> {
        use core::agent::questioned;
        Some(match word {
            questioned::ANSWERED => Self::Answered,
            questioned::DISMISSED => Self::Dismissed,
            questioned::EXPIRED => Self::Expired,
            questioned::CANCELLED => Self::Cancelled,
            _ => return None,
        })
    }
}

/// A `Question` item's state: how many it asked (0 when not known), whether
/// one was secret (then the item never has the answers), and how it ended
/// (null while it waits) and on which device.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentItemQuestion {
    pub count: u8,
    pub secret: bool,
    pub outcome: Option<AgentQuestionOutcome>,
    pub by_name: Option<String>,
}

quiet_debug!(AgentItemQuestion { count, secret, outcome });

impl From<core::AgentItemQuestion> for AgentItemQuestion {
    fn from(q: core::AgentItemQuestion) -> Self {
        AgentItemQuestion {
            count: q.count,
            secret: q.secret,
            outcome: q.outcome.as_deref().and_then(AgentQuestionOutcome::from_wire),
            by_name: q.by_name,
        }
    }
}

/// The answer to one ask (`id`): the options picked by their labels, and/or
/// the user's own words (`other`, "Something else", when the ask allows it).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentAnswer {
    pub id: String,
    pub labels: Vec<String>,
    pub other: Option<String>,
}

quiet_debug!(AgentAnswer {});

impl From<AgentAnswer> for core::AgentQuestionAnswer {
    fn from(a: AgentAnswer) -> Self {
        core::AgentQuestionAnswer { id: a.id, labels: a.labels, other: a.other }
    }
}

// ---- Files, commands, drafts ----

/// A file or directory for `@`, relative to the thread's folder. Insert
/// `path` (with `/` after a directory, which opens the list again).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentFileHit {
    pub path: String,
    pub dir: bool,
}

quiet_debug!(AgentFileHit { dir });

/// Files of a folder, best first. `more`: there are more than these.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentFiles {
    pub files: Vec<AgentFileHit>,
    pub more: bool,
}

impl From<core::AgentFiles> for AgentFiles {
    fn from(f: core::AgentFiles) -> Self {
        AgentFiles {
            files: f.files.into_iter().map(|h| AgentFileHit { path: h.path, dir: h.dir }).collect(),
            more: f.more,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentCommandKind {
    Command,
    Skill,
}

/// A `/` command an agent offers. Add the app's own (`/model`, `/effort`,
/// `/plan`) first; send the rest as the message's first word.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentCommand {
    /// Without the `/`.
    pub name: String,
    pub note: Option<String>,
    /// Its arguments: placeholder text after it.
    pub hint: Option<String>,
    pub kind: AgentCommandKind,
}

quiet_debug!(AgentCommand { kind });

impl From<core::AgentCommand> for AgentCommand {
    fn from(c: core::AgentCommand) -> Self {
        let kind = if c.kind == core::agent::command_kind::SKILL {
            AgentCommandKind::Skill
        } else {
            AgentCommandKind::Command
        };
        AgentCommand { name: c.name, note: c.note, hint: c.hint, kind }
    }
}

/// A message being written, kept on the PC so another device can carry on
/// with it. `key`: a thread id, or `new:` and a folder id.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentDraft {
    pub key: String,
    pub text: String,
    pub images: Vec<AgentImage>,
    pub rev: u64,
    /// The device that wrote it last ("Use the draft from %1").
    pub by_name: Option<String>,
}

quiet_debug!(AgentDraft { rev });

impl From<core::AgentDraft> for AgentDraft {
    fn from(d: core::AgentDraft) -> Self {
        AgentDraft {
            key: d.key,
            text: d.text,
            images: d.images.into_iter().map(Into::into).collect(),
            rev: d.rev,
            by_name: d.by_name,
        }
    }
}

/// A draft stored (`rev` is its new one), or not (`conflict`: another device
/// changed it since; this is it as it is now).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentDraftSaved {
    pub rev: u64,
    pub conflict: Option<AgentDraft>,
}

/// The thread's record, or `Internal` for a state this build doesn't know.
fn thread_or_internal(thread: core::AgentThread) -> Result<AgentThread> {
    AgentThread::from_core(thread)
        .ok_or_else(|| G7LinkError::Internal { reason: "unknown thread state".into() })
}

// Errors as the other agent methods; `Unsupported` also when the PC doesn't
// offer `agent.control` (`AgentCatalog.can_control`), and nothing is sent.
#[uniffi::export]
impl G7LinkNode {
    /// Changes a thread's model, effort or mode for its next turn; `None`
    /// leaves one as it is, an empty `model` or `effort` goes back to the
    /// default. A phone sets `Supervised` or `Plan` only (`Denied`
    /// otherwise). `BUSY` while it works. The thread as it is now.
    #[uniffi::method(default(custom_model = false))]
    pub async fn agent_configure(
        &self,
        id: String,
        thread: String,
        model: Option<String>,
        effort: Option<String>,
        mode: Option<AgentMode>,
        custom_model: bool,
    ) -> Result<AgentThread> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let body = core::AgentConfigure {
            thread,
            model,
            effort,
            mode: mode.map(|m| m.wire().to_owned()),
            custom_model,
        };
        thread_or_internal(self.run(async move { node.agent_configure(id, body).await }).await?)
    }

    /// Edits (`text`), removes or sends now the queued message `queued`.
    /// `NotFound` when it was sent already.
    #[uniffi::method(default(text = None))]
    pub async fn agent_queue(
        &self,
        id: String,
        thread: String,
        queued: String,
        op: AgentQueueOp,
        text: Option<String>,
    ) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let change = core::AgentQueueChange { thread, id: queued, op: op.wire().to_owned(), text };
        self.run(async move { Ok(node.agent_queue(id, change).await?) }).await
    }

    /// Answers a question: one answer per ask. `NotFound` when it was
    /// answered already (the first answer wins) or it ended; `Denied` for a
    /// secret one.
    pub async fn agent_reply(&self, id: String, token: String, answers: Vec<AgentAnswer>) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let reply = core::AgentQuestionReply {
            token,
            answers: answers.into_iter().map(Into::into).collect(),
            dismiss: false,
        };
        self.run(async move { Ok(node.agent_reply(id, reply).await?) }).await
    }

    /// Dismisses a question: the agent is told the user didn't answer.
    pub async fn agent_dismiss(&self, id: String, token: String) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let reply = core::AgentQuestionReply { token, answers: Vec::new(), dismiss: true };
        self.run(async move { Ok(node.agent_reply(id, reply).await?) }).await
    }

    /// Implements the thread's newest proposed plan (`item`, its id),
    /// asking before edits and commands. `BUSY` while the thread runs;
    /// `NotFound` for any other plan.
    pub async fn agent_implement(&self, id: String, thread: String, item: String) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.agent_implement(id, thread, item).await?) }).await
    }

    /// Files for `@`: of a thread's folder (`thread`) or a folder
    /// (`folder`), exactly one; `query` is what follows the `@` (empty: the
    /// folder's top level and recent files); at most `limit` (1 to 50).
    pub async fn agent_files(
        &self,
        id: String,
        thread: Option<String>,
        folder: Option<String>,
        query: String,
        limit: u32,
    ) -> Result<AgentFiles> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let request = core::AgentFilesRequest { thread, folder, query, limit: Some(limit.clamp(1, 50)) };
        Ok(self.run(async move { node.agent_files(id, request).await }).await?.into())
    }

    /// The `/` commands `agent` offers in `folder` (ids from the catalog).
    pub async fn agent_commands(
        &self,
        id: String,
        agent: String,
        folder: String,
    ) -> Result<Vec<AgentCommand>> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let commands = self.run(async move { node.agent_commands(id, agent, folder).await }).await?;
        Ok(commands.into_iter().map(Into::into).collect())
    }

    /// Draft `key` (a thread id, or `new:` and a folder id), if there is one.
    pub async fn agent_draft_get(&self, id: String, key: String) -> Result<Option<AgentDraft>> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        Ok(self.run(async move { node.agent_draft_get(id, key).await }).await?.map(Into::into))
    }

    /// Stores a draft when `rev` is still its `rev` (0 for a new one);
    /// `images` are ids from `agent_image_send`. An empty one clears it.
    pub async fn agent_draft_set(
        &self,
        id: String,
        key: String,
        text: String,
        images: Vec<String>,
        rev: u64,
    ) -> Result<AgentDraftSaved> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let set = core::AgentDraftSet { key, text, images, rev };
        let saved = self.run(async move { node.agent_draft_set(id, set).await }).await?;
        Ok(AgentDraftSaved { rev: saved.rev, conflict: saved.conflict.map(Into::into) })
    }

    /// Sends one picture (PNG, JPEG, WebP or GIF, at most 10 MiB) for a
    /// message; put its id in `agent_start` or `agent_send` (at most 4).
    /// One at a time (`BUSY` while another goes up). The PC decodes it
    /// itself and keeps a copy without its location or camera data.
    pub async fn agent_image_send(&self, id: String, name: String, bytes: Vec<u8>) -> Result<AgentImage> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        Ok(self.run(async move { node.agent_image_send(id, name, bytes).await }).await?.into())
    }

    /// A JPEG of image `image`, at most 150 KiB and `max` px a side (64 to
    /// 1024; 512 when null). Not on the main thread.
    #[uniffi::method(default(max = None))]
    pub async fn agent_image_thumb(&self, id: String, image: String, max: Option<u32>) -> Result<Vec<u8>> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.agent_image_thumb(id, image, max).await?) }).await
    }
}

/// The dispatch word for `agent_send`.
pub(crate) fn dispatch_wire(dispatch: Option<AgentDispatch>) -> Option<String> {
    dispatch.map(|d| d.wire().to_owned())
}
