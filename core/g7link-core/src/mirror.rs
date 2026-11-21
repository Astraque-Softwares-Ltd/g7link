// SPDX-License-Identifier: MPL-2.0
//! Screen mirroring (docs/protocol/mirror.md): a PC asks a phone for its
//! screen; once the phone's user agrees, the phone streams encoded video
//! on a stream of its own, which the PC decodes and shows.
//!
//! The phone side keeps latency low over throughput: when the network
//! can't keep up, frames are dropped (never queued for long) and the
//! encoder is asked for a keyframe to start clean from.
//!
//! When the PC asks for it and the phone can, the phone's sound comes on a
//! second stream (PCM), dropped rather than delayed in the same way.
//!
//! Each mirroring is a session the PC numbers: 0 for the phone's screen,
//! others for apps the phone runs on displays of their own (Elevated), each
//! in a window of its own on the PC.

use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use g7link_protocol::{
    DeviceId, Envelope, ErrorCode, PacketKind, ProtocolError,
    messages::{
        MirrorAudioConfig, MirrorClock, MirrorConfig, MirrorInput, MirrorNotice, MirrorPower, MirrorResize,
        MirrorSession, MirrorSound, MirrorStart, PhoneApp, PhoneApps, StreamHeader, mirror, types,
    },
    read_video_packet, video_packet_header, write_frame,
};
use iroh::endpoint::{RecvStream, SendStream, VarInt};
use tokio::sync::mpsc;

use crate::{Error, Result, dispatch::Responder, events::NodeEvent, node::Shared, session::Session};

/// The device toggle that allows phone-to-PC mirroring for a device.
pub(crate) const TOGGLE: &str = "mirroring";
/// The per-device toggle that allows PC-to-phone reverse mirroring (off by default).
pub const PC_MIRROR_TOGGLE: &str = "pc_mirror";

fn is_local_desktop(shared: &Shared) -> bool {
    let local = shared.local.read().unwrap_or_else(|e| e.into_inner());
    matches!(crate::features::role_of(&local.device), Some(crate::features::Role::Desktop))
}

fn pc_mirror_allowed(shared: &Shared, peer: &DeviceId) -> bool {
    shared.toggle_on(peer, PC_MIRROR_TOGGLE)
        || shared.pc_mirror_sessions.lock().unwrap_or_else(|e| e.into_inner()).contains(peer)
}
/// Packets waiting to go out, at most. Beyond this the network is behind:
/// sending more would only add delay.
const QUEUE: usize = 4;
/// The same for a stream that's held back (`Pacer`): enough to ride out a
/// short wait, as overflowing costs a keyframe, which is what's slow to send.
const PACED_QUEUE: usize = 8;
/// Sound packets waiting to go out, at most (each is a few milliseconds).
const AUDIO_QUEUE: usize = 24;
/// Video stops going out once the oldest packet not yet reported received is
/// this much later than a round trip takes. QUIC would take megabytes more, and
/// on a slow link that's many seconds between moving the pointer and seeing it.
const MAX_LAG: Duration = Duration::from_millis(250);
/// A hold longer than this leaves what waited through it too old to show:
/// it's dropped for a fresh keyframe instead of being sent late, which would
/// only start the next hold.
const STALE_AFTER: Duration = Duration::from_millis(500);
/// How long a measured round trip stays in use: long enough to span a busy
/// spell, short enough to notice a move from Wi-Fi to a relay.
const TRIP_MEMORY: Duration = Duration::from_secs(10);
/// A hold also needs at least this much on its way (and a quarter of a
/// second of the stream's rate): a few late reports of a quiet stream say the
/// other side's radio dozed for a moment, not that the link is full.
const HOLD_MIN_BYTES: u64 = 32 * 1024;
/// Report trips kept for the stats between two looks, at most.
const MAX_TRIP_SAMPLES: usize = 4096;
/// The PC's clock is timed this many times, the last few kept: the shortest
/// round trip among them is the least skewed by queues on the way.
const CLOCK_SAMPLES: usize = 8;
/// A clock exchange slower than this says nothing useful.
const MAX_CLOCK_TRIP_US: u64 = 2_000_000;
/// How often the PC's clock is timed while its screen shows: often at first,
/// then enough to follow a change of path.
const CLOCK_FIRST_EVERY: Duration = Duration::from_secs(1);
const CLOCK_FIRST_COUNT: u32 = 5;
const CLOCK_EVERY: Duration = Duration::from_secs(5);

/// What the PC's app gets of a mirroring stream. Called from the core's
/// threads; must return quickly (hand the work to a thread of its own).
pub trait MirrorSink: Send + Sync {
    /// The stream's format: first, and again when it changes.
    fn config(&self, config: MirrorConfig);
    /// Encoded video (Annex B), `keyframe` when it starts fresh.
    fn packet(&self, keyframe: bool, time_us: u64, data: Vec<u8>);
    /// The stream ended (the phone stopped, or the connection dropped).
    fn ended(&self);
    /// The sound's format: first on the sound stream, and when it changes.
    fn audio_config(&self, _config: MirrorAudioConfig) {}
    /// Sound, in that format.
    fn audio(&self, _time_us: u64, _data: Vec<u8>) {}
    /// The sound stream ended.
    fn audio_ended(&self) {}
    /// A newer estimate of the sender's clock (the one its packets' `time` is
    /// on) against this device's `CLOCK_MONOTONIC`: the sender's minus ours,
    /// and the round trip it was timed over, both in microseconds. Phones
    /// showing a PC's screen only.
    fn clock(&self, _offset_us: i64, _rtt_us: u32) {}
}

/// This device's `CLOCK_MONOTONIC`, in microseconds: the clock a PC's video
/// packets are stamped with, and the one Android's `System.nanoTime` reads.
#[cfg(unix)]
#[allow(unsafe_code)]
pub(crate) fn monotonic_us() -> u64 {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `now` is a valid timespec for the call to fill; CLOCK_MONOTONIC
    // always exists on Linux and Android.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) } != 0 {
        return 0;
    }
    let secs = u64::try_from(now.tv_sec).unwrap_or(0);
    let micros = u64::try_from(now.tv_nsec).unwrap_or(0) / 1000;
    secs.saturating_mul(1_000_000).saturating_add(micros)
}

/// Elsewhere, a monotonic clock of this process's own.
#[cfg(not(unix))]
pub(crate) fn monotonic_us() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    u64::try_from(START.get_or_init(Instant::now).elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// The other side's clock against ours, from timed exchanges.
#[derive(Debug, Default)]
struct ClockEstimate {
    /// The last few: the other's clock minus ours at the middle of the
    /// exchange, and its round trip, in microseconds.
    samples: VecDeque<(i64, u32)>,
}

impl ClockEstimate {
    /// An exchange sent at `sent_us` and answered at `received_us` (our
    /// clock) whose answer read `their_us` (theirs). Returns the estimate:
    /// the kept sample with the shortest round trip.
    fn add(&mut self, sent_us: u64, their_us: u64, received_us: u64) -> Option<(i64, u32)> {
        let trip = received_us.checked_sub(sent_us).filter(|trip| *trip <= MAX_CLOCK_TRIP_US);
        if let Some(trip) = trip {
            let middle = sent_us + trip / 2;
            if let Ok(offset) = i64::try_from(i128::from(their_us) - i128::from(middle)) {
                if self.samples.len() == CLOCK_SAMPLES {
                    self.samples.pop_front();
                }
                self.samples.push_back((offset, u32::try_from(trip).unwrap_or(u32::MAX)));
            }
        }
        self.samples.iter().min_by_key(|(_, trip)| *trip).copied()
    }
}

/// Phone: times the PC's clock while its screen shows here, and tells `sink`
/// each new estimate. Runs until it's aborted.
async fn follow_clock(shared: std::sync::Weak<Shared>, peer: DeviceId, sink: Arc<dyn MirrorSink>) {
    let mut estimate = ClockEstimate::default();
    let mut timed = 0u32;
    loop {
        let session = shared.upgrade().and_then(|shared| shared.session(&peer));
        if let Some(session) = session {
            let sent = monotonic_us();
            let answer =
                session.request(Envelope::empty(types::MIRROR_CLOCK), crate::session::REQUEST_TIMEOUT).await;
            let received = monotonic_us();
            match answer.map(|env| env.expect_body::<MirrorClock>(types::MIRROR_CLOCK)) {
                Ok(Ok(clock)) => {
                    if let Some((offset, trip)) = estimate.add(sent, clock.now_us, received) {
                        sink.clock(offset, trip);
                    }
                }
                Ok(Err(e)) => {
                    tracing::debug!(error = %e, "the PC didn't tell its clock");
                    return;
                }
                Err(e) => tracing::debug!(error = %e, "couldn't time the PC's clock"),
            }
        }
        timed = timed.saturating_add(1);
        tokio::time::sleep(if timed < CLOCK_FIRST_COUNT { CLOCK_FIRST_EVERY } else { CLOCK_EVERY }).await;
    }
}

/// Aborts its task when dropped.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl std::fmt::Debug for dyn MirrorSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MirrorSink")
    }
}

// ---- The PC ----

pub(crate) async fn start(shared: &Shared, session: &Session, options: MirrorStart) -> Result<()> {
    let allowed = if is_local_desktop(shared) {
        shared.toggle_on(&session.peer, TOGGLE)
    } else {
        shared.store.toggle(&session.peer, PC_MIRROR_TOGGLE)?.unwrap_or(true)
    };
    if !allowed {
        return Err(Error::Denied);
    }
    if !options.is_valid() {
        // Refused here, before anything reaches the phone: say which options.
        tracing::warn!(
            session = options.session,
            app = options.app.is_some(),
            watch = options.watch.is_some(),
            stay_awake = options.stay_awake,
            screen_off = options.screen_off,
            sound_only = options.sound_only,
            "not asking the phone: invalid mirroring options"
        );
        return Err(Error::Protocol("invalid mirroring options".into()));
    }
    let env = Envelope::new(types::MIRROR_START, &options)?;
    match session.request(env, crate::session::REQUEST_TIMEOUT).await?.expect(types::OK) {
        Ok(()) => Ok(()),
        // The phone is locked: it opens nothing without a tap until it's unlocked.
        Err(ProtocolError::Remote { code: ErrorCode::Denied, msg }) if msg == mirror::NOTICE_LOCKED => {
            Err(Error::Declined)
        }
        Err(e) => Err(e.into()),
    }
}

/// Tells the PC something about its mirroring (`mirror.notice`), by `code`.
pub(crate) async fn notice(session: &Session, mirroring: u32, code: &str) -> Result<()> {
    let body = MirrorNotice { session: mirroring, code: code.to_owned() };
    if !body.is_valid() {
        return Err(Error::Protocol("invalid mirror notice".into()));
    }
    session.send(Envelope::new(types::MIRROR_NOTICE, &body)?).await
}

pub(crate) async fn stop(session: &Session, mirroring: u32) -> Result<()> {
    let env = Envelope::new(types::MIRROR_STOP, &MirrorSession { session: mirroring })?;
    session.request(env, crate::session::REQUEST_TIMEOUT).await?.expect(types::OK)?;
    Ok(())
}

/// Mouse and keyboard on a phone's mirrored screen or app window (not
/// answered: input is only worth it right away).
pub(crate) async fn input(
    shared: &Shared,
    session: &Session,
    mirroring: u32,
    input: MirrorInput,
) -> Result<()> {
    let allowed = if is_local_desktop(shared) {
        shared.toggle_on(&session.peer, TOGGLE)
    } else {
        // Whether this phone may control the PC is the PC's to say: it refuses,
        // and asks its user, when `remote_input` is off there.
        shared.store.toggle(&session.peer, PC_MIRROR_TOGGLE)?.unwrap_or(true)
    };
    if !allowed {
        return Err(Error::Denied);
    }
    if !input.is_valid() {
        return Err(Error::Protocol("invalid input".into()));
    }
    session.send(input_envelope(&input, mirroring)?).await
}

fn input_envelope(input: &MirrorInput, mirroring: u32) -> Result<Envelope> {
    Ok(g7link_protocol::messages::mirror_input_envelope(input, mirroring)?)
}

pub(crate) async fn request_keyframe(session: &Session, mirroring: u32) {
    if let Ok(env) = Envelope::new(types::MIRROR_KEYFRAME, &MirrorSession { session: mirroring }) {
        let _ = session.send(env).await;
    }
}

/// Resize an app window's display (`mirroring` != 0) on a phone.
pub(crate) async fn resize(
    shared: &Shared,
    session: &Session,
    mirroring: u32,
    width: u32,
    height: u32,
) -> Result<()> {
    if !shared.toggle_on(&session.peer, TOGGLE) {
        return Err(Error::Denied);
    }
    let msg = MirrorResize { session: mirroring, width, height };
    if !msg.is_valid() {
        return Err(Error::Protocol("invalid resize".into()));
    }
    session.send(Envelope::new(types::MIRROR_RESIZE, &msg)?).await
}

/// Update `stay_awake` and `screen_off` on a phone while mirroring its screen (`session == 0`).
pub(crate) async fn power(
    shared: &Shared,
    session: &Session,
    stay_awake: bool,
    screen_off: bool,
) -> Result<()> {
    if !shared.toggle_on(&session.peer, TOGGLE) {
        return Err(Error::Denied);
    }
    let msg = MirrorPower { stay_awake, screen_off };
    session.send(Envelope::new(types::MIRROR_POWER, &msg)?).await
}

/// Sends the phone's sound here, or leaves it on the phone (not answered).
pub(crate) async fn sound(shared: &Shared, session: &Session, on: bool) -> Result<()> {
    if !shared.toggle_on(&session.peer, TOGGLE) {
        return Err(Error::Denied);
    }
    session.send(Envelope::new(types::MIRROR_SOUND, &MirrorSound { on })?).await
}

/// The apps a phone can open in windows of their own, by name.
pub(crate) async fn apps(shared: &Shared, session: &Session) -> Result<Vec<PhoneApp>> {
    if !shared.toggle_on(&session.peer, TOGGLE) {
        return Err(Error::Denied);
    }
    let env = Envelope::empty(types::MIRROR_APPS);
    let PhoneApps { mut apps } = session.request_paged(env, crate::session::REQUEST_TIMEOUT).await?;
    apps.retain(|a| g7link_protocol::messages::is_package_name(&a.pkg));
    apps.truncate(mirror::MAX_APPS);
    for app in &mut apps {
        app.label = app.label.chars().take(100).collect();
        if app.icon.as_ref().is_some_and(|i| i.len() > mirror::MAX_ICON_BYTES) {
            app.icon = None;
        }
    }
    apps.sort_by_cached_key(|a| a.label.to_lowercase());
    Ok(apps)
}

/// A phone opened a video stream (its header already read). Its first
/// packet says which mirroring it is.
pub(crate) async fn receive(shared: Arc<Shared>, peer: DeviceId, mut send: SendStream, mut recv: RecvStream) {
    let refuse = |mut send: SendStream, mut recv: RecvStream| {
        let _ = send.reset(VarInt::from_u32(mirror::STOPPED));
        let _ = recv.stop(VarInt::from_u32(mirror::STOPPED));
    };
    let shows = shared.we_offer(mirror::VIEW);
    let toggle_on = || {
        if is_local_desktop(&shared) {
            shared.toggle_on(&peer, TOGGLE)
        } else {
            shared.store.toggle(&peer, PC_MIRROR_TOGGLE).ok().flatten().unwrap_or(true)
        }
    };
    if !shows || !toggle_on() {
        return refuse(send, recv);
    }
    let first = match read_video_packet(&mut recv).await {
        Ok(Some(p)) if p.kind == PacketKind::Config => {
            let config = MirrorConfig::from_cbor(&p.data).ok().filter(|c| c.is_valid());
            if config.is_none() {
                tracing::warn!("a phone sent a video format this PC can't show");
            }
            config
        }
        Ok(Some(_)) => {
            tracing::warn!("a phone's video stream started without its format");
            None
        }
        Ok(None) | Err(_) => {
            tracing::warn!("a phone's video stream ended before its first frame");
            None
        }
    };
    let Some(config) = first else {
        return refuse(send, recv);
    };
    let mirroring = config.session;
    // Turned off, or unpaired, while the format was on its way: it doesn't start.
    let allowed = || shared.session(&peer).is_some() && toggle_on();
    if !allowed() {
        return refuse(send, recv);
    }
    let Some(sink) = shared.platform.mirror_sink(&peer, mirroring) else {
        return refuse(send, recv);
    };
    // Ready to be stopped before anyone hears it's showing: a stop sent the
    // moment it appears must not be lost.
    let stopped = shared.new_mirror_stop(&peer, mirroring, mirror::OP_VIDEO);
    // Turned off since: no one could stop it, so it stops itself.
    if !allowed() {
        stopped.notify_one();
    }
    let started_at = std::time::Instant::now();
    let title = if mirroring == mirror::SCREEN {
        "Screen mirroring".to_owned()
    } else {
        format!("App window #{mirroring}")
    };
    let tl_row = shared.record_timeline(crate::timeline::NewTimelineEntry {
        kind: crate::TimelineKind::Session,
        device_id: peer,
        device_name: shared.peer_name(&peer),
        incoming: true,
        timestamp: crate::now_unix(),
        title,
        detail: format!("{}×{}", config.width, config.height),
        target: "mirror".into(),
        size_bytes: 0,
        duration_secs: 0,
        ref_id: None,
    });
    shared.emit(NodeEvent::Mirroring { device: peer, session: mirroring, on: true });
    sink.config(config);
    // A phone showing a PC's screen times the PC's clock, which its packets
    // are stamped with, for as long as the stream lasts.
    let _clock = (mirroring == mirror::SCREEN && !is_local_desktop(&shared))
        .then(|| AbortOnDrop(tokio::spawn(follow_clock(Arc::downgrade(&shared), peer, sink.clone()))));
    // Reports what arrived, for a sender that uses it (`Pacer`). They go ahead
    // of anything else this side is sending, or they'd measure that instead.
    let reports = peer_reports(&shared, &peer);
    let _ = send.set_priority(AUDIO_PRIORITY);
    if reports {
        let _ = send.write_all(&[1]).await;
    }
    loop {
        let packet = tokio::select! {
            _ = stopped.notified() => break,
            packet = read_video_packet(&mut recv) => packet,
        };
        match packet {
            Ok(Some(p)) if p.kind == PacketKind::Config => match MirrorConfig::from_cbor(&p.data) {
                Ok(config) if config.is_valid() && config.session == mirroring => sink.config(config),
                _ => {
                    tracing::warn!("a phone sent a video format this PC can't show");
                    break;
                }
            },
            Ok(Some(p)) => sink.packet(p.kind == PacketKind::Keyframe, p.time_us, p.data),
            Ok(None) => break,
            Err(e) => {
                tracing::debug!(error = %e, "mirroring stream ended");
                break;
            }
        }
        if reports && send.write_all(&[1]).await.is_err() {
            break;
        }
    }
    let _ = recv.stop(VarInt::from_u32(mirror::STOPPED));
    let _ = send.finish();
    if let Some(row_id) = tl_row {
        shared.finish_timeline_session(row_id, started_at.elapsed().as_secs().max(1));
    }
    if shared.clear_mirror_stop(&peer, mirroring, mirror::OP_VIDEO, &stopped) {
        sink.ended();
        shared.emit(NodeEvent::Mirroring { device: peer, session: mirroring, on: false });
    }
}

/// A phone opened a sound stream (its header already read).
pub(crate) async fn receive_audio(
    shared: Arc<Shared>,
    peer: DeviceId,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    // Sound comes with the screen, or on its own (`MirrorStart::sound_only`);
    // the app gives a sink for either.
    let listens = shared.we_offer(mirror::LISTEN);
    let allowed = if is_local_desktop(&shared) {
        shared.toggle_on(&peer, TOGGLE)
    } else {
        shared.store.toggle(&peer, PC_MIRROR_TOGGLE).ok().flatten().unwrap_or(true)
    };
    let sink = (listens && allowed).then(|| shared.platform.mirror_sink(&peer, mirror::SCREEN)).flatten();
    let Some(sink) = sink else {
        let _ = send.reset(VarInt::from_u32(mirror::STOPPED));
        let _ = recv.stop(VarInt::from_u32(mirror::STOPPED));
        return;
    };
    let stopped = shared.new_mirror_stop(&peer, mirror::SCREEN, mirror::OP_AUDIO);
    loop {
        let packet = tokio::select! {
            _ = stopped.notified() => break,
            packet = read_video_packet(&mut recv) => packet,
        };
        match packet {
            Ok(Some(p)) if p.kind == PacketKind::Config => match MirrorAudioConfig::from_cbor(&p.data) {
                Ok(config) if config.is_valid() => sink.audio_config(config),
                _ => {
                    tracing::warn!("a phone sent a sound format this PC can't play");
                    break;
                }
            },
            Ok(Some(p)) => sink.audio(p.time_us, p.data),
            Ok(None) => break,
            Err(e) => {
                tracing::debug!(error = %e, "mirrored sound ended");
                break;
            }
        }
    }
    let _ = recv.stop(VarInt::from_u32(mirror::STOPPED));
    let _ = send.finish();
    if shared.clear_mirror_stop(&peer, mirror::SCREEN, mirror::OP_AUDIO, &stopped) {
        sink.audio_ended();
    }
}

// ---- The phone ----

/// Handles `mirror.*` on a session. Returns false for other types.
pub(crate) async fn handle(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<bool> {
    let peer = session.peer;
    let platform = shared.platform.clone();
    match env.t.as_str() {
        types::MIRROR_START => {
            let options: MirrorStart = env.body()?;
            let desktop = is_local_desktop(shared);
            let needs = if options.sound_only {
                mirror::SOUND
            } else if options.app.is_some() {
                mirror::VIRTUAL_DISPLAY
            } else if desktop && options.watch.is_some() {
                g7link_protocol::messages::media_watch::STREAM
            } else {
                mirror::CAPTURE
            };
            let allowed = match &options.watch {
                // Play on phone: only what this PC offered that phone.
                Some(player) if desktop => crate::media::watch_allowed(shared, &peer, player),
                Some(_) => false,
                // Sound on its own is the phone's (a PC shares its screen).
                None if desktop && options.sound_only => false,
                None if desktop => pc_mirror_allowed(shared, &peer),
                None => shared.toggle_on(&peer, TOGGLE),
            };
            let reply = if !options.is_valid() {
                Envelope::error(ErrorCode::BadMessage, "invalid mirroring options")
            } else if !allowed {
                if desktop && options.app.is_none() && options.watch.is_none() {
                    shared.emit(NodeEvent::PcMirrorRequested { device: peer, options: options.clone() });
                }
                Envelope::error(ErrorCode::Denied, "mirroring is off for this device")
            } else if !shared.we_offer(needs) {
                Envelope::error(ErrorCode::Unsupported, "this device doesn't share that")
            } else {
                let requested_session = options.session;
                // An app window opens without a tap on the phone: not while it's locked.
                let app_window = !desktop && options.app.is_some();
                let started = tokio::task::spawn_blocking(move || {
                    if app_window && platform.phone_locked() {
                        return Ok(false);
                    }
                    platform.mirror_requested(&peer, &options).map(|()| true)
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
                match started {
                    Ok(true) => {
                        shared.record_outgoing_mirror_session(peer, requested_session);
                        Envelope::empty(types::OK)
                    }
                    Ok(false) => Envelope::error(ErrorCode::Denied, mirror::NOTICE_LOCKED),
                    Err(reason) => {
                        tracing::warn!(reason, "can't ask to share the screen");
                        if desktop {
                            Envelope::error(ErrorCode::Denied, reason)
                        } else {
                            Envelope::error(ErrorCode::Internal, "the phone couldn't ask")
                        }
                    }
                }
            };
            rsp.send(reply).await?;
        }
        types::MIRROR_STOP => {
            let MirrorSession { session: mirroring } = env.body()?;
            // The PC's side: stop showing; the phone's: stop sharing.
            shared.stop_showing(&peer, mirroring);
            shared.remove_outgoing_mirror_session(&peer, mirroring);
            if mirroring == mirror::SCREEN {
                shared.pc_mirror_sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(&peer);
                // Watching stopped: the phone may come back to it for a while
                // (the screen rotated, the app went to the background).
                crate::media::renew_watch(shared, &peer);
            }
            shared.emit(NodeEvent::Mirroring { device: peer, session: mirroring, on: false });
            tokio::task::spawn_blocking(move || platform.mirror_stop_requested(&peer, mirroring));
            rsp.send(Envelope::empty(types::OK)).await?;
        }
        types::MIRROR_NOTICE => {
            let notice: MirrorNotice = env.body()?;
            if notice.is_valid() && shared.we_offer(mirror::VIEW) {
                shared.emit(NodeEvent::MirrorNotice {
                    device: peer,
                    session: notice.session,
                    code: notice.code,
                });
            }
        }
        types::MIRROR_KEYFRAME => {
            let MirrorSession { session: mirroring } = env.body()?;
            tokio::task::spawn_blocking(move || platform.mirror_keyframe_requested(&peer, mirroring));
        }
        types::MIRROR_CLOCK => {
            // A PC's screen is stamped with its clock; the phone showing it
            // times it against its own.
            if is_local_desktop(shared) {
                rsp.send(Envelope::new(types::MIRROR_CLOCK, &MirrorClock { now_us: monotonic_us() })?)
                    .await?;
            } else {
                rsp.error(ErrorCode::Unsupported, "only a PC's screen goes by its clock").await?;
            }
        }
        types::MIRROR_RESIZE => {
            let resize: MirrorResize = env.body()?;
            let allowed = shared.toggle_on(&peer, TOGGLE) && shared.we_offer(mirror::VIRTUAL_DISPLAY);
            if allowed && resize.is_valid() {
                tokio::task::spawn_blocking(move || {
                    platform.mirror_resize_requested(&peer, resize.session, resize.width, resize.height);
                });
            }
        }
        types::MIRROR_SOUND => {
            let MirrorSound { on } = env.body()?;
            if shared.toggle_on(&peer, TOGGLE) && shared.we_offer(mirror::CAPTURE) {
                tokio::task::spawn_blocking(move || platform.mirror_sound_requested(&peer, on));
            }
        }
        types::MIRROR_POWER => {
            let power: MirrorPower = env.body()?;
            let allowed = shared.toggle_on(&peer, TOGGLE) && shared.we_offer(mirror::CAPTURE);
            if allowed {
                tokio::task::spawn_blocking(move || {
                    platform.mirror_power_requested(&peer, power.stay_awake, power.screen_off);
                });
            }
        }
        types::MIRROR_INPUT => {
            let input: MirrorInput = env.body()?;
            let MirrorSession { session: mirroring } = env.body()?;
            // App windows take input as they're Elevated; the screen, with mirror.input.
            let needs = if mirroring == mirror::SCREEN { mirror::INPUT } else { mirror::VIRTUAL_DISPLAY };
            let desktop = is_local_desktop(shared);
            let mirror_on =
                if desktop { pc_mirror_allowed(shared, &peer) } else { shared.toggle_on(&peer, TOGGLE) };
            let remote_input_on = !desktop || shared.toggle_on(&peer, crate::remote::TOGGLE);
            let has_cap = shared.we_offer(needs);
            let allowed = mirror_on && remote_input_on && has_cap;
            if allowed && input.is_valid() {
                let _ =
                    tokio::task::spawn_blocking(move || platform.mirror_input(&peer, mirroring, input)).await;
            } else if mirroring == mirror::SCREEN && mirror_on && input.is_valid() {
                if desktop {
                    if !remote_input_on {
                        let first_time =
                            shared.remote.lock().unwrap_or_else(|e| e.into_inner()).mark_prompted(peer);
                        if first_time {
                            shared.emit(NodeEvent::RemoteInputRequested { device: peer });
                        }
                    }
                } else if !has_cap {
                    shared.emit(NodeEvent::RemoteInputRequested { device: peer });
                }
            }
        }
        types::MIRROR_APPS => {
            if !shared.toggle_on(&peer, TOGGLE) {
                rsp.error(ErrorCode::Denied, "mirroring is off for this device").await?;
            } else if !shared.we_offer(mirror::VIRTUAL_DISPLAY) {
                rsp.error(ErrorCode::Unsupported, "this device doesn't open apps in windows").await?;
            } else {
                match tokio::task::spawn_blocking(move || platform.phone_apps())
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()))
                {
                    Ok(apps) => rsp.paged(PhoneApps { apps: fit_apps(apps) }).await?,
                    Err(reason) => {
                        tracing::warn!(reason, "can't list the apps");
                        rsp.error(ErrorCode::Internal, "the phone couldn't list its apps").await?;
                    }
                }
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Apps within the limits: at most [`mirror::MAX_APPS`], each icon small.
/// A long list goes in parts.
fn fit_apps(mut apps: Vec<PhoneApp>) -> Vec<PhoneApp> {
    apps.retain(|a| g7link_protocol::messages::is_package_name(&a.pkg));
    apps.truncate(mirror::MAX_APPS);
    for app in &mut apps {
        app.label = app.label.chars().take(100).collect();
        if app.icon.as_ref().is_some_and(|icon| icon.len() > mirror::MAX_ICON_BYTES) {
            app.icon = None;
        }
    }
    apps
}

/// What became of a packet handed to [`MirrorStream::send`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorSend {
    Queued,
    /// Sound dropped to keep the delay low (the next packet may go).
    Dropped,
    /// Dropped to keep the delay low: send a keyframe next (frames are
    /// dropped until one comes).
    NeedKeyframe,
    /// The PC stopped watching, or the connection is gone: stop sharing.
    Closed,
}

/// A phone's video (or sound) stream to one PC.
#[derive(Debug)]
pub struct MirrorStream {
    queue: mpsc::Sender<(PacketKind, u64, Vec<u8>)>,
    closed: Arc<AtomicBool>,
    /// Wakes the sending task when the stream is closed, so it finishes the
    /// stream even if no more packets come.
    stop: Arc<tokio::sync::Notify>,
    /// Frames are being dropped until a keyframe. The sending task sets it
    /// too, when it drops what waited too long.
    resyncing: Arc<Mutex<bool>>,
    /// Sound: each packet stands alone, so a dropped one needs no keyframe.
    audio: bool,
    /// What the link was last measured to carry, in bits per second; 0 for
    /// nothing new (`link_measure`).
    link_rate: Arc<AtomicU64>,
    /// How many bytes that measure was made over.
    link_bytes: Arc<AtomicU64>,
    /// The stream's bitrate, as its sender last said (`set_rate`).
    rate: Arc<AtomicU64>,
    /// Whether the sending task holds video back now (`is_held`).
    holding: Arc<AtomicBool>,
    /// What the pacing did since the stats were last taken.
    pace: Arc<Mutex<PaceStats>>,
}

/// What a video stream's pacing did since it was last asked
/// ([`MirrorStream::take_stats`]). Empty for a stream that isn't paced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MirrorStats {
    /// Holds started: video waited for the other side to catch up.
    pub holds: u32,
    /// How long video was held, in all.
    pub held: Duration,
    /// The shortest time from writing a packet to its report.
    pub shortest_trip: Option<Duration>,
    /// The 95th percentile of those times.
    pub p95_trip: Option<Duration>,
}

/// [`MirrorStats`] as they build up.
#[derive(Debug, Default)]
struct PaceStats {
    holds: u32,
    held: Duration,
    /// When the hold under way started.
    holding_since: Option<Instant>,
    /// Report trips, at most [`MAX_TRIP_SAMPLES`].
    trips: Vec<Duration>,
}

impl PaceStats {
    /// What built up since the last time, as of `now` (a hold under way
    /// counts up to now).
    fn take(&mut self, now: Instant) -> MirrorStats {
        let mut held = std::mem::take(&mut self.held);
        if let Some(since) = self.holding_since.as_mut() {
            held += now.saturating_duration_since(*since);
            *since = now;
        }
        let mut trips = std::mem::take(&mut self.trips);
        trips.sort_unstable();
        let p95_trip = trips.len().checked_sub(1).map(|last| trips[last * 95 / 100]);
        MirrorStats {
            holds: std::mem::take(&mut self.holds),
            held,
            shortest_trip: trips.first().copied(),
            p95_trip,
        }
    }
}

impl MirrorStream {
    /// Queues a packet without waiting. A config packet waits for room:
    /// the PC can't decode without it.
    pub fn send(&self, kind: PacketKind, time_us: u64, data: Vec<u8>) -> MirrorSend {
        if self.closed.load(Ordering::Acquire) {
            return MirrorSend::Closed;
        }
        {
            let resyncing = self.resyncing.lock().unwrap_or_else(|e| e.into_inner());
            if *resyncing && kind == PacketKind::Frame && !self.audio {
                return MirrorSend::NeedKeyframe;
            }
        }
        let sent = if kind == PacketKind::Config {
            self.queue.blocking_send((kind, time_us, data)).map_err(|_| ())
        } else {
            self.queue.try_send((kind, time_us, data)).map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => (),
                mpsc::error::TrySendError::Closed(_) => self.closed.store(true, Ordering::Release),
            })
        };
        match sent {
            Ok(()) => {
                if kind == PacketKind::Keyframe {
                    *self.resyncing.lock().unwrap_or_else(|e| e.into_inner()) = false;
                }
                MirrorSend::Queued
            }
            Err(()) if self.closed.load(Ordering::Acquire) => MirrorSend::Closed,
            Err(()) if self.audio => MirrorSend::Dropped,
            Err(()) => {
                *self.resyncing.lock().unwrap_or_else(|e| e.into_inner()) = true;
                MirrorSend::NeedKeyframe
            }
        }
    }

    /// Ends the stream (the phone stopped sharing).
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.stop.notify_one();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) || self.queue.is_closed()
    }

    /// What the link was last measured to carry, in bits per second, and
    /// over how many bytes, once, when video got too far ahead of it. Never
    /// from a device that doesn't report what it receives.
    ///
    /// The measure is those bytes over the time they took, so it says what
    /// the link carries only if they kept it busy: a few bytes that arrived
    /// late (a quiet picture, a phone's radio asleep for a moment) measure
    /// the delay, not the link. The caller knows its bitrate and judges.
    pub fn link_measure(&self) -> Option<(u64, u64)> {
        let rate = Some(self.link_rate.swap(0, Ordering::AcqRel)).filter(|rate| *rate > 0)?;
        Some((rate, self.link_bytes.load(Ordering::Acquire)))
    }

    /// Whether everything queued is still waiting to go out: the link is
    /// held, and anything made now would only be dropped.
    pub fn is_backed_up(&self) -> bool {
        self.queue.capacity() == 0 && !self.is_closed()
    }

    /// Whether video is held back now, until the other side has what's on its
    /// way. A picture made meanwhile would only wait: keep the newest, and
    /// send it once this ends.
    pub fn is_held(&self) -> bool {
        self.holding.load(Ordering::Acquire) && !self.is_closed()
    }

    /// The bitrate the stream is encoded at now. A hold needs a quarter of a
    /// second of it on its way (and at least [`HOLD_MIN_BYTES`]).
    pub fn set_rate(&self, bits_per_second: u32) {
        self.rate.store(u64::from(bits_per_second), Ordering::Release);
    }

    /// What the pacing did since the last call: holds, how long they lasted,
    /// and how long reports took.
    pub fn take_stats(&self) -> MirrorStats {
        self.pace.lock().unwrap_or_else(|e| e.into_inner()).take(Instant::now())
    }
}

/// Keeps video from getting ahead of what the other side has received, from
/// its one-byte report per packet. Works at any bitrate and through any queue
/// on the way, a relay's included, which QUIC's own windows can't see.
#[derive(Debug, Default)]
struct Pacer {
    /// Written and not yet reported: when, and how many bytes.
    in_flight: VecDeque<(Instant, usize)>,
    in_flight_bytes: u64,
    /// Recent times from writing a packet to its report, the shortest first:
    /// the shortest is close to a round trip with nothing queued.
    trips: VecDeque<(Instant, Duration)>,
    /// Too far ahead: writes nothing until all of it has arrived. This is also
    /// what keeps `trips` honest, as the next packet finds the way clear.
    draining: Option<Drain>,
    /// How many bytes the last wait measured the link over.
    measured_bytes: u64,
    /// The stream's bitrate, in bits per second (0: not known).
    rate: u64,
    /// Holds and report trips, for the stats.
    stats: Arc<Mutex<PaceStats>>,
}

/// A wait for what's on its way to arrive, which also measures the link: it's
/// been busy with these bytes since the first of them was written.
#[derive(Debug)]
struct Drain {
    since: Instant,
    bytes: u64,
}

impl Pacer {
    fn written(&mut self, now: Instant, bytes: usize) {
        self.in_flight.push_back((now, bytes));
        self.in_flight_bytes += bytes as u64;
    }

    /// `reports` more packets arrived. Returns what the link carries, in bits
    /// per second, if that ended a wait.
    fn reported(&mut self, now: Instant, reports: usize) -> Option<u64> {
        let mut stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..reports {
            let Some((written, bytes)) = self.in_flight.pop_front() else { break };
            self.in_flight_bytes -= bytes as u64;
            let trip = now.saturating_duration_since(written);
            if stats.trips.len() < MAX_TRIP_SAMPLES {
                stats.trips.push(trip);
            }
            while self.trips.back().is_some_and(|(_, t)| *t >= trip) {
                self.trips.pop_back();
            }
            self.trips.push_back((now, trip));
        }
        while self.trips.len() > 1
            && self.trips.front().is_some_and(|(at, _)| now.saturating_duration_since(*at) > TRIP_MEMORY)
        {
            self.trips.pop_front();
        }
        if !self.in_flight.is_empty() {
            return None;
        }
        let drain = self.draining.take()?;
        if let Some(since) = stats.holding_since.take() {
            stats.held += now.saturating_duration_since(since);
        }
        self.measured_bytes = drain.bytes;
        // Less a round trip: the last report took that on top of the link's time.
        let queued = now.saturating_duration_since(drain.since).saturating_sub(self.round_trip());
        Some(drain.bytes * 8 * 1000 / (queued.as_millis() as u64).max(50))
    }

    fn round_trip(&self) -> Duration {
        self.trips.front().map_or(Duration::ZERO, |(_, trip)| *trip)
    }

    /// Whether to hold video back for now: the oldest packet is late by more
    /// than [`MAX_LAG`] beyond a round trip, and enough is on its way to keep
    /// the link busy ([`Self::busy_bytes`]). A late report of a quiet stream
    /// alone says the other side dozed, and holding would only stall the
    /// picture. Nothing's known until the first report, which arrives within
    /// a round trip.
    fn hold(&mut self, now: Instant) -> bool {
        if self.draining.is_some() {
            return true;
        }
        let Some((oldest, _)) = self.in_flight.front() else { return false };
        if self.trips.is_empty()
            || now.saturating_duration_since(*oldest).saturating_sub(self.round_trip()) <= MAX_LAG
            || self.in_flight_bytes <= self.busy_bytes()
        {
            return false;
        }
        self.draining = Some(Drain { since: *oldest, bytes: self.in_flight_bytes });
        let mut stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        stats.holds += 1;
        stats.holding_since = Some(now);
        true
    }

    /// More than this on its way keeps the link busy: a quarter of a second
    /// of the stream's rate, and at least [`HOLD_MIN_BYTES`].
    fn busy_bytes(&self) -> u64 {
        (self.rate / 8 / 4).max(HOLD_MIN_BYTES)
    }

    /// When the wait for the oldest packet will have gone on too long, for a
    /// link with nothing coming back to wake the sender. None while too little
    /// is on its way to hold for: a write or a report changes that, and both
    /// wake the sender anyway.
    fn next_check(&self) -> Option<Instant> {
        let (oldest, _) = self.in_flight.front()?;
        (self.draining.is_none() && !self.trips.is_empty() && self.in_flight_bytes > self.busy_bytes())
            .then(|| *oldest + self.round_trip() + MAX_LAG)
    }
}

/// Opens a video stream to a PC that asked for the screen, or (`audio`) a
/// sound stream to one that asked for the sound too.
pub(crate) async fn open(shared: &Arc<Shared>, session: &Session, audio: bool) -> Result<MirrorStream> {
    let allowed = if is_local_desktop(shared) {
        pc_mirror_allowed(shared, &session.peer) || crate::media::watch_open(shared, &session.peer)
    } else {
        shared.toggle_on(&session.peer, TOGGLE)
    };
    if !allowed {
        return Err(Error::Denied);
    }
    let (mut send, recv) = session.conn.open_bi().await.map_err(crate::error::net)?;
    let op = if audio { mirror::OP_AUDIO } else { mirror::OP_VIDEO };
    let header = StreamHeader { svc: mirror::SERVICE.into(), op: op.into(), v: mirror::VERSION };
    write_frame(&mut send, &Envelope::new(types::STREAM, &header)?.to_cbor()).await?;
    let paced = !audio && peer_reports(shared, &session.peer);
    let on_end = if !audio {
        shared
            .record_timeline(crate::timeline::NewTimelineEntry {
                kind: crate::TimelineKind::Session,
                device_id: session.peer,
                device_name: shared.peer_name(&session.peer),
                incoming: false,
                timestamp: crate::now_unix(),
                title: "Screen mirroring".into(),
                detail: String::new(),
                target: "mirror".into(),
                size_bytes: 0,
                duration_secs: 0,
                ref_id: None,
            })
            .map(|id| (shared.clone(), id, std::time::Instant::now()))
    } else {
        None
    };
    Ok(MirrorStream::spawn_stream(send, recv, audio, paced, on_end))
}

/// Removes what a keyframe in `batch` replaces: sending it would only delay the
/// keyframe, and start another wait. A format change stays, as what follows
/// is in that format.
fn skip_to_keyframe(batch: &mut Vec<(PacketKind, u64, Vec<u8>)>) {
    let Some(key) = batch.iter().rposition(|p| p.0 == PacketKind::Keyframe) else { return };
    let mut index = 0;
    batch.retain(|p| {
        index += 1;
        index > key || p.0 == PacketKind::Config
    });
}

/// What still goes out of `waiting` after a long hold. From its last
/// keyframe on, if it has one. Otherwise nothing but format changes: the
/// frames are old by now, and sending them would only delay the keyframe
/// that has to follow. Returns whether frames were dropped, so one must.
fn drop_stale(waiting: &mut Vec<(PacketKind, u64, Vec<u8>)>) -> bool {
    if waiting.iter().any(|p| p.0 == PacketKind::Keyframe) {
        skip_to_keyframe(waiting);
        return false;
    }
    let before = waiting.len();
    waiting.retain(|p| p.0 == PacketKind::Config);
    waiting.len() < before
}

/// Whether this device reports video packets as they arrive. Anything else
/// mustn't be sent reports: it would take them for the end of the stream.
fn peer_reports(shared: &Shared, peer: &DeviceId) -> bool {
    shared.peer_offers_cap(peer, mirror::PACE)
}

/// Send priorities: sound before video, as a gap in it is the more
/// noticeable; both after control, the lanes and RPCs ([`crate::lane::priority_of`]).
pub(crate) use crate::lane::{AUDIO_PRIORITY, VIDEO_PRIORITY};

impl MirrorStream {
    pub(crate) fn spawn_stream(
        mut send: SendStream,
        mut recv: RecvStream,
        audio: bool,
        paced: bool,
        on_end: Option<(Arc<Shared>, i64, std::time::Instant)>,
    ) -> MirrorStream {
        // Sound and video go out as soon as they're written; sound first.
        let _ = send.set_priority(if audio { AUDIO_PRIORITY } else { VIDEO_PRIORITY });
        let depth = match (audio, paced) {
            (true, _) => AUDIO_QUEUE,
            (false, true) => PACED_QUEUE,
            (false, false) => QUEUE,
        };
        let (queue, mut packets) = mpsc::channel::<(PacketKind, u64, Vec<u8>)>(depth);
        let closed = Arc::new(AtomicBool::new(false));
        let done = closed.clone();
        let stop = Arc::new(tokio::sync::Notify::new());
        let stopped = stop.clone();
        let link_rate = Arc::new(AtomicU64::new(0));
        let measured = link_rate.clone();
        let link_bytes = Arc::new(AtomicU64::new(0));
        let measured_bytes = link_bytes.clone();
        let resyncing = Arc::new(Mutex::new(false));
        let resync = resyncing.clone();
        let rate = Arc::new(AtomicU64::new(0));
        let stream_rate = rate.clone();
        let holding = Arc::new(AtomicBool::new(false));
        let held_now = holding.clone();
        let pace = Arc::new(Mutex::new(PaceStats::default()));
        let pace_stats = pace.clone();
        tokio::spawn(async move {
            let mut pacer = paced.then(|| Pacer { stats: pace_stats, ..Pacer::default() });
            let mut reports = [0u8; 256];
            let mut held_since: Option<Instant> = None;
            'stream: loop {
                let now = Instant::now();
                if let Some(pacer) = &mut pacer {
                    pacer.rate = stream_rate.load(Ordering::Acquire);
                }
                let hold = pacer.as_mut().is_some_and(|p| p.hold(now));
                held_now.store(hold, Ordering::Release);
                let mut batch = Vec::new();
                // How long the hold that just ended lasted.
                let held = match held_since {
                    Some(since) if !hold => now.saturating_duration_since(since),
                    _ => Duration::ZERO,
                };
                held_since = if hold { held_since.or(Some(now)) } else { None };
                if held >= STALE_AFTER {
                    // Under the sender's lock, so no frame slips in between
                    // dropping what it follows and asking for a keyframe.
                    let mut needs_keyframe = resync.lock().unwrap_or_else(|e| e.into_inner());
                    while let Ok(packet) = packets.try_recv() {
                        batch.push(packet);
                    }
                    if drop_stale(&mut batch) {
                        *needs_keyframe = true;
                    }
                }
                if batch.is_empty() {
                    let check = pacer.as_ref().and_then(Pacer::next_check);
                    let next = tokio::select! {
                        _ = stopped.notified() => break,
                        // Each byte is a packet received. The other side stops
                        // reading (STOP_SENDING) when it's done watching.
                        read = recv.read(&mut reports) => match read {
                            Ok(Some(count)) => {
                                let rate = pacer.as_mut().and_then(|p| p.reported(Instant::now(), count));
                                if let (Some(rate), Some(pacer)) = (rate, &pacer) {
                                    measured_bytes.store(pacer.measured_bytes, Ordering::Release);
                                    measured.store(rate.max(1), Ordering::Release);
                                }
                                continue;
                            }
                            Ok(None) | Err(_) => break,
                        },
                        () = tokio::time::sleep_until(check.unwrap_or(now).into()), if check.is_some() => continue,
                        next = packets.recv(), if !hold => next,
                    };
                    let Some(first) = next else { break };
                    if done.load(Ordering::Acquire) {
                        break;
                    }
                    batch.push(first);
                    if pacer.is_some() {
                        while let Ok(packet) = packets.try_recv() {
                            batch.push(packet);
                        }
                        skip_to_keyframe(&mut batch);
                    }
                }
                for (kind, time, data) in batch {
                    let Ok(header) = video_packet_header(kind, time, data.len()) else { continue };
                    if let Some(pacer) = &mut pacer {
                        pacer.written(Instant::now(), header.len() + data.len());
                    }
                    let written = async {
                        send.write_all(&header).await?;
                        send.write_all(&data).await
                    };
                    // A write that can't finish means megabytes are already waiting: the
                    // viewer is gone or the link is hopeless, and a stop mustn't wait on it.
                    let write_res = tokio::select! {
                        _ = stopped.notified() => break 'stream,
                        res = tokio::time::timeout(Duration::from_secs(5), written) => res,
                    };
                    if !matches!(write_res, Ok(Ok(()))) {
                        break 'stream;
                    }
                }
            }
            done.store(true, Ordering::Release);
            held_now.store(false, Ordering::Release);
            let _ = send.finish();
            if let Some((shared, row_id, started_at)) = on_end {
                shared.finish_timeline_session(row_id, started_at.elapsed().as_secs().max(1));
            }
        });
        MirrorStream { queue, closed, stop, resyncing, audio, link_rate, link_bytes, rate, holding, pace }
    }
}

impl Shared {
    /// A new stop signal for the PC's reader of `peer`'s video or sound
    /// stream (`op`) of a mirroring.
    fn new_mirror_stop(&self, peer: &DeviceId, mirroring: u32, op: &'static str) -> Arc<tokio::sync::Notify> {
        let signal = Arc::new(tokio::sync::Notify::new());
        let replaced = self
            .mirror_stops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((*peer, mirroring, op), signal.clone());
        // A stream this one replaces stops: nothing else could reach it now.
        if let Some(replaced) = replaced {
            replaced.notify_one();
        }
        signal
    }

    fn clear_mirror_stop(
        &self,
        peer: &DeviceId,
        mirroring: u32,
        op: &'static str,
        signal: &Arc<tokio::sync::Notify>,
    ) -> bool {
        let mut stops = self.mirror_stops.lock().unwrap_or_else(|e| e.into_inner());
        if stops.get(&(*peer, mirroring, op)).is_some_and(|cur| Arc::ptr_eq(cur, signal)) {
            stops.remove(&(*peer, mirroring, op));
            true
        } else {
            false
        }
    }

    /// Stops showing (and playing) a mirroring of `peer` here (kept until
    /// the readers see it).
    pub(crate) fn stop_showing(&self, peer: &DeviceId, mirroring: u32) {
        let stops = self.mirror_stops.lock().unwrap_or_else(|e| e.into_inner());
        for op in [mirror::OP_VIDEO, mirror::OP_AUDIO] {
            if let Some(signal) = stops.get(&(*peer, mirroring, op)) {
                signal.notify_one();
            }
        }
    }

    /// Stops showing (and playing) all mirror sessions of `peer` here and
    /// returns the session IDs that were active.
    pub(crate) fn stop_showing_all(&self, peer: &DeviceId) -> Vec<u32> {
        let stops = self.mirror_stops.lock().unwrap_or_else(|e| e.into_inner());
        let mut sessions = Vec::new();
        for ((p, mirroring, _), signal) in stops.iter() {
            if p == peer {
                signal.notify_one();
                if !sessions.contains(mirroring) {
                    sessions.push(*mirroring);
                }
            }
        }
        sessions
    }

    pub(crate) fn record_outgoing_mirror_session(&self, peer: DeviceId, session: u32) {
        self.outgoing_mirror_sessions.lock().unwrap_or_else(|e| e.into_inner()).insert((peer, session));
    }

    pub(crate) fn remove_outgoing_mirror_session(&self, peer: &DeviceId, session: u32) {
        self.outgoing_mirror_sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(&(*peer, session));
    }

    pub(crate) fn take_outgoing_mirror_sessions(&self, peer: &DeviceId) -> Vec<u32> {
        let mut set = self.outgoing_mirror_sessions.lock().unwrap_or_else(|e| e.into_inner());
        let sessions: Vec<u32> = set.iter().filter(|(p, _)| p == peer).map(|(_, s)| *s).collect();
        for s in &sessions {
            set.remove(&(*peer, *s));
        }
        sessions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn video_is_held_only_once_it_is_too_far_ahead() {
        let t = Instant::now();
        let mut pacer = Pacer::default();
        pacer.written(t, 100);
        assert!(!pacer.hold(t + ms(900)), "nothing's known before the first report");
        assert_eq!(pacer.reported(t + ms(100), 1), None);
        assert_eq!(pacer.round_trip(), ms(100));

        // A keyframe's worth: enough to keep the link busy.
        pacer.written(t + ms(1000), 100_000);
        assert!(!pacer.hold(t + ms(1000 + 100 + 250)), "a round trip plus what's allowed");
        assert_eq!(pacer.next_check(), Some(t + ms(1000 + 100 + 250)));
        assert!(pacer.hold(t + ms(1000 + 100 + 251)));
        assert_eq!(pacer.next_check(), None, "a report ends this wait");
    }

    #[test]
    fn one_late_report_on_a_quiet_stream_does_not_hold() {
        let t = Instant::now();
        let mut pacer = Pacer::default();
        pacer.written(t, 100);
        pacer.reported(t + ms(20), 1);
        // A small frame whose report is a second late: the phone's radio dozed.
        pacer.written(t + ms(1000), 3_000);
        assert!(!pacer.hold(t + ms(2000)), "late, but nothing that fills the link");
        assert_eq!(pacer.next_check(), None, "and nothing to wake up for: a write or a report will");
        // More of it, past a quarter of a second of 1 Mbit/s and 32 KiB: busy and late.
        pacer.rate = 1_000_000;
        pacer.written(t + ms(2000), 40_000);
        assert!(pacer.hold(t + ms(2001)), "a busy late stream holds");
    }

    #[test]
    fn a_busy_stream_needs_a_quarter_second_of_its_rate_on_its_way() {
        let t = Instant::now();
        let mut pacer = Pacer { rate: 8_000_000, ..Pacer::default() };
        pacer.written(t, 100);
        pacer.reported(t + ms(20), 1);
        // 8 Mbit/s: a quarter of a second is 250 kB.
        pacer.written(t + ms(100), 200_000);
        assert!(!pacer.hold(t + ms(1000)), "under a quarter of a second of the stream");
        pacer.written(t + ms(1000), 60_000);
        assert!(pacer.hold(t + ms(1001)));
    }

    #[test]
    fn holds_and_report_trips_are_counted_for_the_stats() {
        let t = Instant::now();
        let mut pacer = Pacer::default();
        for i in 0..100u64 {
            pacer.written(t + ms(i * 10), 100);
            pacer.reported(t + ms(i * 10 + 5 + i), 1);
        }
        pacer.written(t + ms(2000), 100_000);
        assert!(pacer.hold(t + ms(2400)));
        pacer.reported(t + ms(2600), 1);
        let stats = pacer.stats.lock().unwrap().take(t + ms(3000));
        assert_eq!(stats.holds, 1);
        assert_eq!(stats.held, ms(200), "from when the hold started to its last report");
        assert_eq!(stats.shortest_trip, Some(ms(5)));
        assert_eq!(stats.p95_trip, Some(ms(100)), "of 101 trips: 5..=104 ms, and the held one's 600");
        let again = pacer.stats.lock().unwrap().take(t + ms(3000));
        assert_eq!(again, MirrorStats::default(), "taken: they start again");

        // A hold under way counts up to the moment the stats are taken.
        pacer.written(t + ms(4000), 100_000);
        assert!(pacer.hold(t + ms(4500)));
        let during = pacer.stats.lock().unwrap().take(t + ms(4700));
        assert_eq!((during.holds, during.held), (1, ms(200)));
        pacer.reported(t + ms(4800), 1);
        assert_eq!(pacer.stats.lock().unwrap().take(t + ms(5000)).held, ms(100), "the rest of it");
    }

    #[test]
    fn the_clock_estimate_takes_the_shortest_round_trip() {
        let mut clock = ClockEstimate::default();
        // The PC's clock is 1 s ahead; this exchange took 10 ms, evenly.
        assert_eq!(clock.add(5_000_000, 6_005_000, 5_010_000), Some((1_000_000, 10_000)));
        // A slower one, skewed by a queue on the way back: the faster stands.
        assert_eq!(clock.add(6_000_000, 7_001_000, 6_050_000), Some((1_000_000, 10_000)));
        // A faster one replaces it.
        assert_eq!(clock.add(7_000_000, 8_000_500, 7_002_000), Some((999_500, 2_000)));
        // An answer from before the question, or one that took too long, is no measure.
        assert_eq!(clock.add(9_000_000, 1, 8_000_000), Some((999_500, 2_000)));
        assert_eq!(clock.add(9_000_000, 1, 9_000_000 + MAX_CLOCK_TRIP_US + 1), Some((999_500, 2_000)));
        // A PC whose clock is behind the phone's.
        let mut behind = ClockEstimate::default();
        assert_eq!(behind.add(9_000_000_000, 4_000, 9_000_000_400), Some((-8_999_996_200, 400)));
        // Only the last few count: the old fast one ages out.
        for i in 0..CLOCK_SAMPLES as u64 {
            clock.add(10_000_000 + i, 11_000_000 + i + 2_500, 10_000_000 + i + 5_000);
        }
        assert_eq!(clock.add(20_000_000, 21_002_500, 20_005_000), Some((1_000_000, 5_000)));
        assert_eq!(clock.samples.len(), CLOCK_SAMPLES);
    }

    #[test]
    fn the_monotonic_clock_goes_forward_in_microseconds() {
        let first = monotonic_us();
        std::thread::sleep(ms(2));
        let second = monotonic_us();
        assert!(first > 0 && second >= first + 2_000, "{first} {second}");
    }

    #[test]
    fn a_wait_lasts_until_everything_arrived_and_measures_the_link() {
        let t = Instant::now();
        let mut pacer = Pacer::default();
        pacer.written(t, 100);
        pacer.reported(t + ms(100), 1);

        // Two 50 kB packets, on a link that takes 1.3 s over them and 100 ms back.
        pacer.written(t + ms(1000), 50_000);
        pacer.written(t + ms(1001), 50_000);
        assert!(pacer.hold(t + ms(1400)));
        assert_eq!(pacer.reported(t + ms(1600), 1), None);
        assert!(pacer.hold(t + ms(1700)), "half of it hasn't arrived");
        let rate = pacer.reported(t + ms(2400), 1).expect("measured");
        assert_eq!(rate, 100_000 * 8 * 1000 / 1300);
        assert_eq!(pacer.measured_bytes, 100_000, "and over how much");
        assert!(!pacer.hold(t + ms(2400)));
        assert_eq!(pacer.round_trip(), ms(100), "still the shortest seen");
    }

    #[test]
    fn a_slower_path_replaces_the_round_trip_it_remembers() {
        let t = Instant::now();
        let mut pacer = Pacer::default();
        pacer.written(t, 100);
        pacer.reported(t + ms(5), 1);
        assert_eq!(pacer.round_trip(), ms(5));
        // Wi-Fi went away: everything takes 300 ms now, through a relay.
        let later = t + TRIP_MEMORY + ms(100);
        pacer.written(later, 100);
        pacer.reported(later + ms(300), 1);
        assert_eq!(pacer.round_trip(), ms(300));
    }

    #[test]
    fn a_queued_keyframe_replaces_what_is_ahead_of_it() {
        let kinds = |kinds: &[PacketKind]| -> Vec<PacketKind> {
            let mut batch: Vec<_> = kinds.iter().map(|k| (*k, 0, Vec::new())).collect();
            skip_to_keyframe(&mut batch);
            batch.into_iter().map(|p| p.0).collect()
        };
        use PacketKind::{Config, Frame, Keyframe};
        assert_eq!(kinds(&[Frame, Frame]), [Frame, Frame], "nothing replaces these");
        assert_eq!(kinds(&[Frame, Keyframe, Frame]), [Keyframe, Frame]);
        assert_eq!(kinds(&[Keyframe, Frame, Keyframe]), [Keyframe], "the last one does");
        assert_eq!(kinds(&[Frame, Config, Frame, Keyframe]), [Config, Keyframe], "format changes stay");
    }

    #[test]
    fn what_waited_through_a_long_hold_is_dropped_for_a_keyframe() {
        let left = |kinds: &[PacketKind]| -> (Vec<PacketKind>, bool) {
            let mut waiting: Vec<_> = kinds.iter().map(|k| (*k, 0, Vec::new())).collect();
            let dropped = drop_stale(&mut waiting);
            (waiting.into_iter().map(|p| p.0).collect(), dropped)
        };
        use PacketKind::{Config, Frame, Keyframe};
        assert_eq!(left(&[Frame, Frame, Frame]), (vec![], true), "old frames go, a keyframe must follow");
        assert_eq!(left(&[Frame, Config, Frame]), (vec![Config], true), "format changes stay");
        assert_eq!(left(&[Frame, Keyframe, Frame]), (vec![Keyframe, Frame], false), "a fresh start");
        assert_eq!(left(&[]), (vec![], false));
        assert_eq!(left(&[Config]), (vec![Config], false), "nothing dropped, nothing to ask for");
    }

    #[test]
    fn reports_of_more_than_was_sent_are_ignored() {
        let t = Instant::now();
        let mut pacer = Pacer::default();
        pacer.written(t, 100);
        assert_eq!(pacer.reported(t + ms(10), 50), None);
        assert_eq!((pacer.in_flight.len(), pacer.in_flight_bytes), (0, 0));
        assert!(!pacer.hold(t + ms(5000)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_full_queue_drops_frames_until_a_keyframe() {
        let (queue, mut packets) = mpsc::channel(2);
        let stream = MirrorStream {
            queue,
            closed: Arc::default(),
            stop: Arc::default(),
            resyncing: Arc::default(),
            audio: false,
            link_rate: Arc::default(),
            link_bytes: Arc::default(),
            rate: Arc::default(),
            holding: Arc::default(),
            pace: Arc::default(),
        };
        let send = |kind| tokio::task::block_in_place(|| stream.send(kind, 0, vec![1]));
        assert_eq!(send(PacketKind::Keyframe), MirrorSend::Queued);
        assert_eq!(send(PacketKind::Frame), MirrorSend::Queued);
        assert_eq!(send(PacketKind::Frame), MirrorSend::NeedKeyframe, "full");
        assert!(stream.is_backed_up(), "nothing made now would go out");
        packets.recv().await;
        assert!(!stream.is_backed_up());
        // Room again, but the decoder needs a fresh start first.
        assert_eq!(send(PacketKind::Frame), MirrorSend::NeedKeyframe);
        assert_eq!(send(PacketKind::Keyframe), MirrorSend::Queued);
        packets.recv().await;
        assert_eq!(send(PacketKind::Frame), MirrorSend::Queued);
        drop(packets);
        assert_eq!(send(PacketKind::Keyframe), MirrorSend::Closed);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sound_drops_only_what_doesnt_fit() {
        let (queue, mut packets) = mpsc::channel(1);
        let stream = MirrorStream {
            queue,
            closed: Arc::default(),
            stop: Arc::default(),
            resyncing: Arc::default(),
            audio: true,
            link_rate: Arc::default(),
            link_bytes: Arc::default(),
            rate: Arc::default(),
            holding: Arc::default(),
            pace: Arc::default(),
        };
        let send = |kind| tokio::task::block_in_place(|| stream.send(kind, 0, vec![1]));
        assert_eq!(send(PacketKind::Frame), MirrorSend::Queued);
        assert_eq!(send(PacketKind::Frame), MirrorSend::Dropped, "full");
        packets.recv().await;
        // No keyframe to wait for: the next one goes.
        assert_eq!(send(PacketKind::Frame), MirrorSend::Queued);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn config_packet_wait_does_not_block_resyncing_lock() {
        let (queue, mut packets) = mpsc::channel(1);
        let stream = Arc::new(MirrorStream {
            queue,
            closed: Arc::default(),
            stop: Arc::default(),
            resyncing: Arc::default(),
            audio: false,
            link_rate: Arc::default(),
            link_bytes: Arc::default(),
            rate: Arc::default(),
            holding: Arc::default(),
            pace: Arc::default(),
        });
        // Fill the 1-slot queue.
        let s1 = stream.clone();
        assert_eq!(
            tokio::task::spawn_blocking(move || s1.send(PacketKind::Keyframe, 0, vec![1])).await.unwrap(),
            MirrorSend::Queued
        );
        // Start a blocking Config send on a background thread while the queue is full.
        let s2 = stream.clone();
        let config_send = tokio::task::spawn_blocking(move || s2.send(PacketKind::Config, 0, vec![2]));
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        // Another caller must still be able to lock `resyncing` and return immediately.
        let s3 = stream.clone();
        let frame_res = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            tokio::task::spawn_blocking(move || s3.send(PacketKind::Frame, 0, vec![3])),
        )
        .await
        .expect("Frame send must not block on resyncing lock")
        .unwrap();
        assert_eq!(frame_res, MirrorSend::NeedKeyframe);
        // Drain the queue so the Config send finishes.
        packets.recv().await;
        assert_eq!(config_send.await.unwrap(), MirrorSend::Queued);
    }
}
