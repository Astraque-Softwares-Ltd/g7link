// SPDX-License-Identifier: MPL-2.0
//! Sending while a thread runs (§Q3.4): a message steers the running turn
//! or waits in the thread's queue; a turn that ends done starts the next
//! queued message, one that fails or is stopped holds the queue until the
//! user acts. Also the settings a thread takes between turns (model,
//! effort, mode) and implementing a proposed plan.
//!
//! A queued message is one a person wrote and sent; nothing else enters
//! the queue, and nothing in it is sent after a usage limit until the user
//! says so.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::api::*;
use crate::host::agents::{SessionCmd, UserInput};
use crate::host::catalog;
use crate::host::threads::{CODEX_AUTO_EDIT, Registry, emit_item, emit_state, err, random_hex};
use crate::host::{Host, sessions};

/// A thread's settings as a start, send or configure asks to change them:
/// `None` unchanged, `Some(None)` back to the CLI's default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Wanted {
    pub model: Option<Option<String>>,
    pub effort: Option<Option<String>>,
    pub mode: Option<Mode>,
}

/// Checks what a client asks a thread to run with (§Q7): a model the agent
/// lists unless `custom` (and then a safe single argument), an effort word
/// the host knows and the model takes, a mode the agent and the asker may
/// use. `current_model`: the thread's model now (for an effort alone).
pub(crate) fn check_settings(
    info: &AgentInfo,
    current_model: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    custom: bool,
    mode: Option<Mode>,
    by_phone: bool,
) -> Result<Wanted, RpcError> {
    let agent = info.id.display_name();
    let model = match model {
        None => None,
        Some("") => Some(None),
        Some(id) if !is_valid_model_id(id) => {
            return Err(err(ErrorCode::BadRequest, "That isn't a model name."));
        }
        Some(id) if info.model(id).is_none() && !custom => {
            return Err(err(ErrorCode::BadRequest, format!("{agent} doesn't list that model.")));
        }
        Some(id) => Some(Some(id.to_owned())),
    };
    let resulting = match &model {
        Some(m) => m.as_deref(),
        None => current_model,
    };
    // The model the effort is for: the chosen one, else the CLI's default.
    let listed = match resulting {
        Some(id) => info.model(id),
        None => info.default_model(),
    };
    let effort = match effort {
        None => None,
        Some("") => Some(None),
        Some(word) => {
            let Some(word) = catalog::effort_word(word) else {
                return Err(err(ErrorCode::BadRequest, "That isn't an effort level."));
            };
            if let Some(listed) = listed
                && !listed.efforts.iter().any(|e| e == word)
            {
                return Err(err(ErrorCode::BadRequest, "This model doesn't take that effort level."));
            }
            Some(Some(word.to_owned()))
        }
    };
    if let Some(mode) = mode {
        if by_phone && !mode.phone_may() {
            return Err(err(
                ErrorCode::Denied,
                "A phone can only choose to be asked before each change, or Plan.",
            ));
        }
        if mode == Mode::Plan && !info.can(Can::Plan) {
            let message = match info.id {
                AgentId::Codex => "Plan isn't available with this version of Codex.".to_owned(),
                AgentId::Claude => format!("{agent} can't plan here."),
            };
            return Err(err(ErrorCode::Unavailable, message));
        }
        if info.id == AgentId::Codex && mode == Mode::AutoEdit && !CODEX_AUTO_EDIT {
            return Err(err(ErrorCode::Unavailable, "Codex can't edit without asking yet."));
        }
    }
    Ok(Wanted { model, effort, mode })
}

impl Wanted {
    /// Applies these to a thread; whether anything changed. An effort the
    /// new model doesn't take is dropped with the model change.
    pub(crate) fn apply(&self, info: &mut ThreadInfo, agent: &AgentInfo) -> bool {
        let before = (info.model.clone(), info.effort.clone(), info.mode);
        if let Some(model) = &self.model {
            info.model = model.clone();
            if self.effort.is_none()
                && let Some(effort) = &info.effort
            {
                let listed = match info.model.as_deref() {
                    Some(id) => agent.model(id),
                    None => agent.default_model(),
                };
                if listed.is_some_and(|m| !m.efforts.contains(effort)) {
                    info.effort = None;
                }
            }
        }
        if let Some(effort) = &self.effort {
            info.effort = effort.clone();
        }
        if let Some(mode) = self.mode {
            info.mode = mode;
        }
        before != (info.model.clone(), info.effort.clone(), info.mode)
    }
}

/// Whether the thread's model (or the CLI's default) reads images; true
/// when the list doesn't say.
fn model_takes_images(agent: &AgentInfo, model: Option<&str>) -> bool {
    let listed = match model {
        Some(id) => agent.model(id),
        None => agent.default_model(),
    };
    listed.is_none_or(|m| m.images)
}

/// The text of a message: 1 to [`MAX_PROMPT`] bytes.
fn check_text(text: &str) -> Result<(), RpcError> {
    if text.is_empty() || text.len() > MAX_PROMPT {
        return Err(err(ErrorCode::BadRequest, "The text must be 1 to 16384 bytes."));
    }
    Ok(())
}

/// A message that starts a turn.
pub(crate) struct Message {
    pub text: String,
    pub origin: Origin,
    pub via: Option<Via>,
    pub images: Vec<ImageRef>,
    /// The images as the agent takes them.
    pub blocks: Vec<Value>,
}

impl Host {
    /// The agent's probe info (from the cache unless stale).
    pub(crate) async fn agent_info(&self, agent: AgentId) -> crate::host::agents::Probe {
        self.probe(false).await[agent as usize].clone()
    }

    /// Moves a message's pending images into the thread; their refs.
    pub(crate) async fn claim_images(
        &self,
        ids: &[String],
        owner: &str,
        thread_id: &str,
    ) -> Result<Vec<ImageRef>, RpcError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        if ids.len() > MAX_IMAGES {
            return Err(err(ErrorCode::BadRequest, "Up to 4 images go with a message."));
        }
        let Some(images) = &self.images else {
            return Err(err(ErrorCode::Unavailable, "Images aren't available here."));
        };
        let claimed =
            images.claim(ids, Some(owner), thread_id).await.map_err(|e| err(e.code(), e.message()))?;
        Ok(claimed
            .into_iter()
            .map(|i| ImageRef { id: i.id, name: i.name, bytes: i.bytes, w: i.w, h: i.h })
            .collect())
    }

    /// A message's images (already in the thread) as `agent` takes them.
    pub(crate) async fn agent_images(
        &self,
        agent: AgentId,
        thread_id: &str,
        images: &[ImageRef],
    ) -> Result<Vec<Value>, RpcError> {
        if images.is_empty() {
            return Ok(Vec::new());
        }
        let Some(store) = &self.images else {
            return Err(err(ErrorCode::Unavailable, "Images aren't available here."));
        };
        let ids: Vec<String> = images.iter().map(|i| i.id.clone()).collect();
        let blocks = match agent {
            AgentId::Claude => store.claude_blocks(thread_id, &ids).await,
            AgentId::Codex => store.codex_inputs(thread_id, &ids),
        };
        blocks.map_err(|e| err(e.code(), e.message()))
    }

    /// Starts a turn of an idle thread with `message`: the user item, then
    /// the message to the live process, else a new one (`bin`).
    pub(crate) fn begin_turn(
        self: &Arc<Self>,
        reg: &mut Registry,
        thread_id: &str,
        message: Message,
        bin: Option<&Path>,
    ) -> Result<(), RpcError> {
        let placed = self.place_for(reg, thread_id);
        let Some(entry) = reg.threads.get_mut(thread_id) else {
            return Err(err(ErrorCode::NotFound, "No such thread."));
        };
        entry.turn = Default::default();
        entry.advance_turn();
        entry.set_state(State::Working);
        entry.info.error = None;
        entry.steering.clear();
        let user = entry.message_item(&message.text, &message.origin, message.via, message.images);
        emit_state(self, entry);
        emit_item(self, thread_id, &user);
        let input = UserInput { id: user.id, text: message.text, images: message.blocks };
        // A restore point first, then the message (§U5.3).
        match self.open_turn(entry, user.seq, Some(placed)) {
            Some(pre) => self.deliver_after(thread_id, pre, input, bin.map(Path::to_path_buf)),
            None => self.deliver_input(reg, thread_id, input, bin),
        }
        Ok(())
    }

    /// Gives the running turn's message to the live process, else to a new
    /// one (`bin`); the turn fails when there is neither.
    pub(crate) fn deliver_input(
        self: &Arc<Self>,
        reg: &mut Registry,
        thread_id: &str,
        input: UserInput,
        bin: Option<&Path>,
    ) {
        let Some(entry) = reg.threads.get_mut(thread_id) else { return };
        let delivered = entry.alive()
            && entry.session.as_ref().is_some_and(|s| s.cmds.send(SessionCmd::Send(input.clone())).is_ok());
        if delivered {
            return;
        }
        entry.close_session(&mut reg.bridges);
        let Some(bin) = bin else {
            self.finish_turn(
                reg,
                thread_id,
                crate::host::agents::TurnEnd::failed(
                    crate::host::agents::code::START_FAILED,
                    "The agent isn't available.",
                ),
            );
            return;
        };
        self.start_session(reg, thread_id, input, bin);
    }

    /// `thread.send` (§Q3.4). `owner`: whose pending images it may use. A
    /// thread sent to goes back on the main list.
    pub async fn thread_send(
        self: &Arc<Self>,
        p: ThreadSendParams,
        owner: &str,
    ) -> Result<ThreadSendResult, RpcError> {
        let thread_id = p.thread_id.clone();
        let key = p.key.clone();
        if let Some(key) = &key {
            if !is_valid_send_key(key) {
                return Err(err(ErrorCode::BadRequest, "Bad message key."));
            }
            if !self.lock_reg().threads.contains_key(&thread_id) {
                return Err(err(ErrorCode::NotFound, "No such thread."));
            }
            // Taken once: a retry after a lost reply sends nothing.
            let (thread, key) = (thread_id.clone(), key.clone());
            let now = crate::host::store::now_ms();
            let taken = self.store.db(move |db| db.take_send_key(&thread, &key, now)).await;
            match taken {
                Ok(true) => {}
                Ok(false) => return Ok(ThreadSendResult { queued: None, duplicate: true }),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(err(ErrorCode::NotFound, "No such thread."));
                }
                Err(_) => return Err(err(ErrorCode::Internal, "Couldn't keep the message's key.")),
            }
        }
        let result = self.send_message(p, owner).await;
        match result {
            Ok(result) => {
                self.sent_to(&thread_id);
                Ok(result)
            }
            Err(e) => {
                // Nothing was sent: the key is free for the retry.
                if let Some(key) = key {
                    let thread = thread_id.clone();
                    self.store
                        .db_later("give a message key back", move |db| db.release_send_key(&thread, &key));
                }
                Err(e)
            }
        }
    }

    /// `thread.start`, and whether its key had started a thread already
    /// (then that thread, and nothing started).
    pub async fn thread_start_keyed(
        self: &Arc<Self>,
        p: ThreadStartParams,
        owner: &str,
    ) -> Result<(ThreadInfo, bool), RpcError> {
        let Some(key) = p.key.clone() else { return Ok((self.thread_start(p, owner).await?, false)) };
        if !is_valid_send_key(&key) {
            return Err(err(ErrorCode::BadRequest, "Bad message key."));
        }
        let now = crate::host::store::now_ms();
        if let Some(info) = self.lock_reg().started_by_key(&key, now) {
            return Ok((info, true));
        }
        let info = self.thread_start(p, owner).await?;
        self.lock_reg().keep_start_key(key, &info.id, now);
        Ok((info, false))
    }

    async fn send_message(
        self: &Arc<Self>,
        p: ThreadSendParams,
        owner: &str,
    ) -> Result<ThreadSendResult, RpcError> {
        check_text(&p.text)?;
        if !p.origin.is_valid() {
            return Err(err(ErrorCode::BadRequest, "Bad origin."));
        }
        if p.images.len() > MAX_IMAGES {
            return Err(err(ErrorCode::BadRequest, "Up to 4 images go with a message."));
        }
        let (agent, running, model) = {
            let reg = self.lock_reg();
            let entry =
                reg.threads.get(&p.thread_id).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
            if entry.info.state == State::Waiting {
                return Err(err(ErrorCode::Busy, "Answer the request first."));
            }
            (entry.info.agent, entry.info.state.is_running(), entry.info.model.clone())
        };
        if running {
            if p.purpose.is_some() {
                return Err(crate::host::source::refusal(ErrorCode::Busy, crate::host::source::THREAD_BUSY));
            }
            if p.model.is_some() || p.effort.is_some() || p.mode.is_some() {
                return Err(err(
                    ErrorCode::BadRequest,
                    format!("Change the model, effort or mode when {} stops.", agent.display_name()),
                ));
            }
            let probe = self.agent_info(agent).await;
            if !p.images.is_empty() && !model_takes_images(&probe.info, model.as_deref()) {
                return Err(err(
                    ErrorCode::BadRequest,
                    format!("{} can't take images with this model.", agent.display_name()),
                ));
            }
            let images = self.claim_images(&p.images, owner, &p.thread_id).await?;
            let steer = p.dispatch == Some(Dispatch::Steer) && probe.info.can(Can::Steer);
            let blocks =
                if steer { self.agent_images(agent, &p.thread_id, &images).await? } else { Vec::new() };
            let mut reg = self.lock_reg();
            let still = reg.threads.get(&p.thread_id).map(|e| e.info.state);
            match still {
                None => return Err(err(ErrorCode::NotFound, "No such thread.")),
                Some(State::Waiting) => return Err(err(ErrorCode::Busy, "Answer the request first.")),
                Some(State::Working) => {
                    let result = if steer {
                        self.steer(&mut reg, &p.thread_id, p.text.clone(), &p.origin, images, blocks)
                    } else {
                        self.enqueue(&mut reg, &p.thread_id, p.text.clone(), &p.origin, images)
                    };
                    if result.is_ok() {
                        drop(reg);
                        self.remember_prompt(&p.text, &p.origin);
                        self.draft_sent(&p.thread_id, &p.origin);
                    }
                    return result;
                }
                // It stopped meanwhile: the message starts the next turn.
                Some(_) => {}
            }
        }
        self.send_idle(p, owner).await
    }

    /// `thread.send` to a thread that isn't running: the settings asked
    /// for, then a turn.
    async fn send_idle(
        self: &Arc<Self>,
        p: ThreadSendParams,
        owner: &str,
    ) -> Result<ThreadSendResult, RpcError> {
        let (agent, resume, model) = {
            let reg = self.lock_reg();
            let entry =
                reg.threads.get(&p.thread_id).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
            let ours_until = entry.info.updated_ms.max(entry.process_ended_ms());
            let resume =
                entry.info.native_id.clone().filter(|_| !entry.alive() && !entry.info.state.is_running());
            (entry.info.agent, resume.map(|native| (native, ours_until)), entry.info.model.clone())
        };
        // A new process would resume the agent's session: refuse while
        // something else (a terminal, the agent's app) has it open.
        if let Some((native, ours_until)) = resume
            && let Some(source) = self.open_elsewhere(agent, &native, ours_until).await
        {
            return Err(err(ErrorCode::Busy, sessions::still_open_message(agent, source)));
        }
        let probe = self.agent_info(agent).await;
        let by_phone = p.origin.kind == OriginKind::Phone;
        let wanted = check_settings(
            &probe.info,
            model.as_deref(),
            p.model.as_deref(),
            p.effort.as_deref(),
            p.custom_model,
            p.mode,
            by_phone,
        )?;
        let next_model = wanted.model.clone().unwrap_or(model);
        if !p.images.is_empty() && !model_takes_images(&probe.info, next_model.as_deref()) {
            return Err(err(
                ErrorCode::BadRequest,
                format!("{} can't take images with this model.", agent.display_name()),
            ));
        }
        let images = self.claim_images(&p.images, owner, &p.thread_id).await?;
        let blocks = self.agent_images(agent, &p.thread_id, &images).await?;
        let mut guard = self.lock_reg();
        let reg = &mut *guard;
        let (total, per) = reg.running();
        let blocked = reg.login_blocked.get(&agent).cloned();
        let cwd = {
            let Some(entry) = reg.threads.get(&p.thread_id) else {
                return Err(err(ErrorCode::NotFound, "No such thread."));
            };
            if entry.info.state.is_running() {
                return Err(err(
                    ErrorCode::Busy,
                    format!("You can reply when {} stops.", agent.display_name()),
                ));
            }
            reg.work_dir(entry).filter(|path| path.is_dir())
        };
        if let Some(message) = self.busy_message(agent, total, &per) {
            return Err(err(ErrorCode::Busy, message));
        }
        let Some(cwd) = cwd else {
            return Err(err(ErrorCode::Denied, "The folder was removed or is gone."));
        };
        let Some(entry) = reg.threads.get_mut(&p.thread_id) else {
            return Err(err(ErrorCode::NotFound, "No such thread."));
        };
        if wanted.apply(&mut entry.info, &probe.info) {
            entry.close_session(&mut reg.bridges);
        }
        let Some(entry) = reg.threads.get_mut(&p.thread_id) else {
            return Err(err(ErrorCode::NotFound, "No such thread."));
        };
        if !entry.alive() {
            if let Some(message) = blocked {
                return Err(err(ErrorCode::Unavailable, message));
            }
            if probe.bin.is_none() || !probe.info.ready() {
                return Err(Self::unavailable(&probe));
            }
        }
        entry.cwd = Some(cwd);
        let message = Message { text: p.text.clone(), origin: p.origin.clone(), via: None, images, blocks };
        self.begin_turn(reg, &p.thread_id, message, probe.bin.as_deref())?;
        // "Ask <agent> to write it": this turn's answer fills the field (§SC5.4).
        if let Some(purpose) = p.purpose
            && let Some(entry) = reg.threads.get_mut(&p.thread_id)
        {
            entry.asked = Some((purpose, entry.turn_no()));
        }
        drop(guard);
        if p.purpose.is_none() {
            self.remember_prompt(&p.text, &p.origin);
        }
        self.draft_sent(&p.thread_id, &p.origin);
        Ok(ThreadSendResult::default())
    }

    /// Steers the running turn with a message (§Q4.5): written at once as
    /// a user item `via: steer`; the agent takes it in at its next step.
    fn steer(
        self: &Arc<Self>,
        reg: &mut Registry,
        thread_id: &str,
        text: String,
        origin: &Origin,
        images: Vec<ImageRef>,
        blocks: Vec<Value>,
    ) -> Result<ThreadSendResult, RpcError> {
        let Some(entry) = reg.threads.get_mut(thread_id) else {
            return Err(err(ErrorCode::NotFound, "No such thread."));
        };
        if !entry.alive() {
            return self.enqueue(reg, thread_id, text, origin, images);
        }
        let user = entry.message_item(&text, origin, Some(Via::Steer), images);
        let input = UserInput { id: user.id.clone(), text, images: blocks };
        if entry.session.as_ref().is_none_or(|s| s.cmds.send(SessionCmd::Steer(input.clone())).is_err()) {
            return self.enqueue(reg, thread_id, input.text, origin, user.images);
        }
        emit_item(self, thread_id, &user);
        entry.steering.push(user);
        entry.info.updated_ms = crate::host::store::now_ms();
        emit_state(self, entry);
        Ok(ThreadSendResult::default())
    }

    /// Adds a message to the thread's queue.
    fn enqueue(
        self: &Arc<Self>,
        reg: &mut Registry,
        thread_id: &str,
        text: String,
        origin: &Origin,
        images: Vec<ImageRef>,
    ) -> Result<ThreadSendResult, RpcError> {
        let Some(entry) = reg.threads.get_mut(thread_id) else {
            return Err(err(ErrorCode::NotFound, "No such thread."));
        };
        if entry.info.queued.len() >= MAX_QUEUED {
            return Err(err(ErrorCode::Limit, "Up to 5 messages can wait. Send or remove one first."));
        }
        let mut id = random_hex(QUEUED_ID_LEN / 2);
        while entry.info.queued.iter().any(|q| q.id == id) {
            id = random_hex(QUEUED_ID_LEN / 2);
        }
        let queued =
            Queued { id, text, origin: origin.clone(), images, created_ms: crate::host::store::now_ms() };
        entry.info.queued.push(queued.clone());
        emit_state(self, entry);
        Ok(ThreadSendResult { queued: Some(queued), duplicate: false })
    }

    /// The turn ended done: the oldest queued message starts the next one,
    /// unless the queue is held or a steer may still start a turn.
    pub(crate) fn dispatch_queue_later(self: &Arc<Self>, thread_id: &str) {
        let host = self.clone();
        let thread = thread_id.to_owned();
        tokio::spawn(async move { host.dispatch_queue(&thread).await });
    }

    async fn dispatch_queue(self: &Arc<Self>, thread_id: &str) {
        let (agent, first) = {
            let reg = self.lock_reg();
            let Some(entry) = reg.threads.get(thread_id) else { return };
            if entry.info.state.is_running() || entry.info.queue_held || !entry.steering.is_empty() {
                return;
            }
            let Some(first) = entry.info.queued.first().cloned() else { return };
            (entry.info.agent, first)
        };
        let probe = self.agent_info(agent).await;
        let blocks = self.agent_images(agent, thread_id, &first.images).await;
        let mut guard = self.lock_reg();
        let reg = &mut *guard;
        let cwd = {
            let Some(entry) = reg.threads.get(thread_id) else { return };
            if entry.info.state.is_running()
                || entry.info.queue_held
                || !entry.steering.is_empty()
                || entry.info.queued.first().map(|q| &q.id) != Some(&first.id)
            {
                return;
            }
            reg.work_dir(entry).filter(|p| p.is_dir())
        };
        let (total, per) = reg.running();
        let busy = self.busy_message(agent, total, &per).is_some();
        let Some(entry) = reg.threads.get_mut(thread_id) else { return };
        let ready = entry.alive() || (probe.bin.is_some() && probe.info.ready());
        let (Some(cwd), true, false, Ok(blocks)) = (cwd, ready, busy, blocks) else {
            // It can't go now: it waits for the user.
            entry.info.queue_held = true;
            emit_state(self, entry);
            return;
        };
        entry.info.queued.remove(0);
        entry.cwd = Some(cwd);
        let message = Message {
            text: first.text,
            origin: first.origin,
            via: Some(Via::Queue),
            images: first.images,
            blocks,
        };
        let _ = self.begin_turn(reg, thread_id, message, probe.bin.as_deref());
    }

    /// `queue.edit`.
    pub fn queue_edit(self: &Arc<Self>, p: QueueEditParams) -> Result<(), RpcError> {
        check_text(&p.text)?;
        if p.by.as_ref().is_some_and(|b| !b.is_valid()) {
            return Err(err(ErrorCode::BadRequest, "Bad origin."));
        }
        let mut reg = self.lock_reg();
        let entry =
            reg.threads.get_mut(&p.thread_id).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
        let queued = entry
            .info
            .queued
            .iter_mut()
            .find(|q| q.id == p.id)
            .ok_or_else(|| err(ErrorCode::NotFound, "That message was already sent."))?;
        queued.text = p.text;
        if let Some(by) = p.by {
            queued.origin = by;
        }
        emit_state(self, entry);
        Ok(())
    }

    /// `queue.remove`.
    pub fn queue_remove(self: &Arc<Self>, p: QueueItemParams) -> Result<(), RpcError> {
        let mut reg = self.lock_reg();
        let entry =
            reg.threads.get_mut(&p.thread_id).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
        let before = entry.info.queued.len();
        entry.info.queued.retain(|q| q.id != p.id);
        if entry.info.queued.len() == before {
            return Err(err(ErrorCode::NotFound, "That message was already sent."));
        }
        if entry.info.queued.is_empty() {
            entry.info.queue_held = false;
        }
        emit_state(self, entry);
        Ok(())
    }

    /// `queue.send_now`: steers with it while the thread works (when the
    /// agent can), else starts a turn with it.
    pub async fn queue_send_now(self: &Arc<Self>, p: QueueItemParams) -> Result<(), RpcError> {
        let (agent, queued, state) = {
            let reg = self.lock_reg();
            let entry =
                reg.threads.get(&p.thread_id).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
            let queued = entry
                .info
                .queued
                .iter()
                .find(|q| q.id == p.id)
                .cloned()
                .ok_or_else(|| err(ErrorCode::NotFound, "That message was already sent."))?;
            (entry.info.agent, queued, entry.info.state)
        };
        if state == State::Waiting {
            return Err(err(ErrorCode::Busy, "Answer the request first."));
        }
        let probe = self.agent_info(agent).await;
        if state == State::Working && !probe.info.can(Can::Steer) {
            return Err(err(ErrorCode::Busy, format!("It's sent when {} finishes.", agent.display_name())));
        }
        let blocks = self.agent_images(agent, &p.thread_id, &queued.images).await?;
        let mut guard = self.lock_reg();
        let reg = &mut *guard;
        let (total, per) = reg.running();
        let busy = self.busy_message(agent, total, &per);
        let (state, cwd) = {
            let entry =
                reg.threads.get(&p.thread_id).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
            if !entry.info.queued.iter().any(|q| q.id == p.id) {
                return Err(err(ErrorCode::NotFound, "That message was already sent."));
            }
            let cwd = reg.work_dir(entry).filter(|p| p.is_dir());
            (entry.info.state, cwd)
        };
        match state {
            State::Waiting => Err(err(ErrorCode::Busy, "Answer the request first.")),
            State::Working => {
                let Some(entry) = reg.threads.get_mut(&p.thread_id) else {
                    return Err(err(ErrorCode::NotFound, "No such thread."));
                };
                if !entry.alive() {
                    return Err(err(
                        ErrorCode::Busy,
                        format!("It's sent when {} finishes.", agent.display_name()),
                    ));
                }
                entry.info.queued.retain(|q| q.id != p.id);
                let user = entry.message_item(&queued.text, &queued.origin, Some(Via::Steer), queued.images);
                let input = UserInput { id: user.id.clone(), text: queued.text, images: blocks };
                if let Some(session) = &entry.session {
                    let _ = session.cmds.send(SessionCmd::Steer(input));
                }
                emit_item(self, &p.thread_id, &user);
                entry.steering.push(user);
                emit_state(self, entry);
                Ok(())
            }
            _ => {
                if let Some(message) = busy {
                    return Err(err(ErrorCode::Busy, message));
                }
                let Some(cwd) = cwd else {
                    return Err(err(ErrorCode::Denied, "The folder was removed or is gone."));
                };
                let Some(entry) = reg.threads.get_mut(&p.thread_id) else {
                    return Err(err(ErrorCode::NotFound, "No such thread."));
                };
                if !entry.alive() && (probe.bin.is_none() || !probe.info.ready()) {
                    return Err(Self::unavailable(&probe));
                }
                entry.info.queued.retain(|q| q.id != p.id);
                if entry.info.queued.is_empty() {
                    entry.info.queue_held = false;
                }
                entry.cwd = Some(cwd);
                let message = Message {
                    text: queued.text,
                    origin: queued.origin,
                    via: Some(Via::Queue),
                    images: queued.images,
                    blocks,
                };
                self.begin_turn(reg, &p.thread_id, message, probe.bin.as_deref())
            }
        }
    }

    /// `thread.configure`: stored now, used from the next turn. A change
    /// lets the idle process go so the next turn starts one with the new
    /// settings (`--resume`).
    pub async fn thread_configure(
        self: &Arc<Self>,
        p: ThreadConfigureParams,
    ) -> Result<ThreadInfo, RpcError> {
        if p.by.as_ref().is_some_and(|b| !b.is_valid()) {
            return Err(err(ErrorCode::BadRequest, "Bad origin."));
        }
        let agent = {
            let reg = self.lock_reg();
            reg.threads
                .get(&p.thread_id)
                .ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?
                .info
                .agent
        };
        let probe = self.agent_info(agent).await;
        let by_phone = p.by.as_ref().is_some_and(|b| b.kind == OriginKind::Phone);
        let mut guard = self.lock_reg();
        let reg = &mut *guard;
        let entry =
            reg.threads.get_mut(&p.thread_id).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
        if entry.info.state.is_running() {
            return Err(err(ErrorCode::Busy, format!("Change it when {} stops.", agent.display_name())));
        }
        let wanted = check_settings(
            &probe.info,
            entry.info.model.as_deref(),
            p.model.as_deref(),
            p.effort.as_deref(),
            p.custom_model,
            p.mode,
            by_phone,
        )?;
        if wanted.apply(&mut entry.info, &probe.info) {
            entry.close_session(&mut reg.bridges);
            let Some(entry) = reg.threads.get_mut(&p.thread_id) else {
                return Err(err(ErrorCode::NotFound, "No such thread."));
            };
            emit_state(self, entry);
            return Ok(entry.info.clone());
        }
        Ok(entry.info.clone())
    }

    /// `plan.implement` (§Q4.3): the thread leaves plan mode, the plan is
    /// marked implemented, and "Implement the plan." starts the next turn
    /// as the user's message.
    pub async fn plan_implement(self: &Arc<Self>, p: PlanImplementParams) -> Result<(), RpcError> {
        if !p.by.is_valid() {
            return Err(err(ErrorCode::BadRequest, "Bad origin."));
        }
        if !matches!(p.mode, Mode::Supervised | Mode::AutoEdit) {
            return Err(err(
                ErrorCode::BadRequest,
                "Implement asks before edits and commands, or before commands.",
            ));
        }
        if p.by.kind == OriginKind::Phone && p.mode != Mode::Supervised {
            return Err(err(ErrorCode::Denied, "A phone implements a plan asking before each change."));
        }
        let agent = {
            let reg = self.lock_reg();
            reg.threads
                .get(&p.thread_id)
                .ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?
                .info
                .agent
        };
        if agent == AgentId::Codex && p.mode == Mode::AutoEdit && !CODEX_AUTO_EDIT {
            return Err(err(ErrorCode::Unavailable, "Codex can't edit without asking yet."));
        }
        let probe = self.agent_info(agent).await;
        let mut guard = self.lock_reg();
        let reg = &mut *guard;
        let (total, per) = reg.running();
        let busy = self.busy_message(agent, total, &per);
        let cwd: Option<PathBuf> = {
            let entry =
                reg.threads.get(&p.thread_id).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
            if entry.plan.as_ref().is_none_or(|plan| plan.id != p.item_id) {
                return Err(err(ErrorCode::NotFound, "That isn't the newest plan."));
            }
            if entry.info.state.is_running() {
                return Err(err(
                    ErrorCode::Busy,
                    format!("Implement it when {} stops.", agent.display_name()),
                ));
            }
            reg.work_dir(entry).filter(|p| p.is_dir())
        };
        if let Some(message) = busy {
            return Err(err(ErrorCode::Busy, message));
        }
        let Some(cwd) = cwd else {
            return Err(err(ErrorCode::Denied, "The folder was removed or is gone."));
        };
        if probe.bin.is_none() || !probe.info.ready() {
            return Err(Self::unavailable(&probe));
        }
        let entry =
            reg.threads.get_mut(&p.thread_id).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
        if let Some(mut plan) = entry.plan.take() {
            plan.plan_state = Some(PlanState::Implemented);
            plan.rev += 1;
            emit_item(self, &p.thread_id, &plan);
        }
        if entry.info.mode != p.mode {
            entry.info.mode = p.mode;
        }
        // The process ran in plan mode: the next turn starts a new one.
        entry.close_session(&mut reg.bridges);
        let Some(entry) = reg.threads.get_mut(&p.thread_id) else {
            return Err(err(ErrorCode::NotFound, "No such thread."));
        };
        entry.cwd = Some(cwd);
        let message = Message {
            text: IMPLEMENT_PLAN_TEXT.to_owned(),
            origin: p.by,
            via: None,
            images: Vec::new(),
            blocks: Vec::new(),
        };
        self.begin_turn(reg, &p.thread_id, message, probe.bin.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> AgentInfo {
        let model = |id: &str, efforts: &[&str], default: bool, images: bool| ModelInfo {
            id: id.into(),
            name: id.into(),
            note: None,
            default,
            efforts: efforts.iter().map(|e| (*e).to_owned()).collect(),
            default_effort: None,
            images,
            short: false,
            exact: None,
        };
        AgentInfo {
            installed: true,
            signed_in: true,
            models: vec![
                model("default", &["low", "medium", "high"], true, true),
                model("haiku", &[], false, false),
            ],
            can: vec![Can::Plan, Can::Steer],
            ..AgentInfo::missing(AgentId::Claude)
        }
    }

    #[test]
    fn settings_are_checked_against_the_agents_own_list() {
        let info = agent();
        let ok = |model: Option<&str>, effort: Option<&str>, custom: bool| {
            check_settings(&info, None, model, effort, custom, None, false)
        };
        assert_eq!(ok(Some("default"), Some("high"), false).unwrap().effort, Some(Some("high".into())));
        assert_eq!(ok(Some("opus-9"), None, false).unwrap_err().code, ErrorCode::BadRequest, "not listed");
        assert_eq!(ok(Some("opus-9"), Some("max"), true).unwrap().model, Some(Some("opus-9".into())));
        for bad in ["-p", "--model", "a b", "x\u{202e}y", &"m".repeat(65)] {
            assert_eq!(ok(Some(bad), None, true).unwrap_err().code, ErrorCode::BadRequest, "{bad:?}");
        }
        assert_eq!(
            ok(None, Some("ultra"), false).unwrap_err().code,
            ErrorCode::BadRequest,
            "default lacks it"
        );
        assert_eq!(ok(Some("haiku"), Some("low"), false).unwrap_err().code, ErrorCode::BadRequest);
        assert_eq!(ok(None, Some("High"), false).unwrap_err().code, ErrorCode::BadRequest, "words are fixed");
        assert_eq!(
            ok(Some(""), Some(""), false).unwrap(),
            Wanted { model: Some(None), effort: Some(None), mode: None }
        );
        // An effort for the thread's own model.
        assert!(check_settings(&info, Some("haiku"), None, Some("low"), false, None, false).is_err());
        // Modes.
        let mode =
            |mode, phone, info: &AgentInfo| check_settings(info, None, None, None, false, Some(mode), phone);
        assert_eq!(mode(Mode::AutoEdit, true, &info).unwrap_err().code, ErrorCode::Denied);
        assert!(mode(Mode::Plan, true, &info).is_ok());
        let codex = AgentInfo { can: vec![], ..AgentInfo::missing(AgentId::Codex) };
        assert_eq!(mode(Mode::Plan, false, &codex).unwrap_err().code, ErrorCode::Unavailable);
        assert_eq!(mode(Mode::AutoEdit, false, &codex).unwrap_err().code, ErrorCode::Unavailable);
    }

    #[test]
    fn a_new_model_drops_an_effort_it_doesnt_take() {
        let info = agent();
        let mut thread = ThreadInfo {
            model: Some("default".into()),
            effort: Some("high".into()),
            ..crate::host::threads::test_info(AgentId::Claude)
        };
        let to_haiku =
            check_settings(&info, thread.model.as_deref(), Some("haiku"), None, false, None, false).unwrap();
        assert!(to_haiku.apply(&mut thread, &info));
        assert_eq!((thread.model.as_deref(), thread.effort.as_deref()), (Some("haiku"), None));
        assert!(!to_haiku.apply(&mut thread, &info), "nothing changed the second time");
        assert!(model_takes_images(&info, None) && !model_takes_images(&info, Some("haiku")));
        assert!(model_takes_images(&info, Some("custom-one")), "unknown: let the CLI say");
    }
}

/// The control flows end to end inside one host: sessions are channels, so
/// what each thread's agent would be told is visible.
#[cfg(test)]
mod flow {
    use std::time::Duration;

    use tokio::sync::mpsc;

    use super::*;
    use crate::host::agents::{ApprovalSpec, QuestionReply, QuestionSpec, Step, TurnEnd};
    use crate::host::store::Store;
    use crate::host::threads::{Registry, test_info};
    use crate::host::{Caps, test_host};

    fn info(agent: AgentId, can: &[Can]) -> AgentInfo {
        let model = |id: &str, default: bool| ModelInfo {
            id: id.into(),
            name: id.into(),
            note: None,
            default,
            efforts: vec!["low".into(), "high".into()],
            default_effort: Some("low".into()),
            images: true,
            short: true,
            exact: None,
        };
        AgentInfo {
            installed: true,
            signed_in: true,
            models: vec![model("default", true), model("haiku", false)],
            can: can.to_vec(),
            ..AgentInfo::missing(agent)
        }
    }

    const ALL: [Can; 4] = [Can::Plan, Can::Questions, Can::Steer, Can::Images];

    async fn host(dir: &std::path::Path, can: &[Can], lifetime: u64) -> Arc<Host> {
        let host = test_host(dir, lifetime);
        host.test_agents(info(AgentId::Claude, can), info(AgentId::Codex, can)).await;
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        host.test_folder(&work);
        host
    }

    fn send(thread: &str, text: &str, dispatch: Option<Dispatch>) -> ThreadSendParams {
        ThreadSendParams { dispatch, ..ThreadSendParams::new(thread.into(), text.into(), Origin::launcher()) }
    }

    /// What the thread's agent was told since last asked: `(steer, text)`.
    fn told(cmds: &mut mpsc::UnboundedReceiver<SessionCmd>) -> Vec<(bool, String)> {
        let mut out = Vec::new();
        while let Ok(cmd) = cmds.try_recv() {
            match cmd {
                SessionCmd::Send(input) => out.push((false, input.text)),
                SessionCmd::Steer(input) => out.push((true, input.text)),
                SessionCmd::Close => out.push((false, "<close>".into())),
                SessionCmd::Interrupt | SessionCmd::Kill => {}
            }
        }
        out
    }

    async fn settle() {
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    fn ask(command: &str) -> ApprovalSpec {
        ApprovalSpec {
            kind: ApprovalKind::Command,
            title: "Run command".into(),
            command: Some(command.into()),
            diff: None,
            path: None,
            reason: None,
            tool_key: None,
            tool: None,
        }
    }

    #[tokio::test]
    async fn a_message_sent_while_it_works_steers_or_waits_its_turn() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &ALL, APPROVAL_LIFETIME_MS).await;
        let (id, generation, mut cmds) = host.test_thread(AgentId::Claude);
        // Steer: written at once, and the agent gets it now.
        let steered =
            host.thread_send(send(&id, "Use the other file", Some(Dispatch::Steer)), "test").await.unwrap();
        assert_eq!(steered.queued, None);
        assert_eq!(told(&mut cmds), [(true, "Use the other file".to_owned())]);
        let items = host.test_items(&id).await;
        let user = items.last().unwrap();
        assert_eq!((user.kind, user.via, user.turn), (ItemKind::User, Some(Via::Steer), Some(1)));
        // Queue, and a bare `auto` or nothing: the queue.
        for dispatch in [Some(Dispatch::Queue), Some(Dispatch::Auto), None] {
            let queued = host.thread_send(send(&id, "Then run the tests", dispatch), "test").await.unwrap();
            assert!(
                queued.queued.is_some_and(|q| q.text == "Then run the tests" && q.id.len() == QUEUED_ID_LEN)
            );
        }
        assert!(told(&mut cmds).is_empty(), "nothing queued reaches the agent while it works");
        let state = host.test_state(&id);
        assert_eq!((state.queued.len(), state.queue_held), (3, false));
        // Settings change between turns only.
        let with_model = ThreadSendParams { model: Some("haiku".into()), ..send(&id, "x", None) };
        assert_eq!(host.thread_send(with_model, "test").await.unwrap_err().code, ErrorCode::BadRequest);
        // At most five wait.
        for n in 0..2 {
            host.thread_send(send(&id, &format!("m{n}"), None), "test").await.unwrap();
        }
        assert_eq!(
            host.thread_send(send(&id, "sixth", None), "test").await.unwrap_err().code,
            ErrorCode::Limit
        );
        // Waiting on a request: answer it first.
        let _approval = host.test_ask(&id, generation, ask("ls"));
        let busy = host.thread_send(send(&id, "hello?", Some(Dispatch::Steer)), "test").await.unwrap_err();
        assert_eq!(busy.code, ErrorCode::Busy);

        // An agent that can't be steered: the steer waits in the queue.
        let dir = tempfile::tempdir().unwrap();
        let host = super::flow::host(dir.path(), &[], APPROVAL_LIFETIME_MS).await;
        let (id, _generation, mut cmds) = host.test_thread(AgentId::Codex);
        let result =
            host.thread_send(send(&id, "Stop after this", Some(Dispatch::Steer)), "test").await.unwrap();
        assert!(result.queued.is_some(), "the result tells the client it was queued");
        assert!(told(&mut cmds).is_empty());
    }

    #[tokio::test]
    async fn a_done_turn_starts_the_next_queued_message_one_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &ALL, APPROVAL_LIFETIME_MS).await;
        let (id, _generation, mut cmds) = host.test_thread(AgentId::Codex);
        host.thread_send(send(&id, "first", Some(Dispatch::Queue)), "test").await.unwrap();
        host.thread_send(send(&id, "second", Some(Dispatch::Queue)), "test").await.unwrap();
        host.test_step(&id, Step::TurnEnded(TurnEnd::done(Some("Done.".into()))));
        settle().await;
        assert_eq!(told(&mut cmds), [(false, "first".to_owned())]);
        let state = host.test_state(&id);
        assert_eq!((state.state, state.queued.len()), (State::Working, 1));
        let user = host.test_items(&id).await.into_iter().rfind(|i| i.kind == ItemKind::User).unwrap();
        assert_eq!((user.text.as_deref(), user.via, user.turn), (Some("first"), Some(Via::Queue), Some(2)));
        host.test_step(&id, Step::TurnEnded(TurnEnd::done(None)));
        settle().await;
        assert_eq!(told(&mut cmds), [(false, "second".to_owned())]);
        host.test_step(&id, Step::TurnEnded(TurnEnd::done(None)));
        settle().await;
        assert!(told(&mut cmds).is_empty());
        assert_eq!(host.test_state(&id).state, State::Done);
    }

    #[tokio::test]
    async fn a_failed_or_stopped_turn_holds_the_queue_until_the_user_acts() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &ALL, APPROVAL_LIFETIME_MS).await;
        let (id, _generation, mut cmds) = host.test_thread(AgentId::Claude);
        let first = host.thread_send(send(&id, "first", None), "test").await.unwrap().queued.unwrap();
        let second = host.thread_send(send(&id, "second", None), "test").await.unwrap().queued.unwrap();
        host.test_step(&id, Step::TurnEnded(TurnEnd::failed("usage_limit", "Usage limit reached")));
        settle().await;
        assert!(told(&mut cmds).is_empty(), "never resumed by itself");
        assert!(host.test_state(&id).queue_held);
        // A new message goes first; the queue stays held.
        host.thread_send(send(&id, "go on", None), "test").await.unwrap();
        assert_eq!(told(&mut cmds), [(false, "go on".to_owned())]);
        host.test_step(&id, Step::TurnEnded(TurnEnd::done(None)));
        settle().await;
        assert!(told(&mut cmds).is_empty());
        assert_eq!(host.test_state(&id).queued.len(), 2);
        // Send now: that one starts a turn.
        host.queue_send_now(QueueItemParams { thread_id: id.clone(), id: first.id.clone(), by: None })
            .await
            .unwrap();
        assert_eq!(told(&mut cmds), [(false, "first".to_owned())]);
        // Stopped: held again; edit and remove what is left.
        host.test_step(&id, Step::TurnEnded(TurnEnd::interrupted()));
        assert!(host.test_state(&id).queue_held);
        let edit = |id: &str, q: &str| QueueEditParams {
            thread_id: id.into(),
            id: q.into(),
            text: "second, edited".into(),
            by: Some(Origin::phone("p", "Pixel")),
        };
        host.queue_edit(edit(&id, &second.id)).unwrap();
        let queued = host.test_state(&id).queued;
        assert_eq!(queued[0].text, "second, edited");
        assert_eq!(queued[0].origin, Origin::phone("p", "Pixel"), "the PC shows who edited it");
        assert_eq!(
            host.queue_edit(edit(&id, &first.id)).unwrap_err().code,
            ErrorCode::NotFound,
            "already sent"
        );
        host.queue_remove(QueueItemParams { thread_id: id.clone(), id: second.id, by: None }).unwrap();
        let state = host.test_state(&id);
        assert!(state.queued.is_empty() && !state.queue_held);
    }

    #[tokio::test]
    async fn send_now_steers_a_working_thread() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &ALL, APPROVAL_LIFETIME_MS).await;
        let (id, _generation, mut cmds) = host.test_thread(AgentId::Claude);
        let queued = host.thread_send(send(&id, "later", None), "test").await.unwrap().queued.unwrap();
        host.queue_send_now(QueueItemParams { thread_id: id.clone(), id: queued.id, by: None })
            .await
            .unwrap();
        assert_eq!(told(&mut cmds), [(true, "later".to_owned())]);
        assert!(host.test_state(&id).queued.is_empty());
    }

    #[tokio::test]
    async fn the_queue_outlives_a_host_restart_held() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_owned()).unwrap();
        let mut db = crate::host::db::Db::open(dir.path()).unwrap();
        let queued = Queued {
            id: "0123456789abcdef".into(),
            text: "Then run the tests".into(),
            origin: Origin::launcher(),
            images: Vec::new(),
            created_ms: 1,
        };
        for (id, state) in [("a".repeat(32), State::Done), ("b".repeat(32), State::Working)] {
            let info = ThreadInfo { id, state, queued: vec![queued.clone()], ..test_info(AgentId::Claude) };
            db.save_thread(
                &info,
                &crate::host::db::ThreadCols { next_seq: 1, next_turn: 1, ..Default::default() },
            )
            .unwrap();
        }
        let reg = Registry::load(&store, Some(&mut db), APPROVAL_LIFETIME_MS);
        for entry in reg.threads.values() {
            assert_eq!(entry.info.queued, std::slice::from_ref(&queued));
            assert!(entry.info.queue_held, "nothing is sent until the user says");
        }
    }

    #[tokio::test]
    async fn configure_applies_between_turns_and_lets_the_idle_process_go() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &ALL, APPROVAL_LIFETIME_MS).await;
        let (id, _generation, mut cmds) = host.test_thread(AgentId::Claude);
        let configure = |model: Option<&str>, mode: Option<Mode>, by: Option<Origin>| ThreadConfigureParams {
            thread_id: id.clone(),
            model: model.map(str::to_owned),
            effort: None,
            mode,
            custom_model: false,
            by,
        };
        let busy = host.thread_configure(configure(Some("haiku"), None, None)).await.unwrap_err();
        assert_eq!(busy.code, ErrorCode::Busy, "never during a turn");
        host.test_step(&id, Step::TurnEnded(TurnEnd::done(None)));
        // No change: the process stays.
        host.thread_configure(configure(None, Some(Mode::Supervised), None)).await.unwrap();
        assert!(told(&mut cmds).is_empty());
        let thread = host.thread_configure(configure(Some("haiku"), None, None)).await.unwrap();
        assert_eq!(thread.model.as_deref(), Some("haiku"));
        assert_eq!(
            told(&mut cmds),
            [(false, "<close>".to_owned())],
            "restarted with the new flags next turn"
        );
        assert!(host.test_entry(&id, |e| e.session.is_none()));
        // A phone: supervised or plan only.
        let phone = Some(Origin::phone("p", "Pixel"));
        let denied =
            host.thread_configure(configure(None, Some(Mode::AutoEdit), phone.clone())).await.unwrap_err();
        assert_eq!(denied.code, ErrorCode::Denied);
        let plan = host.thread_configure(configure(None, Some(Mode::Plan), phone)).await.unwrap();
        assert_eq!(plan.mode, Mode::Plan);
        // An older client reads plan mode as supervised.
        assert_eq!(plan.for_client(false).mode, Mode::Supervised);
    }

    #[tokio::test]
    async fn a_phone_may_not_start_or_implement_with_auto_edit() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &ALL, APPROVAL_LIFETIME_MS).await;
        let start = ThreadStartParams::new(
            AgentId::Claude,
            "f000000000001".into(),
            "hi".into(),
            Mode::AutoEdit,
            Origin::phone("p", "Pixel"),
        );
        assert_eq!(host.thread_start(start, "link").await.unwrap_err().code, ErrorCode::Denied);
        let (id, _generation, _cmds) = host.test_thread(AgentId::Claude);
        host.test_step(&id, Step::PlanDone { key: "p1".into(), text: "1. Do it".into() });
        host.test_step(&id, Step::TurnEnded(TurnEnd::done(None)));
        let implement = PlanImplementParams {
            thread_id: id.clone(),
            item_id: "p1".into(),
            mode: Mode::AutoEdit,
            by: Origin::phone("p", "Pixel"),
        };
        assert_eq!(host.plan_implement(implement).await.unwrap_err().code, ErrorCode::Denied);
    }

    #[tokio::test]
    async fn a_plan_is_proposed_replaced_then_implemented() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &ALL, APPROVAL_LIFETIME_MS).await;
        let (id, _generation, mut cmds) = host.test_thread(AgentId::Codex);
        host.test_entry(&id, |e| e.info.mode = Mode::Plan);
        host.test_step(&id, Step::PlanOpen { key: "p1".into() });
        host.test_step(&id, Step::PlanDelta { key: "p1".into(), text: "1. Add notes".into() });
        host.test_step(&id, Step::PlanDone { key: "p1".into(), text: "1. Add notes.txt".into() });
        host.test_step(&id, Step::TurnEnded(TurnEnd::done(Some("Here is the plan.".into()))));
        assert_eq!(host.test_state(&id).summary.as_deref(), Some(crate::host::plan::PLAN_READY));
        // Refine: a new plan replaces it.
        host.thread_send(send(&id, "Also update the README", None), "test").await.unwrap();
        host.test_step(&id, Step::PlanDone { key: "p2".into(), text: "1. Add notes.txt\n2. README".into() });
        host.test_step(&id, Step::TurnEnded(TurnEnd::done(None)));
        let plans: Vec<(String, Option<PlanState>, u32)> = host
            .test_items(&id)
            .await
            .into_iter()
            .filter(|i| i.kind == ItemKind::Plan)
            .map(|i| (i.id, i.plan_state, i.rev))
            .collect();
        assert_eq!(
            plans,
            [("p1".into(), Some(PlanState::Replaced), 1), ("p2".into(), Some(PlanState::Proposed), 0)]
        );
        let implement = |item: &str| PlanImplementParams {
            thread_id: id.clone(),
            item_id: item.into(),
            mode: Mode::Supervised,
            by: Origin::launcher(),
        };
        assert_eq!(host.plan_implement(implement("p1")).await.unwrap_err().code, ErrorCode::NotFound);
        told(&mut cmds);
        host.plan_implement(implement("p2")).await.unwrap();
        let state = host.test_state(&id);
        assert_eq!((state.mode, state.state), (Mode::Supervised, State::Working));
        let items = host.test_items(&id).await;
        let plan = items.iter().find(|i| i.id == "p2").unwrap();
        assert_eq!((plan.plan_state, plan.rev), (Some(PlanState::Implemented), 1));
        let user = items.last().unwrap();
        assert_eq!(user.text.as_deref(), Some(IMPLEMENT_PLAN_TEXT));
        assert_eq!(
            told(&mut cmds).first().map(|c| c.1.as_str()),
            Some("<close>"),
            "the plan-mode process went"
        );
        assert_eq!(host.plan_implement(implement("p2")).await.unwrap_err().code, ErrorCode::NotFound);
    }

    fn question(asks: Vec<Ask>) -> QuestionSpec {
        QuestionSpec { key: Some("q-item".into()), asks }
    }

    fn choice(id: &str, text: &str, options: &[&str], multi: bool) -> Ask {
        Ask {
            id: id.into(),
            header: "Pick".into(),
            text: text.into(),
            multi,
            other: false,
            secret: false,
            options: options.iter().map(|o| AskOption { label: (*o).into(), note: None }).collect(),
        }
    }

    fn answer(id: &str, labels: &[&str], other: Option<&str>) -> Answer {
        Answer {
            id: id.into(),
            labels: labels.iter().map(|l| (*l).to_owned()).collect(),
            other: other.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn the_first_answer_to_a_question_wins() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &ALL, APPROVAL_LIFETIME_MS).await;
        let watcher = host.hub.test_subscriber(Caps { items2: true, items3: true });
        let (id, generation, _cmds) = host.test_thread(AgentId::Codex);
        let rx = host.test_question(
            &id,
            generation,
            question(vec![
                choice("file", "Which file?", &["a.txt", "b.txt"], false),
                choice("checks", "What to run?", &["tests", "lint"], true),
            ]),
        );
        let state = host.test_state(&id);
        assert_eq!((state.state, state.asking, state.pending), (State::Waiting, 1, 0));
        let asked = host.questions_of(&id).pop().unwrap();
        assert_eq!((asked.asks.len(), asked.item_id.as_str(), asked.secret), (2, "q-item", false));
        let by = || Origin::phone("p", "Pixel");
        let params =
            |answers: Vec<Answer>| QuestionAnswerParams { token: asked.token.clone(), answers, by: by() };
        for bad in [
            vec![answer("file", &["a.txt"], None)],
            vec![answer("file", &["c.txt"], None), answer("checks", &["tests"], None)],
            vec![answer("file", &["a.txt", "b.txt"], None), answer("checks", &["tests"], None)],
            vec![answer("file", &[], Some("my own")), answer("checks", &["tests"], None)],
            vec![answer("file", &["a.txt"], None), answer("file", &["b.txt"], None)],
        ] {
            assert_eq!(host.question_answer(params(bad)).unwrap_err().code, ErrorCode::BadRequest);
        }
        host.question_answer(params(vec![
            answer("checks", &["tests", "lint"], None),
            answer("file", &["a.txt"], None),
        ]))
        .unwrap();
        let QuestionReply::Answered(answers) = rx.await.unwrap() else { panic!("not answered") };
        assert_eq!(answers.get("file").unwrap().labels, ["a.txt"]);
        assert_eq!(answers.get("checks").unwrap().labels, ["tests", "lint"]);
        let again = host.question_answer(params(vec![answer("file", &["b.txt"], None)])).unwrap_err();
        assert_eq!(again.code, ErrorCode::AlreadyAnswered);
        let state = host.test_state(&id);
        assert_eq!((state.state, state.asking), (State::Working, 0));
        let item = host.test_items(&id).await.into_iter().find(|i| i.kind == ItemKind::Question).unwrap();
        assert_eq!(item.text.as_deref(), Some("- Which file?: a.txt\n- What to run?: tests, lint"));
        assert_eq!(
            item.question,
            Some(QuestionInfo { count: 2, secret: false, outcome: Some(QuestionOutcome::Answered) })
        );
        assert_eq!(item.origin, Some(by()));
        let lines = host.hub.test_lines(watcher).join("\n");
        assert!(lines.contains("\"question.requested\"") && lines.contains("\"question.resolved\""));
    }

    #[tokio::test]
    async fn questions_expire_are_bounded_and_end_with_the_turn() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &ALL, 50).await;
        let (id, generation, _cmds) = host.test_thread(AgentId::Codex);
        let one = || question(vec![choice("0", "Which?", &["a"], false)]);
        let expiring = host.test_question(&id, generation, one());
        assert_eq!(expiring.await.unwrap(), QuestionReply::Expired);
        let item = host.test_items(&id).await.into_iter().find(|i| i.kind == ItemKind::Question).unwrap();
        assert_eq!(item.question.unwrap().outcome, Some(QuestionOutcome::Expired));

        let host = super::flow::host(dir.path(), &ALL, APPROVAL_LIFETIME_MS).await;
        let (id, generation, _cmds) = host.test_thread(AgentId::Codex);
        let first = host.test_question(&id, generation, one());
        let second = host.test_question(&id, generation, one());
        let third = host.test_question(&id, generation, one());
        assert_eq!(third.await.unwrap(), QuestionReply::Dismissed, "two wait at most");
        let token = host.questions_of(&id)[0].token.clone();
        host.question_dismiss(&token, Origin::launcher()).unwrap();
        assert_eq!(first.await.unwrap(), QuestionReply::Dismissed);
        host.test_step(&id, Step::TurnEnded(TurnEnd::done(None)));
        assert_eq!(second.await.unwrap(), QuestionReply::Cancelled);
        assert!(host.questions_of(&id).is_empty());
        let empty = host.test_question(&id, generation, question(vec![]));
        assert_eq!(empty.await.unwrap(), QuestionReply::Cancelled, "nothing to ask, or the turn is over");
    }

    #[tokio::test]
    async fn a_secret_answer_is_never_kept_logged_or_sent() {
        const SECRET: &str = "hunter2-secret-7f3a";
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &ALL, APPROVAL_LIFETIME_MS).await;
        let watcher = host.hub.test_subscriber(Caps { items2: true, items3: true });
        let (id, generation, _cmds) = host.test_thread(AgentId::Codex);
        let secret = Ask {
            secret: true,
            other: true,
            options: vec![],
            ..choice("token", "Your API token?", &[], false)
        };
        let rx = host.test_question(&id, generation, question(vec![secret]));
        let token = host.questions_of(&id)[0].token.clone();
        assert!(host.questions_of(&id)[0].redacted().asks.is_empty(), "a phone never gets its asks");
        let params = |by: Origin| QuestionAnswerParams {
            token: token.clone(),
            answers: vec![answer("token", &[], Some(SECRET))],
            by,
        };
        assert!(!format!("{:?}", params(Origin::launcher())).contains(SECRET), "Debug prints nothing of it");
        assert_eq!(
            host.question_answer(params(Origin::phone("p", "Pixel"))).unwrap_err().code,
            ErrorCode::Denied
        );
        host.question_answer(params(Origin::launcher())).unwrap();
        let QuestionReply::Answered(answers) = rx.await.unwrap() else { panic!("not answered") };
        assert_eq!(answers.get("token").unwrap().other.as_deref(), Some(SECRET), "only the agent gets it");
        assert!(!format!("{answers:?}").contains(SECRET));
        drop(answers);
        host.test_step(&id, Step::TurnEnded(TurnEnd::done(Some("Thanks.".into()))));
        let export = host
            .thread_export(ThreadExportParams {
                thread_id: Some(id.clone()),
                session_id: None,
                format: ExportFormat::Markdown,
            })
            .await
            .unwrap();
        assert!(export.text.contains("Asked a question") && !export.text.contains(SECRET));
        host.store.flush().await;
        let mut stack = vec![host.config.data_dir.clone()];
        let mut files = 0;
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    files += 1;
                    let bytes = std::fs::read(&path).unwrap();
                    assert!(!String::from_utf8_lossy(&bytes).contains(SECRET), "in {}", path.display());
                }
            }
        }
        assert!(files > 0, "the thread was stored");
        let lines = host.hub.test_lines(watcher).join("\n");
        assert!(lines.contains("question.resolved") && !lines.contains(SECRET), "no event carries it");
        let item = host.test_items(&id).await.into_iter().find(|i| i.kind == ItemKind::Question).unwrap();
        assert_eq!((item.text, item.question.unwrap().secret), (None, true));
    }

    #[tokio::test]
    async fn older_clients_get_plans_and_questions_as_steps_with_dense_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &ALL, APPROVAL_LIFETIME_MS).await;
        let old = host.hub.test_subscriber(Caps { items2: true, items3: false });
        let (id, generation, _cmds) = host.test_thread(AgentId::Codex);
        host.test_entry(&id, |e| e.info.mode = Mode::Plan);
        host.test_step(
            &id,
            Step::AssistantDone { key: "m1".into(), text: "Looking.".into(), final_answer: false },
        );
        host.test_step(&id, Step::PlanDone { key: "p1".into(), text: "1. Do it".into() });
        let rx = host.test_question(&id, generation, question(vec![choice("0", "Which?", &["a"], false)]));
        let token = host.questions_of(&id)[0].token.clone();
        host.question_answer(QuestionAnswerParams {
            token,
            answers: vec![answer("0", &["a"], None)],
            by: Origin::launcher(),
        })
        .unwrap();
        assert!(matches!(rx.await.unwrap(), QuestionReply::Answered(_)));
        let read = ThreadReadParams { thread_id: id.clone(), ..Default::default() };
        let legacy = host.thread_read(read, Caps { items2: true, items3: false }).await.unwrap();
        assert_eq!(legacy.thread.mode, Mode::Supervised);
        let kinds: Vec<(u64, ItemKind)> = legacy.items.iter().map(|i| (i.seq, i.kind)).collect();
        assert_eq!(kinds, [(1, ItemKind::Assistant), (2, ItemKind::Assistant), (3, ItemKind::Tool)]);
        assert_eq!(legacy.items[1].text.as_deref(), Some("1. Do it"));
        let step = legacy.items[2].tool.as_ref().unwrap();
        assert_eq!(
            (step.name.as_str(), step.title.as_str(), step.status),
            ("question", "Question", ToolStatus::Ok)
        );
        assert_eq!(legacy.items[2].text, None);
        // The events say the same.
        let lines = host.hub.test_lines(old);
        let events: Vec<Event> = lines
            .iter()
            .filter_map(|l| match ServerLine::parse(l).ok()? {
                ServerLine::Event(e) => Event::from_line(&e),
                ServerLine::Response(_) => None,
            })
            .collect();
        assert!(events.iter().all(|e| match e {
            Event::ThreadItem(e) => !matches!(e.item.kind, ItemKind::Plan | ItemKind::Question),
            Event::ThreadState(e) => e.thread.mode != Mode::Plan,
            _ => true,
        }));
        assert!(events.iter().any(
            |e| matches!(e, Event::ThreadItem(e) if e.item.kind == ItemKind::Assistant && e.item.seq == 2)
        ));
    }

    #[tokio::test]
    async fn a_steer_is_marked_taken_or_starts_its_own_turn() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &ALL, APPROVAL_LIFETIME_MS).await;
        let (id, _generation, mut cmds) = host.test_thread(AgentId::Claude);
        host.thread_send(send(&id, "Use b.txt", Some(Dispatch::Steer)), "test").await.unwrap();
        let steered = host.test_items(&id).await.pop().unwrap();
        host.test_step(&id, Step::Delivered { id: steered.id.clone() });
        let taken = host.test_items(&id).await.pop().unwrap();
        assert_eq!((taken.id, taken.rev), (steered.id, 1));
        // A steer still waiting when the turn ends holds the queue back
        // until the agent starts a turn for it.
        host.thread_send(send(&id, "And then c.txt", Some(Dispatch::Steer)), "test").await.unwrap();
        host.thread_send(send(&id, "queued", Some(Dispatch::Queue)), "test").await.unwrap();
        told(&mut cmds);
        host.test_step(&id, Step::TurnEnded(TurnEnd::done(None)));
        settle().await;
        assert!(told(&mut cmds).is_empty(), "the queue waits for the steer's turn");
        host.test_step(&id, Step::TurnStarted);
        let state = host.test_state(&id);
        assert_eq!(state.state, State::Working);
        let moved = host.test_items(&id).await.into_iter().rfind(|i| i.via == Some(Via::Steer)).unwrap();
        assert_eq!((moved.turn, moved.rev), (Some(2), 1), "the steer is that turn's");
        host.test_step(&id, Step::TurnEnded(TurnEnd::done(None)));
        settle().await;
        assert_eq!(told(&mut cmds), [(false, "queued".to_owned())]);
    }

    #[tokio::test]
    async fn context_mcp_and_the_answering_model_reach_the_thread() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &ALL, APPROVAL_LIFETIME_MS).await;
        let (id, _generation, _cmds) = host.test_thread(AgentId::Claude);
        host.test_step(&id, Step::Context { used: 120_000, max: 1_000_000 });
        host.test_step(&id, Step::AnsweredModel("claude-opus-5".into()));
        host.test_step(&id, Step::AnsweredModel(crate::host::catalog::SYNTHETIC_MODEL.into()));
        host.test_step(
            &id,
            Step::Mcp { names: vec!["github".into(), "\u{1b}linear".into()], connected: 1, need_login: 1 },
        );
        let state = host.test_state(&id);
        assert_eq!(state.context, Some(ContextUse { used: 120_000, max: 1_000_000 }));
        assert_eq!(state.answered_model.as_deref(), Some("claude-opus-5"), "a synthetic reply doesn't count");
        let mcp = state.mcp.unwrap();
        assert_eq!(
            (mcp.on, mcp.connected, mcp.need_login, mcp.names),
            (true, 1, 1, vec!["github".into(), "linear".into()])
        );
        settle().await;
        let folder = host.folders_list().pop().unwrap();
        assert_eq!(
            folder.mcp_seen,
            [FolderMcp { agent: AgentId::Claude, names: vec!["github".into(), "linear".into()] }]
        );
        // The switch: stored, and used from the next process.
        let set = FoldersSetParams { id: "f000000000001".into(), mcp: Some(false), ..Default::default() };
        let off = host.folders_set(&set).await.unwrap();
        assert!(!off.mcp);
        assert_eq!(off.mcp_seen.len(), 1, "what threads reported stays");
    }

    #[tokio::test]
    async fn drafts_reach_the_other_connections_and_go_when_sent() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &ALL, APPROVAL_LIFETIME_MS).await;
        let me = host.hub.test_subscriber(Caps::default());
        let other = host.hub.test_subscriber(Caps::default());
        let (id, _generation, _cmds) = host.test_thread(AgentId::Claude);
        let set = |text: &str, rev: u64| DraftSetParams {
            key: id.clone(),
            text: text.into(),
            images: vec![],
            rev,
            by: Some(Origin::launcher()),
        };
        assert_eq!(host.draft_set(set("Fix the", 0), vec![], me).unwrap(), 1);
        assert!(host.hub.test_lines(me).is_empty(), "the writer isn't told");
        assert!(host.hub.test_lines(other).iter().any(|l| l.contains("draft.changed")));
        let conflict = host.draft_set(set("Fix it", 0), vec![], other).unwrap_err();
        assert_eq!(conflict.code, ErrorCode::Conflict);
        assert_eq!(conflict.conflict_draft().map(|d| (d.text, d.rev)), Some(("Fix the".into(), 1)));
        host.thread_send(send(&id, "Fix the build", None), "test").await.unwrap();
        let draft = host.draft_get(&id).unwrap().unwrap();
        assert_eq!((draft.text.as_str(), draft.rev), ("", 2), "sent: emptied everywhere");
        assert_eq!(host.history_list(None), ["Fix the build"], "a launcher's prompt is kept");
        let phone =
            ThreadSendParams { origin: Origin::phone("p", "Pixel"), ..send(&id, "from the phone", None) };
        host.thread_send(phone, "link").await.unwrap();
        assert_eq!(host.history_list(None), ["Fix the build"], "a phone's never is");
    }
}
