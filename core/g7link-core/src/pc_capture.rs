// SPDX-License-Identifier: MPL-2.0
//! Screenshots and recordings of a PC's screen, asked for by a paired phone
//! (docs/protocol/pc-capture.md). The PC's app takes them through
//! [`Platform`](crate::Platform); the files go to the phone as ordinary
//! transfers, and the answers say which.

use std::{path::PathBuf, sync::Arc, time::Duration};

use g7link_protocol::{
    DeviceId, Envelope, ErrorCode, ProtocolError,
    messages::{is_valid_file_name, types},
};

pub use g7link_protocol::messages::{
    PcCaptureRecordEnded, PcCaptureResult,
    pc_capture::{CAP, MAX_RECORD_SECS},
};

use crate::{
    Error, NodeEvent, Result,
    node::Shared,
    session::Session,
    transfer::{FileSource, OutgoingFile},
};

/// The device toggle that lets a phone see this PC's screen, and so take
/// screenshots and recordings of it.
pub const TOGGLE: &str = crate::mirror::PC_MIRROR_TOGGLE;

/// How long a phone waits for a screenshot or for a recording to start: the
/// person at the PC may have to answer its screen-sharing dialog first.
const SHOT_WAIT: Duration = Duration::from_secs(150);
const RECORD_START_WAIT: Duration = Duration::from_secs(150);
/// How long a phone waits for a stopped recording to be saved.
const RECORD_STOP_WAIT: Duration = Duration::from_secs(120);

/// Tells the phone that the PC shows its screen-sharing dialog (`pc.capture.prompt`).
/// Cheap, and safe to call from any thread, more than once.
pub type CapturePrompt = Arc<dyn Fn() + Send + Sync>;

/// Why a PC didn't take a screenshot, start or stop a recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcCaptureError {
    /// The PC's `pc_mirror` toggle for the phone is off.
    Off,
    /// The PC's session is locked.
    Locked,
    /// The person at the PC dismissed the screen-sharing dialog.
    Declined,
    /// The PC's `files` toggle for the phone is off: the file can't go to it.
    FilesOff,
    /// A recording is already running, or another screenshot is being taken.
    Busy,
    /// The PC can't capture its screen (no portal, no PipeWire), or doesn't offer it.
    Unsupported,
    /// There is no recording to stop.
    NotRecording,
    /// The PC isn't paired or connected.
    Offline,
    /// Anything else. For logs.
    Failed(String),
}

impl PcCaptureError {
    fn envelope(&self) -> Envelope {
        use g7link_protocol::messages::pc_capture::{DECLINED, FILES_OFF, LOCKED, OFF};
        match self {
            PcCaptureError::Off => Envelope::error(ErrorCode::Denied, OFF),
            PcCaptureError::Locked => Envelope::error(ErrorCode::Denied, LOCKED),
            PcCaptureError::Declined => Envelope::error(ErrorCode::Denied, DECLINED),
            PcCaptureError::FilesOff => Envelope::error(ErrorCode::Denied, FILES_OFF),
            PcCaptureError::Busy => Envelope::error(ErrorCode::Busy, "already capturing"),
            PcCaptureError::Unsupported => Envelope::error(ErrorCode::Unsupported, "can't capture here"),
            PcCaptureError::NotRecording => Envelope::error(ErrorCode::NotFound, "not recording"),
            PcCaptureError::Offline | PcCaptureError::Failed(_) => {
                Envelope::error(ErrorCode::Internal, "capture failed")
            }
        }
    }

    /// What an `error` answer from the PC means.
    fn from_remote(code: ErrorCode, msg: &str) -> Self {
        use g7link_protocol::messages::pc_capture::{DECLINED, FILES_OFF, LOCKED};
        match code {
            ErrorCode::Denied if msg == LOCKED => PcCaptureError::Locked,
            ErrorCode::Denied if msg == DECLINED => PcCaptureError::Declined,
            ErrorCode::Denied if msg == FILES_OFF => PcCaptureError::FilesOff,
            ErrorCode::Denied => PcCaptureError::Off,
            ErrorCode::Busy => PcCaptureError::Busy,
            ErrorCode::Unsupported => PcCaptureError::Unsupported,
            ErrorCode::NotFound => PcCaptureError::NotRecording,
            other => PcCaptureError::Failed(other.to_string()),
        }
    }

    fn from_core(err: Error) -> Self {
        match err {
            Error::NotPaired | Error::Offline | Error::Network(_) => PcCaptureError::Offline,
            Error::Unsupported => PcCaptureError::Unsupported,
            other => PcCaptureError::Failed(other.to_string()),
        }
    }
}

impl std::fmt::Display for PcCaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PcCaptureError::Off => f.write_str("seeing the screen is off for this device"),
            PcCaptureError::Locked => f.write_str("the PC is locked"),
            PcCaptureError::Declined => f.write_str("declined on the PC"),
            PcCaptureError::FilesOff => f.write_str("files are off for this device"),
            PcCaptureError::Busy => f.write_str("already capturing"),
            PcCaptureError::Unsupported => f.write_str("the PC can't capture its screen"),
            PcCaptureError::NotRecording => f.write_str("not recording"),
            PcCaptureError::Offline => f.write_str("the PC is offline"),
            PcCaptureError::Failed(reason) => write!(f, "capture failed: {reason}"),
        }
    }
}

impl std::error::Error for PcCaptureError {}

// ---- Phone ----

/// Asks the PC `peer` for `t` and waits up to `wait` for the answer.
async fn ask(shared: &Shared, peer: DeviceId, t: &str, wait: Duration) -> Result<Envelope, PcCaptureError> {
    if !shared.store.is_paired(&peer).map_err(PcCaptureError::from_core)? {
        return Err(PcCaptureError::Offline);
    }
    let session = shared.session(&peer).ok_or(PcCaptureError::Offline)?;
    if !shared.offers(&peer, CAP).unwrap_or(false) {
        return Err(PcCaptureError::Unsupported);
    }
    session.request(Envelope::empty(t), wait).await.map_err(PcCaptureError::from_core)
}

/// The file in an answer, or why there is none.
fn file_in(reply: &Envelope) -> Result<PcCaptureResult, PcCaptureError> {
    match reply.expect_body::<PcCaptureResult>(types::PC_CAPTURE_RESULT) {
        Ok(file) if file.is_valid() => Ok(file),
        Ok(_) => Err(PcCaptureError::Failed("invalid file in the answer".into())),
        Err(ProtocolError::Remote { code, msg }) => Err(PcCaptureError::from_remote(code, &msg)),
        Err(e) => Err(PcCaptureError::Failed(e.to_string())),
    }
}

/// Asks a paired PC for a screenshot; the answer names the transfer bringing it.
pub(crate) async fn shot(shared: &Shared, peer: DeviceId) -> Result<PcCaptureResult, PcCaptureError> {
    let reply = ask(shared, peer, types::PC_CAPTURE_SHOT, SHOT_WAIT).await?;
    file_in(&reply)
}

/// Asks a paired PC to record its screen, once it is recording.
pub(crate) async fn record_start(shared: &Shared, peer: DeviceId) -> Result<(), PcCaptureError> {
    let reply = ask(shared, peer, types::PC_CAPTURE_RECORD_START, RECORD_START_WAIT).await?;
    match reply.expect(types::OK) {
        Ok(()) => Ok(()),
        Err(ProtocolError::Remote { code, msg }) => Err(PcCaptureError::from_remote(code, &msg)),
        Err(e) => Err(PcCaptureError::Failed(e.to_string())),
    }
}

/// Stops a recording this phone started; the answer names the transfer bringing it.
pub(crate) async fn record_stop(shared: &Shared, peer: DeviceId) -> Result<PcCaptureResult, PcCaptureError> {
    let reply = ask(shared, peer, types::PC_CAPTURE_RECORD_STOP, RECORD_STOP_WAIT).await?;
    file_in(&reply)
}

// ---- PC ----

/// Handles `pc.capture.*` messages. Returns false for other types.
pub(crate) async fn handle(shared: &Arc<Shared>, session: &Arc<Session>, env: &Envelope) -> Result<bool> {
    let peer = session.peer;
    match env.t.as_str() {
        types::PC_CAPTURE_SHOT | types::PC_CAPTURE_RECORD_START | types::PC_CAPTURE_RECORD_STOP => {
            let reply = answer(shared, session, &env.t).await.unwrap_or_else(|e| {
                tracing::info!(peer = %peer.short(), t = %env.t, error = %e, "no capture");
                e.envelope()
            });
            if env.id.is_some() {
                session.send(reply.reply_to(env.id)).await?;
            }
            Ok(true)
        }
        types::PC_CAPTURE_PROMPT => {
            shared.emit(NodeEvent::PcCapturePrompt { device: peer });
            Ok(true)
        }
        types::PC_CAPTURE_RECORD_ENDED => {
            let ended: PcCaptureRecordEnded = env.body()?;
            let file = ended.file.filter(PcCaptureResult::is_valid);
            shared.emit(NodeEvent::PcCaptureRecordEnded { device: peer, file });
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Runs a platform call that may wait on the person at the PC.
async fn blocking<T: Send + 'static>(
    call: impl FnOnce() -> Result<T, PcCaptureError> + Send + 'static,
) -> Result<T, PcCaptureError> {
    tokio::task::spawn_blocking(call).await.unwrap_or_else(|e| Err(PcCaptureError::Failed(e.to_string())))
}

async fn answer(shared: &Arc<Shared>, session: &Arc<Session>, t: &str) -> Result<Envelope, PcCaptureError> {
    let peer = session.peer;
    if !shared.we_offer(CAP) {
        return Err(PcCaptureError::Unsupported);
    }
    let platform = shared.platform.clone();
    // Stopping is always allowed: it ends a capture the phone was allowed to start.
    if t == types::PC_CAPTURE_RECORD_STOP {
        let path = blocking(move || platform.pc_capture_record_stop(&peer)).await?;
        return result_envelope(&send_file(shared, peer, path).await?);
    }
    if !shared.toggle_on(&peer, TOGGLE) {
        return Err(PcCaptureError::Off);
    }
    let prompt = prompt_for(session);
    if t == types::PC_CAPTURE_RECORD_START {
        blocking(move || platform.pc_capture_record_start(&peer, prompt)).await?;
        return Ok(Envelope::empty(types::OK));
    }
    let path = blocking(move || platform.pc_capture_shot(&peer, prompt)).await?;
    result_envelope(&send_file(shared, peer, path).await?)
}

fn result_envelope(file: &PcCaptureResult) -> Result<Envelope, PcCaptureError> {
    Envelope::new(types::PC_CAPTURE_RESULT, file).map_err(|e| PcCaptureError::Failed(e.to_string()))
}

fn prompt_for(session: &Arc<Session>) -> CapturePrompt {
    let session = session.clone();
    Arc::new(move || {
        let _ = session.send_now(Envelope::empty(types::PC_CAPTURE_PROMPT));
    })
}

/// Starts sending a screenshot or recording at `path` to `peer`.
async fn send_file(
    shared: &Arc<Shared>,
    peer: DeviceId,
    path: PathBuf,
) -> Result<PcCaptureResult, PcCaptureError> {
    let name = path
        .file_name()
        .map(|n| crate::transfer::safe_file_name(&n.to_string_lossy()))
        .filter(|n| is_valid_file_name(n))
        .ok_or_else(|| PcCaptureError::Failed("the capture has no file name".into()))?;
    let file = OutgoingFile { name: name.clone(), folder: None, source: FileSource::Path(path) };
    match crate::transfer::send(shared, peer, vec![file]).await {
        Ok(transfer) => Ok(PcCaptureResult { transfer, name }),
        Err(Error::Denied) => Err(PcCaptureError::FilesOff),
        Err(e) => Err(PcCaptureError::from_core(e)),
    }
}

/// A recording a phone started ended on the PC by itself: sends the file (when
/// there is one) and tells the phone.
pub(crate) async fn record_ended(shared: &Arc<Shared>, peer: DeviceId, path: Option<PathBuf>) {
    let file = match path {
        Some(path) => match send_file(shared, peer, path).await {
            Ok(file) => Some(file),
            Err(e) => {
                tracing::warn!(peer = %peer.short(), error = %e, "can't send the recording");
                None
            }
        },
        None => None,
    };
    let Some(session) = shared.session(&peer) else { return };
    if let Ok(env) = Envelope::new(types::PC_CAPTURE_RECORD_ENDED, &PcCaptureRecordEnded { file }) {
        let _ = session.send(env).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_survives_the_trip_to_the_phone() {
        for err in [
            PcCaptureError::Off,
            PcCaptureError::Locked,
            PcCaptureError::Declined,
            PcCaptureError::FilesOff,
            PcCaptureError::Busy,
            PcCaptureError::Unsupported,
            PcCaptureError::NotRecording,
        ] {
            let reply = Envelope::from_cbor(&err.envelope().to_cbor()).unwrap();
            assert_eq!(file_in(&reply), Err(err));
        }
        let reply = PcCaptureError::Failed("disk full".into()).envelope();
        assert!(matches!(file_in(&reply), Err(PcCaptureError::Failed(_))));
    }

    #[test]
    fn an_answer_names_a_valid_file_or_is_refused() {
        let good = PcCaptureResult { transfer: "abcdefghijklmnop0123".into(), name: "shot.png".into() };
        let reply = result_envelope(&good).unwrap();
        assert_eq!(file_in(&reply), Ok(good.clone()));
        let bad = PcCaptureResult { name: "../shot.png".into(), ..good };
        let reply = result_envelope(&bad).unwrap();
        assert!(matches!(file_in(&reply), Err(PcCaptureError::Failed(_))));
    }
}
