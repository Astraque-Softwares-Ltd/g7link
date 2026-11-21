// SPDX-License-Identifier: MPL-2.0
//! Bodies of the phone beside the PC (`docs/protocol/agents.md` §11): a PC
//! switches the pane a phone shows beside a thread that is on the PC's
//! screen (`agent.beside`), and a thread lists the TCP ports its agent's
//! processes listen on (`AgentThread.ports`), so a phone can open a dev
//! server that already answers on the network. Plain protocol v1: no
//! capability, governed by the `agents` toggle.
//!
//! Nothing here opens, forwards or tunnels a port: a phone opens one in its
//! own browser over its own network, or not at all.

use serde::{Deserialize, Serialize};

use super::{AgentThread, is_agent_word, is_valid_agent_id, presence};

/// Limits and wire words of §11.
pub mod beside {
    /// `AgentThread.ports`, oldest first.
    pub const MAX_PORTS: usize = 4;
    /// A PC lists no port under this.
    pub const MIN_PORT: u16 = 1024;

    /// `AgentPort.reach`. A phone leaves out a port whose word it doesn't know.
    pub mod reach {
        /// Bound to loopback only: it answers on the PC alone.
        pub const PC: &str = "pc";
        /// Bound to every address or a network one: a phone on the same
        /// network may open it.
        pub const LAN: &str = "lan";
        pub const ALL: &[&str] = &[PC, LAN];
    }
}

/// Body of `agent.beside` (PC → phone, `agent` lane): show `pane` beside
/// `thread`, which the phone says it shows (`agent.viewing`). `pane` is one
/// of [`presence::pane`]; a phone drops a word it doesn't know.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentBeside {
    pub thread: String,
    pub pane: String,
}

impl AgentBeside {
    /// A thread id and a word, perhaps a pane this build doesn't know.
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.thread) && is_agent_word(&self.pane)
    }

    /// Whether `pane` is one this build knows: all a PC may send.
    pub fn is_known(&self) -> bool {
        presence::pane::ALL.contains(&self.pane.as_str())
    }
}

/// A TCP port a thread's agent, or a process it started, listens on: a dev
/// server (`AgentThread.ports`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentPort {
    pub port: u16,
    /// One of [`beside::reach`].
    pub reach: String,
}

impl AgentPort {
    /// Port `port`, answering on the network (`lan`) or on the PC alone.
    pub fn new(port: u16, lan: bool) -> Self {
        Self { port, reach: if lan { beside::reach::LAN } else { beside::reach::PC }.to_owned() }
    }

    /// It answers on the network: a phone on a LAN link may open it.
    pub fn is_lan(&self) -> bool {
        self.reach == beside::reach::LAN
    }

    fn is_valid(&self) -> bool {
        self.port >= beside::MIN_PORT && is_agent_word(&self.reach)
    }

    fn is_known(&self) -> bool {
        beside::reach::ALL.contains(&self.reach.as_str())
    }
}

/// Whether the field §11 added to `thread` is within its limits.
pub(super) fn is_valid_thread_ports(thread: &AgentThread) -> bool {
    let ports = &thread.ports;
    ports.len() <= beside::MAX_PORTS
        && ports.iter().all(AgentPort::is_valid)
        && ports.iter().enumerate().all(|(i, p)| !ports[..i].iter().any(|q| q.port == p.port))
}

/// The field §11 added to `thread`, within its limits: ports out of them, a
/// repeat, and a `reach` this build doesn't know left out; the first
/// [`beside::MAX_PORTS`] kept.
pub(super) fn clean_thread_ports(thread: &mut AgentThread) {
    let mut kept: Vec<AgentPort> = Vec::with_capacity(thread.ports.len().min(beside::MAX_PORTS));
    for port in std::mem::take(&mut thread.ports) {
        if kept.len() == beside::MAX_PORTS {
            break;
        }
        if port.is_valid() && port.is_known() && !kept.iter().any(|k| k.port == port.port) {
            kept.push(port);
        }
    }
    thread.ports = kept;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Envelope, ble,
        lanes::{Lane, lane_of, svc},
        messages::{agent, types},
    };

    const THREAD: &str = "0123456789abcdef0123456789abcdef";

    fn thread(ports: Vec<AgentPort>) -> AgentThread {
        AgentThread {
            id: THREAD.into(),
            agent: "claude".into(),
            folder: "g7linux".into(),
            title: "Start the dev server".into(),
            state: agent::state::WORKING.into(),
            mode: agent::mode::SUPERVISED.into(),
            updated: 1_795_000_000_000,
            ports,
            ..Default::default()
        }
    }

    #[test]
    fn the_beside_and_a_threads_ports_round_trip() {
        let sent = AgentBeside { thread: THREAD.into(), pane: presence::pane::PLAN.into() };
        let env = Envelope::new(types::AGENT_BESIDE, &sent).unwrap();
        assert_eq!(Envelope::from_cbor(&env.to_cbor()).unwrap().body::<AgentBeside>().unwrap(), sent);
        let with = thread(vec![AgentPort::new(5173, true), AgentPort::new(8080, false)]);
        let env = Envelope::new(types::AGENT_THREAD, &with).unwrap();
        let back = Envelope::from_cbor(&env.to_cbor()).unwrap().body::<AgentThread>().unwrap();
        assert_eq!(back, with);
        assert!(back.is_valid());
        assert!(back.ports[0].is_lan() && !back.ports[1].is_lan());
        assert_eq!(back.ports[1].reach, beside::reach::PC);
        // None: the field stays off the wire, and an older PC's thread reads as none.
        let bare = thread(Vec::new());
        let env = Envelope::new(types::AGENT_THREAD, &bare).unwrap();
        assert_eq!(Envelope::from_cbor(&env.to_cbor()).unwrap().body::<AgentThread>().unwrap(), bare);
    }

    #[test]
    fn a_beside_names_a_thread_and_a_pane() {
        let plan = AgentBeside { thread: THREAD.into(), pane: presence::pane::PLAN.into() };
        assert!(plan.is_valid() && plan.is_known());
        for pane in presence::pane::ALL {
            assert!(AgentBeside { pane: (*pane).into(), ..plan.clone() }.is_known());
        }
        let later = AgentBeside { pane: "diff_beside".into(), ..plan.clone() };
        assert!(later.is_valid() && !later.is_known(), "a word a later PC may send");
        assert!(!AgentBeside { pane: "Plan!".into(), ..plan.clone() }.is_valid());
        assert!(!AgentBeside { pane: String::new(), ..plan.clone() }.is_valid());
        assert!(!AgentBeside { pane: "p".repeat(33), ..plan.clone() }.is_valid());
        assert!(!AgentBeside { thread: "/home/me".into(), ..plan.clone() }.is_valid());
        assert!(!AgentBeside { thread: String::new(), ..plan }.is_valid());
    }

    #[test]
    fn ports_out_of_their_limits_are_refused_and_left_out() {
        let five: Vec<AgentPort> = (0..5).map(|n| AgentPort::new(3000 + n, true)).collect();
        assert!(thread(five[..4].to_vec()).is_valid());
        assert!(!thread(five.clone()).is_valid(), "five");
        assert!(!thread(vec![AgentPort::new(80, true)]).is_valid(), "under 1024");
        assert!(!thread(vec![AgentPort::new(3000, true), AgentPort::new(3000, false)]).is_valid(), "twice");
        assert!(!thread(vec![AgentPort { port: 3000, reach: "Lan!".into() }]).is_valid());
        let later = AgentPort { port: 3001, reach: "tunnel".into() };
        assert!(thread(vec![later.clone()]).is_valid(), "a word a later PC may send");

        let mut sent = vec![
            AgentPort::new(80, true),
            later,
            AgentPort::new(3000, false),
            AgentPort::new(3000, true),
            AgentPort { port: 3002, reach: String::new() },
        ];
        sent.extend(five);
        let got = thread(sent).sanitized().unwrap();
        assert!(got.is_valid());
        let kept: Vec<(u16, bool)> = got.ports.iter().map(|p| (p.port, p.is_lan())).collect();
        assert_eq!(kept, [(3000, false), (3001, true), (3002, true), (3003, true)]);
    }

    #[test]
    fn the_beside_rides_the_agent_lane_and_never_bluetooth() {
        assert_eq!(lane_of(types::AGENT_BESIDE), Some(Lane::Service(svc::AGENT)));
        assert!(!ble::carries(types::AGENT_BESIDE));
    }
}
