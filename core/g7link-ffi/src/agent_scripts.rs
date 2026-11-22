// SPDX-License-Identifier: MPL-2.0
//! Project scripts on a PC's agent threads (`docs/protocol/agents.md` §12):
//! a thread's runs and their output, and starting and stopping one of its
//! folder's scripts.
//!
//! Plain protocol v1: no capability. Everything follows the PC's `agents`
//! toggle. Starting and stopping also follow the folder's "Phones can run
//! these" switch on the PC (`AgentFolder.phone_scripts`), which the PC's
//! agent host checks on every request: `Denied` while it's off. Ask the
//! user before every run, showing the command. Nothing of a run is kept
//! here. No `Debug` here prints a name, a command or a line.

use g7link_core as core;

use crate::{G7LinkError, G7LinkNode, Result, parse_id};

fn malformed(what: &str) -> G7LinkError {
    G7LinkError::Internal { reason: format!("invalid {what} reply") }
}

/// A script of a folder, as the PC's user typed it there: show `command`
/// in mono in the confirmation ("Run “%1” on %2?").
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentScript {
    /// Pass it to `agent_script_run`.
    pub id: String,
    pub name: String,
    /// The line the PC's shell runs, exactly.
    pub command: String,
}

impl std::fmt::Debug for AgentScript {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentScript").field("id", &self.id).finish_non_exhaustive()
    }
}

impl From<core::AgentScript> for AgentScript {
    fn from(s: core::AgentScript) -> Self {
        Self { id: s.id, name: s.name, command: s.command }
    }
}

/// Where a run stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentRunPhase {
    /// "Running"
    Running,
    /// "Finished"
    Done,
    /// "Failed · exit %1" with `exit`; "Couldn't start it." with `reason`
    /// `spawn`; else "Failed".
    Failed,
    /// "Stopped"
    Stopped,
}

impl AgentRunPhase {
    pub(crate) fn from_wire(word: &str) -> Option<Self> {
        use core::scripts::phase;
        Some(match word {
            phase::RUNNING => Self::Running,
            phase::DONE => Self::Done,
            phase::FAILED => Self::Failed,
            phase::STOPPED => Self::Stopped,
            _ => return None,
        })
    }
}

/// A run of a thread, in brief (`AgentThread.runs`).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentRunBrief {
    pub id: String,
    /// The script's name when it started.
    pub name: String,
    pub phase: AgentRunPhase,
    /// Its exit status, once it exited.
    pub exit: Option<i32>,
}

impl std::fmt::Debug for AgentRunBrief {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRunBrief")
            .field("id", &self.id)
            .field("phase", &self.phase)
            .field("exit", &self.exit)
            .finish_non_exhaustive()
    }
}

impl AgentRunBrief {
    pub(crate) fn from_core(r: core::AgentRunBrief) -> Option<Self> {
        Some(Self { phase: AgentRunPhase::from_wire(&r.phase)?, id: r.id, name: r.name, exit: r.exit })
    }
}

/// A run of a folder's script in a thread. Its later lines and its end come
/// as `Event.AgentScriptRun`; `agent_script_output` reads its newest 200
/// lines again.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentScriptRun {
    pub id: String,
    pub thread: String,
    /// The script's id.
    pub script: String,
    /// The script's name when it started.
    pub name: String,
    pub phase: AgentRunPhase,
    /// Its exit status, once it exited.
    pub exit: Option<i32>,
    /// Unix milliseconds.
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
    /// The phone that started it, by its name ("from %1"); null: the PC.
    pub by: Option<String>,
    /// This phone started it: it may always stop it.
    pub mine: bool,
    /// Plain text, oldest first; never Markdown or links.
    pub lines: Vec<String>,
    /// The number of `lines[0]`, counting from the run's first line.
    pub from: u64,
    /// Lines not kept, or left out to fit.
    pub dropped: u32,
    /// Why a failed run failed when it isn't its exit status: `spawn`
    /// ("Couldn't start it.") or `signal`; null otherwise.
    pub reason: Option<String>,
}

impl std::fmt::Debug for AgentScriptRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentScriptRun")
            .field("id", &self.id)
            .field("thread", &self.thread)
            .field("phase", &self.phase)
            .field("exit", &self.exit)
            .field("mine", &self.mine)
            .field("from", &self.from)
            .field("dropped", &self.dropped)
            .finish_non_exhaustive()
    }
}

impl AgentScriptRun {
    pub(crate) fn from_core(r: core::AgentScriptRun) -> Option<Self> {
        Some(Self {
            phase: AgentRunPhase::from_wire(&r.phase)?,
            id: r.id,
            thread: r.thread,
            script: r.script,
            name: r.name,
            exit: r.exit,
            started_ms: r.started_ms,
            ended_ms: r.ended_ms,
            by: r.by,
            mine: r.mine,
            lines: r.lines,
            from: r.from,
            dropped: r.dropped,
            reason: r.reason,
        })
    }
}

fn run_or_internal(run: core::AgentScriptRun, what: &str) -> Result<AgentScriptRun> {
    AgentScriptRun::from_core(run).ok_or_else(|| malformed(what))
}

// Errors as the other agent methods: `Denied` while the folder's "Phones
// can run these" is off on the PC (or its `agents` toggle is); `NotFound`
// for a thread, a run the PC no longer keeps, or a script no longer in the
// folder's list; `Internal` for the rest, whose reason starts `BUSY` while
// that script runs there already (`BUSY: script_running`) or too many run
// (`BUSY: runs`).
#[uniffi::export]
impl G7LinkNode {
    /// Runs script `script` of the thread's folder, after the user said yes
    /// to it on this phone. A run that couldn't start comes back `Failed`
    /// with `reason` `spawn`.
    pub async fn agent_script_run(
        &self,
        id: String,
        thread: String,
        script: String,
    ) -> Result<AgentScriptRun> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let run = self.run(async move { node.agent_script_run(id, thread, script).await }).await?;
        run_or_internal(run, "agent.script.run")
    }

    /// Stops a run: one this phone started (`mine`), or any of a folder
    /// whose "Phones can run these" is on.
    pub async fn agent_script_stop(&self, id: String, run: String) -> Result<AgentScriptRun> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let run = self.run(async move { node.agent_script_stop(id, run).await }).await?;
        run_or_internal(run, "agent.script.stop")
    }

    /// A thread's runs, newest first, at most 12, without lines.
    pub async fn agent_script_runs(&self, id: String, thread: String) -> Result<Vec<AgentScriptRun>> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let runs = self.run(async move { node.agent_script_runs(id, thread).await }).await?;
        Ok(runs.into_iter().filter_map(AgentScriptRun::from_core).collect())
    }

    /// A run with its newest 200 lines and 32 KiB: "Showing the last lines.
    /// The rest is on %1." when `from` or `dropped` isn't 0.
    pub async fn agent_script_output(&self, id: String, run: String) -> Result<AgentScriptRun> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let run = self.run(async move { node.agent_script_output(id, run).await }).await?;
        run_or_internal(run, "agent.script.output")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_crosses_with_its_words() {
        let run = core::AgentScriptRun {
            id: "0123456789abcdef".into(),
            thread: "t1".into(),
            script: "s0a1b2c3d".into(),
            name: "secret name".into(),
            phase: "failed".into(),
            exit: Some(3),
            started_ms: 1_795_000_000_000,
            ended_ms: Some(1_795_000_001_000),
            by: Some("Pixel".into()),
            mine: true,
            lines: vec!["secret line".into()],
            from: 4,
            dropped: 4,
            reason: None,
        };
        let run = AgentScriptRun::from_core(run).unwrap();
        assert_eq!((run.phase, run.exit, run.mine), (AgentRunPhase::Failed, Some(3), true));
        assert_eq!((run.started_ms, run.ended_ms), (1_795_000_000_000, Some(1_795_000_001_000)));
        assert!(!format!("{run:?}").contains("secret"));
        let later = core::AgentScriptRun { phase: "paused".into(), ..Default::default() };
        assert!(AgentScriptRun::from_core(later).is_none(), "a phase this build doesn't know");
        let brief =
            core::AgentRunBrief { id: "x".into(), name: "dev".into(), phase: "done".into(), exit: Some(0) };
        assert_eq!(AgentRunBrief::from_core(brief).map(|b| b.phase), Some(AgentRunPhase::Done));
        let script = AgentScript::from(core::AgentScript {
            id: "s0a1b2c3d".into(),
            name: "secret".into(),
            command: "secret".into(),
        });
        assert!(!format!("{script:?}").contains("secret"));
    }
}
