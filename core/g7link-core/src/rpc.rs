// SPDX-License-Identifier: MPL-2.0
//! RPC streams (`docs/protocol/v1.md` §4.3): one bidirectional stream per
//! request. The opener sends the header and one request frame without an
//! `id`, then finishes; the responder answers with `part`s and one `ok` or
//! `error`. Dropping the caller's future cancels: the stream is reset and
//! stopped with `CANCELLED`, and the responder drops its handler.

use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use g7link_protocol::{
    Envelope, ErrorCode,
    lanes::{Paged, RpcOp},
    messages::{StreamHeader, types},
    read_frame, write_frame,
};
use iroh::endpoint::{RecvStream, SendStream, VarInt};
use serde::de::DeserializeOwned;
use tokio::sync::OwnedSemaphorePermit;

use crate::{
    Error, Result,
    dispatch::{self, Responder},
    lane::{RPC_PRIORITY, priority_of},
    node::Shared,
    session::{Session, decode_frame},
    wire_stats::{self, Counters, LaneKind, Outcome},
};

/// RPCs this side has in flight to one peer at most; more wait their turn,
/// first come first served, inside their own time limit.
pub(crate) const MAX_OUTGOING_RPCS: usize = 16;
/// RPCs one peer may have this side answering at once; past that, `BUSY`.
pub(crate) const MAX_INCOMING_RPCS: usize = 64;
/// The most an answer may total, and the most parts it may come in.
pub(crate) const MAX_RPC_REPLY_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_RPC_PARTS: usize = 4096;
/// The message of the BUSY an RPC past [`MAX_INCOMING_RPCS`] is answered with.
pub(crate) const TOO_MANY_REQUESTS: &str = "too many requests at once";
/// How long the responder waits for the request frame after the header.
const REQUEST_FRAME_TIMEOUT: Duration = Duration::from_secs(30);

fn cancelled() -> VarInt {
    VarInt::from_u32(ErrorCode::Cancelled.close_code())
}

/// What came back for an RPC: its parts in order, and the final `ok` or `error`.
#[derive(Debug)]
pub(crate) struct Answer {
    pub parts: Vec<Envelope>,
    pub last: Envelope,
}

/// An open RPC stream on the caller's side. Dropped before the answer is
/// whole, it tells the responder to stop.
struct Call {
    send: SendStream,
    recv: RecvStream,
    done: bool,
    op: RpcOp,
    started: Instant,
}

impl Drop for Call {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.send.reset(cancelled());
            let _ = self.recv.stop(cancelled());
            wire_stats::request(wire_stats::RequestStats {
                svc: self.op.svc,
                op: self.op.op,
                lane: LaneKind::Rpc,
                took: self.started.elapsed(),
                outcome: Outcome::Cancelled,
                parts: 0,
                bytes: 0,
            });
        }
    }
}

impl Session {
    /// Runs the RPC `op` with request `env`, all of it (the wait for a turn
    /// included) within `wait`.
    pub(crate) async fn rpc(&self, op: RpcOp, env: Envelope, wait: Duration) -> Result<Answer> {
        self.rpc_tracked(op, env, wait).await.0
    }

    /// Like [`Session::rpc`], and also says whether the request went out.
    pub(crate) async fn rpc_tracked(
        &self,
        op: RpcOp,
        env: Envelope,
        wait: Duration,
    ) -> (Result<Answer>, bool) {
        let started = Instant::now();
        let (mut bytes, mut sent) = (0, false);
        let result = tokio::select! {
            r = tokio::time::timeout(wait, self.exchange(op, env, &mut bytes, &mut sent)) => {
                r.unwrap_or(Err(Error::Timeout))
            }
            () = self.cancel.cancelled() => Err(Error::Offline),
        };
        let outcome = match &result {
            Ok(answer) if answer.last.t == types::ERROR => {
                match answer.last.body::<g7link_protocol::messages::ErrorBody>() {
                    // The responder's limit, not a service's own BUSY (an SMS
                    // limit, an agent at work), which keeps its meaning.
                    Ok(body)
                        if body.code == ErrorCode::Busy && body.msg.as_deref() == Some(TOO_MANY_REQUESTS) =>
                    {
                        Outcome::Busy
                    }
                    _ => Outcome::Error("error"),
                }
            }
            other => Outcome::of(other),
        };
        if outcome == Outcome::Busy {
            wire_stats::busy(&self.counters, op.svc, op.op, LaneKind::Rpc, false);
        }
        wire_stats::request(wire_stats::RequestStats {
            svc: op.svc,
            op: op.op,
            lane: LaneKind::Rpc,
            took: started.elapsed(),
            outcome,
            parts: result.as_ref().map_or(0, |a| a.parts.len()),
            bytes,
        });
        // The other device has too much in flight: the caller may try again later.
        if outcome == Outcome::Busy {
            return (Err(Error::Busy), sent);
        }
        (result, sent)
    }

    async fn exchange(
        &self,
        op: RpcOp,
        mut env: Envelope,
        bytes: &mut usize,
        sent: &mut bool,
    ) -> Result<Answer> {
        let queued = Instant::now();
        let _turn: OwnedSemaphorePermit =
            self.rpc_out.clone().acquire_owned().await.map_err(|_| Error::Offline)?;
        let waited = queued.elapsed();
        if waited >= Duration::from_millis(1) {
            let in_flight = self.rpc_limit.saturating_sub(self.rpc_out.available_permits());
            wire_stats::rpc_queue(in_flight, 0, waited);
        }
        let (send, recv) = self.conn.open_bi().await.map_err(crate::error::net)?;
        let mut call = Call { send, recv, done: false, op, started: queued };
        let _ = call.send.set_priority(RPC_PRIORITY);
        let header = Envelope::new(types::STREAM, &StreamHeader::rpc(op.svc, op.op))?;
        write_frame(&mut call.send, &header.to_cbor()).await?;
        env.id = None;
        env.re = None;
        write_frame(&mut call.send, &env.to_cbor()).await?;
        *sent = true;
        let _ = call.send.finish();
        let mut parts = Vec::new();
        loop {
            let frame = read_frame(&mut call.recv)
                .await?
                .ok_or_else(|| Error::Protocol("an RPC stream ended without its answer".into()))?;
            *bytes = bytes.saturating_add(frame.len());
            if *bytes > MAX_RPC_REPLY_BYTES {
                return Err(Error::Protocol("an RPC answer was too large".into()));
            }
            let reply = decode_frame(frame).await?;
            match reply.t.as_str() {
                types::PART if parts.len() < MAX_RPC_PARTS => {
                    Counters::add(&self.counters.parts_received, 1);
                    parts.push(reply);
                }
                types::PART => return Err(Error::Protocol("an RPC answer came in too many parts".into())),
                types::OK | types::ERROR => {
                    call.done = true;
                    return Ok(Answer { parts, last: reply });
                }
                other => return Err(Error::Protocol(format!("{other} on an RPC stream"))),
            }
        }
    }

    /// Sends the paged request `env` (its type names the RPC) and puts the
    /// answer's parts back together.
    pub(crate) async fn request_paged<T>(&self, env: Envelope, wait: Duration) -> Result<T>
    where
        T: Paged + DeserializeOwned + Send + 'static,
    {
        let op = g7link_protocol::lanes::rpc_of(&env.t)
            .filter(|op| op.paged)
            .ok_or_else(|| Error::Internal(format!("{} isn't a paged RPC", env.t)))?;
        self.rpc_paged(op, env, wait).await
    }

    /// Runs a paged RPC and puts its parts back together.
    pub(crate) async fn rpc_paged<T>(&self, op: RpcOp, env: Envelope, wait: Duration) -> Result<T>
    where
        T: Paged + DeserializeOwned + Send + 'static,
    {
        let answer = self.rpc(op, env, wait).await?;
        answer.last.expect(types::OK)?;
        assemble(answer).await
    }
}

/// A paged answer put back together: its parts' bodies and the `ok`'s, in
/// order, decoded on a blocking thread. The `ok` is checked by the caller.
pub(crate) async fn assemble<T>(answer: Answer) -> Result<T>
where
    T: Paged + DeserializeOwned + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let mut bodies = answer.parts.into_iter().chain(std::iter::once(answer.last));
        let mut first = bodies.next().ok_or_else(|| Error::Protocol("an empty answer".into()))?;
        let mut whole: T = first.body_owned()?;
        for mut part in bodies {
            whole.append(part.body_owned::<T>()?);
        }
        Ok(whole)
    })
    .await
    .map_err(|e| Error::Internal(e.to_string()))?
}

/// Counts an RPC this side is answering while it runs.
struct Running(Arc<Counters>);

impl Running {
    fn new(counters: &Arc<Counters>) -> Self {
        counters.rpcs_in.fetch_add(1, Ordering::Relaxed);
        Running(counters.clone())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.0.rpcs_in.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Turns away an RPC over the limit: `error BUSY`, then the stream finished.
pub(crate) async fn refuse_busy(session: &Session, op: RpcOp, mut send: SendStream) {
    wire_stats::busy(&session.counters, op.svc, op.op, LaneKind::Rpc, true);
    let _ = send.set_priority(RPC_PRIORITY);
    let busy = Envelope::error(ErrorCode::Busy, TOO_MANY_REQUESTS);
    let _ = write_frame(&mut send, &busy.to_cbor()).await;
    let _ = send.finish();
}

/// Answers the RPC stream the peer opened for `op` (its header already read).
pub(crate) async fn serve(
    shared: Arc<Shared>,
    session: Arc<Session>,
    op: RpcOp,
    send: SendStream,
    mut recv: RecvStream,
    _slot: OwnedSemaphorePermit,
) {
    let _running = Running::new(&session.counters);
    let _ = send.set_priority(priority_of(op.svc, op.op));
    // Resolves when the caller stops our side: it gave up.
    let stopped = send.stopped();
    let send = tokio::sync::Mutex::new(send);
    let rsp = Responder::rpc(&send);
    let request = match tokio::time::timeout(REQUEST_FRAME_TIMEOUT, read_frame(&mut recv)).await {
        Ok(Ok(Some(frame))) => decode_frame(frame).await.ok(),
        _ => None,
    };
    let Some(env) = request.filter(|env| env.t == op.t && env.id.is_none() && env.re.is_none()) else {
        let _ = rsp.error(ErrorCode::BadMessage, "not the request this stream names").await;
        return;
    };
    let gave_up = async {
        // `None`: our answer was finished and taken, nothing to watch for.
        if let Ok(None) = stopped.await {
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        () = dispatch::run_handler(&shared, &session, &env, &rsp, LaneKind::Rpc) => {}
        () = gave_up => {
            tracing::debug!(peer = %session.peer.short(), svc = op.svc, op = op.op, "the caller gave up on an RPC");
        }
        () = session.cancel.cancelled() => {}
    }
}
