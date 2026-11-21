// SPDX-License-Identifier: MPL-2.0
//! Bodies of presence (`docs/protocol/agents.md` §7): what a phone learns
//! about its PC (someone is at it, it is locked, it shows no notifications,
//! and which of the phone's threads and sessions are on its screen), and
//! which thread or session the phone shows. Plain protocol v1: no
//! capability, governed by the `agents` toggle (and `agent_sessions` for
//! sessions).
//!
//! A phone never learns an idle time, a window title, an app name or
//! another device's view. A PC learns no more of a phone than the thread or
//! session on its screen and the pane it shows.

use serde::{Deserialize, Serialize};

use super::{fits_word, is_valid_agent_id, known_word};

/// Limits, times and wire words of §7.
pub mod presence {
    /// Threads in `agent.presence`'s `viewing`.
    pub const MAX_VIEWING: usize = 4;
    /// Sessions in `agent.presence`'s `sessions`.
    pub const MAX_SESSIONS: usize = 4;
    /// The range of `agent.presence`'s `ttl_ms`.
    pub const MIN_TTL_MS: u64 = 1_000;
    pub const MAX_TTL_MS: u64 = 120_000;
    /// The `ttl_ms` a PC sends.
    pub const TTL_MS: u64 = 60_000;
    /// A PC sends `agent.presence` at least this often to a phone that is
    /// online, besides on every change.
    pub const HEARTBEAT_MS: u64 = 20_000;
    /// A phone sends `agent.viewing` this often while a thread or session
    /// shows.
    pub const VIEW_HEARTBEAT_MS: u64 = 30_000;
    /// How long a PC trusts one `agent.viewing`.
    pub const VIEW_LIFE_MS: u64 = 60_000;
    /// The least time between two `agent.viewing` from one phone; a
    /// `visible: false` doesn't wait.
    pub const VIEW_GAP_MS: u64 = 2_000;

    /// `agent.viewing`'s `pane`. A receiver reads a word it doesn't know as absent.
    pub mod pane {
        pub const THREAD: &str = "thread";
        pub const CHANGES: &str = "changes";
        pub const PLAN: &str = "plan";
        pub const REQUESTS: &str = "requests";
        pub const ALL: &[&str] = &[THREAD, CHANGES, PLAN, REQUESTS];
    }
}

/// Ids, each an id and none twice.
fn are_distinct_ids(ids: &[String]) -> bool {
    ids.iter().all(|id| is_valid_agent_id(id)) && ids.iter().enumerate().all(|(i, id)| !ids[..i].contains(id))
}

/// `ids` without what isn't an id and without repeats, the first `max` kept.
fn distinct_ids(ids: Vec<String>, max: usize) -> Vec<String> {
    let mut kept: Vec<String> = Vec::with_capacity(ids.len().min(max));
    for id in ids {
        if kept.len() == max {
            break;
        }
        if is_valid_agent_id(&id) && !kept.contains(&id) {
            kept.push(id);
        }
    }
    kept
}

/// Body of `agent.presence` (PC → phone, `agent` lane): what the PC says
/// about itself. The phone trusts it for `ttl_ms` from when it arrives.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPresence {
    /// Someone used the PC in the last 30 s, and it isn't locked.
    pub at_pc: bool,
    pub locked: bool,
    /// The PC shows no notifications (Do Not Disturb).
    pub dnd: bool,
    /// Threads on the PC's screen, newest first; at most
    /// [`presence::MAX_VIEWING`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub viewing: Vec<String>,
    /// Sessions on the PC's screen, newest first; at most
    /// [`presence::MAX_SESSIONS`], and none to a phone without
    /// `agent_sessions`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<String>,
    /// [`presence::MIN_TTL_MS`] to [`presence::MAX_TTL_MS`].
    pub ttl_ms: u64,
}

impl AgentPresence {
    pub fn is_valid(&self) -> bool {
        (presence::MIN_TTL_MS..=presence::MAX_TTL_MS).contains(&self.ttl_ms)
            && self.viewing.len() <= presence::MAX_VIEWING
            && self.sessions.len() <= presence::MAX_SESSIONS
            && are_distinct_ids(&self.viewing)
            && are_distinct_ids(&self.sessions)
    }

    /// Within its limits, to send: what isn't an id and repeats left out,
    /// each list cut to its most, `ttl_ms` brought into its range, and not
    /// at the PC while it's locked.
    pub fn sanitized(mut self) -> Self {
        self.viewing = distinct_ids(self.viewing, presence::MAX_VIEWING);
        self.sessions = distinct_ids(self.sessions, presence::MAX_SESSIONS);
        self.ttl_ms = self.ttl_ms.clamp(presence::MIN_TTL_MS, presence::MAX_TTL_MS);
        self.at_pc &= !self.locked;
        self
    }
}

/// Body of `agent.viewing` (phone → PC, RPC `agent/viewing`, answered with
/// a plain `ok`): the thread or session this phone shows (`visible`), or
/// that it stopped (`visible: false`; without a thread or session: whatever
/// it showed).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentViewing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub visible: bool,
    /// One of [`presence::pane`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
}

impl AgentViewing {
    /// At most one of a thread and a session, which a visible view needs;
    /// a `pane` this build may not know is still a word.
    pub fn is_valid(&self) -> bool {
        self.thread.as_deref().is_none_or(is_valid_agent_id)
            && self.session.as_deref().is_none_or(is_valid_agent_id)
            && !(self.thread.is_some() && self.session.is_some())
            && (!self.visible || self.thread.is_some() || self.session.is_some())
            && fits_word(&self.pane)
    }

    /// With a `pane` this build doesn't know read as absent.
    pub fn sanitized(mut self) -> Self {
        known_word(&mut self.pane, presence::pane::ALL);
        self
    }

    /// Whether this names the same thread or session as `other`.
    pub fn same_target(&self, other: &AgentViewing) -> bool {
        self.thread == other.thread && self.session == other.session
    }

    /// Whether it names neither a thread nor a session.
    pub fn names_nothing(&self) -> bool {
        self.thread.is_none() && self.session.is_none()
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
    const OTHER: &str = "fedcba9876543210fedcba9876543210";
    const SESSION: &str = "sc0123456789abcdef0123456789abcdef";

    fn presence(viewing: &[&str]) -> AgentPresence {
        AgentPresence {
            at_pc: true,
            locked: false,
            dnd: false,
            viewing: viewing.iter().map(|id| (*id).to_owned()).collect(),
            sessions: vec![SESSION.into()],
            ttl_ms: presence::TTL_MS,
        }
    }

    fn view(thread: Option<&str>, visible: bool) -> AgentViewing {
        AgentViewing { thread: thread.map(Into::into), session: None, visible, pane: None }
    }

    #[test]
    fn both_bodies_round_trip() {
        let sent = presence(&[THREAD, OTHER]);
        let env = Envelope::new(types::AGENT_PRESENCE, &sent).unwrap();
        let back = Envelope::from_cbor(&env.to_cbor()).unwrap();
        assert_eq!(back.body::<AgentPresence>().unwrap(), sent);
        let viewing = AgentViewing { pane: Some(presence::pane::CHANGES.into()), ..view(Some(THREAD), true) };
        let env = Envelope::new(types::AGENT_VIEWING, &viewing).unwrap();
        let back = Envelope::from_cbor(&env.to_cbor()).unwrap();
        assert_eq!(back.body::<AgentViewing>().unwrap(), viewing);
        // Empty lists and absent fields stay off the wire.
        let bare = AgentPresence { ttl_ms: presence::TTL_MS, ..Default::default() };
        let env = Envelope::new(types::AGENT_PRESENCE, &bare).unwrap();
        assert_eq!(Envelope::from_cbor(&env.to_cbor()).unwrap().body::<AgentPresence>().unwrap(), bare);
    }

    #[test]
    fn a_presence_out_of_its_limits_is_refused() {
        assert!(presence(&[THREAD]).is_valid());
        let five: Vec<String> = (0..5).map(|i| format!("{i}{}", &THREAD[1..])).collect();
        assert!(!AgentPresence { viewing: five.clone(), ..presence(&[]) }.is_valid());
        assert!(!AgentPresence { sessions: five, ..presence(&[]) }.is_valid());
        assert!(!AgentPresence { ttl_ms: 999, ..presence(&[]) }.is_valid());
        assert!(!AgentPresence { ttl_ms: 120_001, ..presence(&[]) }.is_valid());
        assert!(AgentPresence { ttl_ms: 1_000, ..presence(&[]) }.is_valid());
        assert!(AgentPresence { ttl_ms: 120_000, ..presence(&[]) }.is_valid());
        assert!(!presence(&[THREAD, THREAD]).is_valid(), "twice");
        assert!(!presence(&["/home/me"]).is_valid(), "not an id");
    }

    #[test]
    fn a_presence_is_made_to_fit_before_it_goes() {
        let mut many: Vec<String> = vec!["Not An Id".into(), THREAD.into(), THREAD.into()];
        many.extend((0..6).map(|i| format!("{i}{}", &OTHER[1..])));
        let sent =
            AgentPresence { viewing: many, locked: true, ttl_ms: 600_000, ..presence(&[]) }.sanitized();
        assert!(sent.is_valid());
        assert_eq!(sent.viewing.len(), presence::MAX_VIEWING);
        assert_eq!(sent.viewing[0], THREAD);
        assert_eq!(sent.ttl_ms, presence::MAX_TTL_MS);
        assert!(!sent.at_pc, "a locked PC has nobody at it");
        assert_eq!(AgentPresence { ttl_ms: 5, ..presence(&[]) }.sanitized().ttl_ms, presence::MIN_TTL_MS);
    }

    #[test]
    fn a_view_names_one_thing_and_an_unknown_pane_reads_as_absent() {
        assert!(view(Some(THREAD), true).is_valid());
        assert!(view(None, false).is_valid(), "stopped showing whatever it showed");
        assert!(!view(None, true).is_valid(), "showing nothing");
        let both = AgentViewing { session: Some(SESSION.into()), ..view(Some(THREAD), true) };
        assert!(!both.is_valid());
        let session = AgentViewing { session: Some(SESSION.into()), ..view(None, true) };
        assert!(session.is_valid());
        assert!(!view(Some("../x"), true).is_valid());
        let later = AgentViewing { pane: Some("diff_beside".into()), ..view(Some(THREAD), true) };
        assert!(later.is_valid());
        assert_eq!(later.sanitized().pane, None);
        let plan = AgentViewing { pane: Some(presence::pane::PLAN.into()), ..view(Some(THREAD), true) };
        assert_eq!(plan.sanitized().pane.as_deref(), Some(presence::pane::PLAN));
        let long = AgentViewing { pane: Some("x".repeat(33)), ..view(Some(THREAD), true) };
        assert!(!long.is_valid());
        assert!(view(Some(THREAD), true).same_target(&view(Some(THREAD), false)));
        assert!(!view(Some(THREAD), true).same_target(&view(Some(OTHER), true)));
        assert!(view(None, false).names_nothing());
    }

    #[test]
    fn presence_has_a_lane_and_viewing_is_an_rpc_and_neither_is_on_bluetooth() {
        assert_eq!(lane_of(types::AGENT_PRESENCE), Some(Lane::Service(svc::AGENT)));
        let op = rpc_of(types::AGENT_VIEWING).expect("an RPC");
        assert_eq!((op.svc, op.op, op.paged), (svc::AGENT, "viewing", false));
        assert!(!ble::carries(types::AGENT_PRESENCE) && !ble::carries(types::AGENT_VIEWING));
    }
}
