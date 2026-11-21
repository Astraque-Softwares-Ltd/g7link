// SPDX-License-Identifier: MPL-2.0
//! Presence (`docs/protocol/agents.md` §7): what a PC says about itself
//! (someone is at it, it's locked, it shows no notifications, which threads
//! and sessions are on its screen), and which thread or session this phone
//! shows. Used for one thing: holding a notification for a thread you're
//! reading on the PC, and showing it the moment you stop.
//!
//! Plain protocol v1: no capability. It follows the PC's `agents` toggle;
//! sessions also its `agent_sessions` toggle. Nothing here is retried,
//! queued or carried over Bluetooth.

use g7link_core as core;

use crate::{G7LinkNode, parse_id};

/// What a PC says about itself, until `until_ms`. Unknown, stale or
/// offline is "not there": notify here.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPresence {
    /// Someone used the PC in the last 30 s, and it isn't locked.
    pub at_pc: bool,
    pub locked: bool,
    /// The PC is in Do Not Disturb: it shows no banner, so the phone must.
    pub dnd: bool,
    /// This phone's threads on the PC's screen, newest first; at most 4.
    pub viewing: Vec<String>,
    /// Sessions on the PC's screen, newest first; at most 4, and none
    /// while the PC's `agent_sessions` toggle is off for this phone.
    pub sessions: Vec<String>,
    /// This phone's clock when it came, plus how long it holds (Unix ms).
    /// `agent_presence` is null after it.
    pub until_ms: i64,
}

impl From<core::AgentPcPresence> for AgentPresence {
    fn from(p: core::AgentPcPresence) -> Self {
        Self {
            at_pc: p.at_pc,
            locked: p.locked,
            dnd: p.dnd,
            viewing: p.viewing,
            sessions: p.sessions,
            until_ms: p.until_ms,
        }
    }
}

/// What part of a thread this phone shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentPane {
    Thread,
    Changes,
    Plan,
    Requests,
}

impl AgentPane {
    fn wire(self) -> &'static str {
        use core::presence::pane;
        match self {
            Self::Thread => pane::THREAD,
            Self::Changes => pane::CHANGES,
            Self::Plan => pane::PLAN,
            Self::Requests => pane::REQUESTS,
        }
    }
}

#[uniffi::export]
impl G7LinkNode {
    /// What PC `id` last said about itself, while it holds: null once
    /// `until_ms` is past, while the link is down, before it said anything,
    /// and for an ID that isn't a paired PC's. No event says it lapsed:
    /// look again at `until_ms` and when the link changes.
    pub fn agent_presence(&self, id: String) -> Option<AgentPresence> {
        let id = parse_id(&id).ok()?;
        self.node.agent_presence_of(id).map(Into::into)
    }

    /// This phone shows `thread` or `session` of PC `id` (`visible`), or
    /// stopped (`visible` false; with neither: whatever it showed). Say it
    /// when a thread or session screen comes to the front or leaves, when
    /// the screen turns on or off, and every 30 s while it shows. The core
    /// sends them in order, at most one every 2 s: a repeat inside that is
    /// dropped, a change waits for its turn (the newest wins), and
    /// `visible` false goes at once. Never retried; ignored while the link
    /// is down, or with both a thread and a session.
    pub fn agent_viewing(
        &self,
        id: String,
        thread: Option<String>,
        session: Option<String>,
        pane: Option<AgentPane>,
        visible: bool,
    ) {
        let Ok(id) = parse_id(&id) else { return };
        let view = core::AgentViewing { thread, session, visible, pane: pane.map(|p| p.wire().to_owned()) };
        if let Err(error) = self.node.agent_viewing(id, view) {
            tracing::debug!(%error, "couldn't say what this phone shows");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pane_is_a_wire_word() {
        for pane in [AgentPane::Thread, AgentPane::Changes, AgentPane::Plan, AgentPane::Requests] {
            assert!(core::presence::pane::ALL.contains(&pane.wire()), "{pane:?}");
        }
    }

    #[test]
    fn a_presence_crosses_as_it_came() {
        let presence = core::AgentPcPresence {
            at_pc: true,
            locked: false,
            dnd: true,
            viewing: vec!["0123456789abcdef0123456789abcdef".into()],
            sessions: Vec::new(),
            until_ms: 1_795_000_060_000,
        };
        let crossed = AgentPresence::from(presence);
        assert!(crossed.at_pc && crossed.dnd && !crossed.locked);
        assert_eq!(crossed.viewing.len(), 1);
        assert_eq!(crossed.until_ms, 1_795_000_060_000);
    }
}
