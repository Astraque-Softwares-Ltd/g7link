// SPDX-License-Identifier: MPL-2.0
//! Hand-off (`docs/protocol/agents.md` §10): a PC hands a thread to a phone
//! ("Continue on <phone>", or offered when the PC locks with it on screen),
//! and a phone hands one to its PC. Either side only opens the thread at
//! the item that was on the sender's screen: nothing is sent, typed or
//! unlocked, and drafts travel as they always do.
//!
//! PC side: the platform hands a thread to one phone or to every phone that
//! gets agent pushes ([`Node::agent_handoff`]), and answers a phone's
//! `agent.handoff.open` ([`AgentRequest::Handoff`]) after the core checked
//! the toggle (whose first refusal prompts, as every agent request's does)
//! and the limits. Phone side: a PC's `agent.handoff` becomes
//! [`NodeEvent::AgentHandoff`], with the time it holds on this phone's
//! clock; [`Node::agent_handoff_open`] asks the PC to open a thread. Nothing
//! here is retried, queued while offline, or carried over Bluetooth.

use std::sync::Arc;

pub use g7link_protocol::messages::{AgentHandedOff, AgentHandoff, AgentHandoffOpen, handoff};
use g7link_protocol::{
    DeviceId, Envelope,
    messages::{agent, types},
};

use crate::{
    Error, Node, Result,
    agents::{self, AgentFailure, AgentReply, AgentRequest, TOGGLE},
    dispatch::Responder,
    events::NodeEvent,
    node::Shared,
};

/// Phone: a thread a PC handed to this phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPcHandoff {
    pub thread: String,
    /// The first item on the PC's screen.
    pub seq: Option<u64>,
    /// The PC locked with the thread on screen: offered, not asked for.
    pub offer: bool,
    /// Unix ms, this phone's clock: when it came, plus its `ttl_ms`. It
    /// says nothing after that.
    pub expires_ms: i64,
}

// ---- PC side ----

/// PC: why `phone` can't be handed a thread now.
fn unreachable(shared: &Shared, phone: &DeviceId) -> Error {
    if !shared.we_offer(agent::HOST) {
        return Error::Unsupported;
    }
    if !shared.store.is_paired(phone).unwrap_or(false) {
        return Error::NotPaired;
    }
    if shared.session(phone).is_none() {
        return Error::Offline;
    }
    if !shared.toggle_on(phone, TOGGLE) {
        return Error::Denied;
    }
    Error::Unsupported
}

/// PC: sends `handoff` to `phone`, or with `None` to every phone that gets
/// agent pushes; how many it went to.
async fn send(shared: &Arc<Shared>, phone: Option<DeviceId>, handoff: AgentHandoff) -> Result<u32> {
    let handoff = handoff.sanitized();
    if !handoff.is_valid() {
        return Err(agents::invalid(types::AGENT_HANDOFF));
    }
    let mut sessions = shared.agent_recipients();
    if let Some(phone) = phone {
        sessions.retain(|s| s.peer == phone);
        if sessions.is_empty() {
            return Err(unreachable(shared, &phone));
        }
    }
    let env = Envelope::new(types::AGENT_HANDOFF, &handoff)?;
    let mut sent = 0u32;
    for session in sessions {
        match session.send(env.clone()).await {
            Ok(()) => sent = sent.saturating_add(1),
            Err(e) if phone.is_some() => return Err(e),
            Err(e) => tracing::debug!(phone = %session.peer.short(), error = %e, "agent.handoff didn't go"),
        }
    }
    tracing::debug!(thread = %handoff.thread, offer = handoff.offer, sent, "handed a thread over");
    Ok(sent)
}

/// PC: the answer to a phone's `agent.handoff.open`.
async fn answer_open(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) -> Envelope {
    if let Some(refused) = agents::refusal(shared, peer, false) {
        return refused;
    }
    let Some(body) = agents::decode::<AgentHandoffOpen>(env, AgentHandoffOpen::is_valid) else {
        return AgentFailure::BadMessage.reply("invalid agent.handoff.open");
    };
    let request = AgentRequest::Handoff { thread: body.thread, seq: body.seq };
    match agents::ask_platform(shared, peer, request).await {
        Ok(AgentReply::HandedOff(done)) if done.is_known() => {
            Envelope::new(types::OK, &done).unwrap_or_else(|_| AgentFailure::Internal.reply("failed"))
        }
        Ok(AgentReply::HandedOff(done)) => {
            tracing::warn!(peer = %peer.short(), result = %done.result, "the platform's hand-off answer was unknown");
            AgentFailure::Internal.reply("the host's answer was malformed")
        }
        Ok(_) => AgentFailure::Internal.reply("the host answered something else"),
        Err(failure) => failure.reply(agents::failure_message(failure)),
    }
}

// ---- Both ----

/// Whether `t` is one of §10's messages.
pub(crate) fn is_message(t: &str) -> bool {
    matches!(t, types::AGENT_HANDOFF | types::AGENT_HANDOFF_OPEN)
}

/// One of §10's messages from `peer`: a phone's `agent.handoff.open` on a
/// PC, a PC's `agent.handoff` on a phone.
pub(crate) async fn handle(
    shared: &Arc<Shared>,
    peer: DeviceId,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<()> {
    match env.t.as_str() {
        types::AGENT_HANDOFF_OPEN => rsp.send(answer_open(shared, peer, env).await).await,
        types::AGENT_HANDOFF => {
            handoff_arrived(shared, peer, env);
            Ok(())
        }
        _ => Ok(()),
    }
}

// ---- Phone side ----

/// Phone: a PC's `agent.handoff`. One out of its limits is dropped.
fn handoff_arrived(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) {
    if !shared.we_offer(agent::REMOTE) {
        return;
    }
    let Some(body) = agents::decode::<AgentHandoff>(env, AgentHandoff::is_valid) else {
        tracing::debug!(peer = %peer.short(), "dropped an agent.handoff out of its limits");
        return;
    };
    let ttl_ms = i64::try_from(body.ttl_ms).unwrap_or(i64::MAX);
    let handoff = AgentPcHandoff {
        thread: body.thread,
        seq: body.seq,
        offer: body.offer,
        expires_ms: crate::now_unix_ms().saturating_add(ttl_ms),
    };
    shared.emit(NodeEvent::AgentHandoff { device: peer, handoff });
}

/// The node's §10 methods.
impl Node {
    /// PC: hands `handoff.thread` to `phone` ("Continue on <phone>"), or with
    /// `None` to every phone that gets agent pushes (the offer when this PC
    /// locks with the thread on screen); how many it went to. Give `ttl_ms`
    /// [`handoff::TTL_MS`]; it is brought into its range. Flush the draft
    /// first (the host's `draft.set`). For one phone: `Offline`, `Denied`
    /// (its `agents` toggle is off), `NotPaired`, or `Unsupported` (this PC
    /// has no agent host, or the phone doesn't take agent pushes) when it
    /// can't go. Sent once, never queued.
    pub async fn agent_handoff(&self, phone: Option<DeviceId>, handoff: AgentHandoff) -> Result<u32> {
        send(self.shared(), phone, handoff).await
    }

    /// Phone: asks PC `pc` to open `thread` on its screen, at item `seq`.
    /// Answered `opened`, `locked` (it opens when the PC is unlocked, within
    /// 10 minutes) or `away` (it's open there, but nobody is at the PC); a
    /// word this build doesn't know reads as `away`. `Denied` while the
    /// PC's `agents` toggle is off for this phone (the first time, the PC
    /// asks its user); `NotFound` for a thread the PC doesn't have. Sent
    /// once; `Offline` while the link is down.
    pub async fn agent_handoff_open(
        &self,
        pc: DeviceId,
        thread: String,
        seq: Option<u64>,
    ) -> Result<AgentHandedOff> {
        let body = AgentHandoffOpen { thread, seq };
        if !body.is_valid() {
            return Err(agents::invalid(types::AGENT_HANDOFF_OPEN));
        }
        let env = Envelope::new(types::AGENT_HANDOFF_OPEN, &body)?;
        let done: AgentHandedOff = agents::request_of(self.shared(), pc, env, agent::HOST).await?;
        if !done.is_valid() {
            return Err(agents::invalid("agent.handoff.open reply"));
        }
        Ok(done.sanitized())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_handoff_and_its_open_are_this_modules() {
        assert!(is_message(types::AGENT_HANDOFF));
        assert!(is_message(types::AGENT_HANDOFF_OPEN));
        assert!(!is_message(types::AGENT_PRESENCE));
        assert!(!is_message(types::AGENT_SESSION_CONTINUE));
    }

    #[test]
    fn a_handoff_request_prints_its_thread_and_place() {
        let request = AgentRequest::Handoff { thread: "0123".into(), seq: Some(42) };
        let shown = format!("{request:?}");
        assert!(shown.contains("0123") && shown.contains("42"), "{shown}");
        let reply = AgentReply::HandedOff(AgentHandedOff::locked());
        assert!(format!("{reply:?}").contains("locked"), "{reply:?}");
    }
}
