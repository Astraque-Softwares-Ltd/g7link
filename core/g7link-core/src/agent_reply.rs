// SPDX-License-Identifier: MPL-2.0
//! Reply on the PC (`docs/protocol/agents.md` §9): a phone enters a reply
//! into a Claude or Codex session started on the PC outside G7 Link, by
//! having the PC paste it into the session's own window and press Enter.
//!
//! PC side: the core checks that this PC offers `agent.reply`, then the
//! `agents`, `agent_sessions` and `agent_reply` toggles (the first refusal
//! of each shows the PC's user a one-time prompt for that phone), then the
//! request's limits, and only then asks the platform
//! ([`AgentRequest::SessionReply`]). The platform does the rest: the rate,
//! the confirmation, finding and proving the window, and the keys. The core
//! passes its answer on only when it's a whole answer to that request, made
//! to fit. Phone side: it sends the request once, never holds or retries
//! it, and says plainly when an answer that went out never came back.
//!
//! The text is never logged and never printed by a `Debug`.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

pub use g7link_protocol::messages::{
    AgentReplyTarget, AgentSessionReplied, AgentSessionReply, is_reply_nonce, is_reply_text, session_reply,
};
use g7link_protocol::{
    DeviceId, Envelope, ErrorCode,
    messages::{ErrorBody, clean_typed_text, types},
};

use crate::{
    Error, Node, Result,
    agents::{self, AgentFailure, AgentReply, AgentRequest},
    dispatch::Responder,
    events::NodeEvent,
    node::Shared,
};

/// How long a phone waits for a PC's answer: longer than the PC gives its
/// platform, so the PC's own answer arrives first.
const REPLY_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

// ---- PC side ----

/// PC: the phones that already caused the one-time `agent_reply` prompt
/// this run.
#[derive(Debug, Default)]
pub(crate) struct Replies {
    prompted: Mutex<HashSet<DeviceId>>,
}

impl Replies {
    /// Whether this is `peer`'s first refusal for the toggle this run.
    fn first_refusal(&self, peer: DeviceId) -> bool {
        self.prompted.lock().unwrap_or_else(|e| e.into_inner()).insert(peer)
    }

    /// `peer` was unpaired.
    pub(crate) fn forget(&self, peer: &DeviceId) {
        self.prompted.lock().unwrap_or_else(|e| e.into_inner()).remove(peer);
    }
}

/// Whether `t` is §9's request.
pub(crate) fn is_request(t: &str) -> bool {
    t == types::AGENT_SESSION_REPLY
}

/// PC: answers `agent.session.reply` from `peer`.
pub(crate) async fn handle_request(
    shared: &Arc<Shared>,
    peer: DeviceId,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<()> {
    rsp.send(answer(shared, peer, env).await).await
}

/// PC: the refusal of a reply, or of a move (§9.1), from `peer` before its
/// body is read: this PC doesn't offer `cap` now (`unsupported` says what),
/// or one of §9's toggles is off. Both share `agent_reply`'s one-time
/// prompt.
pub(crate) fn refusal(
    shared: &Arc<Shared>,
    peer: DeviceId,
    cap: &str,
    unsupported: &str,
) -> Option<Envelope> {
    if !shared.we_offer(cap) {
        return Some(Envelope::error(ErrorCode::Unsupported, unsupported));
    }
    if let Some(refused) = agents::refusal(shared, peer, true) {
        return Some(refused);
    }
    if !shared.toggle_on(&peer, session_reply::TOGGLE) {
        if shared.agents.reply.first_refusal(peer) {
            shared.emit(NodeEvent::AgentReplyRequested { device: peer });
        }
        return Some(Envelope::error(ErrorCode::Denied, "agent_reply is off for this phone"));
    }
    None
}

async fn answer(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) -> Envelope {
    if let Some(refused) =
        refusal(shared, peer, session_reply::CAP, "replies can't be entered on this PC now")
    {
        return refused;
    }
    let Some(body) = agents::decode::<AgentSessionReply>(env, AgentSessionReply::is_valid) else {
        return Envelope::error(ErrorCode::BadMessage, "invalid agent.session.reply");
    };
    let check = body.check;
    let session = body.session.clone();
    let request =
        AgentRequest::SessionReply { session: body.session, text: body.text, confirm: body.confirm };
    match agents::ask_platform(shared, peer, request).await {
        Ok(AgentReply::Replied(replied)) => {
            let replied = replied.sanitized();
            if !replied.answers(check) {
                tracing::warn!(peer = %peer.short(), ?replied, "the host's reply answer was malformed");
                return AgentFailure::Internal.reply("the host's answer was malformed");
            }
            tracing::debug!(
                peer = %peer.short(),
                session = %session,
                check,
                result = %replied.result,
                reason = replied.reason.as_deref().unwrap_or(""),
                "reply on the PC"
            );
            Envelope::new(types::OK, &replied).unwrap_or_else(|_| AgentFailure::Internal.reply("failed"))
        }
        Ok(_) => AgentFailure::Internal.reply("the host answered something else"),
        Err(failure) => failure.reply(agents::failure_message(failure)),
    }
}

// ---- Phone side ----

/// Phone: sends `body` to `peer` once. An answer is definite; an error
/// after the request went out without a usable answer (the link dropped,
/// no answer in time, an answer out of its limits, or the PC's own
/// `INTERNAL`) is [`Error::Timeout`]: whether it was typed isn't known.
async fn ask(shared: &Arc<Shared>, peer: DeviceId, body: AgentSessionReply) -> Result<AgentSessionReplied> {
    if !body.is_valid() {
        return Err(agents::invalid(types::AGENT_SESSION_REPLY));
    }
    let check = body.check;
    let env = Envelope::new(types::AGENT_SESSION_REPLY, &body)?;
    let reply = send_once(shared, peer, session_reply::CAP, env).await?;
    match reply.expect_body::<AgentSessionReplied>(types::OK) {
        Ok(replied) if replied.is_valid() => Ok(replied.for_request(check)),
        _ => Err(Error::Timeout),
    }
}

/// Phone: sends `env`, a request that types on `peer` (§9, §9.1), once,
/// and gives back the PC's `ok` unread. `Unsupported`, without sending,
/// while `peer` doesn't offer `cap`. An error after it went out with no
/// usable answer, the PC's `INTERNAL` included, is [`Error::Timeout`]; the
/// PC's `BUSY` (another one is being typed there) is a `Protocol` error
/// starting `BUSY`; a request the PC turned away unread is `Busy`.
pub(crate) async fn send_once(
    shared: &Arc<Shared>,
    peer: DeviceId,
    cap: &str,
    env: Envelope,
) -> Result<Envelope> {
    let session = shared.session(&peer).ok_or(Error::Offline)?;
    if !shared.peer_offers_cap(&peer, cap) {
        return Err(Error::Unsupported);
    }
    let (answer, sent) = session.request_tracked(env, REPLY_WAIT).await;
    let reply = match answer {
        Ok(reply) => reply,
        // The PC turned it away unread (too many requests in flight).
        Err(Error::Busy) => return Err(Error::Busy),
        Err(_) if sent => return Err(Error::Timeout),
        Err(e) => return Err(e),
    };
    if reply.t == types::ERROR {
        let error = reply.body::<ErrorBody>().map_err(|_| Error::Timeout)?;
        return Err(match error.code {
            ErrorCode::Internal => Error::Timeout,
            ErrorCode::Busy => Error::Protocol(format!("BUSY: {}", error.msg.unwrap_or_default())),
            code => {
                g7link_protocol::ProtocolError::Remote { code, msg: error.msg.unwrap_or_default() }.into()
            }
        });
    }
    Ok(reply)
}

/// The node's §9 methods (phone).
impl Node {
    /// Phone: whether PC `peer` could enter a reply into `session` now.
    /// Enters nothing. Answered `ready` (with the target) or `refused`
    /// (with a reason). `Unsupported`, without asking, while the PC
    /// doesn't offer `agent.reply`; `Denied` while one of its toggles is
    /// off for this phone; `NotFound` for a session it doesn't list.
    pub async fn agent_session_reply_check(
        &self,
        peer: DeviceId,
        session: String,
    ) -> Result<AgentSessionReplied> {
        ask(self.shared(), peer, AgentSessionReply::check(session)).await
    }

    /// Phone: has PC `peer` enter `text` into `session`'s window and press
    /// Enter; `confirm` is the `nonce` of an earlier `confirm` answer. The
    /// text is cleaned here (line ends made `\n`, other control characters
    /// dropped) and must then be 1 to 8,192 bytes and not blank. Sent once,
    /// never held or retried. Answered `sent`, `refused` (nothing typed) or
    /// `failed`. Errors as [`agent_session_reply_check`](Self::agent_session_reply_check),
    /// and `Timeout` when the request went out and no usable answer came:
    /// it isn't known whether it was typed.
    pub async fn agent_session_reply(
        &self,
        peer: DeviceId,
        session: String,
        text: String,
        confirm: Option<String>,
    ) -> Result<AgentSessionReplied> {
        let text = clean_typed_text(&text);
        ask(self.shared(), peer, AgentSessionReply::text(session, text, confirm)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_request_never_prints_its_text_or_nonce() {
        let request = AgentRequest::SessionReply {
            session: "sc0123".into(),
            text: Some("secret plan".into()),
            confirm: Some("fedcba9876543210fedcba9876543210".into()),
        };
        let shown = format!("{request:?}");
        assert!(!shown.contains("secret") && !shown.contains("fedcba"), "{shown}");
        assert!(shown.contains("sc0123") && shown.contains("11"), "{shown}");
        let check = AgentRequest::SessionReply { session: "sc0123".into(), text: None, confirm: None };
        assert!(format!("{check:?}").contains("check: true"), "{check:?}");
        let reply = AgentReply::Replied(AgentSessionReplied::confirm(
            AgentReplyTarget {
                app: "Terminal".into(),
                agent: "Claude".into(),
                folder: "g7linux".into(),
                kind: session_reply::kind::TERMINAL.into(),
            },
            "fedcba9876543210fedcba9876543210".into(),
        ));
        assert!(!format!("{reply:?}").contains("fedcba"), "{reply:?}");
    }

    #[test]
    fn the_prompt_shows_once_per_phone() {
        let replies = Replies::default();
        let (a, b) = (DeviceId([1; 32]), DeviceId([2; 32]));
        assert!(replies.first_refusal(a));
        assert!(!replies.first_refusal(a));
        assert!(replies.first_refusal(b));
        replies.forget(&a);
        assert!(replies.first_refusal(a), "unpaired and paired again");
    }

    #[test]
    fn only_the_reply_is_this_modules() {
        assert!(is_request(types::AGENT_SESSION_REPLY));
        assert!(!is_request(types::AGENT_REPLY), "§5's answer to a question");
        assert!(!is_request(types::AGENT_SESSION_CONTINUE));
    }
}
