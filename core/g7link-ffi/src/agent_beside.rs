// SPDX-License-Identifier: MPL-2.0
//! The phone beside the PC (`docs/protocol/agents.md` §11): a PC asks this
//! phone to show a pane beside a thread on the PC's screen
//! (`Event.AgentBeside`), and a thread lists the ports its agent listens on
//! (`AgentThread.ports`), so a dev server that answers on the network can be
//! opened here (`agent_port_url`).
//!
//! Plain protocol v1: no capability. It follows the PC's `agents` toggle.
//! Nothing here opens, forwards or tunnels a port, is retried, queued or
//! carried over Bluetooth.

use g7link_core as core;

use crate::{AgentPane, G7LinkNode, parse_id};

/// A TCP port a thread's agent, or a process it started, listens on on the
/// PC: a dev server. `lan`: it answers on the network ("Open on phone");
/// otherwise only on the PC itself ("It only answers on %1 itself.").
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPort {
    pub port: u16,
    pub lan: bool,
}

impl From<core::AgentPort> for AgentPort {
    fn from(p: core::AgentPort) -> Self {
        Self { lan: p.is_lan(), port: p.port }
    }
}

impl AgentPane {
    /// A pane word this build knows.
    pub(crate) fn from_wire(word: &str) -> Option<Self> {
        use core::presence::pane;
        Some(match word {
            pane::THREAD => Self::Thread,
            pane::CHANGES => Self::Changes,
            pane::PLAN => Self::Plan,
            pane::REQUESTS => Self::Requests,
            _ => return None,
        })
    }
}

#[uniffi::export]
impl G7LinkNode {
    /// Where a dev server on `port` of PC `id` answers here:
    /// `http://<the PC's address on this link>:<port>/`. Null unless the link
    /// to that PC is up over the local network (not the relay, not USB) at
    /// an address this phone's browser can open, and for a port under 1024.
    /// Ask it for a `lan` port only, each time before opening it (the link
    /// may have changed); open it through the link sheet's confirmation.
    pub fn agent_port_url(&self, id: String, port: u16) -> Option<String> {
        let id = parse_id(&id).ok()?;
        self.node.agent_port_url(id, port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pane_word_reads_back_and_a_later_one_doesnt() {
        for pane in [AgentPane::Thread, AgentPane::Changes, AgentPane::Plan, AgentPane::Requests] {
            assert_eq!(AgentPane::from_wire(pane.wire()), Some(pane));
        }
        assert_eq!(AgentPane::from_wire("diff_beside"), None);
    }

    #[test]
    fn a_threads_ports_cross_in_their_order() {
        let thread = core::AgentThread {
            id: "0123456789abcdef0123456789abcdef".into(),
            agent: "claude".into(),
            folder: "g7linux".into(),
            title: "Start the dev server".into(),
            state: "working".into(),
            mode: "supervised".into(),
            updated: 1_795_000_000_000,
            ports: vec![core::AgentPort::new(5173, true), core::AgentPort::new(8080, false)],
            ..Default::default()
        };
        let crossed = crate::AgentThread::from_core(thread.clone()).unwrap();
        assert_eq!(
            crossed.ports,
            [AgentPort { port: 5173, lan: true }, AgentPort { port: 8080, lan: false }]
        );
        let none = crate::AgentThread::from_core(core::AgentThread { ports: Vec::new(), ..thread }).unwrap();
        assert!(none.ports.is_empty());
    }

    #[test]
    fn a_port_crosses_with_its_reach() {
        assert_eq!(AgentPort::from(core::AgentPort::new(5173, true)), AgentPort { port: 5173, lan: true });
        assert_eq!(AgentPort::from(core::AgentPort::new(8080, false)), AgentPort { port: 8080, lan: false });
    }
}
