// SPDX-License-Identifier: MPL-2.0
//! Move to G7 (`docs/protocol/agents.md` §9.1): a phone asks the PC to close
//! a Claude or Codex session started there outside G7 Link and, once nothing
//! has it open, continue it as a thread started by that phone.
//!
//! PC side: the core checks that this PC offers `agent.move`, then §9's
//! `agents`, `agent_sessions` and `agent_reply` toggles (with §9's one-time
//! prompts), then the request's limits, and only then asks the platform
//! ([`AgentRequest::SessionMove`]). The platform does the rest: the rate and
//! the confirmation it shares with replies, closing the agent, and asking
//! the host to continue the session, whose own checks decide. The core
//! passes its answer on only when it's whole, made to fit, its thread in
//! the form the asking phone reads. Phone side: it sends the request once,
//! never holds or retries it, and says plainly when an answer that went out
//! never came back.

use std::sync::Arc;

pub use g7link_protocol::messages::{AgentSessionMove, AgentSessionMoved, reply_needs, session_move};
use g7link_protocol::{DeviceId, Envelope, ErrorCode, messages::types};

use crate::{
    Error, Node, Result,
    agent_reply::{refusal, send_once},
    agents::{self, AgentFailure, AgentReply, AgentRequest},
    dispatch::Responder,
    node::Shared,
};

/// Whether `t` is §9.1's request.
pub(crate) fn is_request(t: &str) -> bool {
    t == types::AGENT_SESSION_MOVE
}

/// PC: answers `agent.session.move` from `peer`.
pub(crate) async fn handle_request(
    shared: &Arc<Shared>,
    peer: DeviceId,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<()> {
    rsp.send(answer(shared, peer, env).await).await
}

async fn answer(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) -> Envelope {
    if let Some(refused) = refusal(shared, peer, session_move::CAP, "sessions can't be moved on this PC now")
    {
        return refused;
    }
    let Some(body) = agents::decode::<AgentSessionMove>(env, AgentSessionMove::is_valid) else {
        return Envelope::error(ErrorCode::BadMessage, "invalid agent.session.move");
    };
    let session = body.session.clone();
    let control = shared.control_with(&peer);
    let request = AgentRequest::SessionMove { session: body.session, confirm: body.confirm };
    match agents::ask_platform(shared, peer, request).await {
        Ok(AgentReply::Moved(moved)) => {
            let mut moved = moved.sanitized();
            if !moved.answers() {
                tracing::warn!(peer = %peer.short(), ?moved, "the host's move answer was malformed");
                return AgentFailure::Internal.reply("the host's answer was malformed");
            }
            moved.thread = moved.thread.map(|thread| agents::thread_for(thread, control));
            tracing::debug!(
                peer = %peer.short(),
                session = %session,
                result = %moved.result,
                reason = moved.reason.as_deref().unwrap_or(""),
                "move to G7"
            );
            Envelope::new(types::OK, &moved).unwrap_or_else(|_| AgentFailure::Internal.reply("failed"))
        }
        Ok(_) => AgentFailure::Internal.reply("the host answered something else"),
        Err(failure) => failure.reply(agents::failure_message(failure)),
    }
}

/// Phone: sends `body` to `peer` once; errors as `send_once`. A `moved`
/// whose thread can't be read is [`Error::Timeout`]: the agent may have
/// closed, but where the session went isn't known.
async fn ask(shared: &Arc<Shared>, peer: DeviceId, body: AgentSessionMove) -> Result<AgentSessionMoved> {
    if !body.is_valid() {
        return Err(agents::invalid(types::AGENT_SESSION_MOVE));
    }
    let env = Envelope::new(types::AGENT_SESSION_MOVE, &body)?;
    let reply = send_once(shared, peer, session_move::CAP, env).await?;
    match reply.expect_body::<AgentSessionMoved>(types::OK) {
        Ok(moved) if moved.is_valid() => moved.received().ok_or(Error::Timeout),
        _ => Err(Error::Timeout),
    }
}

/// The node's §9.1 methods (phone).
impl Node {
    /// Phone: has PC `peer` close `session` where it runs and continue it
    /// as a thread started by this phone; `confirm` is the `nonce` of an
    /// earlier `confirm` answer (§9's: a phone that confirmed replies into
    /// this session where it runs now isn't asked again). Sent once, never
    /// held or retried. Answered `moved` (with the thread), `closed` (the
    /// agent closed and the PC won't continue it yet: continue it with
    /// [`agent_session_continue`](Self::agent_session_continue) once the
    /// session says it can), `refused` (nothing was typed) or `failed`.
    /// `Unsupported`, without asking, while the PC doesn't offer
    /// `agent.move`; `Denied` while one of §9's toggles is off for this
    /// phone; `NotFound` for a session it doesn't list; a `Protocol` error
    /// starting `BUSY` while a reply or a move is being typed there;
    /// `Timeout` when the request went out and no usable answer came: it
    /// isn't known whether it was moved.
    pub async fn agent_session_move(
        &self,
        peer: DeviceId,
        session: String,
        confirm: Option<String>,
    ) -> Result<AgentSessionMoved> {
        ask(self.shared(), peer, AgentSessionMove::new(session, confirm)).await
    }

    /// Phone: whether paired PC `peer` offers `agent.move`, as it said
    /// when it last connected: what [`agent_session_move`](Self::agent_session_move)
    /// checks before it sends.
    pub fn agent_move_offered(&self, peer: DeviceId) -> bool {
        self.shared().peer_offers_cap(&peer, session_move::CAP)
    }

    /// Phone: what paired PC `peer` said, when it last connected, it needs
    /// before it can type replies: one of [`crate::session_reply::needs`].
    /// `None` while it offers `agent.reply`, or says nothing (an older PC).
    pub fn agent_reply_needs(&self, peer: DeviceId) -> Option<&'static str> {
        let caps = self.shared().peer_caps(&peer);
        reply_needs(|cap| caps.contains(cap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentReplyTarget, session_reply};

    const NONCE: &str = "fedcba9876543210fedcba9876543210";

    #[test]
    fn a_move_never_prints_its_nonce() {
        let request = AgentRequest::SessionMove { session: "sc0123".into(), confirm: Some(NONCE.into()) };
        let shown = format!("{request:?}");
        assert!(!shown.contains("fedcba") && shown.contains("sc0123"), "{shown}");
        assert!(shown.contains("confirm: true"), "{shown}");
        let reply = AgentReply::Moved(AgentSessionMoved::confirm(
            AgentReplyTarget {
                app: "Terminal".into(),
                agent: "Claude".into(),
                folder: "g7linux".into(),
                kind: session_reply::kind::TERMINAL.into(),
            },
            NONCE.into(),
        ));
        assert!(!format!("{reply:?}").contains("fedcba"), "{reply:?}");
    }

    #[test]
    fn only_the_move_is_this_modules() {
        assert!(is_request(types::AGENT_SESSION_MOVE));
        assert!(!is_request(types::AGENT_SESSION_REPLY));
        assert!(!is_request(types::AGENT_SESSION_CONTINUE));
        assert!(!crate::agent_reply::is_request(types::AGENT_SESSION_MOVE));
    }
}
