// SPDX-License-Identifier: GPL-3.0-or-later
//! Move to G7 (`plans/move-and-focus.md` §4 and the owner's choice in §2,
//! docs/protocol/agents.md §9.1): a phone ends a session's agent where it
//! runs on this PC and continues the session as a thread of its own.
//!
//! Nothing is typed and no window comes forward. Once the phone's user
//! confirmed (§9's confirmation, rate and one-at-a-time lock, shared with
//! replies), the agent host ends the session's one proven process with a
//! single SIGTERM (`session.close`), so a move works while the PC is locked
//! or in use, in any terminal. G7 never signals a process itself. After the
//! agent exited, its terminal gets the one sequence that turns off the
//! mouse reporting it may have left on, and nothing else. Only then, and
//! only once a fresh `session.place` says nothing runs the session, is the
//! host asked to continue it; the host's own check of what has the session
//! open decides. A Codex session it still sees open (90 s after its last
//! write) is answered `closed`, for the phone to continue it once it can.

use std::time::{Duration, Instant};

use g7_agents::{
    api::{
        CloseOutcome, CloseRefusal, ErrorCode, Origin, SessionCloseParams, SessionCloseResult,
        SessionContinueParams, cap,
    },
    client::{Client, ClientError},
};
use g7link_core::{AgentFailure, AgentReply, AgentSessionMoved, AgentThread, DeviceId, session_move};

use super::{
    Agent, Blocked, Bound, Error, Found, Host, HostDown, LiveHost, LiveWindows, Now, Place, Refused, Replies,
    Source, TERMINALS, Target, Windows, place_target, replies, target, wire_error, wire_target,
};

/// The terminal's name when nothing better is known.
const ANY_TERMINAL: &str = "Terminal";

/// A phone's `agent.session.move`.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct MoveAsk {
    pub session: String,
    pub confirm: Option<String>,
}

/// Never prints the nonce.
impl std::fmt::Debug for MoveAsk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MoveAsk")
            .field("session", &self.session)
            .field("confirm", &self.confirm.is_some())
            .finish()
    }
}

/// What the host's `session.close` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Closing {
    /// The agent exited after its one SIGTERM.
    Closed,
    /// It was sent one and still runs; nothing more is sent to it.
    Open,
    /// Nothing was sent.
    Refused(Refused),
    /// The host doesn't list the session.
    Unknown,
}

/// What the host's `session.continue` answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Continued {
    /// The session is this thread now.
    Thread(Box<AgentThread>),
    /// The host still sees it open (Codex within 90 s of its last write).
    StillOpen,
    /// Another refusal: the phone's own continue will show the host's reason.
    Refused,
}

/// What a move asks of the agent host, beside `session.place`.
pub(super) trait Mover: Host {
    /// The host closes sessions (`cap::MOVE`).
    fn closes(&self) -> bool;
    /// `session.close`: the process `pid` started at `start_ticks`, if it
    /// still has the session, in a terminal, idle.
    fn close(&self, session: &str, pid: u32, start_ticks: u64) -> Result<Closing, HostDown>;
    /// `session.continue` for the phone that asked.
    fn continue_here(&self, session: &str) -> Result<Continued, HostDown>;
}

/// What became of a move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MoveOutcome {
    /// The agent closed and the session is this thread now.
    Moved(Box<AgentThread>),
    /// The agent closed; the session isn't continued yet.
    Closed,
    /// Nothing was sent.
    Refused(Refused),
    /// SIGTERM went and the agent didn't close, or another process has the
    /// session now.
    Open,
}

impl MoveOutcome {
    pub(super) fn result(&self) -> &'static str {
        match self {
            MoveOutcome::Moved(_) => session_move::result::MOVED,
            MoveOutcome::Closed => session_move::result::CLOSED,
            MoveOutcome::Refused(_) => session_move::result::REFUSED,
            MoveOutcome::Open => session_move::result::FAILED,
        }
    }

    pub(super) fn reason(&self) -> Option<&'static str> {
        match self {
            MoveOutcome::Moved(_) | MoveOutcome::Closed => None,
            MoveOutcome::Refused(reason) => Some(reason.word()),
            MoveOutcome::Open => Some(session_move::reason::OPEN),
        }
    }
}

/// A move's answer.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Moved {
    pub outcome: MoveOutcome,
    /// With `confirm`: where the session runs, as the phone's sheet shows it.
    pub target: Option<Target>,
    pub nonce: Option<String>,
    pub retry_ms: Option<u32>,
    /// How long `session.close` took, for the log line only.
    pub exit_ms: Option<u64>,
}

/// Never prints the nonce, nor the thread's title.
impl std::fmt::Debug for Moved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Moved")
            .field("result", &self.outcome.result())
            .field("reason", &self.outcome.reason())
            .field("target", &self.target)
            .field("nonce", &self.nonce.is_some())
            .field("retry_ms", &self.retry_ms)
            .field("exit_ms", &self.exit_ms)
            .finish()
    }
}

impl Moved {
    fn of(outcome: MoveOutcome) -> Self {
        Moved { outcome, target: None, nonce: None, retry_ms: None, exit_ms: None }
    }

    fn refused(reason: Refused) -> Self {
        Moved::of(MoveOutcome::Refused(reason))
    }
}

// ---- The sequence ----

/// Answers a phone's move. `phone` is the asking phone's id.
pub(super) fn answer<H: Mover, W: Windows>(
    host: &H,
    windows: &W,
    replies: &Replies,
    phone: &str,
    ask: &MoveAsk,
) -> Result<Moved, Error> {
    let moved = run(host, windows, replies, phone, ask)?;
    tracing::info!(
        session = %ask.session,
        result = moved.outcome.result(),
        reason = moved.outcome.reason().unwrap_or_default(),
        exit_ms = moved.exit_ms.unwrap_or_default(),
        "a phone's move of a session to G7"
    );
    Ok(moved)
}

fn run<H: Mover, W: Windows>(
    host: &H,
    windows: &W,
    replies: &Replies,
    phone: &str,
    ask: &MoveAsk,
) -> Result<Moved, Error> {
    // A host older than `session.close` can't end the session's process.
    if !host.closes() {
        return Ok(Moved::refused(Refused::Setup));
    }
    // The pace replies keep: a move is one entered reply.
    let now = Instant::now();
    {
        let mut ledger = replies.ledger();
        let exempt = ask.confirm.as_deref().is_some_and(|n| ledger.holds(phone, &ask.session, n, now));
        if let Err(wait) = ledger.attempt(phone, &ask.session, now, exempt) {
            return Ok(Moved { retry_ms: Some(super::wait_ms(wait)), ..Moved::refused(Refused::Rate) });
        }
    }
    let Ok(_entering) = replies.entering.try_lock() else { return Err(Error::Busy) };

    // The session's one process, idle in a terminal, and nothing that would
    // stop the continue: all before the phone's user is asked.
    let place = match host.place(&ask.session).map_err(|HostDown| Error::Internal)? {
        Found::Place(place) => place,
        Found::Unknown => return Err(Error::NotFound),
        Found::Ended => return Ok(Moved::refused(Refused::Ended)),
        Found::Ambiguous => return Ok(Moved::refused(Refused::Ambiguous)),
        Found::Elsewhere => return Ok(Moved::refused(Refused::Place)),
    };
    if let Err(reason) = movable(&place) {
        return Ok(Moved::refused(reason));
    }

    // The confirmation: this phone, this session, this run of the agent.
    let listed = windows.list().unwrap_or_default();
    let (emulator, emulator_start) = place
        .chain
        .iter()
        .find(|a| listed.iter().any(|w| w.pid == a.pid))
        .map_or((0, 0), |a| (a.pid, a.start_ticks));
    let bound = Bound {
        phone: phone.to_owned(),
        session: ask.session.clone(),
        agent: place.pid,
        agent_start: place.start_ticks,
        emulator,
        emulator_start,
    };
    let confirmed = {
        let mut ledger = replies.ledger();
        ledger.run_confirmed(&bound, now)
            || ask.confirm.as_deref().is_some_and(|nonce| ledger.redeem_run(nonce, &bound, now))
    };
    if !confirmed {
        let target = move_target(windows, &place, &listed);
        let nonce = replies.ledger().mint(bound, now);
        return Ok(Moved { target: Some(target), nonce: Some(nonce), ..Moved::refused(Refused::Confirm) });
    }
    replies.ledger().entered(phone, &ask.session, Instant::now());

    // One SIGTERM, sent by the host, to the process placed above.
    let started = Instant::now();
    let closing =
        host.close(&ask.session, place.pid, place.start_ticks).map_err(|HostDown| Error::Internal)?;
    let exit_ms = Some(millis(started.elapsed()));
    match closing {
        Closing::Closed => {}
        Closing::Open => return Ok(Moved { exit_ms, ..Moved::of(MoveOutcome::Open) }),
        Closing::Refused(reason) => return Ok(Moved { exit_ms, ..Moved::refused(reason) }),
        Closing::Unknown => return Err(Error::NotFound),
    }
    // A Codex ended by a signal leaves mouse reporting on (spike S11).
    if let Some(tty) = place.tty
        && !windows.terminal_reset(tty)
    {
        tracing::info!("a closed agent's terminal couldn't be reset");
    }

    // Nothing runs the session now, as the host sees it again.
    match host.place(&ask.session) {
        Ok(Found::Ended | Found::Elsewhere) => {}
        Ok(Found::Place(_) | Found::Ambiguous) => {
            return Ok(Moved { exit_ms, ..Moved::of(MoveOutcome::Open) });
        }
        Ok(Found::Unknown) | Err(HostDown) => return Err(Error::Internal),
    }
    let outcome = match host.continue_here(&ask.session).map_err(|HostDown| Error::Internal)? {
        Continued::Thread(thread) => MoveOutcome::Moved(thread),
        Continued::StillOpen | Continued::Refused => MoveOutcome::Closed,
    };
    Ok(Moved { exit_ms, ..Moved::of(outcome) })
}

/// What the host's `session.close` would refuse, said before the phone's
/// user is asked: a terminal, idle (Claude by its status file: a guess
/// could end a running turn), and continuable once it's gone.
fn movable(place: &Place) -> Result<(), Refused> {
    if place.source != Source::Terminal {
        return Err(Refused::Place);
    }
    match place.state {
        Now::Idle => {}
        Now::Working => return Err(Refused::Busy),
        Now::NeedsYou | Now::MayNeedYou => return Err(Refused::NeedsYou),
        Now::Ended => return Err(Refused::Ended),
    }
    if place.agent == Agent::Claude && !place.state_exact {
        return Err(Refused::Place);
    }
    match place.continue_block {
        None => Ok(()),
        Some(Blocked::Folder) => Err(Refused::Folder),
        Some(Blocked::Agent) => Err(Refused::Agent),
        Some(Blocked::Open) => Err(Refused::Place),
    }
}

/// Where the session runs, for the phone's sheet: the app whose window
/// holds it, or the program in between, as for a reply; with no window to
/// ask (the PC locked), the terminal G7 knows by its program, else a plain
/// "Terminal".
fn move_target<W: Windows>(windows: &W, place: &Place, listed: &[super::Win]) -> Target {
    place_target(windows, place, listed).unwrap_or_else(|| {
        let known = place.chain.iter().find_map(|a| TERMINALS.iter().find(|t| t.exe == a.exe_name));
        target(place, known.map_or(ANY_TERMINAL, |t| t.name), "terminal")
    })
}

fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

// ---- This PC ----

/// `session.close`'s answer: a word this build doesn't know for what it
/// did is a host that can't be read (whether it closed isn't known); one
/// for why it sent nothing is `place`.
pub(super) fn closing(answer: SessionCloseResult) -> Result<Closing, HostDown> {
    Ok(match answer.result {
        CloseOutcome::Closed => Closing::Closed,
        CloseOutcome::Open => Closing::Open,
        CloseOutcome::Refused => Closing::Refused(match answer.reason {
            Some(CloseRefusal::Ended) => Refused::Ended,
            Some(CloseRefusal::Ambiguous) => Refused::Ambiguous,
            Some(CloseRefusal::NoProcess) => Refused::NoProcess,
            Some(CloseRefusal::Busy) => Refused::Busy,
            Some(CloseRefusal::NeedsYou) => Refused::NeedsYou,
            Some(CloseRefusal::Folder) => Refused::Folder,
            Some(CloseRefusal::Agent) => Refused::Agent,
            Some(CloseRefusal::Place | CloseRefusal::Unknown) | None => Refused::Place,
        }),
        CloseOutcome::Unknown => return Err(HostDown),
    })
}

/// The agent host, asked from the move's own thread, for the phone that asked.
#[derive(Debug)]
struct LiveMover {
    host: LiveHost,
    from: DeviceId,
    origin: Origin,
}

impl Host for LiveMover {
    fn place(&self, session: &str) -> Result<Found, HostDown> {
        self.host.place(session)
    }
}

impl Mover for LiveMover {
    fn closes(&self) -> bool {
        let host = self.host.client.host();
        host.has(cap::MOVE) && host.has(cap::PLACE)
    }

    fn close(&self, session: &str, pid: u32, start_ticks: u64) -> Result<Closing, HostDown> {
        let params = SessionCloseParams { session_id: session.to_owned(), pid, start_ticks };
        match self.host.runtime.block_on(self.host.client.session_close(params)) {
            Ok(answer) => closing(answer)
                .inspect_err(|_| tracing::info!("the agent host's close said a word not known")),
            Err(ClientError::Remote(e)) if e.code == ErrorCode::NotFound => Ok(Closing::Unknown),
            // Not G7 Link's to ask, as the host sees it.
            Err(ClientError::Remote(e)) if e.code == ErrorCode::Denied => {
                Ok(Closing::Refused(Refused::Setup))
            }
            Err(error) if error.is_unknown_method() => Ok(Closing::Refused(Refused::Setup)),
            Err(error) => {
                tracing::info!(%error, "the agent host didn't close a session");
                Err(HostDown)
            }
        }
    }

    fn continue_here(&self, session: &str) -> Result<Continued, HostDown> {
        // In the mode the session's folder remembers for its agent.
        let mode = super::super::continue_mode(&super::super::lock(), session);
        let params =
            SessionContinueParams { session_id: session.to_owned(), mode, origin: self.origin.clone() };
        match self.host.runtime.block_on(self.host.client.session_continue(params)) {
            Ok(info) => {
                super::super::bridge().remember(&info);
                super::super::lists_changed();
                Ok(Continued::Thread(Box::new(super::super::thread(&info, Some(&self.from)))))
            }
            Err(ClientError::Remote(e)) if e.code == ErrorCode::Busy => Ok(Continued::StillOpen),
            Err(ClientError::Remote(e)) if e.code != ErrorCode::Internal => Ok(Continued::Refused),
            Err(error) => {
                tracing::info!(%error, "the agent host didn't continue a closed session");
                Err(HostDown)
            }
        }
    }
}

/// The answer as the core passes it to the phone.
pub(super) fn wire(moved: Moved) -> AgentSessionMoved {
    let result = moved.outcome.result().to_owned();
    let reason = moved.outcome.reason().map(str::to_owned);
    let thread = match moved.outcome {
        MoveOutcome::Moved(thread) => Some(*thread),
        _ => None,
    };
    AgentSessionMoved {
        result,
        reason,
        target: moved.target.map(wire_target),
        nonce: moved.nonce,
        retry_ms: moved.retry_ms.map(u64::from),
        thread,
    }
}

/// A phone's `agent.session.move`, after the core checked the capability,
/// the toggles and the limits. `origin` is the asking phone's, for the
/// thread; `folder` the session's folder label when the page's list has
/// it. Runs on a thread of its own: it waits on the host from there.
pub(crate) async fn phone(
    from: DeviceId,
    client: &Client,
    origin: Origin,
    folder: Option<String>,
    session: String,
    confirm: Option<String>,
) -> Result<AgentReply, AgentFailure> {
    let host = LiveHost::new(client, folder, &session).await;
    let mover = LiveMover { host, from, origin };
    let ask = MoveAsk { session, confirm };
    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new().name("agent-move".into()).spawn(move || {
        let _ = tx.send(answer(&mover, &LiveWindows, replies(), &from.to_string(), &ask));
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "can't move a phone's session");
        return Err(AgentFailure::Internal);
    }
    match rx.await {
        Ok(Ok(moved)) => Ok(AgentReply::Moved(wire(moved))),
        Ok(Err(error)) => Err(wire_error(error)),
        Err(_) => Err(AgentFailure::Internal),
    }
}
