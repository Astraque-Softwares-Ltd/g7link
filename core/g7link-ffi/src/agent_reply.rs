// SPDX-License-Identifier: MPL-2.0
//! Reply on the PC (`docs/protocol/agents.md` §9): a followed session's
//! reply box. The PC brings the session's own window forward, proves it
//! shows that session, pastes the message and presses Enter; the message
//! then arrives as the session's next item, as everything else does.
//!
//! The PC offers it as `agent.reply` (`Unsupported`, without asking, while
//! it doesn't); its `agents`, `agent_sessions` and `agent_reply` toggles
//! for this phone govern it (`Denied`). A reply is sent once, never held
//! or retried, and never over Bluetooth. No `Debug` here prints a message
//! or a nonce.

use g7link_core as core;

use crate::{G7LinkNode, Result, parse_id};

/// What became of a check or a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentReplyResult {
    /// A check: a reply could be entered now. Show the box.
    Ready,
    /// Enter was pressed in the session's window. It isn't known yet that
    /// the agent took it: its item arrives when it does.
    Sent,
    /// Nothing was typed. Keep the draft.
    Refused,
    /// It went wrong on the way; the reason says whether text was pasted.
    /// A result this build doesn't know comes as this.
    Failed,
}

/// Why a reply was refused, or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentReplyReason {
    // Refused: nothing was typed.
    /// "Unlock <PC> to reply".
    Locked,
    /// "<PC> is in use. Try again in a moment." (`retry_ms` when known).
    InUse,
    /// The first reply to this window: show the target and ask; send again
    /// with the answer's `nonce` as `confirm`.
    Confirm,
    /// "It's waiting for an answer on <PC>."
    NeedsYou,
    /// "Codex is mid-turn. Send when it's done."
    Busy,
    /// The session ended: offer Continue here.
    Ended,
    /// "G7 can't find this session's window."
    NoProcess,
    NoWindow,
    /// "G7 can't tell which window it's in."
    Ambiguous,
    /// "It's in a background tab. Bring it forward on <PC>."
    Tab,
    /// The session runs where G7 can't type (an app, VS Code, ssh).
    Place,
    /// "Something is copied on <PC> that G7 can't put back."
    Clipboard,
    /// It starts with `!` or `/`, which the agent reads as a command.
    Text,
    /// Too many replies: `retry_ms` when known.
    Rate,
    /// "Replying needs the G7 Linux Companion 1.4 on <PC>."
    Setup,
    // Failed.
    /// "The window didn't come forward. Nothing was typed."
    Focus,
    /// "Your message was typed on <PC> but not sent. Check the window."
    /// Never retry: it would paste twice.
    Interrupted,
    /// A key couldn't be pressed: the paste may have landed. Show it as
    /// `Interrupted`.
    Input,
}

impl AgentReplyReason {
    fn from_wire(word: &str) -> Option<Self> {
        use core::session_reply::reason;
        Some(match word {
            reason::LOCKED => Self::Locked,
            reason::IN_USE => Self::InUse,
            reason::CONFIRM => Self::Confirm,
            reason::NEEDS_YOU => Self::NeedsYou,
            reason::BUSY => Self::Busy,
            reason::ENDED => Self::Ended,
            reason::NO_PROCESS => Self::NoProcess,
            reason::NO_WINDOW => Self::NoWindow,
            reason::AMBIGUOUS => Self::Ambiguous,
            reason::TAB => Self::Tab,
            reason::PLACE => Self::Place,
            reason::CLIPBOARD => Self::Clipboard,
            reason::TEXT => Self::Text,
            reason::RATE => Self::Rate,
            reason::SETUP => Self::Setup,
            reason::FOCUS => Self::Focus,
            reason::INTERRUPTED => Self::Interrupted,
            reason::INPUT => Self::Input,
            _ => return None,
        })
    }
}

/// Where the reply goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentReplyKind {
    /// A terminal window: the terminal icon.
    Terminal,
    /// The agent's own app: its icon.
    App,
}

/// Where a reply goes, for the target line: "Terminal · Claude · g7linux".
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentReplyTarget {
    /// The app's name, from its desktop entry.
    pub app: String,
    /// The agent's name.
    pub agent: String,
    /// The folder's name, never a path.
    pub folder: String,
    /// Null for a kind this build doesn't know: a plain icon.
    pub kind: Option<AgentReplyKind>,
}

impl From<core::AgentReplyTarget> for AgentReplyTarget {
    fn from(t: core::AgentReplyTarget) -> Self {
        use core::session_reply::kind;
        let kind = match t.kind.as_str() {
            kind::TERMINAL => Some(AgentReplyKind::Terminal),
            kind::APP => Some(AgentReplyKind::App),
            _ => None,
        };
        Self { app: t.app, agent: t.agent, folder: t.folder, kind }
    }
}

/// A PC's answer to a check or a reply.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentReplyAnswer {
    pub result: AgentReplyResult,
    /// For `Refused` and `Failed`. Null there: "Couldn't reply" (a reason
    /// this build doesn't know).
    pub reason: Option<AgentReplyReason>,
    /// On `Ready`, `Sent` and `Confirm`; may come with others.
    pub target: Option<AgentReplyTarget>,
    /// With `Confirm` only: pass it back as `confirm`. Good once, for 2
    /// minutes.
    pub nonce: Option<String>,
    /// With `InUse` and `Rate`, when known: try again after this long (at
    /// most 60 s).
    pub retry_ms: Option<u32>,
}

/// Never prints the nonce.
impl std::fmt::Debug for AgentReplyAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentReplyAnswer")
            .field("result", &self.result)
            .field("reason", &self.reason)
            .field("target", &self.target)
            .field("nonce", &self.nonce.is_some())
            .field("retry_ms", &self.retry_ms)
            .finish()
    }
}

impl From<core::AgentSessionReplied> for AgentReplyAnswer {
    /// From an answer the core already fitted to its request.
    fn from(a: core::AgentSessionReplied) -> Self {
        use core::session_reply::result;
        let result = match a.result.as_str() {
            result::READY => AgentReplyResult::Ready,
            result::SENT => AgentReplyResult::Sent,
            result::REFUSED => AgentReplyResult::Refused,
            _ => AgentReplyResult::Failed,
        };
        Self {
            result,
            reason: a.reason.as_deref().and_then(AgentReplyReason::from_wire),
            target: a.target.map(Into::into),
            nonce: a.nonce,
            retry_ms: a.retry_ms.map(|ms| u32::try_from(ms).unwrap_or(u32::MAX)),
        }
    }
}

#[uniffi::export]
impl G7LinkNode {
    /// Whether PC `id` could enter a reply into `session` now; enters
    /// nothing. Call it when the session's screen opens, when its state
    /// changes, and when the reply box gains focus. `Ready` (with the
    /// target) or `Refused` (with a reason). `Unsupported`, without asking,
    /// while the PC doesn't offer replies: show no box at all. `Denied`
    /// while one of the PC's toggles is off for this phone (the first time,
    /// the PC asks its user). `NotFound`: the PC doesn't list the session.
    pub async fn agent_session_reply_check(&self, id: String, session: String) -> Result<AgentReplyAnswer> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let answer = self.run(async move { node.agent_session_reply_check(id, session).await }).await?;
        Ok(answer.into())
    }

    /// Has PC `id` enter `text` into `session`'s window and press Enter;
    /// `confirm` is the `nonce` of an earlier `Confirm` answer, after the
    /// user said Send. The text is cleaned here and must be 1 to 8,192
    /// bytes, not blank. Sent once; never held while offline, never
    /// retried. Errors as `agent_session_reply_check`, and:
    /// `Timeout` when the request went out and no usable answer came:
    /// "Not known if it was sent" until the session's next item settles
    /// it. `Internal` whose reason starts with `BUSY`: another reply is
    /// being entered on the PC; nothing was typed. `Offline`: nothing was
    /// sent.
    pub async fn agent_session_reply(
        &self,
        id: String,
        session: String,
        text: String,
        confirm: Option<String>,
    ) -> Result<AgentReplyAnswer> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let answer =
            self.run(async move { node.agent_session_reply(id, session, text, confirm).await }).await?;
        Ok(answer.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use g7link_core::session_reply::{kind, reason, result};

    fn target(kind: &str) -> core::AgentReplyTarget {
        core::AgentReplyTarget {
            app: "Terminal".into(),
            agent: "Claude".into(),
            folder: "g7linux".into(),
            kind: kind.into(),
        }
    }

    #[test]
    fn every_reason_is_a_wire_word() {
        for word in reason::REFUSED.iter().chain(reason::FAILED) {
            assert!(AgentReplyReason::from_wire(word).is_some(), "{word}");
        }
        assert_eq!(AgentReplyReason::from_wire("too_sunny"), None);
    }

    #[test]
    fn an_answer_crosses_as_it_came() {
        let confirm = core::AgentSessionReplied::confirm(target(kind::TERMINAL), "ab".repeat(16));
        let crossed = AgentReplyAnswer::from(confirm);
        assert_eq!(
            (crossed.result, crossed.reason),
            (AgentReplyResult::Refused, Some(AgentReplyReason::Confirm))
        );
        assert_eq!(crossed.target.as_ref().and_then(|t| t.kind), Some(AgentReplyKind::Terminal));
        assert_eq!(crossed.nonce.as_deref(), Some("ab".repeat(16).as_str()));
        assert!(!format!("{crossed:?}").contains("abab"), "{crossed:?}");

        let rate =
            AgentReplyAnswer::from(core::AgentSessionReplied::refused(reason::RATE).retry_after(5_000));
        assert_eq!((rate.reason, rate.retry_ms), (Some(AgentReplyReason::Rate), Some(5_000)));
        let later = core::AgentSessionReplied {
            result: "queued".into(),
            ..core::AgentSessionReplied::sent(target("tmux"))
        };
        let crossed = AgentReplyAnswer::from(later);
        assert_eq!(crossed.result, AgentReplyResult::Failed);
        assert_eq!(crossed.target.and_then(|t| t.kind), None, "a plain icon");
        let sent = AgentReplyAnswer::from(core::AgentSessionReplied::sent(target(kind::APP)));
        assert_eq!(sent.result, AgentReplyResult::Sent);
        assert_eq!(sent.target.and_then(|t| t.kind), Some(AgentReplyKind::App));
        assert_eq!(
            AgentReplyAnswer::from(core::AgentSessionReplied::failed(reason::INPUT)).reason,
            Some(AgentReplyReason::Input)
        );
        assert_eq!(result::ALL.len(), 4);
    }
}
