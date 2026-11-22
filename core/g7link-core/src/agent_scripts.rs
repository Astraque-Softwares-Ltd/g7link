// SPDX-License-Identifier: MPL-2.0
//! Project scripts on a PC's agent threads, from a phone
//! (`docs/protocol/agents.md` §12): a thread's runs and their output, and
//! starting and stopping one of its folder's scripts.
//!
//! PC side: the core checks the `agents` toggle and every limit before
//! anything reaches the platform, and refuses an op it doesn't know. It
//! never decides whether a phone may run a folder's scripts: the agent host
//! checks the folder's "Phones can run these" on every request, and the
//! platform always passes the request on as the phone's (`by` it). Every
//! answer and event is made to fit the phone: no command, no path, the
//! newest lines only. Phone side: it sends the requests and turns
//! `agent.script` into [`NodeEvent::AgentScriptRun`]. Nothing of a run is
//! kept on the phone.

use std::sync::Arc;

pub use g7link_protocol::messages::{
    AgentRunBrief, AgentRunRef, AgentScript, AgentScriptProgress, AgentScriptRun, AgentScriptRunRequest,
    AgentScriptRuns, AgentScriptRunsRequest, is_run_id, is_script_id, scripts,
};
use g7link_protocol::{
    DeviceId, Envelope, ErrorCode,
    messages::{agent, types},
};

use crate::{
    Error, Node, Result,
    agent_git::AgentJobError,
    agents::{self, AgentFailure, AgentReply, AgentRequest},
    dispatch::Responder,
    events::NodeEvent,
    node::Shared,
};

/// Whether `t` is one of §12's requests.
pub(crate) fn is_request(t: &str) -> bool {
    matches!(
        t,
        types::AGENT_SCRIPT_RUN
            | types::AGENT_SCRIPT_STOP
            | types::AGENT_SCRIPT_RUNS
            | types::AGENT_SCRIPT_OUTPUT
    )
}

/// Whether `t` is §12's event (PC → phone).
pub(crate) fn is_event(t: &str) -> bool {
    t == types::AGENT_SCRIPT
}

// ---- PC side ----

/// A PC's answer to one of §12's requests: one envelope, or a run whose
/// lines may go in parts.
enum Answer {
    Plain(Envelope),
    Output(Box<AgentScriptRun>),
}

fn ok_with<T: serde::Serialize>(body: &T) -> Answer {
    Answer::Plain(Envelope::new(types::OK, body).unwrap_or_else(|_| AgentFailure::Internal.reply("failed")))
}

fn bad() -> Envelope {
    Envelope::error(ErrorCode::BadMessage, "invalid agent request")
}

fn malformed() -> Answer {
    Answer::Plain(AgentFailure::Internal.reply("the host's answer was malformed"))
}

/// PC: answers one of §12's requests from `peer`.
pub(crate) async fn handle_request(
    shared: &Arc<Shared>,
    peer: DeviceId,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<()> {
    match answer(shared, peer, env).await {
        Answer::Plain(reply) => rsp.send(reply).await,
        Answer::Output(run) => rsp.paged(*run).await,
    }
}

/// The request `env` asks for, once its body is within its limits; the
/// answer to send instead when it isn't, or the op isn't one of §12's.
fn request_of(env: &Envelope) -> std::result::Result<AgentRequest, Envelope> {
    Ok(match env.t.as_str() {
        types::AGENT_SCRIPT_RUN => {
            let r = agents::decode::<AgentScriptRunRequest>(env, AgentScriptRunRequest::is_valid)
                .ok_or_else(bad)?;
            AgentRequest::ScriptRun { thread: r.thread, script: r.script }
        }
        types::AGENT_SCRIPT_STOP => {
            let r = agents::decode::<AgentRunRef>(env, AgentRunRef::is_valid).ok_or_else(bad)?;
            AgentRequest::ScriptStop { run: r.run }
        }
        types::AGENT_SCRIPT_RUNS => {
            let r = agents::decode::<AgentScriptRunsRequest>(env, AgentScriptRunsRequest::is_valid)
                .ok_or_else(bad)?;
            AgentRequest::ScriptRuns { thread: r.thread }
        }
        types::AGENT_SCRIPT_OUTPUT => {
            let r = agents::decode::<AgentRunRef>(env, AgentRunRef::is_valid).ok_or_else(bad)?;
            AgentRequest::ScriptOutput { run: r.run }
        }
        _ => return Err(Envelope::error(ErrorCode::Unsupported, "unknown agent request")),
    })
}

/// The refusal for a host's reason: the error a phone words, with the
/// reason as its message.
fn refusal_of(error: &AgentJobError) -> Envelope {
    let error = error.clone().sanitized();
    let code = match error.reason.as_str() {
        scripts::refused::SCRIPT_RUNNING | scripts::refused::RUNS => ErrorCode::Busy,
        scripts::refused::PHONE_SCRIPTS_OFF | "denied" | "readonly" => ErrorCode::Denied,
        scripts::refused::NO_SCRIPT | "not_found" => ErrorCode::NotFound,
        _ => ErrorCode::BadMessage,
    };
    Envelope::error(code, error.reason)
}

/// `run` for `peer`: its newest lines within `max_lines` and `max_bytes`,
/// `mine` when `peer` started it.
fn run_for(
    run: AgentScriptRun,
    by: Option<DeviceId>,
    peer: &DeviceId,
    max_lines: usize,
    max_bytes: usize,
) -> Option<AgentScriptRun> {
    let mut run = run.sanitized(max_lines, max_bytes)?;
    run.mine = by.as_ref() == Some(peer);
    Some(run)
}

/// Whether the host's run answers `request`: the thread asked about, or the
/// run named.
fn answers(request: &AgentRequest, run: &AgentScriptRun) -> bool {
    match request {
        AgentRequest::ScriptRun { thread, .. } => run.thread == *thread,
        AgentRequest::ScriptStop { run: id } | AgentRequest::ScriptOutput { run: id } => run.id == *id,
        _ => false,
    }
}

async fn answer(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) -> Answer {
    if let Some(refused) = agents::refusal(shared, peer, false) {
        return Answer::Plain(refused);
    }
    let request = match request_of(env) {
        Ok(request) => request,
        Err(refused) => return Answer::Plain(refused),
    };
    let asked = request.clone();
    let reply = match agents::ask_platform(shared, peer, request).await {
        Ok(reply) => reply,
        Err(failure) => return Answer::Plain(failure.reply(agents::failure_message(failure))),
    };
    match (asked, reply) {
        (_, AgentReply::Refused(error)) => Answer::Plain(refusal_of(&error)),
        (AgentRequest::ScriptRuns { thread }, AgentReply::ScriptRuns(runs)) => {
            let runs = runs
                .into_iter()
                .filter(|(run, _)| run.thread == thread)
                .filter_map(|(run, by)| run_for(run, by, &peer, 0, 0))
                .take(scripts::MAX_RUNS)
                .collect();
            ok_with(&AgentScriptRuns { runs })
        }
        (asked @ AgentRequest::ScriptOutput { .. }, AgentReply::ScriptRun { run, by })
            if answers(&asked, &run) =>
        {
            match run_for(run, by, &peer, scripts::MAX_OUTPUT_LINES, scripts::MAX_OUTPUT_BYTES) {
                Some(run) => Answer::Output(Box::new(run)),
                None => malformed(),
            }
        }
        (
            asked @ (AgentRequest::ScriptRun { .. } | AgentRequest::ScriptStop { .. }),
            AgentReply::ScriptRun { run, by },
        ) if answers(&asked, &run) => {
            match run_for(run, by, &peer, scripts::MAX_LIVE_LINES, scripts::MAX_LIVE_BYTES) {
                Some(run) => ok_with(&run),
                None => malformed(),
            }
        }
        _ => Answer::Plain(AgentFailure::Internal.reply("the host answered something else")),
    }
}

/// PC: a run's new lines and where it stands, to every phone that gets
/// agent events (`agent.script`): its newest 40 lines and 8 KiB, `mine` for
/// the phone that started it.
pub(crate) fn send_progress(shared: &Arc<Shared>, run: AgentScriptRun, by: Option<DeviceId>) -> Result<()> {
    let run = run.for_live().ok_or_else(|| Error::Protocol("invalid agent.script".into()))?;
    for session in shared.agent_recipients() {
        let mut run = run.clone();
        run.mine = by == Some(session.peer);
        let env = Envelope::new(types::AGENT_SCRIPT, &AgentScriptProgress { run })?;
        let _ = session.send_now(env);
    }
    Ok(())
}

// ---- Phone side ----

/// Phone: a PC's `agent.script`. One out of its limits, or with a phase
/// this build doesn't know, is dropped.
pub(crate) fn handle_event(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) {
    if !shared.we_offer(agent::REMOTE) || env.t != types::AGENT_SCRIPT {
        return;
    }
    let Some(progress) = agents::decode::<AgentScriptProgress>(env, AgentScriptProgress::is_valid) else {
        tracing::debug!(peer = %peer.short(), "dropped an agent.script out of its limits");
        return;
    };
    if let Some(run) = progress.run.for_live() {
        shared.emit(NodeEvent::AgentScriptRun { device: peer, run });
    }
}

fn invalid(what: &str) -> Error {
    agents::invalid(what)
}

async fn single<T: serde::de::DeserializeOwned>(
    shared: &Arc<Shared>,
    peer: DeviceId,
    t: &str,
    body: &impl serde::Serialize,
) -> Result<T> {
    agents::request_of(shared, peer, Envelope::new(t, body)?, agent::HOST).await
}

/// A run a PC answered with, checked: within an answer's limits, and the
/// one asked about.
fn checked(run: AgentScriptRun, t: &str, fits: impl Fn(&AgentScriptRun) -> bool) -> Result<AgentScriptRun> {
    run.for_output().filter(|r| fits(r)).ok_or_else(|| invalid(&format!("{t} reply")))
}

/// The node's §12 methods. PC: what the platform pushes. Phone: requests to
/// a PC (`peer`).
impl Node {
    /// PC: a run's new lines and where it stands (the host's
    /// `script.progress`), at most 4 a second; `by` is the device that
    /// started it (`None`: this PC). Each phone that gets agent events gets
    /// its newest 40 lines and 8 KiB.
    pub async fn agent_script_progress(&self, run: AgentScriptRun, by: Option<DeviceId>) -> Result<()> {
        send_progress(self.shared(), run, by)
    }

    /// Phone: runs script `script` of the thread's folder in the thread's
    /// folder. `Denied` while the folder's "Phones can run these" is off on
    /// the PC (or its `agents` toggle is); `NotFound` when the thread or the
    /// script is gone; `Protocol("BUSY: script_running")` while it runs
    /// there already, `"BUSY: runs"` with too many running. A run that
    /// couldn't start comes back `failed` with `reason: "spawn"`.
    pub async fn agent_script_run(
        &self,
        peer: DeviceId,
        thread: String,
        script: String,
    ) -> Result<AgentScriptRun> {
        let body = AgentScriptRunRequest { thread, script };
        if !body.is_valid() {
            return Err(invalid(types::AGENT_SCRIPT_RUN));
        }
        let run: AgentScriptRun = single(self.shared(), peer, types::AGENT_SCRIPT_RUN, &body).await?;
        checked(run, types::AGENT_SCRIPT_RUN, |r| r.thread == body.thread && r.script == body.script)
    }

    /// Phone: stops a run: one this phone started, or any of a folder whose
    /// "Phones can run these" is on (`Denied` otherwise).
    pub async fn agent_script_stop(&self, peer: DeviceId, run: String) -> Result<AgentScriptRun> {
        let body = AgentRunRef { run };
        if !body.is_valid() {
            return Err(invalid(types::AGENT_SCRIPT_STOP));
        }
        let run: AgentScriptRun = single(self.shared(), peer, types::AGENT_SCRIPT_STOP, &body).await?;
        checked(run, types::AGENT_SCRIPT_STOP, |r| r.id == body.run)
    }

    /// Phone: a thread's runs, newest first, at most 12, without lines.
    pub async fn agent_script_runs(&self, peer: DeviceId, thread: String) -> Result<Vec<AgentScriptRun>> {
        let body = AgentScriptRunsRequest { thread };
        if !body.is_valid() {
            return Err(invalid(types::AGENT_SCRIPT_RUNS));
        }
        let runs: AgentScriptRuns = single(self.shared(), peer, types::AGENT_SCRIPT_RUNS, &body).await?;
        Ok(runs.sanitized().runs.into_iter().filter(|r| r.thread == body.thread).collect())
    }

    /// Phone: a run with its newest 200 lines and 32 KiB (`from` numbers
    /// the first; `dropped` counts those not kept).
    pub async fn agent_script_output(&self, peer: DeviceId, run: String) -> Result<AgentScriptRun> {
        let body = AgentRunRef { run };
        if !body.is_valid() {
            return Err(invalid(types::AGENT_SCRIPT_OUTPUT));
        }
        let env = Envelope::new(types::AGENT_SCRIPT_OUTPUT, &body)?;
        let run: AgentScriptRun = agents::request_paged_of(self.shared(), peer, env, agent::HOST).await?;
        checked(run, types::AGENT_SCRIPT_OUTPUT, |r| r.id == body.run)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, thread: &str) -> AgentScriptRun {
        AgentScriptRun {
            id: id.into(),
            thread: thread.into(),
            script: "s0a1b2c3d".into(),
            name: "dev".into(),
            phase: scripts::phase::RUNNING.into(),
            ..Default::default()
        }
    }

    #[test]
    fn every_request_and_the_event_is_named() {
        for t in [
            types::AGENT_SCRIPT_RUN,
            types::AGENT_SCRIPT_STOP,
            types::AGENT_SCRIPT_RUNS,
            types::AGENT_SCRIPT_OUTPUT,
        ] {
            assert!(is_request(t) && !is_event(t), "{t}");
            assert!(g7link_protocol::lanes::rpc_of(t).is_some(), "{t}");
        }
        assert!(is_event(types::AGENT_SCRIPT) && !is_request(types::AGENT_SCRIPT));
        assert!(!is_request("agent.script.edit") && !is_request(types::AGENT_GIT_JOB));
    }

    #[test]
    fn a_hosts_refusal_keeps_its_word() {
        let refusal = |reason: &str| {
            let env = refusal_of(&AgentJobError { reason: reason.into(), ..Default::default() });
            let body = env.body::<g7link_protocol::messages::ErrorBody>().unwrap();
            (body.code, body.msg.unwrap_or_default())
        };
        assert_eq!(refusal("phone_scripts_off"), (ErrorCode::Denied, "phone_scripts_off".into()));
        assert_eq!(refusal("script_running"), (ErrorCode::Busy, "script_running".into()));
        assert_eq!(refusal("runs"), (ErrorCode::Busy, "runs".into()));
        assert_eq!(refusal("no_script"), (ErrorCode::NotFound, "no_script".into()));
        assert_eq!(refusal("folder_missing"), (ErrorCode::BadMessage, "folder_missing".into()));
        assert_eq!(refusal("Not A Word"), (ErrorCode::BadMessage, "failed".into()));
    }

    #[test]
    fn a_run_is_mine_only_for_the_phone_that_started_it() {
        let (phone, other) = (DeviceId([1; 32]), DeviceId([2; 32]));
        let id = "0123456789abcdef";
        let thread = "t1";
        assert!(run_for(run(id, thread), Some(phone), &phone, 40, 8192).unwrap().mine);
        assert!(!run_for(run(id, thread), Some(other), &phone, 40, 8192).unwrap().mine);
        assert!(!run_for(run(id, thread), None, &phone, 40, 8192).unwrap().mine, "the PC's");
        assert!(run_for(run("bad", thread), None, &phone, 40, 8192).is_none());
        let asked = AgentRequest::ScriptStop { run: id.into() };
        assert!(answers(&asked, &run(id, thread)));
        assert!(!answers(&asked, &run("fedcba9876543210", thread)), "another run");
        let started = AgentRequest::ScriptRun { thread: thread.into(), script: "s0a1b2c3d".into() };
        assert!(answers(&started, &run(id, thread)) && !answers(&started, &run(id, "t2")));
    }
}
