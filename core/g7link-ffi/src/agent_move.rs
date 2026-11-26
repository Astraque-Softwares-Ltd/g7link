// SPDX-License-Identifier: MPL-2.0
//! Move to G7 (`docs/protocol/agents.md` §9.1): a followed session's "Move
//! to G7". The PC closes the agent where it runs and, once nothing has the
//! session open, continues it as a thread started by this phone.
//!
//! The PC offers it as `agent.move` (`Unsupported`, without asking, while it
//! doesn't); §9's `agents`, `agent_sessions` and `agent_reply` toggles for
//! this phone govern it (`Denied`), and §9's confirmation is shared with
//! replies. Each call sends once, never holds a move while offline and never
//! goes over Bluetooth; only a `Refused` move may be sent again. No `Debug`
//! here prints a nonce.
//!
//! Also what a PC that can't type replies says it needs
//! ([`AgentReplyNeeds`]), so the phone can say why.

use g7link_core as core;

use crate::{AgentReplyTarget, AgentThread, G7LinkError, G7LinkNode, Result, parse_id};

/// What became of a move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentMoveResult {
    /// The agent closed and the session is a thread now: open `thread`.
    Moved,
    /// The agent closed; the PC won't continue it yet. Continue it with
    /// `agent_session_continue` once the session's `can_continue` says so
    /// (while its screen is open, for up to 2 minutes).
    Closed,
    /// Nothing was typed.
    Refused,
    /// It went wrong on the way; the reason says what was typed. A result
    /// this build doesn't know comes as this.
    Failed,
}

/// Why a move was refused, or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentMoveReason {
    // Refused: nothing was typed.
    /// "Unlock <PC> to move it."
    Locked,
    /// Someone is using <PC>. With `retry_ms`, the same move may be sent
    /// again after that long, for up to a minute, while the user sees it
    /// waiting; without it, waiting won't help.
    InUse,
    /// The first move or reply to this session where it runs now: show the
    /// target and ask; send again with the answer's `nonce` as `confirm`.
    Confirm,
    /// The agent waits for an answer on <PC>.
    NeedsYou,
    /// The agent is mid-turn: "Move it when it's done."
    Busy,
    /// The session ended: offer Continue here.
    Ended,
    NoProcess,
    NoWindow,
    Ambiguous,
    /// It's in a background tab G7 couldn't switch to.
    Tab,
    /// G7 couldn't make sure which terminal is the session's.
    Unproven,
    /// The session runs where G7 can't close it (an app, VS Code, ssh).
    Place,
    /// Too many at once: `retry_ms` when known.
    Rate,
    /// <PC> needs something set up first.
    Setup,
    /// The session's folder isn't one of <PC>'s agent folders.
    Folder,
    /// The agent can't run on <PC> now.
    Agent,
    // Failed.
    /// The window didn't come forward. Nothing was typed.
    Focus,
    /// The exit command was typed and not entered. Never retry.
    Interrupted,
    /// A key couldn't be pressed: the command may sit in the box. Show it
    /// as `Interrupted`.
    Input,
    /// Something else was in the agent's box: the exit command was added
    /// and not entered. Never retry.
    Draft,
    /// The exit command was entered and the agent didn't close in time.
    Open,
}

impl AgentMoveReason {
    fn from_wire(word: &str) -> Option<Self> {
        use core::session_move::reason;
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
            reason::UNPROVEN => Self::Unproven,
            reason::PLACE => Self::Place,
            reason::RATE => Self::Rate,
            reason::SETUP => Self::Setup,
            reason::FOLDER => Self::Folder,
            reason::AGENT => Self::Agent,
            reason::FOCUS => Self::Focus,
            reason::INTERRUPTED => Self::Interrupted,
            reason::INPUT => Self::Input,
            reason::DRAFT => Self::Draft,
            reason::OPEN => Self::Open,
            _ => return None,
        })
    }
}

/// A PC's answer to a move.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentMoveAnswer {
    pub result: AgentMoveResult,
    /// For `Refused` and `Failed`. Null there: "Couldn't move it" (a
    /// reason this build doesn't know).
    pub reason: Option<AgentMoveReason>,
    /// With `Confirm`; may come with others.
    pub target: Option<AgentReplyTarget>,
    /// With `Confirm` only: pass it back as `confirm`. Good once, for 2
    /// minutes.
    pub nonce: Option<String>,
    /// With `InUse` and `Rate`, when known: try again after this long (at
    /// most 60 s).
    pub retry_ms: Option<u32>,
    /// With `Moved`, always: the thread to open.
    pub thread: Option<AgentThread>,
}

/// Never prints the nonce.
impl std::fmt::Debug for AgentMoveAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentMoveAnswer")
            .field("result", &self.result)
            .field("reason", &self.reason)
            .field("target", &self.target)
            .field("nonce", &self.nonce.is_some())
            .field("retry_ms", &self.retry_ms)
            .field("thread", &self.thread)
            .finish()
    }
}

impl AgentMoveAnswer {
    /// From an answer the core already read; `None` for a `Moved` whose
    /// thread this build can't show.
    fn from_core(a: core::AgentSessionMoved) -> Option<Self> {
        use core::session_move::result;
        let result = match a.result.as_str() {
            result::MOVED => AgentMoveResult::Moved,
            result::CLOSED => AgentMoveResult::Closed,
            result::REFUSED => AgentMoveResult::Refused,
            _ => AgentMoveResult::Failed,
        };
        let thread = a.thread.and_then(AgentThread::from_core);
        if result == AgentMoveResult::Moved && thread.is_none() {
            return None;
        }
        Some(Self {
            result,
            reason: a.reason.as_deref().and_then(AgentMoveReason::from_wire),
            target: a.target.map(Into::into),
            nonce: a.nonce,
            retry_ms: a.retry_ms.map(|ms| u32::try_from(ms).unwrap_or(u32::MAX)),
            thread,
        })
    }
}

/// What a PC that can't type replies says it needs: "Can't reply to it from
/// here. Why?"
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentReplyNeeds {
    /// The G7 Linux Companion on its GNOME desktop is missing or too old.
    Extension,
    /// "Replying works on the GNOME desktop for now."
    Desktop,
    /// G7 Link can't press keys there: its installer sets that up.
    Keyboard,
}

impl AgentReplyNeeds {
    fn from_wire(cap: &str) -> Option<Self> {
        use core::session_reply::needs;
        Some(match cap {
            needs::EXTENSION => Self::Extension,
            needs::DESKTOP => Self::Desktop,
            needs::KEYBOARD => Self::Keyboard,
            _ => return None,
        })
    }
}

#[uniffi::export]
impl G7LinkNode {
    /// Has PC `id` close `session` where it runs (the agent's own exit
    /// command) and continue it here as a thread started by this phone.
    /// `confirm` is the `nonce` of an earlier `Confirm` answer, after the
    /// user said Move. Sent once; never held while offline, never retried.
    /// `Moved` carries the thread to open; `Closed`: continue it once the
    /// session says it can. Errors: `Unsupported`, without asking, while
    /// the PC doesn't offer moves (`agent_move_offered`); `Denied` while one
    /// of the PC's reply toggles is off for this phone; `NotFound`: the PC
    /// doesn't list the session; `Internal` whose reason starts with
    /// `BUSY`: a reply or a move is being typed on the PC, nothing was
    /// typed; `Timeout` when the request went out and no usable answer
    /// came: "Not known if it was moved"; `Offline`: nothing was sent.
    pub async fn agent_session_move(
        &self,
        id: String,
        session: String,
        confirm: Option<String>,
    ) -> Result<AgentMoveAnswer> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let answer = self.run(async move { node.agent_session_move(id, session, confirm).await }).await?;
        AgentMoveAnswer::from_core(answer).ok_or(G7LinkError::Timeout)
    }

    /// Whether PC `id` offered moving its sessions here (`agent.move`) when
    /// it last connected. Show "Move to G7" only while it did.
    pub fn agent_move_offered(&self, id: String) -> bool {
        parse_id(&id).is_ok_and(|id| self.node.agent_move_offered(id))
    }

    /// Why PC `id` can't type replies, as it said when it last connected;
    /// null while it can, or when it didn't say (an older PC): then show
    /// nothing.
    pub fn agent_reply_needs(&self, id: String) -> Option<AgentReplyNeeds> {
        let id = parse_id(&id).ok()?;
        self.node.agent_reply_needs(id).and_then(AgentReplyNeeds::from_wire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use g7link_core::{
        agent,
        session_move::{reason, result},
        session_reply::{kind, needs},
    };

    fn target() -> core::AgentReplyTarget {
        core::AgentReplyTarget {
            app: "Terminal".into(),
            agent: "Claude".into(),
            folder: "g7linux".into(),
            kind: kind::TERMINAL.into(),
        }
    }

    fn thread() -> core::AgentThread {
        core::AgentThread {
            id: "th0123".into(),
            agent: "claude".into(),
            folder: "g7linux".into(),
            title: "Fix the build".into(),
            state: agent::state::DONE.into(),
            mode: agent::mode::ASK.into(),
            from_phone: true,
            updated: 1_700_000_000_000,
            ..Default::default()
        }
    }

    #[test]
    fn every_reason_is_a_wire_word() {
        for word in reason::REFUSED.iter().chain(reason::FAILED) {
            assert!(AgentMoveReason::from_wire(word).is_some(), "{word}");
        }
        assert_eq!(AgentMoveReason::from_wire("too_sunny"), None);
        assert_eq!(AgentMoveReason::from_wire("text"), None, "a reply's only");
        assert_eq!(result::ALL.len(), 4);
    }

    #[test]
    fn an_answer_crosses_as_it_came() {
        let moved = AgentMoveAnswer::from_core(core::AgentSessionMoved::moved(thread())).unwrap();
        assert_eq!((moved.result, moved.reason), (AgentMoveResult::Moved, None));
        let opened = moved.thread.expect("the thread to open");
        assert_eq!((opened.id.as_str(), opened.from_phone), ("th0123", true));

        let closed = AgentMoveAnswer::from_core(core::AgentSessionMoved::closed()).unwrap();
        assert_eq!((closed.result, closed.reason, closed.thread), (AgentMoveResult::Closed, None, None));

        let confirm =
            AgentMoveAnswer::from_core(core::AgentSessionMoved::confirm(target(), "ab".repeat(16))).unwrap();
        assert_eq!(
            (confirm.result, confirm.reason),
            (AgentMoveResult::Refused, Some(AgentMoveReason::Confirm))
        );
        assert_eq!(confirm.target.as_ref().and_then(|t| t.kind), Some(crate::AgentReplyKind::Terminal));
        assert_eq!(confirm.nonce.as_deref(), Some("ab".repeat(16).as_str()));
        assert!(!format!("{confirm:?}").contains("abab"), "{confirm:?}");

        let in_use =
            AgentMoveAnswer::from_core(core::AgentSessionMoved::refused(reason::IN_USE).retry_after(4_000))
                .unwrap();
        assert_eq!((in_use.reason, in_use.retry_ms), (Some(AgentMoveReason::InUse), Some(4_000)));
        for (word, expected) in [
            (reason::DRAFT, AgentMoveReason::Draft),
            (reason::OPEN, AgentMoveReason::Open),
            (reason::INPUT, AgentMoveReason::Input),
        ] {
            let failed = AgentMoveAnswer::from_core(core::AgentSessionMoved::failed(word)).unwrap();
            assert_eq!((failed.result, failed.reason), (AgentMoveResult::Failed, Some(expected)));
        }
        let folder = AgentMoveAnswer::from_core(core::AgentSessionMoved::refused(reason::FOLDER)).unwrap();
        assert_eq!(folder.reason, Some(AgentMoveReason::Folder));
        let later = core::AgentSessionMoved { result: "parked".into(), ..Default::default() };
        assert_eq!(AgentMoveAnswer::from_core(later).unwrap().result, AgentMoveResult::Failed);
    }

    #[test]
    fn moved_without_a_thread_to_show_isnt_known() {
        let bare = core::AgentSessionMoved { thread: None, ..core::AgentSessionMoved::moved(thread()) };
        assert_eq!(AgentMoveAnswer::from_core(bare), None);
        let unknown_state =
            core::AgentSessionMoved::moved(core::AgentThread { state: "napping".into(), ..thread() });
        assert_eq!(AgentMoveAnswer::from_core(unknown_state), None);
    }

    #[test]
    fn every_need_is_a_capability_word() {
        for cap in needs::ALL {
            assert!(AgentReplyNeeds::from_wire(cap).is_some(), "{cap}");
        }
        assert_eq!(AgentReplyNeeds::from_wire(needs::DESKTOP), Some(AgentReplyNeeds::Desktop));
        assert_eq!(AgentReplyNeeds::from_wire(needs::EXTENSION), Some(AgentReplyNeeds::Extension));
        assert_eq!(AgentReplyNeeds::from_wire(needs::KEYBOARD), Some(AgentReplyNeeds::Keyboard));
        assert_eq!(AgentReplyNeeds::from_wire(core::session_reply::CAP), None);
        assert_eq!(AgentReplyNeeds::from_wire("agent.reply.needs.wings"), None);
    }
}
