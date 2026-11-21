// SPDX-License-Identifier: MPL-2.0
//! Hand-off (`docs/protocol/agents.md` §10): a PC hands this phone a thread
//! (`Event.AgentHandoff`: "Continue on this phone", or offered quietly when
//! the PC locks with it on screen), and this phone hands one to its PC
//! (`agent_handoff`: "Continue on <PC>"). Either side only opens the thread
//! at the item that was on the sender's screen: nothing is sent, typed or
//! unlocked. Drafts come with `agent_draft_get`, as always.
//!
//! Plain protocol v1: no capability. It follows the PC's `agents` toggle.
//! Nothing here is retried, queued or carried over Bluetooth.

use g7link_core as core;

use crate::{G7LinkNode, Result, parse_id};

/// What a PC did with a thread this phone handed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentHandoffResult {
    /// It's on the PC's screen, and someone is at the PC.
    Opened,
    /// The PC is locked: "Opens when <PC> is unlocked." (within 10 minutes).
    Locked,
    /// It's open on the PC, but nobody has used the PC in the last 30 s. A
    /// result this build doesn't know comes as this.
    Away,
}

impl AgentHandoffResult {
    fn from_wire(word: &str) -> Self {
        use core::agent_handoff::handoff::result;
        match word {
            result::OPENED => Self::Opened,
            result::LOCKED => Self::Locked,
            _ => Self::Away,
        }
    }
}

#[uniffi::export]
impl G7LinkNode {
    /// Hands `thread` to PC `id` ("Continue on <PC>"): it opens on the PC's
    /// screen at item `seq` (the first item on this screen), with "Opened
    /// from <phone>". Save the draft first (`agent_draft_set`). Sent once,
    /// never retried. `Denied` while the PC's `agents` toggle is off for
    /// this phone (the first time, the PC asks its user); `NotFound` for a
    /// thread the PC doesn't have; `Offline`: nothing was sent.
    pub async fn agent_handoff(
        &self,
        id: String,
        thread: String,
        seq: Option<u64>,
    ) -> Result<AgentHandoffResult> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let done = self.run(async move { node.agent_handoff_open(id, thread, seq).await }).await?;
        Ok(AgentHandoffResult::from_wire(&done.result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_result_word_has_a_result_and_a_later_one_reads_as_away() {
        use g7link_core::agent_handoff::handoff::result;
        assert_eq!(AgentHandoffResult::from_wire(result::OPENED), AgentHandoffResult::Opened);
        assert_eq!(AgentHandoffResult::from_wire(result::LOCKED), AgentHandoffResult::Locked);
        assert_eq!(AgentHandoffResult::from_wire(result::AWAY), AgentHandoffResult::Away);
        assert_eq!(AgentHandoffResult::from_wire("on_the_tv"), AgentHandoffResult::Away);
    }
}
