// SPDX-License-Identifier: MPL-2.0
//! Service lanes (`docs/protocol/v1.md` §4.2): one long-lived stream per
//! service per direction. The side that opens a lane sends that service's
//! events and ordered requests on it; the other side handles them strictly
//! in arrival order and writes only answers back. Order holds within a
//! service, and nothing one service does can hold up another.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use g7link_protocol::{
    Envelope, ErrorCode, ProtocolError,
    lanes::{Lane, lane_of},
    messages::{StreamHeader, types},
    read_frame, write_frame,
};
use iroh::endpoint::{Connection, RecvStream, SendStream, VarInt};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    Error, Result,
    dispatch::{self, Responder},
    node::Shared,
    session::{Session, decode_frame},
    wire_stats::{self, Counters, LaneKind},
};

/// Frames one lane holds while its stream takes the ones before them (as
/// many as the control outbox held for all services in v0). A sender waits
/// past that; a sender that can't wait is told the queue is full.
const LANE_QUEUE: usize = 256;
/// A loss-tolerant frame is skipped while the lane's write has been stuck
/// this long.
const LOSSY_AFTER: Duration = Duration::from_millis(100);
/// How long a new lane from the peer waits for its old one to end before
/// it's refused as a second lane: tries, and the wait between them.
const CLAIM_TRIES: usize = 10;
const CLAIM_WAIT: Duration = Duration::from_millis(50);
/// Frames at least this large are encoded and decoded on a blocking thread.
pub(crate) const LARGE_FRAME: usize = 64 * 1024;

/// Send priorities (`docs/protocol/v1.md` §4.1): control first, then the
/// lanes whose answers a finger waits on, the other lanes, RPCs, sound,
/// video, and bulk last.
pub(crate) const CONTROL_PRIORITY: i32 = 8;
pub(crate) const FAST_LANE_PRIORITY: i32 = 7;
pub(crate) const LANE_PRIORITY: i32 = 6;
pub(crate) const RPC_PRIORITY: i32 = 4;
pub(crate) const AUDIO_PRIORITY: i32 = 3;
pub(crate) const VIDEO_PRIORITY: i32 = 2;
pub(crate) const BULK_PRIORITY: i32 = 0;
const _: () = assert!(
    CONTROL_PRIORITY > FAST_LANE_PRIORITY
        && FAST_LANE_PRIORITY > LANE_PRIORITY
        && LANE_PRIORITY > RPC_PRIORITY
        && RPC_PRIORITY > AUDIO_PRIORITY
        && AUDIO_PRIORITY > VIDEO_PRIORITY
        && VIDEO_PRIORITY > BULK_PRIORITY
);

/// The send priority of a stream for `svc`/`op` (`op` is [`OP_LANE`] for a
/// lane). The one place priorities are decided.
///
/// [`OP_LANE`]: g7link_protocol::lanes::OP_LANE
pub(crate) fn priority_of(svc: &str, op: &str) -> i32 {
    use g7link_protocol::lanes::{OP_LANE, rpc_by_header, svc};
    use g7link_protocol::messages::{media_sound, mic, mirror, webcam};
    if op == OP_LANE {
        return if matches!(svc, svc::INPUT | svc::CALL | svc::MEDIA) {
            FAST_LANE_PRIORITY
        } else {
            LANE_PRIORITY
        };
    }
    if rpc_by_header(svc, op).is_some() {
        return RPC_PRIORITY;
    }
    match (svc, op) {
        (mirror::SERVICE, mirror::OP_AUDIO) | (mic::SERVICE, mic::OP_PCM) => AUDIO_PRIORITY,
        (media_sound::SERVICE, media_sound::OP) => AUDIO_PRIORITY,
        (mirror::SERVICE, mirror::OP_VIDEO) | (webcam::SERVICE, webcam::OP_VIDEO) => VIDEO_PRIORITY,
        _ => BULK_PRIORITY,
    }
}

/// The lanes of one session.
#[derive(Debug, Default)]
pub(crate) struct LaneSet {
    /// Lanes this side opened, by service.
    out: Mutex<HashMap<&'static str, Arc<OutLane>>>,
    /// Services the peer has a lane open for.
    incoming: Mutex<HashSet<&'static str>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl LaneSet {
    /// Fails every lane this side opened, and the requests waiting on them.
    pub(crate) fn fail_all(&self) {
        let lanes: Vec<Arc<OutLane>> = lock(&self.out).drain().map(|(_, lane)| lane).collect();
        for lane in lanes {
            lane.fail();
        }
    }

    fn forget(&self, lane: &Arc<OutLane>) {
        let mut out = lock(&self.out);
        if out.get(lane.svc).is_some_and(|current| Arc::ptr_eq(current, lane)) {
            out.remove(lane.svc);
        }
    }

    /// Notes that the peer opened its lane for `svc`; false if it has one.
    fn claim_incoming(&self, svc: &'static str) -> bool {
        lock(&self.incoming).insert(svc)
    }

    fn release_incoming(&self, svc: &'static str) {
        lock(&self.incoming).remove(svc);
    }
}

/// A lane this side opened: frames go out in the order they were queued.
#[derive(Debug)]
pub(crate) struct OutLane {
    svc: &'static str,
    /// Frames to write, with when each was queued.
    tx: mpsc::Sender<(Envelope, Instant)>,
    /// Requests waiting for their answer, by `id`.
    pending: Mutex<HashMap<u64, oneshot::Sender<Envelope>>>,
    failed: AtomicBool,
    /// Ends the lane's task, resetting its stream.
    stop: CancellationToken,
    /// Since when the frame being written has waited for the stream.
    writing_since: Mutex<Option<Instant>>,
}

impl OutLane {
    fn fail(&self) {
        self.failed.store(true, Ordering::Relaxed);
        self.stop.cancel();
        lock(&self.pending).clear();
    }

    fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// Whether the lane's write has been stuck for a while: loss-tolerant
    /// frames then aren't worth queueing.
    fn is_stuck(&self) -> bool {
        lock(&self.writing_since).is_some_and(|since| since.elapsed() >= LOSSY_AFTER)
    }
}

impl Session {
    /// This side's lane for `svc`, opened if there's none, or if the last
    /// one failed.
    fn lane(&self, svc: &'static str) -> Arc<OutLane> {
        let mut out = lock(&self.lanes.out);
        if let Some(lane) = out.get(svc).filter(|lane| !lane.is_failed()) {
            return lane.clone();
        }
        let (tx, rx) = mpsc::channel(LANE_QUEUE);
        let lane = Arc::new(OutLane {
            svc,
            tx,
            pending: Mutex::new(HashMap::new()),
            failed: AtomicBool::new(false),
            stop: CancellationToken::new(),
            writing_since: Mutex::new(None),
        });
        out.insert(svc, lane.clone());
        drop(out);
        let opened = OpenedLane {
            conn: self.conn.clone(),
            cancel: self.cancel.clone(),
            counters: self.counters.clone(),
            lanes: self.lanes.clone(),
            lane: lane.clone(),
        };
        self.runtime.spawn(opened.run(rx));
        lane
    }

    /// Sends a one-way message on `svc`'s lane, waiting while the lane is full.
    pub(crate) async fn lane_send(&self, svc: &'static str, env: Envelope) -> Result<()> {
        self.lane(svc).tx.send((env, Instant::now())).await.map_err(|_| Error::Offline)
    }

    /// Queues a one-way message on `svc`'s lane without waiting.
    pub(crate) fn lane_send_now(&self, svc: &'static str, env: Envelope) -> Result<()> {
        self.lane(svc).tx.try_send((env, Instant::now())).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => Error::QueueFull(format!("the {svc} lane is full")),
            mpsc::error::TrySendError::Closed(_) => Error::Offline,
        })
    }

    /// Queues a loss-tolerant message on `svc`'s lane, or drops it while the
    /// lane is stuck or full. For what a newer message replaces anyway.
    pub(crate) fn lane_send_lossy(&self, svc: &'static str, env: Envelope) -> Result<()> {
        let lane = self.lane(svc);
        if lane.is_stuck() {
            return Ok(());
        }
        match lane.tx.try_send((env, Instant::now())) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(Error::Offline),
        }
    }

    /// Sends a request on `svc`'s lane and waits up to `wait` for its answer,
    /// and says whether it went out. An answer that comes later is dropped.
    pub(crate) async fn lane_request(
        &self,
        svc: &'static str,
        env: Envelope,
        wait: Duration,
    ) -> (Result<Envelope>, bool) {
        let started = Instant::now();
        let mut sent = false;
        let t = env.t.clone();
        let lane = self.lane(svc);
        let id = self.next_request_id();
        let (tx, rx) = oneshot::channel();
        lock(&lane.pending).insert(id, tx);
        let exchange = async {
            lane.tx.send((env.with_id(id), Instant::now())).await.map_err(|_| Error::Offline)?;
            sent = true;
            rx.await.map_err(|_| Error::Offline)
        };
        let result = tokio::select! {
            r = tokio::time::timeout(wait, exchange) => r.unwrap_or(Err(Error::Timeout)),
            () = self.cancel.cancelled() => Err(Error::Offline),
        };
        lock(&lane.pending).remove(&id);
        let outcome = match &result {
            Ok(reply) if reply.t == types::ERROR => wire_stats::Outcome::Error("error"),
            other => wire_stats::Outcome::of(other),
        };
        wire_stats::request(wire_stats::RequestStats {
            svc,
            op: &t,
            lane: LaneKind::Lane,
            took: started.elapsed(),
            outcome,
            parts: 0,
            bytes: 0,
        });
        (result, sent)
    }
}

/// What the task running a lane this side opened needs; nothing that keeps
/// the session itself alive.
struct OpenedLane {
    conn: Connection,
    cancel: CancellationToken,
    counters: Arc<Counters>,
    lanes: Arc<LaneSet>,
    lane: Arc<OutLane>,
}

impl OpenedLane {
    async fn run(self, rx: mpsc::Receiver<(Envelope, Instant)>) {
        let svc = self.lane.svc;
        let result = tokio::select! {
            r = self.stream(rx) => r,
            () = self.cancel.cancelled() => Ok(()),
            () = self.lane.stop.cancelled() => Ok(()),
        };
        if let Err(e) = result {
            tracing::debug!(svc, error = %e, "lane ended");
        }
        self.lane.fail();
        self.lanes.forget(&self.lane);
    }

    async fn stream(&self, mut rx: mpsc::Receiver<(Envelope, Instant)>) -> Result<()> {
        let svc = self.lane.svc;
        let (send, mut recv) = self.conn.open_bi().await.map_err(crate::error::net)?;
        // Ended early (failed, or the session going), the peer's side of the
        // lane ends at once too.
        struct Resets(SendStream);
        impl Drop for Resets {
            fn drop(&mut self) {
                let _ = self.0.reset(VarInt::from_u32(ErrorCode::Cancelled.close_code()));
            }
        }
        let mut send = Resets(send);
        let send = &mut send.0;
        let _ = send.set_priority(priority_of(svc, g7link_protocol::lanes::OP_LANE));
        let header = Envelope::new(types::STREAM, &StreamHeader::lane(svc))?;
        write_frame(&mut *send, &header.to_cbor()).await?;
        let writer = async {
            while let Some((env, queued)) = rx.recv().await {
                wire_stats::lane_wait(svc, queued.elapsed());
                *lock(&self.lane.writing_since) = Some(Instant::now());
                let bytes = encode(env).await?;
                let written = write_frame(&mut *send, &bytes).await;
                *lock(&self.lane.writing_since) = None;
                written?;
            }
            Ok::<_, Error>(())
        };
        let reader = async {
            loop {
                let Some(frame) = read_frame(&mut recv).await? else { return Ok::<_, Error>(()) };
                let env = match decode_frame(frame).await {
                    Ok(env) => env,
                    Err(e) => {
                        tracing::warn!(svc, error = %e, "dropping a malformed answer");
                        continue;
                    }
                };
                let Some(re) = env.re else {
                    // The other side writes only answers on a lane we opened.
                    wire_stats::wrong_lane(&self.counters, LaneKind::Lane, &env.t);
                    continue;
                };
                let waiting = lock(&self.lane.pending).remove(&re);
                match waiting {
                    Some(tx) => {
                        let _ = tx.send(env);
                    }
                    None => wire_stats::late_reply(&self.counters, LaneKind::Lane, &env.t),
                }
            }
        };
        tokio::select! {
            r = writer => r,
            r = reader => r,
        }
    }
}

/// Encodes `env`, on a blocking thread when its body may be large.
async fn encode(env: Envelope) -> Result<Vec<u8>> {
    let started = Instant::now();
    let t = env.t.clone();
    let bytes = if may_be_large(&t) {
        tokio::task::spawn_blocking(move || env.to_cbor())
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
    } else {
        env.to_cbor()
    };
    wire_stats::frame(&t, bytes.len(), started.elapsed());
    Ok(bytes)
}

/// Lane messages that can carry pictures or long lists.
fn may_be_large(t: &str) -> bool {
    matches!(
        t,
        types::MEDIA_STATE
            | types::NOTIFY_SNAPSHOT
            | types::NOTIFY_POSTED
            | types::CLIP_SET
            | types::DECK_LAYOUT
            | types::AGENT_CATALOG
            | types::AGENT_EVENT
            | types::AGENT_SESSION_ITEM
            | types::TOGGLES_STATE
    )
}

/// Serves the lane the peer opened for `svc`: each frame's handler runs to
/// its end before the next frame is read.
pub(crate) async fn serve(
    shared: Arc<Shared>,
    session: Arc<Session>,
    svc: &'static str,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    // A lane the opener gave up on may not have ended here yet: give it a moment.
    let mut claimed = session.lanes.claim_incoming(svc);
    for _ in 0..CLAIM_TRIES {
        if claimed {
            break;
        }
        tokio::time::sleep(CLAIM_WAIT).await;
        claimed = session.lanes.claim_incoming(svc);
    }
    if !claimed {
        tracing::debug!(peer = %session.peer.short(), svc, "refused a second lane for one service");
        let refusal = Envelope::error(ErrorCode::BadMessage, "this service has a lane open");
        let _ = write_frame(&mut send, &refusal.to_cbor()).await;
        let _ = send.finish();
        return;
    }
    struct Claim<'a>(&'a LaneSet, &'static str);
    impl Drop for Claim<'_> {
        fn drop(&mut self) {
            self.0.release_incoming(self.1);
        }
    }
    let _claim = Claim(&session.lanes, svc);
    let _ = send.set_priority(priority_of(svc, g7link_protocol::lanes::OP_LANE));
    let send = tokio::sync::Mutex::new(send);
    loop {
        let frame = match read_frame(&mut recv).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(ProtocolError::FrameTooLarge(len)) => {
                tracing::warn!(peer = %session.peer.short(), svc, len, "peer sent an oversized frame");
                let _ = recv.stop(VarInt::from_u32(ErrorCode::FrameTooLarge.close_code()));
                break;
            }
            Err(e) => {
                tracing::debug!(peer = %session.peer.short(), svc, error = %e, "lane read failed");
                break;
            }
        };
        session.touch_rx();
        let env = match decode_frame(frame).await {
            Ok(env) => env,
            Err(e) => {
                tracing::warn!(peer = %session.peer.short(), svc, error = %e, "dropping malformed message");
                continue;
            }
        };
        if env.re.is_some() {
            // Answers go back on the lane of the request they answer, not on ours.
            wire_stats::wrong_lane(&session.counters, LaneKind::Lane, &env.t);
            continue;
        }
        let rsp = Responder::lane(&send, env.id);
        if lane_of(&env.t) != Some(Lane::Service(svc)) {
            wire_stats::wrong_lane(&session.counters, LaneKind::Lane, &env.t);
            let _ = rsp.error(ErrorCode::BadMessage, "not on this lane").await;
            continue;
        }
        dispatch::run_handler(&shared, &session, &env, &rsp, LaneKind::Lane).await;
    }
    let _ = send.into_inner().finish();
}
