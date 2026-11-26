// SPDX-License-Identifier: MPL-2.0
//! Bodies of moving a session to G7 (`docs/protocol/agents.md` §9.1): a
//! phone asks a PC to close a session started there outside G7 Link and,
//! once nothing has it open, continue it as a thread started by that phone.
//! The PC offers it as `agent.move`; §9's toggles (`agents`,
//! `agent_sessions`, `agent_reply`) and §9's confirmation govern it.
//!
//! A confirmation's nonce never appears in a `Debug`. A receiver reads a
//! `result` it doesn't know as `failed` and a `reason` it doesn't know as
//! absent.

use serde::{Deserialize, Serialize};

use super::{
    AgentReplyTarget, AgentThread, agent_reply::is_reply_word, is_reply_nonce, is_valid_agent_id,
    session_reply,
};

/// The capability, the limits and the wire words of §9.1.
pub mod session_move {
    use crate::messages::session_reply;

    /// Desktop: can move a session started on this PC to G7 now. Offered
    /// only beside `agent.reply`, while the agent host can say what would
    /// stop the continue.
    pub const CAP: &str = "agent.move";
    /// The toggle is §9's: a move is a reply's sibling.
    pub const TOGGLE: &str = session_reply::TOGGLE;
    /// `confirm` and `nonce`: lowercase hex digits, as §9's.
    pub const NONCE_BYTES: usize = session_reply::NONCE_BYTES;
    /// `result` and `reason`: 1 to this many of `[a-z_]`.
    pub const MAX_WORD_BYTES: usize = session_reply::MAX_WORD_BYTES;
    /// `retry_ms`.
    pub const MAX_RETRY_MS: u64 = session_reply::MAX_RETRY_MS;

    /// `result`.
    pub mod result {
        pub use crate::messages::session_reply::result::{FAILED, REFUSED};
        /// The agent closed and the session is a thread now (`thread`); it
        /// left the PC's list.
        pub const MOVED: &str = "moved";
        /// The agent closed; the PC won't continue it yet (Codex's last
        /// write is too recent, or the host refused). The phone continues
        /// it with `agent.session.continue` once the list says it can.
        pub const CLOSED: &str = "closed";
        pub const ALL: &[&str] = &[MOVED, CLOSED, REFUSED, FAILED];
    }

    /// `reason`.
    pub mod reason {
        pub use crate::messages::session_reply::reason::{
            AMBIGUOUS, BUSY, CONFIRM, ENDED, FOCUS, IN_USE, INPUT, INTERRUPTED, LOCKED, NEEDS_YOU,
            NO_PROCESS, NO_WINDOW, PLACE, RATE, SETUP, TAB, UNPROVEN,
        };
        /// The session's folder isn't one of the PC's agent folders.
        pub const FOLDER: &str = "folder";
        /// The agent can't run on the PC now.
        pub const AGENT: &str = "agent";
        /// The exit command was pasted and not entered: the agent's box
        /// held something else, or couldn't be read.
        pub const DRAFT: &str = "draft";
        /// The exit command was entered and the agent didn't close in time.
        pub const OPEN: &str = "open";
        /// Those of `refused`: nothing was typed.
        pub const REFUSED: &[&str] = &[
            LOCKED, IN_USE, CONFIRM, NEEDS_YOU, BUSY, ENDED, NO_PROCESS, NO_WINDOW, AMBIGUOUS, TAB, UNPROVEN,
            PLACE, RATE, SETUP, FOLDER, AGENT,
        ];
        /// Those of `failed`.
        pub const FAILED: &[&str] = &[FOCUS, INTERRUPTED, INPUT, DRAFT, OPEN];
        /// Those `retry_ms` goes with.
        pub const RETRY: &[&str] = &[RATE, IN_USE];
    }
}

use session_move::{reason, result};

/// Body of `agent.session.move` (phone → PC, RPC `agent/session.move`):
/// close `session` where it runs and continue it here. `confirm` is the
/// nonce of an earlier `confirm` answer (§9's, shared with replies).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentSessionMove {
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
}

/// Never prints the nonce.
impl std::fmt::Debug for AgentSessionMove {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSessionMove")
            .field("session", &self.session)
            .field("confirm", &self.confirm.is_some())
            .finish()
    }
}

impl AgentSessionMove {
    pub fn new(session: String, confirm: Option<String>) -> Self {
        Self { session, confirm }
    }

    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.session) && self.confirm.as_deref().is_none_or(is_reply_nonce)
    }
}

/// The `ok` answer of `agent.session.move`: `result` (one of
/// [`session_move::result`]), and for `refused` and `failed` its `reason`.
/// `moved` comes with `thread`; `confirm` with `target` and `nonce`; `rate`
/// and `in_use` with `retry_ms` when it's known.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentSessionMoved {
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AgentReplyTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<AgentThread>,
}

/// Never prints the nonce (nor, through [`AgentThread`]'s own, a title).
impl std::fmt::Debug for AgentSessionMoved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSessionMoved")
            .field("result", &self.result)
            .field("reason", &self.reason)
            .field("kind", &self.target.as_ref().map(|t| t.kind.as_str()))
            .field("nonce", &self.nonce.is_some())
            .field("retry_ms", &self.retry_ms)
            .field("thread", &self.thread.as_ref().map(|t| t.id.as_str()))
            .finish()
    }
}

impl AgentSessionMoved {
    fn of(result: &str, reason: Option<&str>) -> Self {
        Self { result: result.to_owned(), reason: reason.map(str::to_owned), ..Default::default() }
    }

    /// The agent closed and the session is `thread` now.
    pub fn moved(thread: AgentThread) -> Self {
        Self { thread: Some(thread), ..Self::of(result::MOVED, None) }
    }

    /// The agent closed; the session isn't continued yet.
    pub fn closed() -> Self {
        Self::of(result::CLOSED, None)
    }

    /// Nothing typed: show `target` and send again with `confirm: nonce`.
    pub fn confirm(target: AgentReplyTarget, nonce: String) -> Self {
        Self { target: Some(target), nonce: Some(nonce), ..Self::of(result::REFUSED, Some(reason::CONFIRM)) }
    }

    /// Nothing typed, for `reason` (one of [`session_move::reason::REFUSED`]).
    pub fn refused(reason: &str) -> Self {
        Self::of(result::REFUSED, Some(reason))
    }

    /// It went wrong, for `reason` (one of [`session_move::reason::FAILED`]).
    pub fn failed(reason: &str) -> Self {
        Self::of(result::FAILED, Some(reason))
    }

    /// With the place it was about.
    pub fn with_target(mut self, target: AgentReplyTarget) -> Self {
        self.target = Some(target);
        self
    }

    /// With when to try again (for `rate` and `in_use`).
    pub fn retry_after(mut self, ms: u64) -> Self {
        self.retry_ms = Some(ms);
        self
    }

    /// Within its limits as received: words this build may not know are
    /// still words.
    pub fn is_valid(&self) -> bool {
        is_reply_word(&self.result)
            && self.reason.as_deref().is_none_or(is_reply_word)
            && self.nonce.as_ref().is_none_or(|n| n.len() <= session_move::NONCE_BYTES)
            && self.target.as_ref().is_none_or(AgentReplyTarget::is_valid)
            && self.thread.as_ref().is_none_or(AgentThread::is_valid)
    }

    /// Made to fit, on either side: a `result` this build doesn't know is
    /// `failed`; a `reason` it doesn't know for its result, or on `moved`
    /// or `closed`, is dropped; `nonce` stays only with a `confirm` (and
    /// when it's one), `retry_ms` only with `rate` or `in_use`, at most
    /// 60 s; `thread` only with `moved`, cleaned; `target` is cleaned, and
    /// dropped when it can't be.
    pub fn sanitized(mut self) -> Self {
        if !result::ALL.contains(&self.result.as_str()) {
            self.result = result::FAILED.to_owned();
        }
        let known: &[&str] = match self.result.as_str() {
            result::REFUSED => reason::REFUSED,
            result::FAILED => reason::FAILED,
            _ => &[],
        };
        if !self.reason.as_deref().is_some_and(|r| known.contains(&r)) {
            self.reason = None;
        }
        let reason = self.reason.as_deref();
        if reason != Some(reason::CONFIRM) || !self.nonce.as_deref().is_some_and(is_reply_nonce) {
            self.nonce = None;
        }
        self.retry_ms = match reason {
            Some(r) if reason::RETRY.contains(&r) => {
                self.retry_ms.map(|ms| ms.min(session_move::MAX_RETRY_MS))
            }
            _ => None,
        };
        self.thread = match self.result.as_str() {
            result::MOVED => self.thread.and_then(AgentThread::sanitized),
            _ => None,
        };
        self.target = self.target.and_then(AgentReplyTarget::sanitized);
        self
    }

    /// As a phone takes it: sanitized, and a `confirm` without its nonce or
    /// target loses its reason. `None` for a `moved` without a thread it
    /// can read: the agent may have closed, but where the session went
    /// isn't known.
    pub fn received(self) -> Option<Self> {
        let mut answer = self.sanitized();
        if answer.result == result::MOVED && answer.thread.is_none() {
            return None;
        }
        if answer.reason.as_deref() == Some(reason::CONFIRM)
            && (answer.nonce.is_none() || answer.target.is_none())
        {
            answer.reason = None;
        }
        Some(answer)
    }

    /// Whether, sanitized, it is a whole answer, as a PC sends it: a result
    /// this build knows, a known reason exactly when it's `refused` or
    /// `failed`, a thread exactly when it's `moved`, a target and a nonce
    /// with `confirm`, and a target of a known kind whenever there is one.
    pub fn answers(&self) -> bool {
        let result = self.result.as_str();
        let confirm = self.reason.as_deref() == Some(reason::CONFIRM);
        result::ALL.contains(&result)
            && self.reason.is_some() == matches!(result, result::REFUSED | result::FAILED)
            && self.thread.is_some() == (result == result::MOVED)
            && (!confirm || (self.target.is_some() && self.nonce.is_some()))
            && self.target.as_ref().is_none_or(AgentReplyTarget::is_known_kind)
    }
}

/// What a PC offering `offers` says it needs before it can type replies
/// (one of [`session_reply::needs`]): none while it offers `agent.reply`,
/// else the first of [`session_reply::needs::ALL`] it offers. A PC offers
/// at most one; a receiver that sees more reads only that one.
pub fn reply_needs(offers: impl Fn(&str) -> bool) -> Option<&'static str> {
    if offers(session_reply::CAP) {
        return None;
    }
    session_reply::needs::ALL.iter().copied().find(|need| offers(need))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Envelope, ble,
        lanes::{Lane, lane_of, rpc_of, svc},
        messages::{agent, session_reply::kind, session_reply::needs, types},
    };

    const SESSION: &str = "sc0123456789abcdef0123456789abcdef";
    const NONCE: &str = "fedcba9876543210fedcba9876543210";

    fn target() -> AgentReplyTarget {
        AgentReplyTarget {
            app: "Terminal".into(),
            agent: "Claude".into(),
            folder: "g7linux".into(),
            kind: kind::TERMINAL.into(),
        }
    }

    fn thread() -> AgentThread {
        AgentThread {
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

    fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
        t: &str,
        body: &T,
    ) {
        let env = Envelope::new(t, body).unwrap();
        let back = Envelope::from_cbor(&env.to_cbor()).unwrap();
        assert_eq!(&back.body::<T>().unwrap(), body);
    }

    fn keys<T: Serialize>(body: &T) -> Vec<String> {
        let env = Envelope::new(types::OK, body).unwrap();
        let Some(ciborium::Value::Map(fields)) = env.b else { panic!("a map") };
        fields.iter().map(|(k, _)| k.as_text().unwrap().to_owned()).collect()
    }

    #[test]
    fn every_body_round_trips() {
        round_trip(types::AGENT_SESSION_MOVE, &AgentSessionMove::new(SESSION.into(), None));
        round_trip(types::AGENT_SESSION_MOVE, &AgentSessionMove::new(SESSION.into(), Some(NONCE.into())));
        for answer in [
            AgentSessionMoved::moved(thread()),
            AgentSessionMoved::moved(thread()).with_target(target()),
            AgentSessionMoved::closed(),
            AgentSessionMoved::confirm(target(), NONCE.into()),
            AgentSessionMoved::refused(reason::IN_USE).retry_after(4_000),
            AgentSessionMoved::refused(reason::FOLDER),
            AgentSessionMoved::failed(reason::DRAFT).with_target(target()),
            AgentSessionMoved::failed(reason::OPEN),
        ] {
            assert!(answer.is_valid(), "{answer:?}");
            assert!(answer.answers(), "{answer:?}");
            round_trip(types::OK, &answer);
        }
        // What's absent stays off the wire.
        assert_eq!(keys(&AgentSessionMove::new(SESSION.into(), None)), ["session"]);
        assert_eq!(keys(&AgentSessionMoved::closed()), ["result"]);
        assert_eq!(keys(&AgentSessionMoved::refused(reason::AGENT)), ["result", "reason"]);
        assert_eq!(keys(&AgentSessionMoved::moved(thread())), ["result", "thread"]);
    }

    #[test]
    fn a_move_names_a_session_and_maybe_a_nonce() {
        assert!(AgentSessionMove::new(SESSION.into(), None).is_valid());
        assert!(AgentSessionMove::new(SESSION.into(), Some(NONCE.into())).is_valid());
        for bad in [
            AgentSessionMove::new(String::new(), None),
            AgentSessionMove::new("/home/me".into(), None),
            AgentSessionMove::new("../etc".into(), None),
            AgentSessionMove::new("a".repeat(41), None),
            AgentSessionMove::new(SESSION.into(), Some(NONCE[1..].into())),
            AgentSessionMove::new(SESSION.into(), Some(format!("{NONCE}0"))),
            AgentSessionMove::new(SESSION.into(), Some(NONCE.to_uppercase())),
            AgentSessionMove::new(SESSION.into(), Some("g".repeat(32))),
            AgentSessionMove::new(SESSION.into(), Some(String::new())),
        ] {
            assert!(!bad.is_valid(), "{bad:?}");
        }
        // A reply's body isn't a move: unknown fields are ignored, `text` included.
        let reply = session_reply_body();
        let env = Envelope::new(types::AGENT_SESSION_MOVE, &reply).unwrap();
        let read = env.body::<AgentSessionMove>().unwrap();
        assert_eq!(read, AgentSessionMove::new(SESSION.into(), None));
    }

    fn session_reply_body() -> crate::messages::AgentSessionReply {
        crate::messages::AgentSessionReply::text(SESSION.into(), "Go".into(), None)
    }

    #[test]
    fn every_word_is_a_word_and_new_ones_are_the_moves() {
        for word in result::ALL.iter().chain(reason::REFUSED).chain(reason::FAILED) {
            assert!(is_reply_word(word), "{word}");
        }
        let mut all: Vec<_> = reason::REFUSED.iter().chain(reason::FAILED).collect();
        let count = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), count, "a reason is a refusal's or a failure's, never both");
        assert!(!reason::REFUSED.contains(&session_reply::reason::TEXT), "the command is G7's own");
        assert!(!reason::REFUSED.contains(&session_reply::reason::CLIPBOARD));
        for word in [reason::FOLDER, reason::AGENT] {
            assert!(reason::REFUSED.contains(&word), "{word}");
        }
        for word in [reason::DRAFT, reason::OPEN] {
            assert!(reason::FAILED.contains(&word), "{word}");
        }
        // The reply's words mean the same here.
        for word in session_reply::reason::REFUSED {
            assert!(
                reason::REFUSED.contains(word) || [session_reply::reason::TEXT, "clipboard"].contains(word),
                "{word}"
            );
        }
        for word in session_reply::reason::FAILED {
            assert!(reason::FAILED.contains(word), "{word}");
        }
    }

    #[test]
    fn an_answer_a_receiver_doesnt_know_reads_safely() {
        let later =
            AgentSessionMoved { result: "parked".into(), reason: Some("held".into()), ..Default::default() };
        assert!(later.is_valid(), "still words");
        let read = later.received().unwrap();
        assert_eq!((read.result.as_str(), read.reason), (result::FAILED, None));
        let unknown_reason = AgentSessionMoved::refused("too_sunny").received().unwrap();
        assert_eq!((unknown_reason.result.as_str(), unknown_reason.reason), (result::REFUSED, None));
        let crossed = AgentSessionMoved::refused(reason::OPEN).sanitized();
        assert_eq!(crossed.reason, None, "open is a failure's reason");
        let a_replys_only = AgentSessionMoved::refused(session_reply::reason::TEXT).sanitized();
        assert_eq!(a_replys_only.reason, None);
        let closed_reason =
            AgentSessionMoved { reason: Some(reason::RATE.into()), ..AgentSessionMoved::closed() };
        assert_eq!(closed_reason.sanitized().reason, None);
        let stray_thread = AgentSessionMoved { thread: Some(thread()), ..AgentSessionMoved::closed() };
        assert_eq!(stray_thread.sanitized().thread, None, "a thread only with moved");
        // Out of its limits: not words.
        for bad in [
            AgentSessionMoved { result: "Moved".into(), ..Default::default() },
            AgentSessionMoved { result: String::new(), ..Default::default() },
            AgentSessionMoved { result: "x".repeat(33), ..Default::default() },
            AgentSessionMoved { reason: Some("in-use".into()), ..AgentSessionMoved::refused(reason::IN_USE) },
            AgentSessionMoved {
                nonce: Some(format!("{NONCE}0")),
                ..AgentSessionMoved::refused(reason::CONFIRM)
            },
            AgentSessionMoved::refused(reason::TAB)
                .with_target(AgentReplyTarget { app: "a".repeat(65), ..target() }),
            AgentSessionMoved::moved(AgentThread { id: "../th".into(), ..thread() }),
            AgentSessionMoved::moved(AgentThread { state: "Done".into(), ..thread() }),
        ] {
            assert!(!bad.is_valid(), "{bad:?}");
        }
    }

    #[test]
    fn moved_without_a_thread_isnt_known() {
        let bare = AgentSessionMoved { thread: None, ..AgentSessionMoved::moved(thread()) };
        assert!(bare.is_valid());
        assert!(!bare.clone().sanitized().answers(), "a PC never sends it");
        assert_eq!(bare.received(), None, "where it went isn't known");
        let read = AgentSessionMoved::moved(thread()).received().unwrap();
        assert_eq!(read.thread.map(|t| t.id), Some("th0123".into()));
        let closed = AgentSessionMoved::closed().received().unwrap();
        assert_eq!((closed.result.as_str(), closed.reason, closed.thread), (result::CLOSED, None, None));
    }

    #[test]
    fn a_confirm_needs_its_nonce_and_target() {
        let confirm = AgentSessionMoved::confirm(target(), NONCE.into()).received().unwrap();
        assert_eq!(
            (confirm.reason.as_deref(), confirm.nonce.as_deref()),
            (Some(reason::CONFIRM), Some(NONCE))
        );
        let no_nonce =
            AgentSessionMoved { nonce: None, ..AgentSessionMoved::confirm(target(), NONCE.into()) };
        assert!(!no_nonce.answers());
        let read = no_nonce.received().unwrap();
        assert_eq!((read.result.as_str(), read.reason), (result::REFUSED, None), "it can't be confirmed");
        let no_target =
            AgentSessionMoved { target: None, ..AgentSessionMoved::confirm(target(), NONCE.into()) };
        assert!(!no_target.answers());
        assert_eq!(no_target.received().unwrap().reason, None);
        let bad_nonce = AgentSessionMoved {
            nonce: Some("z".repeat(32)),
            ..AgentSessionMoved::confirm(target(), NONCE.into())
        };
        assert_eq!(bad_nonce.received().unwrap().reason, None);
    }

    #[test]
    fn an_answer_is_made_to_fit() {
        let rate = AgentSessionMoved::refused(reason::RATE).retry_after(3_600_000).sanitized();
        assert_eq!(rate.retry_ms, Some(session_move::MAX_RETRY_MS));
        let in_use = AgentSessionMoved::refused(reason::IN_USE).retry_after(4_000).sanitized();
        assert_eq!(in_use.retry_ms, Some(4_000));
        let busy = AgentSessionMoved::refused(reason::BUSY).retry_after(4_000).sanitized();
        assert_eq!(busy.retry_ms, None, "only rate and in_use say when");
        let stray =
            AgentSessionMoved { nonce: Some(NONCE.into()), ..AgentSessionMoved::refused(reason::TAB) };
        assert_eq!(stray.sanitized().nonce, None, "a nonce only with confirm");
        let careless = AgentReplyTarget { folder: "/home/me/src/g7linux/".into(), ..target() };
        let fitted = AgentSessionMoved::failed(reason::DRAFT).with_target(careless).sanitized();
        assert_eq!(fitted.target.map(|t| t.folder), Some("g7linux".into()), "never a path");
        let long_title = AgentThread { title: format!("Fix\u{1b}{}", "x".repeat(400)), ..thread() };
        let moved = AgentSessionMoved::moved(long_title).sanitized();
        let title = moved.thread.unwrap().title;
        assert!(title.len() <= agent::MAX_TITLE_BYTES && !title.contains('\u{1b}'), "{title}");
    }

    #[test]
    fn a_pc_sends_only_whole_answers() {
        assert!(AgentSessionMoved::moved(thread()).answers());
        assert!(AgentSessionMoved::closed().answers());
        assert!(AgentSessionMoved::refused(reason::LOCKED).answers());
        assert!(AgentSessionMoved::failed(reason::OPEN).answers());
        assert!(!AgentSessionMoved { reason: None, ..AgentSessionMoved::refused(reason::TAB) }.answers());
        assert!(!AgentSessionMoved { reason: None, ..AgentSessionMoved::failed(reason::DRAFT) }.answers());
        assert!(!AgentSessionMoved { thread: Some(thread()), ..AgentSessionMoved::closed() }.answers());
        assert!(!AgentSessionMoved { result: "parked".into(), ..Default::default() }.answers());
        let later_kind =
            AgentSessionMoved::closed().with_target(AgentReplyTarget { kind: "tmux".into(), ..target() });
        assert!(!later_kind.answers(), "a PC sends the kinds it knows");
    }

    #[test]
    fn the_nonce_and_the_title_are_never_printed() {
        let body = AgentSessionMove::new(SESSION.into(), Some(NONCE.into()));
        let shown = format!("{body:?}");
        assert!(!shown.contains(NONCE) && shown.contains(SESSION), "{shown}");
        for answer in [AgentSessionMoved::confirm(target(), NONCE.into()), AgentSessionMoved::moved(thread())]
        {
            let shown = format!("{answer:?}");
            assert!(!shown.contains(NONCE) && !shown.contains("Fix the build"), "{shown}");
        }
    }

    #[test]
    fn a_move_is_an_rpc_and_never_on_bluetooth() {
        let op = rpc_of(types::AGENT_SESSION_MOVE).expect("an RPC");
        assert_eq!((op.svc, op.op, op.paged), (svc::AGENT, "session.move", false));
        assert_eq!(
            lane_of(types::AGENT_SESSION_MOVE),
            Some(Lane::Rpc { svc: svc::AGENT, op: "session.move" })
        );
        assert!(!ble::carries(types::AGENT_SESSION_MOVE));
    }

    #[test]
    fn a_pc_says_what_replies_need_and_only_one_is_read() {
        let offering = |caps: &'static [&'static str]| move |cap: &str| caps.contains(&cap);
        assert_eq!(reply_needs(offering(&[])), None, "an older PC says nothing");
        assert_eq!(reply_needs(offering(&[needs::EXTENSION])), Some(needs::EXTENSION));
        assert_eq!(reply_needs(offering(&[needs::DESKTOP])), Some(needs::DESKTOP));
        assert_eq!(reply_needs(offering(&[needs::KEYBOARD])), Some(needs::KEYBOARD));
        assert_eq!(
            reply_needs(offering(&[session_reply::CAP, needs::KEYBOARD])),
            None,
            "it can reply: nothing is missing"
        );
        assert_eq!(
            reply_needs(offering(&[needs::KEYBOARD, needs::EXTENSION, needs::DESKTOP])),
            Some(needs::DESKTOP)
        );
        assert_eq!(reply_needs(offering(&[needs::KEYBOARD, needs::EXTENSION])), Some(needs::EXTENSION));
        assert_eq!(reply_needs(offering(&["agent.reply.needs.wings"])), None, "a word it doesn't know");
        for need in needs::ALL {
            assert!(need.starts_with(session_reply::CAP) && *need != session_reply::CAP, "{need}");
        }
        assert_ne!(session_move::CAP, session_reply::CAP);
    }
}
