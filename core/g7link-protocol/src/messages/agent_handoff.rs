// SPDX-License-Identifier: MPL-2.0
//! Bodies of hand-off (`docs/protocol/agents.md` §10): a PC hands a thread
//! to a phone ("Continue on <phone>", or offered when the PC locks with the
//! thread on screen), and a phone hands one to its PC ("Continue on <PC>").
//! Either side only opens the thread where it was handed, at the item that
//! was on the sender's screen; nothing is sent, typed or unlocked. Drafts
//! travel as they always do (`agent.draft.get`/`set`). Plain protocol v1:
//! no capability, governed by the `agents` toggle.

use serde::{Deserialize, Serialize};

use super::{is_agent_word, is_valid_agent_id};

/// Limits, times and wire words of §10.
pub mod handoff {
    /// The range of `agent.handoff`'s `ttl_ms`.
    pub const MIN_TTL_MS: u64 = 1_000;
    pub const MAX_TTL_MS: u64 = 600_000;
    /// The `ttl_ms` a PC sends: a hand-off or an offer holds 10 minutes.
    pub const TTL_MS: u64 = MAX_TTL_MS;

    /// `agent.handoff.open`'s `result`. A phone reads a word it doesn't
    /// know as [`AWAY`].
    pub mod result {
        /// The thread is on the PC's screen, and someone is at the PC.
        pub const OPENED: &str = "opened";
        /// The PC is locked: it opens the thread when it's unlocked, within
        /// 10 minutes.
        pub const LOCKED: &str = "locked";
        /// The PC isn't locked, but nobody has used it in the last 30 s:
        /// the thread is open there for when someone comes back.
        pub const AWAY: &str = "away";
        pub const ALL: &[&str] = &[OPENED, LOCKED, AWAY];
    }
}

/// Body of `agent.handoff` (PC → phone, `agent` lane): open `thread` here,
/// at item `seq` when it's given. `offer`: the PC locked with it on screen;
/// show it quietly, and take it away when the PC says someone is at it
/// again (`agent.presence`). It holds for `ttl_ms` from when it arrives.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentHandoff {
    pub thread: String,
    /// The first item on the sender's screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    pub offer: bool,
    /// [`handoff::MIN_TTL_MS`] to [`handoff::MAX_TTL_MS`].
    pub ttl_ms: u64,
}

impl AgentHandoff {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread) && (handoff::MIN_TTL_MS..=handoff::MAX_TTL_MS).contains(&self.ttl_ms)
    }

    /// With `ttl_ms` brought into its range, to send.
    pub fn sanitized(mut self) -> Self {
        self.ttl_ms = self.ttl_ms.clamp(handoff::MIN_TTL_MS, handoff::MAX_TTL_MS);
        self
    }
}

/// Body of `agent.handoff.open` (phone → PC, RPC `agent/handoff.open`):
/// open `thread` on the PC's screen, at item `seq` when it's given.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentHandoffOpen {
    pub thread: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

impl AgentHandoffOpen {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread)
    }
}

/// The `ok` answer of `agent.handoff.open`: one of [`handoff::result`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHandedOff {
    pub result: String,
}

impl AgentHandedOff {
    /// On the PC's screen, someone at it.
    pub fn opened() -> Self {
        Self { result: handoff::result::OPENED.into() }
    }

    /// The PC is locked; it opens when it's unlocked.
    pub fn locked() -> Self {
        Self { result: handoff::result::LOCKED.into() }
    }

    /// Open on the PC, nobody at it.
    pub fn away() -> Self {
        Self { result: handoff::result::AWAY.into() }
    }

    /// A word, perhaps one this build doesn't know.
    pub fn is_valid(&self) -> bool {
        is_agent_word(&self.result)
    }

    /// Whether `result` is one this build knows: all a PC may answer.
    pub fn is_known(&self) -> bool {
        handoff::result::ALL.contains(&self.result.as_str())
    }

    /// As a phone reads it: a word it doesn't know as `away`.
    pub fn sanitized(self) -> Self {
        if self.is_known() { self } else { Self::away() }
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

    const THREAD: &str = "0123456789abcdef0123456789abcdef";

    fn handed(offer: bool) -> AgentHandoff {
        AgentHandoff { thread: THREAD.into(), seq: Some(42), offer, ttl_ms: handoff::TTL_MS }
    }

    #[test]
    fn every_body_round_trips() {
        for sent in [handed(false), handed(true), AgentHandoff { seq: None, ..handed(false) }] {
            let env = Envelope::new(types::AGENT_HANDOFF, &sent).unwrap();
            let back = Envelope::from_cbor(&env.to_cbor()).unwrap();
            assert_eq!(back.body::<AgentHandoff>().unwrap(), sent);
        }
        let open = AgentHandoffOpen { thread: THREAD.into(), seq: Some(7) };
        let env = Envelope::new(types::AGENT_HANDOFF_OPEN, &open).unwrap();
        assert_eq!(Envelope::from_cbor(&env.to_cbor()).unwrap().body::<AgentHandoffOpen>().unwrap(), open);
        let env = Envelope::new(types::OK, &AgentHandedOff::locked()).unwrap();
        let back = Envelope::from_cbor(&env.to_cbor()).unwrap().body::<AgentHandedOff>().unwrap();
        assert_eq!(back.result, handoff::result::LOCKED);
    }

    #[test]
    fn a_handoff_out_of_its_limits_is_refused_and_made_to_fit_before_it_goes() {
        assert!(handed(false).is_valid());
        assert!(!AgentHandoff { thread: "/home/me".into(), ..handed(false) }.is_valid());
        assert!(!AgentHandoff { thread: String::new(), ..handed(false) }.is_valid());
        assert!(!AgentHandoff { ttl_ms: 999, ..handed(false) }.is_valid());
        assert!(!AgentHandoff { ttl_ms: 600_001, ..handed(false) }.is_valid());
        assert!(AgentHandoff { ttl_ms: 1_000, ..handed(false) }.is_valid());
        let long = AgentHandoff { ttl_ms: 3_600_000, ..handed(true) }.sanitized();
        assert_eq!(long.ttl_ms, handoff::MAX_TTL_MS);
        assert_eq!(AgentHandoff { ttl_ms: 0, ..handed(true) }.sanitized().ttl_ms, handoff::MIN_TTL_MS);
        assert!(!AgentHandoffOpen { thread: "../x".into(), seq: None }.is_valid());
        assert!(AgentHandoffOpen { thread: THREAD.into(), seq: Some(u64::MAX) }.is_valid());
    }

    #[test]
    fn a_result_this_build_doesnt_know_reads_as_away() {
        for known in [AgentHandedOff::opened(), AgentHandedOff::locked(), AgentHandedOff::away()] {
            assert!(known.is_valid() && known.is_known());
            assert_eq!(known.clone().sanitized(), known);
        }
        let later = AgentHandedOff { result: "on_the_tv".into() };
        assert!(later.is_valid() && !later.is_known());
        assert_eq!(later.sanitized(), AgentHandedOff::away());
        assert!(!AgentHandedOff { result: "Opened!".into() }.is_valid());
        assert!(!AgentHandedOff { result: String::new() }.is_valid());
    }

    #[test]
    fn the_handoff_has_a_lane_the_open_is_an_rpc_and_neither_is_on_bluetooth() {
        assert_eq!(lane_of(types::AGENT_HANDOFF), Some(Lane::Service(svc::AGENT)));
        let op = rpc_of(types::AGENT_HANDOFF_OPEN).expect("an RPC");
        assert_eq!((op.svc, op.op, op.paged), (svc::AGENT, "handoff.open", false));
        assert!(!ble::carries(types::AGENT_HANDOFF) && !ble::carries(types::AGENT_HANDOFF_OPEN));
    }
}
