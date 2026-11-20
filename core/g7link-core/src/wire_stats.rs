// SPDX-License-Identifier: MPL-2.0
//! What the session's transport is doing, for logs and tests
//! (`plans/protocol-v1-contract.md` §3). Events go to `tracing` at `debug`,
//! target `g7link::wire`, as fields only: never a body, a name, a number or
//! a path. Counters are kept per session and readable through
//! [`crate::Node::wire_counters`].

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

/// How a request ended, as the `request` event names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome<'a> {
    Ok,
    Error(&'a str),
    Timeout,
    Cancelled,
    Busy,
    Offline,
}

impl Outcome<'_> {
    fn as_str(&self) -> &str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Error(code) => code,
            Outcome::Timeout => "timeout",
            Outcome::Cancelled => "cancelled",
            Outcome::Busy => "busy",
            Outcome::Offline => "offline",
        }
    }

    pub(crate) fn of<T>(result: &crate::Result<T>) -> Outcome<'static> {
        match result {
            Ok(_) => Outcome::Ok,
            Err(crate::Error::Timeout) => Outcome::Timeout,
            Err(crate::Error::Busy) => Outcome::Busy,
            Err(crate::Error::Offline) => Outcome::Offline,
            Err(_) => Outcome::Error("error"),
        }
    }
}

/// Which kind of stream a message or request went on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaneKind {
    Control,
    Lane,
    Rpc,
    Stream,
}

impl LaneKind {
    fn as_str(self) -> &'static str {
        match self {
            LaneKind::Control => "control",
            LaneKind::Lane => "lane",
            LaneKind::Rpc => "rpc",
            LaneKind::Stream => "stream",
        }
    }
}

/// Counts kept for one session: cheap enough to keep always.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    pub busy_sent: AtomicU64,
    pub busy_received: AtomicU64,
    pub late_replies: AtomicU64,
    pub wrong_lane: AtomicU64,
    pub motion_received: AtomicU64,
    pub motion_summed: AtomicU64,
    pub motion_refused: AtomicU64,
    pub sound_sent: AtomicU64,
    pub sound_received: AtomicU64,
    pub sound_gaps: AtomicU64,
    pub sound_late: AtomicU64,
    pub sound_on_stream: AtomicU64,
    pub unknown_datagrams: AtomicU64,
    /// RPCs this side is answering right now.
    pub rpcs_in: AtomicU64,
    /// Media and bulk streams the peer opened that are running now.
    pub streams_in: AtomicU64,
    /// `part` frames received in RPC answers.
    pub parts_received: AtomicU64,
    /// Pings the heartbeat sent.
    pub heartbeat_pings: AtomicU64,
}

impl Counters {
    pub(crate) fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> WireCounters {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        WireCounters {
            busy_sent: get(&self.busy_sent),
            busy_received: get(&self.busy_received),
            late_replies: get(&self.late_replies),
            wrong_lane: get(&self.wrong_lane),
            motion_received: get(&self.motion_received),
            motion_summed: get(&self.motion_summed),
            motion_refused: get(&self.motion_refused),
            sound_sent: get(&self.sound_sent),
            sound_received: get(&self.sound_received),
            sound_gaps: get(&self.sound_gaps),
            sound_late: get(&self.sound_late),
            sound_on_stream: get(&self.sound_on_stream),
            unknown_datagrams: get(&self.unknown_datagrams),
            rpcs_in: get(&self.rpcs_in),
            streams_in: get(&self.streams_in),
            parts_received: get(&self.parts_received),
            heartbeat_pings: get(&self.heartbeat_pings),
        }
    }
}

/// A session's wire counters at one moment ([`crate::Node::wire_counters`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct WireCounters {
    /// `error BUSY` this side answered, for RPCs and streams.
    pub busy_sent: u64,
    /// `error BUSY` the peer answered.
    pub busy_received: u64,
    /// Replies that came after their request stopped waiting, dropped.
    pub late_replies: u64,
    /// Messages that came on a stream they don't belong on.
    pub wrong_lane: u64,
    /// Pointer motion that arrived, how much of it was added into motion
    /// already waiting, and how much the rate limit turned away.
    pub motion_received: u64,
    pub motion_summed: u64,
    pub motion_refused: u64,
    pub sound_sent: u64,
    pub sound_received: u64,
    /// Pieces of sound missing between ones that arrived, and ones that came
    /// after a newer one and were dropped.
    pub sound_gaps: u64,
    pub sound_late: u64,
    /// Sound sent on the `media/sound` stream because a datagram couldn't carry it.
    pub sound_on_stream: u64,
    pub unknown_datagrams: u64,
    /// RPCs this side is answering right now.
    pub rpcs_in: u64,
    /// Media and bulk streams from the peer running now.
    pub streams_in: u64,
    /// `part` frames received in RPC answers.
    pub parts_received: u64,
    /// Pings the heartbeat sent.
    pub heartbeat_pings: u64,
}

fn ms(d: Duration) -> u64 {
    d.as_millis().min(u128::from(u64::MAX)) as u64
}

pub(crate) struct RequestStats<'a> {
    pub svc: &'a str,
    pub op: &'a str,
    pub lane: LaneKind,
    pub took: Duration,
    pub outcome: Outcome<'a>,
    pub parts: usize,
    pub bytes: usize,
}

pub(crate) fn request(stats: RequestStats<'_>) {
    tracing::debug!(
        target: "g7link::wire",
        event = "request",
        svc = stats.svc,
        op = stats.op,
        lane = stats.lane.as_str(),
        ms = ms(stats.took),
        outcome = stats.outcome.as_str(),
        parts = stats.parts,
        bytes = stats.bytes,
    );
}

pub(crate) fn handler(svc: &str, t: &str, lane: LaneKind, took: Duration) {
    if took >= Duration::from_millis(5) {
        tracing::debug!(target: "g7link::wire", event = "handler", svc, t, lane = lane.as_str(), ms = ms(took));
    }
}

pub(crate) fn lane_wait(svc: &str, waited: Duration) {
    if waited >= Duration::from_millis(20) {
        tracing::debug!(target: "g7link::wire", event = "lane_wait", svc, ms = ms(waited));
    }
}

pub(crate) fn rpc_queue(in_flight: usize, waiting: usize, waited: Duration) {
    tracing::debug!(target: "g7link::wire", event = "rpc_queue", in_flight, waiting, ms = ms(waited));
}

pub(crate) fn busy(counters: &Counters, svc: &str, op: &str, kind: LaneKind, sent: bool) {
    Counters::add(if sent { &counters.busy_sent } else { &counters.busy_received }, 1);
    tracing::debug!(
        target: "g7link::wire",
        event = "busy",
        svc,
        op,
        kind = kind.as_str(),
        dir = if sent { "sent" } else { "received" },
    );
}

pub(crate) fn late_reply(counters: &Counters, lane: LaneKind, t: &str) {
    Counters::add(&counters.late_replies, 1);
    tracing::debug!(target: "g7link::wire", event = "late_reply", lane = lane.as_str(), t);
}

pub(crate) fn wrong_lane(counters: &Counters, lane: LaneKind, t: &str) {
    Counters::add(&counters.wrong_lane, 1);
    tracing::debug!(target: "g7link::wire", event = "wrong_lane", lane = lane.as_str(), t);
}

pub(crate) fn frame(t: &str, bytes: usize, encode: Duration) {
    if bytes >= 65_536 {
        let encode_us = encode.as_micros().min(u128::from(u64::MAX)) as u64;
        tracing::debug!(target: "g7link::wire", event = "frame", t, bytes, encode_us);
    }
}

pub(crate) fn control_depth(depth: usize) {
    if depth >= 32 {
        tracing::debug!(target: "g7link::wire", event = "control_depth", depth);
    }
}

pub(crate) fn motion(received: u64, summed: u64, refused: u64, inject_max: Duration) {
    tracing::debug!(
        target: "g7link::wire",
        event = "motion",
        received,
        summed,
        refused,
        inject_ms_max = ms(inject_max),
    );
}

pub(crate) fn sound(dir: &str, count: u64, gaps: u64, late: u64, via: &str) {
    tracing::debug!(target: "g7link::wire", event = "sound", dir, count, gaps, late, via);
}

pub(crate) fn rtt(heartbeat: Duration, path: Duration) {
    tracing::debug!(target: "g7link::wire", event = "rtt", heartbeat_ms = ms(heartbeat), path_ms = ms(path));
}

pub(crate) fn starved(late: Duration) {
    tracing::debug!(target: "g7link::wire", event = "starved", late_ms = ms(late));
}

/// `cwnd` is 0 where the QUIC stack doesn't say it.
pub(crate) fn conn(lost: u64, cwnd: u64, rtt: Duration, sent_bytes: u64) {
    tracing::debug!(target: "g7link::wire", event = "conn", lost, cwnd, rtt_ms = ms(rtt), sent_bytes);
}

/// Watches the runtime for starvation: a 100 ms tick that comes 50 ms or
/// more late means tasks waited for a worker that long.
pub(crate) async fn watch_starvation(cancel: tokio_util::sync::CancellationToken) {
    const TICK: Duration = Duration::from_millis(100);
    let mut next = tokio::time::Instant::now() + TICK;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(next) => {}
            () = cancel.cancelled() => return,
        }
        let late = tokio::time::Instant::now().saturating_duration_since(next);
        if late >= Duration::from_millis(50) {
            starved(late);
        }
        next = tokio::time::Instant::now() + TICK;
    }
}

/// The core closed a session itself: `why` (`heartbeat`, `path`), and how old it was.
pub(crate) fn liveness(why: &str, age: Duration) {
    tracing::debug!(target: "g7link::wire", event = "liveness", why, age_ms = ms(age));
}

/// The heartbeat's path verdict changed: whether an allowed path is left,
/// how many paths there are, and the kind of the one in use.
pub(crate) fn path_policy(allowed: bool, paths: usize, selected: &str) {
    tracing::debug!(target: "g7link::wire", event = "path_policy", allowed, paths, selected);
}
