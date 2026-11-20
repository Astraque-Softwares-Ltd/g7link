// SPDX-License-Identifier: MPL-2.0
//! PC sound to a phone (`docs/protocol/v1.md` §5): each piece is numbered
//! and timed, and goes as a datagram, or on the `media/sound` stream when
//! datagrams can't carry it. Never on control.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use g7link_protocol::{
    DeviceId, Envelope, MAX_SOUND_PAYLOAD, SOUND_CODEC_PCM_S16LE, SoundDatagram,
    messages::{StreamHeader, media_sound, types},
    read_frame, write_frame,
};
use iroh::endpoint::{RecvStream, SendStream};
use tokio::sync::Notify;

use crate::{Error, Result, node::Shared, session::Session, wire_stats::Counters};

/// How far behind the wall clock the sample count may fall before the clock
/// starts again from the wall clock: the PC's capture paused.
const RESYNC_AFTER_MS: u64 = 200;

/// The sequence numbers and capture times of one session's sound.
#[derive(Debug, Default)]
pub(crate) struct SoundClock {
    seq: u16,
    /// When the session's sound began, and the samples sent since, per channel.
    began: Option<Instant>,
    samples: u64,
    rate: u32,
}

impl SoundClock {
    /// The number and capture time of the next `frames` frames at `rate`.
    /// Pieces handed over together get times one after another by their
    /// length; after a pause, time starts again from the clock.
    pub(crate) fn next(&mut self, rate: u32, frames: usize, now: Instant) -> (u16, u32) {
        let began = *self.began.get_or_insert(now);
        let elapsed_ms = now.saturating_duration_since(began).as_millis().min(u128::from(u64::MAX)) as u64;
        if self.rate != rate {
            // A new rate: count its samples from the time now.
            self.rate = rate;
            self.samples = elapsed_ms.saturating_mul(u64::from(rate)) / 1000;
        }
        let rate = u64::from(rate.max(1));
        let counted_ms = self.samples.saturating_mul(1000) / rate;
        if elapsed_ms > counted_ms.saturating_add(RESYNC_AFTER_MS) {
            self.samples = elapsed_ms.saturating_mul(rate) / 1000;
        }
        let ts_ms = (self.samples.saturating_mul(1000) / rate) as u32;
        self.samples = self.samples.saturating_add(frames as u64);
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        (seq, ts_ms)
    }
}

/// Sends `pcm` (16-bit PCM, channels interleaved) to the phone at the other
/// end of `session`, in pieces of at most [`MAX_SOUND_PAYLOAD`] bytes of whole
/// frames. A ragged last frame is dropped, as a player couldn't play it.
pub(crate) async fn send(session: &Session, rate: u32, channels: u8, pcm: Vec<u8>) -> Result<()> {
    if !(1..=2).contains(&channels) || !(8_000..=96_000).contains(&rate) {
        return Err(Error::Protocol("unplayable sound".into()));
    }
    let frame_len = 2 * usize::from(channels);
    let piece = MAX_SOUND_PAYLOAD - MAX_SOUND_PAYLOAD % frame_len;
    let whole = pcm.len() - pcm.len() % frame_len;
    for chunk in pcm[..whole].chunks(piece) {
        send_piece(session, rate, channels, chunk.to_vec(), chunk.len() / frame_len).await?;
    }
    Ok(())
}

async fn send_piece(
    session: &Session,
    rate: u32,
    channels: u8,
    payload: Vec<u8>,
    frames: usize,
) -> Result<()> {
    let (seq, ts_ms) =
        session.sound_clock.lock().unwrap_or_else(|e| e.into_inner()).next(rate, frames, Instant::now());
    let sound = SoundDatagram { seq, ts_ms, codec: SOUND_CODEC_PCM_S16LE, channels, rate, payload };
    let datagram = sound.encode();
    Counters::add(&session.counters.sound_sent, 1);
    if session.conn.max_datagram_size().is_some_and(|max| datagram.len() <= max)
        && session.send_datagram(bytes::Bytes::from(datagram)).is_ok()
    {
        return Ok(());
    }
    Counters::add(&session.counters.sound_on_stream, 1);
    session.send_sound_on_stream(sound).await
}

/// Pieces the `media/sound` stream holds while it's behind; past that the
/// oldest goes, as it would play too late.
const STREAM_QUEUE: usize = 24;

/// The `media/sound` stream to one peer: sound a datagram couldn't carry.
/// Opened on first need, and again after it fails.
#[derive(Debug, Default)]
pub(crate) struct SoundStream {
    queue: Mutex<VecDeque<SoundDatagram>>,
    ready: Notify,
    open: AtomicBool,
}

impl Session {
    /// Sends PC sound that a datagram can't carry on the `media/sound` stream.
    pub(crate) async fn send_sound_on_stream(&self, sound: SoundDatagram) -> Result<()> {
        let out = self.sound_out.clone();
        {
            let mut queue = out.queue.lock().unwrap_or_else(|e| e.into_inner());
            if queue.len() >= STREAM_QUEUE {
                queue.pop_front();
            }
            queue.push_back(sound);
        }
        out.ready.notify_one();
        if !out.open.swap(true, Ordering::AcqRel) {
            let (conn, cancel) = (self.conn.clone(), self.cancel.clone());
            self.runtime.spawn(async move {
                tokio::select! {
                    r = write_stream(&conn, &out) => {
                        if let Err(e) = r {
                            tracing::debug!(error = %e, "the sound stream ended");
                        }
                    }
                    () = cancel.cancelled() => {}
                }
                out.queue.lock().unwrap_or_else(|e| e.into_inner()).clear();
                out.open.store(false, Ordering::Release);
            });
        }
        Ok(())
    }
}

async fn write_stream(conn: &iroh::endpoint::Connection, out: &SoundStream) -> Result<()> {
    let (mut send, _recv) = conn.open_bi().await.map_err(crate::error::net)?;
    let _ = send.set_priority(crate::lane::priority_of(media_sound::SERVICE, media_sound::OP));
    let header = StreamHeader {
        svc: media_sound::SERVICE.into(),
        op: media_sound::OP.into(),
        v: media_sound::VERSION,
    };
    write_frame(&mut send, &Envelope::new(types::STREAM, &header)?.to_cbor()).await?;
    loop {
        let next = out.queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
        match next {
            Some(sound) => write_frame(&mut send, &sound.encode_body()).await?,
            None => out.ready.notified().await,
        }
    }
}

/// Plays a `media/sound` stream the PC opened: each frame is a sound
/// datagram without its tag.
pub(crate) async fn receive(shared: Arc<Shared>, peer: DeviceId, mut send: SendStream, mut recv: RecvStream) {
    while let Ok(Some(frame)) = read_frame(&mut recv).await {
        let Ok(sound) = SoundDatagram::decode_body(&frame) else { continue };
        let Some(session) = shared.session(&peer) else { break };
        crate::session::emit_sound(&shared, &session, sound);
    }
    let _ = send.finish();
}

/// The sound a session received: what came last, to drop what's older.
#[derive(Debug, Default)]
pub(crate) struct SoundReceived {
    last: Option<u16>,
    reported: Option<Instant>,
}

impl SoundReceived {
    /// Whether `seq` is newer than what came last; counts gaps and late ones.
    pub(crate) fn take(&mut self, seq: u16, counters: &Counters) -> bool {
        if let Some(last) = self.last {
            if !SoundDatagram::is_after(seq, last) {
                Counters::add(&counters.sound_late, 1);
                return false;
            }
            let missing = seq.wrapping_sub(last).wrapping_sub(1);
            Counters::add(&counters.sound_gaps, u64::from(missing));
        }
        self.last = Some(seq);
        Counters::add(&counters.sound_received, 1);
        let now = Instant::now();
        if self.reported.is_none_or(|at| now.saturating_duration_since(at) >= Duration::from_secs(1)) {
            self.reported = Some(now);
            let snapshot = counters.snapshot();
            crate::wire_stats::sound(
                "received",
                snapshot.sound_received,
                snapshot.sound_gaps,
                snapshot.sound_late,
                "any",
            );
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn pieces_sent_together_follow_one_another_in_time() {
        let mut clock = SoundClock::default();
        let start = Instant::now();
        // 240 frames at 48 kHz are 5 ms.
        assert_eq!(clock.next(48_000, 240, start), (0, 0));
        assert_eq!(clock.next(48_000, 240, start), (1, 5));
        assert_eq!(clock.next(48_000, 240, start + Duration::from_millis(3)), (2, 10));
    }

    #[test]
    fn a_pause_starts_the_time_again_from_the_clock() {
        let mut clock = SoundClock::default();
        let start = Instant::now();
        clock.next(48_000, 240, start);
        let (seq, ts) = clock.next(48_000, 240, start + Duration::from_secs(2));
        assert_eq!(seq, 1);
        assert_eq!(ts, 2000);
    }

    #[test]
    fn older_and_repeated_sound_is_dropped_and_gaps_are_counted() {
        let counters = Counters::default();
        let mut received = SoundReceived::default();
        assert!(received.take(10, &counters));
        assert!(received.take(13, &counters), "two missing");
        assert!(!received.take(12, &counters), "older than the last");
        assert!(!received.take(13, &counters), "a repeat");
        assert!(received.take(14, &counters));
        let counted = counters.snapshot();
        assert_eq!((counted.sound_received, counted.sound_gaps, counted.sound_late), (3, 2, 2));
        let mut wrapping = SoundReceived::default();
        assert!(wrapping.take(u16::MAX, &counters));
        assert!(wrapping.take(0, &counters), "across the wrap");
    }

    #[test]
    fn the_number_wraps() {
        let mut clock = SoundClock { seq: u16::MAX, ..Default::default() };
        let now = Instant::now();
        assert_eq!(clock.next(48_000, 1, now).0, u16::MAX);
        assert_eq!(clock.next(48_000, 1, now).0, 0);
    }
}
