// SPDX-License-Identifier: MPL-2.0
//! Bodies of reply on the PC (`docs/protocol/agents.md` §9): a phone asks a
//! PC to enter a reply into a session started there outside G7 Link, or
//! whether it could now (`check`). The PC finds the session's window,
//! proves it shows that session, pastes the text and presses Enter, and
//! answers with a `result` and a `reason` word. The PC offers it as
//! `agent.reply`; the `agents`, `agent_sessions` and `agent_reply` toggles
//! govern it.
//!
//! A reply's text never appears in a `Debug`, only its length, and a
//! confirmation's nonce never at all. A receiver reads a `result` it
//! doesn't know as `failed` and a `reason` it doesn't know as absent.

use std::ops::Not;

use serde::{Deserialize, Serialize};

use super::{clean_line, folder_name, is_bidi_control, is_lower_hex, is_valid_agent_id};

/// The capability, the toggle, the limits and the wire words of §9.
pub mod session_reply {
    /// Desktop: can enter a reply into a session started on this PC now.
    /// Offered only while it can (a window backend, the input device).
    pub const CAP: &str = "agent.reply";
    /// The per-device toggle on the PC (off by default; counts only while
    /// `agents` and `agent_sessions` are on).
    pub const TOGGLE: &str = "agent_reply";
    /// A reply's `text`, in bytes of UTF-8.
    pub const MAX_TEXT_BYTES: usize = 8192;
    /// A confirmation's `nonce`, and `confirm`: lowercase hex digits.
    pub const NONCE_BYTES: usize = 32;
    /// `result` and `reason`: 1 to this many of `[a-z_]`.
    pub const MAX_WORD_BYTES: usize = 32;
    /// A target's `app`, `agent` and `folder`.
    pub const MAX_TARGET_BYTES: usize = 64;
    /// `retry_ms`.
    pub const MAX_RETRY_MS: u64 = 60_000;

    /// `result`.
    pub mod result {
        /// A check: a reply could be entered now.
        pub const READY: &str = "ready";
        /// Enter was pressed in the proven window. The message shows as an
        /// `agent.session.item` once the agent records it.
        pub const SENT: &str = "sent";
        /// Nothing was typed.
        pub const REFUSED: &str = "refused";
        /// It went wrong on the way; the reason says whether text was pasted.
        pub const FAILED: &str = "failed";
        pub const ALL: &[&str] = &[READY, SENT, REFUSED, FAILED];
        /// What answers a check.
        pub const CHECK: &[&str] = &[READY, REFUSED];
        /// What answers a reply with text.
        pub const TEXT: &[&str] = &[SENT, REFUSED, FAILED];
    }

    /// `reason`.
    pub mod reason {
        /// The PC is locked.
        pub const LOCKED: &str = "locked";
        /// Someone is using the PC.
        pub const IN_USE: &str = "in_use";
        /// The first reply from this phone to this session where it runs
        /// now: show `target` and send again with `confirm` set to `nonce`.
        pub const CONFIRM: &str = "confirm";
        /// The agent waits for an answer on the PC.
        pub const NEEDS_YOU: &str = "needs_you";
        /// The agent is mid-turn and can't take a message now.
        pub const BUSY: &str = "busy";
        /// Nothing has the session open.
        pub const ENDED: &str = "ended";
        pub const NO_PROCESS: &str = "no_process";
        pub const NO_WINDOW: &str = "no_window";
        /// It can't be told which window shows the session.
        pub const AMBIGUOUS: &str = "ambiguous";
        /// It's in a background tab the PC couldn't bring forward.
        pub const TAB: &str = "tab";
        /// The PC couldn't make sure which terminal is the session's.
        pub const UNPROVEN: &str = "unproven";
        /// The session runs where G7 can't type (an app, VS Code, ssh).
        pub const PLACE: &str = "place";
        /// Something is copied that G7 couldn't put back.
        pub const CLIPBOARD: &str = "clipboard";
        /// The text starts with what the agent reads as a command.
        pub const TEXT: &str = "text";
        pub const RATE: &str = "rate";
        /// The PC needs something set up first (the desktop's extension).
        pub const SETUP: &str = "setup";
        /// The window didn't come forward. Nothing was typed.
        pub const FOCUS: &str = "focus";
        /// The text was pasted and not sent.
        pub const INTERRUPTED: &str = "interrupted";
        /// A key couldn't be pressed; the paste may have landed.
        pub const INPUT: &str = "input";
        /// Those of `refused`.
        pub const REFUSED: &[&str] = &[
            LOCKED, IN_USE, CONFIRM, NEEDS_YOU, BUSY, ENDED, NO_PROCESS, NO_WINDOW, AMBIGUOUS, TAB, UNPROVEN,
            PLACE, CLIPBOARD, TEXT, RATE, SETUP,
        ];
        /// Those of `failed`.
        pub const FAILED: &[&str] = &[FOCUS, INTERRUPTED, INPUT];
        /// Those `retry_ms` goes with.
        pub const RETRY: &[&str] = &[RATE, IN_USE];
    }

    /// A target's `kind`.
    pub mod kind {
        pub const TERMINAL: &str = "terminal";
        pub const APP: &str = "app";
        pub const ALL: &[&str] = &[TERMINAL, APP];
    }
}

use session_reply::{kind, reason, result};

/// Whether `word` can be a `result` or a `reason`: 1 to 32 of `[a-z_]`.
fn is_reply_word(word: &str) -> bool {
    (1..=session_reply::MAX_WORD_BYTES).contains(&word.len())
        && word.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

/// Whether `text` can be sent as a reply: 1 to 8192 bytes, not blank, with
/// no control character but newline, carriage return and tab, and no
/// bidirectional formatting control. The PC cleans it further and may
/// still refuse it (`refused: text`).
pub fn is_reply_text(text: &str) -> bool {
    (1..=session_reply::MAX_TEXT_BYTES).contains(&text.len())
        && !text.trim().is_empty()
        && !text.chars().any(|c| (c.is_control() && !matches!(c, '\n' | '\r' | '\t')) || is_bidi_control(c))
}

/// Whether `nonce` is a confirmation's nonce: 32 lowercase hex digits.
pub fn is_reply_nonce(nonce: &str) -> bool {
    is_lower_hex(nonce, session_reply::NONCE_BYTES)
}

/// Body of `agent.session.reply` (phone → PC, RPC `agent/session.reply`):
/// enter `text` into `session`'s window, or with `check` only say whether
/// it could be now. Exactly one of the two; `confirm` (the nonce of an
/// earlier `confirm` answer) only with `text`.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentSessionReply {
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Not::not")]
    pub check: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
}

/// Never prints the text or the nonce.
impl std::fmt::Debug for AgentSessionReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSessionReply")
            .field("session", &self.session)
            .field("bytes", &self.text.as_ref().map(String::len))
            .field("check", &self.check)
            .field("confirm", &self.confirm.is_some())
            .finish()
    }
}

impl AgentSessionReply {
    /// A check of `session`: enters nothing.
    pub fn check(session: String) -> Self {
        Self { session, text: None, check: true, confirm: None }
    }

    /// `text` for `session`, confirmed with an earlier answer's nonce.
    pub fn text(session: String, text: String, confirm: Option<String>) -> Self {
        Self { session, text: Some(text), check: false, confirm }
    }

    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.session)
            && match (&self.text, self.check) {
                (Some(text), false) => {
                    is_reply_text(text) && self.confirm.as_deref().is_none_or(is_reply_nonce)
                }
                (None, true) => self.confirm.is_none(),
                _ => false,
            }
    }
}

/// Where a reply goes, said on every `ready`, `sent` and `confirm` answer:
/// the app's name (from its desktop entry, never a window title), the
/// agent's name, and the folder's name, never a path.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentReplyTarget {
    pub app: String,
    pub agent: String,
    pub folder: String,
    /// One of [`session_reply::kind`]; a receiver may not know a later one.
    pub kind: String,
}

impl AgentReplyTarget {
    /// Within its limits as received; a `kind` this build doesn't know is
    /// still a word.
    pub fn is_valid(&self) -> bool {
        self.app.len() <= session_reply::MAX_TARGET_BYTES
            && self.agent.len() <= session_reply::MAX_TARGET_BYTES
            && self.folder.len() <= session_reply::MAX_TARGET_BYTES
            && is_reply_word(&self.kind)
    }

    /// One line each, cut to 64 bytes, the folder cut to its last
    /// component. `None` without an app or an agent, or a `kind` that
    /// isn't a word.
    pub fn sanitized(mut self) -> Option<Self> {
        let max = session_reply::MAX_TARGET_BYTES;
        clean_line(&mut self.app, max);
        clean_line(&mut self.agent, max);
        let mut folder = folder_name(&self.folder).to_owned();
        clean_line(&mut folder, max);
        self.folder = folder;
        (!self.app.trim().is_empty() && !self.agent.trim().is_empty() && is_reply_word(&self.kind))
            .then_some(self)
    }

    /// Whether `kind` is one this build knows.
    pub fn is_known_kind(&self) -> bool {
        kind::ALL.contains(&self.kind.as_str())
    }
}

/// The `ok` answer of `agent.session.reply`: `result` (one of
/// [`session_reply::result`]), and for `refused` and `failed` its
/// `reason`. `confirm` comes with `target` and `nonce`; `rate` and
/// `in_use` with `retry_ms` when it's known.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentSessionReplied {
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AgentReplyTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_ms: Option<u64>,
}

/// Never prints the nonce.
impl std::fmt::Debug for AgentSessionReplied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSessionReplied")
            .field("result", &self.result)
            .field("reason", &self.reason)
            .field("kind", &self.target.as_ref().map(|t| t.kind.as_str()))
            .field("nonce", &self.nonce.is_some())
            .field("retry_ms", &self.retry_ms)
            .finish()
    }
}

impl AgentSessionReplied {
    fn of(result: &str, reason: Option<&str>, target: Option<AgentReplyTarget>) -> Self {
        Self {
            result: result.to_owned(),
            reason: reason.map(str::to_owned),
            target,
            nonce: None,
            retry_ms: None,
        }
    }

    /// A check: a reply to `target` could be entered now.
    pub fn ready(target: AgentReplyTarget) -> Self {
        Self::of(result::READY, None, Some(target))
    }

    /// Enter was pressed in `target`.
    pub fn sent(target: AgentReplyTarget) -> Self {
        Self::of(result::SENT, None, Some(target))
    }

    /// Nothing typed: show `target` and send again with `confirm: nonce`.
    pub fn confirm(target: AgentReplyTarget, nonce: String) -> Self {
        Self { nonce: Some(nonce), ..Self::of(result::REFUSED, Some(reason::CONFIRM), Some(target)) }
    }

    /// Nothing typed, for `reason` (one of [`session_reply::reason::REFUSED`]).
    pub fn refused(reason: &str) -> Self {
        Self::of(result::REFUSED, Some(reason), None)
    }

    /// It went wrong, for `reason` (one of [`session_reply::reason::FAILED`]).
    pub fn failed(reason: &str) -> Self {
        Self::of(result::FAILED, Some(reason), None)
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
            && self.nonce.as_ref().is_none_or(|n| n.len() <= session_reply::NONCE_BYTES)
            && self.target.as_ref().is_none_or(AgentReplyTarget::is_valid)
    }

    /// Made to fit, on either side: a `result` this build doesn't know is
    /// `failed`; a `reason` it doesn't know for its result, or on `ready`
    /// or `sent`, is dropped; `nonce` stays only with a `confirm` (and when
    /// it's one), `retry_ms` only with `rate` or `in_use`, at most 60 s;
    /// `target` is cleaned, and dropped when it can't be.
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
                self.retry_ms.map(|ms| ms.min(session_reply::MAX_RETRY_MS))
            }
            _ => None,
        };
        self.target = self.target.and_then(AgentReplyTarget::sanitized);
        self
    }

    /// As a phone takes it for its request (`check` or text): sanitized,
    /// and a result that doesn't answer that request is `failed`, without
    /// a reason. A `confirm` without its nonce or target loses its reason.
    pub fn for_request(self, check: bool) -> Self {
        let mut answer = self.sanitized();
        let answers = if check { result::CHECK } else { result::TEXT };
        if !answers.contains(&answer.result.as_str()) {
            return Self::of(result::FAILED, None, None);
        }
        if answer.reason.as_deref() == Some(reason::CONFIRM)
            && (answer.nonce.is_none() || answer.target.is_none())
        {
            answer.reason = None;
        }
        answer
    }

    /// Whether, sanitized, it is a whole answer to a request (`check` or
    /// text), as a PC sends it: a result that answers it, a known reason
    /// exactly when it's `refused` or `failed`, a target of a known kind on
    /// `ready`, `sent` and `confirm`, and a nonce with `confirm`. A check is
    /// never answered `confirm`: the first reply asks, so a check that
    /// lacks only the confirmation is `ready`.
    pub fn answers(&self, check: bool) -> bool {
        let answers = if check { result::CHECK } else { result::TEXT };
        let result = self.result.as_str();
        let target_needed =
            matches!(result, result::READY | result::SENT) || self.reason.as_deref() == Some(reason::CONFIRM);
        answers.contains(&result)
            && self.reason.is_some() == matches!(result, result::REFUSED | result::FAILED)
            && (!target_needed || self.target.is_some())
            && self.target.as_ref().is_none_or(AgentReplyTarget::is_known_kind)
            && (self.reason.as_deref() != Some(reason::CONFIRM) || (self.nonce.is_some() && !check))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Envelope, ble,
        lanes::{Lane, lane_of, rpc_of, svc},
        messages::types,
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

    fn reply(text: &str) -> AgentSessionReply {
        AgentSessionReply::text(SESSION.into(), text.into(), None)
    }

    fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
        t: &str,
        body: &T,
    ) {
        let env = Envelope::new(t, body).unwrap();
        let back = Envelope::from_cbor(&env.to_cbor()).unwrap();
        assert_eq!(&back.body::<T>().unwrap(), body);
    }

    #[test]
    fn every_body_round_trips() {
        round_trip(types::AGENT_SESSION_REPLY, &AgentSessionReply::check(SESSION.into()));
        round_trip(types::AGENT_SESSION_REPLY, &reply("Yes, go on.\nAnd run the tests."));
        round_trip(
            types::AGENT_SESSION_REPLY,
            &AgentSessionReply::text(SESSION.into(), "Go".into(), Some(NONCE.into())),
        );
        for answer in [
            AgentSessionReplied::ready(target()),
            AgentSessionReplied::sent(target()),
            AgentSessionReplied::confirm(target(), NONCE.into()),
            AgentSessionReplied::refused(reason::RATE).retry_after(3_000),
            AgentSessionReplied::refused(reason::UNPROVEN),
            AgentSessionReplied::failed(reason::INTERRUPTED).with_target(target()),
        ] {
            assert!(answer.is_valid(), "{answer:?}");
            round_trip(types::OK, &answer);
        }
        // What's absent stays off the wire.
        fn keys<T: Serialize>(body: &T) -> Vec<String> {
            let env = Envelope::new(types::OK, body).unwrap();
            let Some(ciborium::Value::Map(fields)) = env.b else { panic!("a map") };
            fields.iter().map(|(k, _)| k.as_text().unwrap().to_owned()).collect()
        }
        assert_eq!(keys(&reply("Go")), ["session", "text"]);
        assert_eq!(keys(&AgentSessionReply::check(SESSION.into())), ["session", "check"]);
        assert_eq!(keys(&AgentSessionReplied::refused(reason::TAB)), ["result", "reason"]);
    }

    #[test]
    fn a_reply_holds_text_or_a_check_never_both() {
        assert!(AgentSessionReply::check(SESSION.into()).is_valid());
        assert!(reply("Go").is_valid());
        let both = AgentSessionReply { check: true, ..reply("Go") };
        assert!(!both.is_valid(), "both");
        let neither = AgentSessionReply { session: SESSION.into(), ..Default::default() };
        assert!(!neither.is_valid(), "neither");
        let confirmed_check =
            AgentSessionReply { confirm: Some(NONCE.into()), ..AgentSessionReply::check(SESSION.into()) };
        assert!(!confirmed_check.is_valid(), "a check confirms nothing");
        assert!(!AgentSessionReply { session: "/home/me".into(), ..reply("Go") }.is_valid());
        assert!(!AgentSessionReply { session: String::new(), ..reply("Go") }.is_valid());
    }

    #[test]
    fn a_replys_text_and_nonce_keep_their_limits() {
        assert!(reply(&"x".repeat(session_reply::MAX_TEXT_BYTES)).is_valid());
        assert!(!reply(&"x".repeat(session_reply::MAX_TEXT_BYTES + 1)).is_valid());
        // Four-byte characters: the limit is bytes.
        assert!(reply(&"😀".repeat(2048)).is_valid());
        assert!(!reply(&"😀".repeat(2049)).is_valid());
        assert!(!reply("").is_valid());
        assert!(!reply(" \n\t ").is_valid(), "blank");
        assert!(reply("line one\r\nline two\tthree").is_valid(), "the PC makes these one form");
        for bad in ["a\u{1b}[201~b", "a\u{0}b", "a\u{7f}b", "a\u{85}b", "a\u{202E}b", "a\u{2066}b"] {
            assert!(!reply(bad).is_valid(), "{bad:?}");
        }
        let confirmed = |nonce: &str| AgentSessionReply { confirm: Some(nonce.into()), ..reply("Go") };
        assert!(confirmed(NONCE).is_valid());
        assert!(!confirmed(&NONCE[1..]).is_valid());
        assert!(!confirmed(&format!("{NONCE}0")).is_valid());
        assert!(!confirmed(&NONCE.to_uppercase()).is_valid());
        assert!(!confirmed(&"g".repeat(32)).is_valid());
    }

    #[test]
    fn an_answer_a_receiver_doesnt_know_reads_safely() {
        let later = AgentSessionReplied {
            result: "queued".into(),
            reason: Some("held".into()),
            ..Default::default()
        };
        assert!(later.is_valid(), "still words");
        let read = later.for_request(false);
        assert_eq!((read.result.as_str(), read.reason), (result::FAILED, None));
        let unknown_reason = AgentSessionReplied::refused("too_sunny").for_request(false);
        assert_eq!((unknown_reason.result.as_str(), unknown_reason.reason), (result::REFUSED, None));
        let crossed = AgentSessionReplied::refused(reason::FOCUS).sanitized();
        assert_eq!(crossed.reason, None, "focus is a failure's reason");
        let sent_reason =
            AgentSessionReplied { reason: Some(reason::RATE.into()), ..AgentSessionReplied::sent(target()) };
        assert_eq!(sent_reason.sanitized().reason, None);
        // Out of its limits: not words.
        for bad in [
            AgentSessionReplied { result: "Sent".into(), ..Default::default() },
            AgentSessionReplied { result: String::new(), ..Default::default() },
            AgentSessionReplied { result: "x".repeat(33), ..Default::default() },
            AgentSessionReplied {
                reason: Some("in-use".into()),
                ..AgentSessionReplied::refused(reason::IN_USE)
            },
            AgentSessionReplied {
                reason: Some("busy2".into()),
                ..AgentSessionReplied::refused(reason::BUSY)
            },
            AgentSessionReplied {
                nonce: Some(format!("{NONCE}0")),
                ..AgentSessionReplied::refused(reason::CONFIRM)
            },
            AgentSessionReplied::ready(AgentReplyTarget { app: "a".repeat(65), ..target() }),
            AgentSessionReplied::ready(AgentReplyTarget { kind: "Terminal".into(), ..target() }),
        ] {
            assert!(!bad.is_valid(), "{bad:?}");
        }
        assert!(AgentSessionReplied::ready(AgentReplyTarget { app: "a".repeat(64), ..target() }).is_valid());
    }

    #[test]
    fn an_answer_that_doesnt_answer_its_request_is_a_failure() {
        let sent_to_check = AgentSessionReplied::sent(target()).for_request(true);
        assert_eq!((sent_to_check.result.as_str(), sent_to_check.target), (result::FAILED, None));
        let ready_to_text = AgentSessionReplied::ready(target()).for_request(false);
        assert_eq!(ready_to_text.result, result::FAILED);
        assert_eq!(AgentSessionReplied::ready(target()).for_request(true).result, result::READY);
        let confirm = AgentSessionReplied::confirm(target(), NONCE.into()).for_request(false);
        assert_eq!(
            (confirm.reason.as_deref(), confirm.nonce.as_deref()),
            (Some(reason::CONFIRM), Some(NONCE))
        );
        let no_nonce =
            AgentSessionReplied { nonce: None, ..AgentSessionReplied::confirm(target(), NONCE.into()) };
        let read = no_nonce.for_request(false);
        assert_eq!((read.result.as_str(), read.reason), (result::REFUSED, None), "it can't be confirmed");
        let bad_nonce = AgentSessionReplied {
            nonce: Some("z".repeat(32)),
            ..AgentSessionReplied::confirm(target(), NONCE.into())
        };
        assert_eq!(bad_nonce.for_request(false).reason, None);
    }

    #[test]
    fn an_answer_is_made_to_fit() {
        let rate = AgentSessionReplied::refused(reason::RATE).retry_after(3_600_000).sanitized();
        assert_eq!(rate.retry_ms, Some(session_reply::MAX_RETRY_MS));
        let in_use = AgentSessionReplied::refused(reason::IN_USE).retry_after(4_000).sanitized();
        assert_eq!(in_use.retry_ms, Some(4_000));
        let tab = AgentSessionReplied::refused(reason::TAB).retry_after(4_000).sanitized();
        assert_eq!(tab.retry_ms, None, "only rate and in_use say when");
        let unproven = AgentSessionReplied::refused(reason::UNPROVEN).retry_after(4_000).for_request(false);
        assert_eq!((unproven.reason.as_deref(), unproven.retry_ms), (Some(reason::UNPROVEN), None));
        let stray =
            AgentSessionReplied { nonce: Some(NONCE.into()), ..AgentSessionReplied::refused(reason::TAB) };
        assert_eq!(stray.sanitized().nonce, None, "a nonce only with confirm");

        let careless = AgentReplyTarget {
            app: format!("Terminal\u{1b}]0;{}", "x".repeat(100)),
            agent: "Claude\u{202E}".into(),
            folder: "/home/me/src/g7linux/".into(),
            kind: kind::TERMINAL.into(),
        };
        let fitted = AgentSessionReplied::sent(careless).sanitized().target.unwrap();
        assert_eq!(fitted.folder, "g7linux", "never a path");
        assert_eq!(fitted.agent, "Claude");
        assert!(fitted.app.starts_with("Terminal]0;") && fitted.app.len() == session_reply::MAX_TARGET_BYTES);
        let nameless = AgentReplyTarget { app: " ".into(), ..target() };
        assert_eq!(AgentSessionReplied::sent(nameless).sanitized().target, None);
        let windows = AgentReplyTarget { folder: "C:\\work\\app".into(), ..target() };
        assert_eq!(windows.sanitized().unwrap().folder, "app");
    }

    #[test]
    fn a_pc_sends_only_whole_answers() {
        assert!(AgentSessionReplied::ready(target()).answers(true));
        assert!(AgentSessionReplied::refused(reason::LOCKED).answers(true));
        assert!(!AgentSessionReplied::failed(reason::FOCUS).answers(true), "a check presses nothing");
        assert!(!AgentSessionReplied::sent(target()).answers(true));
        assert!(AgentSessionReplied::sent(target()).answers(false));
        assert!(AgentSessionReplied::failed(reason::INTERRUPTED).answers(false));
        assert!(AgentSessionReplied::confirm(target(), NONCE.into()).answers(false));
        assert!(!AgentSessionReplied::confirm(target(), NONCE.into()).answers(true), "the first reply asks");
        assert!(!AgentSessionReplied::ready(target()).answers(false));
        let mut no_target = AgentSessionReplied::sent(target());
        no_target.target = None;
        assert!(!no_target.answers(false), "sent says where");
        let mut bare_confirm = AgentSessionReplied::confirm(target(), NONCE.into());
        bare_confirm.target = None;
        assert!(!bare_confirm.answers(false));
        assert!(
            !AgentSessionReplied { nonce: None, ..AgentSessionReplied::confirm(target(), NONCE.into()) }
                .answers(false)
        );
        assert!(
            !AgentSessionReplied { reason: None, ..AgentSessionReplied::refused(reason::TAB) }.answers(false)
        );
        let later_kind = AgentSessionReplied::sent(AgentReplyTarget { kind: "tmux".into(), ..target() });
        assert!(!later_kind.answers(false), "a PC sends the kinds it knows");
    }

    #[test]
    fn the_text_and_the_nonce_are_never_printed() {
        let body = AgentSessionReply::text(SESSION.into(), "secret plan".into(), Some(NONCE.into()));
        let shown = format!("{body:?}");
        assert!(!shown.contains("secret") && !shown.contains(NONCE), "{shown}");
        assert!(shown.contains("bytes: Some(11)"), "{shown}");
        let answer = format!("{:?}", AgentSessionReplied::confirm(target(), NONCE.into()));
        assert!(!answer.contains(NONCE), "{answer}");
    }

    #[test]
    fn a_reply_is_an_rpc_and_never_on_bluetooth() {
        let op = rpc_of(types::AGENT_SESSION_REPLY).expect("an RPC");
        assert_eq!((op.svc, op.op, op.paged), (svc::AGENT, "session.reply", false));
        assert_eq!(
            lane_of(types::AGENT_SESSION_REPLY),
            Some(Lane::Rpc { svc: svc::AGENT, op: "session.reply" })
        );
        assert!(!ble::carries(types::AGENT_SESSION_REPLY));
    }
}
