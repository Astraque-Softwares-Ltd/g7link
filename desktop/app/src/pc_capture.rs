// SPDX-License-Identifier: GPL-3.0-or-later
//! Screenshots and recordings of this PC's screen for a paired phone
//! (docs/protocol/pc-capture.md). The phone asks through the core, which
//! calls in on its blocking threads; the screen is read on threads of this
//! module's own, never the GUI thread. Every capture shows a notification
//! here, and a recording's has Stop.

use crate::i18n::tr;
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    time::{Duration, Instant},
};

use g7link_core::{CapturePrompt, DeviceId, NodeEvent, PcCaptureError};

use crate::{
    core_host,
    pc_mirror::{FeedFrame, ScreenFeed},
    sys::{
        self,
        mp4::{MirrorRecorder, local_timestamp_filename},
        toast::{self, Toast},
    },
};

/// The group of this module's notifications.
pub const TOAST_GROUP: &str = "pc-capture";
/// The Stop button on a recording's notification.
pub const ACTION_STOP: &str = "stop";
const SHOT_KEY: &str = "shot";
const RECORDING_KEY: &str = "recording";

/// How long a capture waits for someone at the PC to answer the
/// screen-sharing dialog: less than the phone waits (150 s).
const DIALOG_WAIT: Duration = Duration::from_secs(140);
/// How long stopping waits for the video to be saved: less than the phone
/// waits (120 s).
const SAVE_WAIT: Duration = Duration::from_secs(110);
/// How long quitting waits for a recording to be saved.
const QUIT_SAVE_WAIT: Duration = Duration::from_secs(10);
const FPS: u32 = 30;
const BITRATE: u32 = 8_000_000;
/// The longest recording (docs/protocol/pc-capture.md).
const MAX_RECORDING: Duration = Duration::from_secs(g7link_core::pc_capture::MAX_RECORD_SECS);
/// How often a recording checks that it may go on: the lock, the toggle.
const CHECK_EVERY: Duration = Duration::from_secs(1);
/// 10 ms of 48 kHz 16-bit stereo, the piece the PC's sound is read in.
const SOUND_CHUNK: usize = 480 * 4;
/// The most sound kept waiting for the first picture or the encoder (10 s).
const MAX_PENDING_SOUND: usize = 48_000 * 4 * 10;
/// Screenshots waiting to go to a phone are kept this long.
const KEEP_SHOTS: Duration = Duration::from_secs(24 * 60 * 60);

/// "G7 Link screenshot 2026-11-12 14.03.22.png", in the PC's local time.
fn shot_name() -> String {
    local_timestamp_filename("G7 Link screenshot", "png")
}

/// "G7 Link screen recording 2026-11-12 14.03.22.mp4", in the PC's local time.
fn recording_name() -> String {
    local_timestamp_filename("G7 Link screen recording", "mp4")
}

fn phone_name(peer: &DeviceId) -> String {
    core_host::host().hub.read(|s| s.name_of(peer)).unwrap_or_else(|| tr!("PcCapture", "Your phone"))
}

fn show(key: &str, title: String, body: String, actions: Vec<(String, String)>) {
    toast::show(Toast {
        device: TOAST_GROUP.into(),
        key: key.into(),
        title,
        body,
        attribution: "G7 Link".into(),
        icon: None,
        image: None,
        actions,
        reply: None,
        silent: false,
        progress: None,
        call: false,
    });
}

// ---- Screenshots ----

static SHOOTING: AtomicBool = AtomicBool::new(false);

/// Clears [`SHOOTING`] on every way out.
struct Shooting;

impl Drop for Shooting {
    fn drop(&mut self) {
        SHOOTING.store(false, Ordering::SeqCst);
    }
}

/// Takes a screenshot for `peer`: put on this PC's clipboard, and saved for
/// the core to send. Waits while the portal asks the user.
pub fn shot(peer: DeviceId, prompt: CapturePrompt) -> Result<PathBuf, PcCaptureError> {
    if sys::session_locked() {
        return Err(PcCaptureError::Locked);
    }
    if SHOOTING.swap(true, Ordering::SeqCst) {
        return Err(PcCaptureError::Busy);
    }
    let _shooting = Shooting;
    let (taken, result) = sync_channel(1);
    let take = move || {
        let _ = taken.send(take_shot(prompt));
    };
    std::thread::Builder::new()
        .name("g7link-pc-shot".into())
        .spawn(take)
        .map_err(|e| PcCaptureError::Failed(e.to_string()))?;
    let path = match result.recv_timeout(DIALOG_WAIT) {
        Ok(taken) => taken?,
        Err(_) => return Err(PcCaptureError::Failed("no answer from the screen-sharing dialog".into())),
    };
    let name = phone_name(&peer);
    let title = tr!("PcCapture", "%1 took a screenshot", name);
    show(SHOT_KEY, title, tr!("PcCapture", "It's on this PC's clipboard too."), Vec::new());
    Ok(path)
}

fn take_shot(prompt: CapturePrompt) -> Result<PathBuf, PcCaptureError> {
    let picture = crate::pc_mirror::grab_still(prompt)?;
    let png = sys::image::encode_png(&picture).map_err(|e| PcCaptureError::Failed(e.to_string()))?;
    if let Err(error) = sys::clipboard::write_image("image/png", &png) {
        tracing::warn!(%error, "can't put the screenshot on the clipboard");
    }
    save_shot(&png).map_err(|e| PcCaptureError::Failed(format!("can't save the screenshot: {e}")))
}

/// Where screenshots wait to go to a phone.
fn shots_dir() -> PathBuf {
    core_host::host().data_dir.join("pc-shots")
}

fn save_shot(png: &[u8]) -> std::io::Result<PathBuf> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};

    let dir = shots_dir();
    std::fs::create_dir_all(&dir)?;
    forget_old_shots(&dir);
    let path = crate::photos::free_path(&dir, &shot_name());
    // A new file only this user may read: it shows the screen.
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    if let Err(e) = file.write_all(png).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    Ok(path)
}

fn forget_old_shots(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let age = meta.modified().ok().and_then(|at| at.elapsed().ok());
        if meta.is_file() && age.is_some_and(|age| age > KEEP_SHOTS) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

// ---- Recordings ----

/// The recording running for a phone. One at a time.
struct Recording {
    peer: DeviceId,
    stop: Arc<AtomicBool>,
    /// Set (under [`RECORDING`]'s lock) when the phone stopped it: the phone
    /// then waits for the video itself.
    by_phone: Arc<AtomicBool>,
    saved: Receiver<Result<PathBuf, String>>,
}

static RECORDING: Mutex<Option<Recording>> = Mutex::new(None);
/// Recording threads still running, saving included.
static WORKERS: AtomicUsize = AtomicUsize::new(0);

/// Whether this PC's screen is being recorded for a phone.
pub fn is_recording() -> bool {
    RECORDING.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Ends the screencast when nothing else uses it.
fn release_screen() {
    if !is_recording() {
        crate::pc_mirror::release_screencast();
    }
}

/// Takes the recording out of [`RECORDING`] when it is the one `stop` belongs to.
fn forget(stop: &Arc<AtomicBool>) {
    let mut slot = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
    if slot.as_ref().is_some_and(|r| Arc::ptr_eq(&r.stop, stop)) {
        *slot = None;
    }
}

/// Starts recording the screen and its sound for `peer`; returns once it
/// records. Waits while the portal asks the user.
pub fn record_start(peer: DeviceId, prompt: CapturePrompt) -> Result<(), PcCaptureError> {
    if sys::session_locked() {
        return Err(PcCaptureError::Locked);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let by_phone = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready) = sync_channel(1);
    let (saved_tx, saved) = sync_channel(1);
    {
        let mut slot = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return Err(PcCaptureError::Busy);
        }
        *slot = Some(Recording { peer, stop: Arc::clone(&stop), by_phone: Arc::clone(&by_phone), saved });
    }
    let worker = Worker { peer, stop: Arc::clone(&stop), by_phone, ready: ready_tx, saved: saved_tx };
    WORKERS.fetch_add(1, Ordering::SeqCst);
    let record = move || worker.run(prompt);
    let spawned = std::thread::Builder::new().name("g7link-pc-record".into()).spawn(record);
    if let Err(e) = spawned {
        WORKERS.fetch_sub(1, Ordering::SeqCst);
        forget(&stop);
        return Err(PcCaptureError::Failed(e.to_string()));
    }
    match ready.recv_timeout(DIALOG_WAIT) {
        Ok(Ok(())) => {
            let name = phone_name(&peer);
            let title = tr!("PcCapture", "%1 is recording this screen", name);
            let body = tr!("PcCapture", "With its sound. Stop it here or on the phone.");
            show(RECORDING_KEY, title, body, vec![(ACTION_STOP.into(), tr!("PcCapture", "Stop"))]);
            Ok(())
        }
        Ok(Err(e)) => {
            forget(&stop);
            Err(e)
        }
        Err(_) => {
            // The worker sees this once the portal answers, and lets go.
            stop.store(true, Ordering::SeqCst);
            forget(&stop);
            Err(PcCaptureError::Failed("no answer from the screen-sharing dialog".into()))
        }
    }
}

/// Stops the recording `peer` started and waits for the saved video.
pub fn record_stop(peer: DeviceId) -> Result<PathBuf, PcCaptureError> {
    let recording = {
        let mut slot = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
        match slot.take() {
            Some(r) if r.peer == peer => {
                r.by_phone.store(true, Ordering::SeqCst);
                r
            }
            other => {
                *slot = other;
                return Err(PcCaptureError::NotRecording);
            }
        }
    };
    recording.stop.store(true, Ordering::SeqCst);
    match recording.saved.recv_timeout(SAVE_WAIT) {
        Ok(Ok(path)) => Ok(path),
        Ok(Err(error)) => Err(PcCaptureError::Failed(error)),
        Err(_) => Err(PcCaptureError::Failed("saving the recording took too long".into())),
    }
}

/// Stops the recording from this PC (its notification's Stop). The phone
/// gets the video and hears that it ended.
pub fn stop_from_pc() {
    if let Some(r) = RECORDING.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        r.stop.store(true, Ordering::SeqCst);
    }
}

fn stop_for(peer: &DeviceId) {
    if let Some(r) = RECORDING.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        && r.peer == *peer
    {
        r.stop.store(true, Ordering::SeqCst);
    }
}

/// A phone that goes away ends its recording; the video waits for it.
pub fn on_event(event: &NodeEvent) {
    match event {
        NodeEvent::LinkChanged { device, link: g7link_core::LinkState::Offline { .. } }
        | NodeEvent::DeviceRemoved(device) => stop_for(device),
        _ => {}
    }
}

/// Stops a recording when the app quits, and gives it a moment to be saved.
pub fn shutdown() {
    stop_from_pc();
    let deadline = Instant::now() + QUIT_SAVE_WAIT;
    while WORKERS.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Where recordings are saved: the user's Videos folder.
fn videos_dir() -> PathBuf {
    dirs::video_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Videos")))
        .unwrap_or_else(core_host::downloads_dir)
}

/// Whether `peer` may still see this PC's screen.
fn allowed(peer: &DeviceId) -> bool {
    core_host::node()
        .and_then(|n| n.device_toggles(*peer).ok())
        .is_some_and(|toggles| toggles.iter().any(|(k, v)| *k == g7link_core::PC_MIRROR_TOGGLE && *v))
}

/// The thread a recording runs on.
struct Worker {
    peer: DeviceId,
    stop: Arc<AtomicBool>,
    by_phone: Arc<AtomicBool>,
    ready: SyncSender<Result<(), PcCaptureError>>,
    saved: SyncSender<Result<PathBuf, String>>,
}

impl Worker {
    fn run(self, prompt: CapturePrompt) {
        if let Some(recorder) = self.record(prompt) {
            self.save(recorder);
        }
        WORKERS.fetch_sub(1, Ordering::SeqCst);
    }

    /// Leaves [`RECORDING`]; says whether the phone stopped it.
    fn leave(&self) -> bool {
        let mut slot = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref().is_some_and(|r| Arc::ptr_eq(&r.stop, &self.stop)) {
            *slot = None;
        }
        self.by_phone.load(Ordering::SeqCst)
    }

    fn fail(&self, error: PcCaptureError) -> Option<MirrorRecorder> {
        let _ = self.ready.send(Err(error));
        crate::pc_mirror::release_screencast();
        None
    }

    /// Records until stopped; `None` when it never started.
    fn record(&self, prompt: CapturePrompt) -> Option<MirrorRecorder> {
        let (feed, src_w, src_h) = match ScreenFeed::open(prompt) {
            Ok(opened) => opened,
            Err(e) => return self.fail(e),
        };
        let (width, height) =
            crate::pc_mirror::target_dimensions(src_w, src_h, crate::pc_mirror::RECORD_MAX_EDGE);
        let encoder = crate::pc_mirror::ScreenEncoder::new_for_codec("h264", width, height, FPS, BITRATE);
        let mut encoder = match encoder {
            Ok(encoder) => encoder,
            Err(e) => return self.fail(PcCaptureError::Failed(e)),
        };
        let dir = videos_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return self.fail(PcCaptureError::Failed(format!("can't make the Videos folder: {e}")));
        }
        let sound = Sound::start();
        let audio = sound.as_ref().map(|_| (48_000, 2));
        let path = crate::photos::free_path(&dir, &recording_name());
        let mut recorder = MirrorRecorder::new(path, width, height, FPS, audio);
        if self.ready.send(Ok(())).is_err() || self.stop.load(Ordering::SeqCst) {
            // Whoever asked stopped waiting.
            drop(sound);
            release_screen();
            return None;
        }
        tracing::info!(peer = %self.peer.short(), width, height, "recording this PC's screen");

        let started = Instant::now();
        let interval = Duration::from_micros(1_000_000 / u64::from(FPS));
        let dur_hns = i64::from(10_000_000 / FPS);
        let mut bgra = Vec::new();
        let mut nv12 = Vec::new();
        let mut checked = Instant::now();
        let mut pictures = false;
        'recording: while !self.stop.load(Ordering::SeqCst) {
            let tick = Instant::now();
            if started.elapsed() >= MAX_RECORDING {
                tracing::info!("the recording reached its length limit");
                break;
            }
            if checked.elapsed() >= CHECK_EVERY {
                checked = Instant::now();
                if sys::session_locked() || !allowed(&self.peer) {
                    tracing::info!("stopping the recording: locked, or no longer allowed");
                    break;
                }
            }
            match feed.frame(width, height, &mut bgra) {
                FeedFrame::Got => {
                    crate::pc_mirror::bgra_to_nv12(&bgra, width as usize, height as usize, &mut nv12);
                    let time_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
                    let hns = i64::try_from(time_us.saturating_mul(10)).unwrap_or(i64::MAX);
                    let packets = match encoder.encode(&nv12, hns, dur_hns, &self.stop) {
                        Ok(packets) => packets,
                        Err(error) => {
                            tracing::warn!(%error, "can't encode the recording");
                            break;
                        }
                    };
                    for (keyframe, data) in packets {
                        if keyframe && !pictures {
                            pictures = true;
                            // The sound starts with the first picture.
                            if let Some(sound) = &sound {
                                sound.take();
                            }
                        }
                        if let Err(error) = recorder.on_packet(keyframe, time_us, &data) {
                            tracing::warn!(%error, "can't write the recording");
                            break 'recording;
                        }
                    }
                }
                FeedFrame::NotYet => {}
                FeedFrame::Gone => {
                    tracing::info!("the screencast stopped, and so does the recording");
                    break;
                }
            }
            if pictures && let Some(sound) = &sound {
                let pcm = sound.take();
                if !pcm.is_empty() {
                    let _ = recorder.on_audio(0, &pcm);
                }
            }
            if let Some(rest) = interval.checked_sub(tick.elapsed()) {
                std::thread::sleep(rest);
            }
        }
        drop(sound);
        Some(recorder)
    }

    /// Saves the video, and gets it to the phone.
    fn save(&self, recorder: MirrorRecorder) {
        let by_phone = self.leave();
        toast::remove(TOAST_GROUP, RECORDING_KEY);
        release_screen();
        let saved = recorder.finish();
        match &saved {
            Ok(path) => {
                tracing::info!(peer = %self.peer.short(), "saved the screen recording");
                show_saved(path);
            }
            Err(error) => tracing::warn!(%error, "can't save the screen recording"),
        }
        // A phone that stopped waiting (or never stopped it) hears that it ended.
        let unsent = if by_phone { self.saved.send(saved).err().map(|e| e.0) } else { Some(saved) };
        if let Some(saved) = unsent
            && let Some(node) = core_host::node()
        {
            let peer = self.peer;
            core_host::spawn(async move { node.pc_capture_record_ended(peer, saved.ok()).await });
        }
    }
}

fn show_saved(path: &Path) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    toast::show(Toast {
        device: crate::transfers::TOAST_GROUP.into(),
        key: path.to_string_lossy().into_owned(),
        title: name,
        body: tr!("PcCapture", "Screen recording saved in Videos"),
        attribution: "G7 Link".into(),
        icon: None,
        image: None,
        actions: vec![
            (crate::transfers::ACTION_OPEN.into(), tr!("PcCapture", "Open")),
            (crate::transfers::ACTION_SHOW.into(), tr!("PcCapture", "Show in folder")),
        ],
        reply: None,
        silent: true,
        progress: None,
        call: false,
    });
}

/// The PC's sound for a recording: what its speakers play, as 48 kHz 16-bit
/// stereo from `parec` or `pw-record`, kept until taken.
struct Sound {
    child: Child,
    pcm: Arc<Mutex<Vec<u8>>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Sound {
    fn start() -> Option<Sound> {
        let monitor = crate::pc_audio_stream::resolve_default_monitor_source();
        let parec = Command::new("parec")
            .args(["--raw", "--format=s16le", "--rate=48000", "--channels=2", "--latency-msec=20"])
            .arg(format!("--device={monitor}"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match parec {
            Ok(child) => child,
            Err(_) => {
                let args = crate::pc_audio_stream::pw_record_loopback_args(&monitor)
                    .into_iter()
                    .map(|a| if a == "--format=f32" { "--format=s16".to_owned() } else { a });
                Command::new("pw-record")
                    .args(args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .inspect_err(|error| tracing::info!(%error, "recording without the PC's sound"))
                    .ok()?
            }
        };
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        };
        let pcm = Arc::new(Mutex::new(Vec::new()));
        let slot = Arc::clone(&pcm);
        let read = move || {
            let mut chunk = vec![0u8; SOUND_CHUNK];
            while stdout.read_exact(&mut chunk).is_ok() {
                let mut pcm = slot.lock().unwrap_or_else(|e| e.into_inner());
                // Whole pieces only, so the samples stay in step.
                if pcm.len() + chunk.len() <= MAX_PENDING_SOUND {
                    pcm.extend_from_slice(&chunk);
                }
            }
        };
        match std::thread::Builder::new().name("g7link-pc-record-sound".into()).spawn(read) {
            Ok(reader) => Some(Sound { child, pcm, reader: Some(reader) }),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                None
            }
        }
    }

    /// The sound since the last call.
    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.pcm.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl Drop for Sound {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_are_named_by_what_they_are_and_when() {
        for (name, prefix, ext) in [
            (shot_name(), "G7 Link screenshot 2", ".png"),
            (recording_name(), "G7 Link screen recording 2", ".mp4"),
        ] {
            assert!(name.starts_with(prefix) && name.ends_with(ext), "{name}");
            assert!(!name.contains(['/', '\\', ':']), "{name} is a plain file name everywhere");
        }
    }

    #[test]
    fn stopping_what_isnt_recording_says_so() {
        assert_eq!(record_stop(DeviceId([9; 32])), Err(PcCaptureError::NotRecording));
        assert!(!is_recording());
    }
}
