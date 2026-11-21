// SPDX-License-Identifier: MPL-2.0
//! From a message to the service that handles it, and the way back for its
//! answer (`docs/protocol/v1.md` §4). The same table serves a service's
//! lane and its RPCs; a [`Responder`] knows which stream the answer goes on.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use g7link_protocol::{
    Envelope, ErrorCode, MAX_FRAME_LEN,
    lanes::{self, Lane, Paged, RPC_PART_TARGET, svc},
    messages::types,
    write_frame,
};
use iroh::endpoint::SendStream;

use crate::{Error, Result, node::Shared, session::Session, wire_stats};

/// Where the answer to one incoming message goes.
#[derive(Debug)]
pub(crate) struct Responder<'a> {
    to: ReplyTo<'a>,
    answered: AtomicBool,
}

#[derive(Debug)]
enum ReplyTo<'a> {
    /// A one-way message: there's nothing to answer.
    Nobody,
    /// A request on control, answered there with `re`.
    Control { session: &'a Session, id: u64 },
    /// A request on a lane the peer opened, answered on it with `re`.
    Lane { send: &'a tokio::sync::Mutex<SendStream>, id: u64 },
    /// The request of an RPC stream, answered with `part`s and then one
    /// `ok` or `error`, after which the stream is finished.
    Rpc { send: &'a tokio::sync::Mutex<SendStream> },
}

impl<'a> Responder<'a> {
    pub(crate) fn control(session: &'a Session, id: Option<u64>) -> Responder<'a> {
        let to = id.map_or(ReplyTo::Nobody, |id| ReplyTo::Control { session, id });
        Responder { to, answered: AtomicBool::new(false) }
    }

    pub(crate) fn lane(send: &'a tokio::sync::Mutex<SendStream>, id: Option<u64>) -> Responder<'a> {
        let to = id.map_or(ReplyTo::Nobody, |id| ReplyTo::Lane { send, id });
        Responder { to, answered: AtomicBool::new(false) }
    }

    pub(crate) fn rpc(send: &'a tokio::sync::Mutex<SendStream>) -> Responder<'a> {
        Responder { to: ReplyTo::Rpc { send }, answered: AtomicBool::new(false) }
    }

    /// Whether the message was a request: something waits for an answer.
    pub(crate) fn wants_answer(&self) -> bool {
        !matches!(self.to, ReplyTo::Nobody)
    }

    /// Whether an answer went (or there's none to give).
    pub(crate) fn answered(&self) -> bool {
        !self.wants_answer() || self.answered.load(Ordering::Relaxed)
    }

    /// Answers with `env` (`ok`, `error`, or a typed reply on control). A
    /// request is answered once; a later answer is dropped. A stream the
    /// peer gave up on isn't an error here: there's no one left to tell.
    pub(crate) async fn send(&self, mut env: Envelope) -> Result<()> {
        if !self.wants_answer() || self.answered.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        env.id = None;
        match &self.to {
            ReplyTo::Nobody => Ok(()),
            ReplyTo::Control { session, id } => session.send_control(env.reply_to(Some(*id))).await,
            ReplyTo::Lane { send, id } => {
                let bytes = env.reply_to(Some(*id)).to_cbor();
                let mut send = send.lock().await;
                if let Err(e) = write_frame(&mut *send, &bytes).await {
                    tracing::debug!(error = %e, "the lane closed before its answer");
                }
                Ok(())
            }
            ReplyTo::Rpc { send } => {
                env.re = None;
                let t = env.t.clone();
                let bytes = encode_blocking(env).await?;
                let mut send = send.lock().await;
                write_rpc_frames(&mut send, &t, std::iter::once(bytes)).await;
                Ok(())
            }
        }
    }

    /// Answers `ok`.
    pub(crate) async fn ok(&self) -> Result<()> {
        self.send(Envelope::empty(types::OK)).await
    }

    /// Answers `error` with `code`.
    pub(crate) async fn error(&self, code: ErrorCode, msg: impl Into<String>) -> Result<()> {
        self.send(Envelope::error(code, msg)).await
    }

    /// Answers a list: on an RPC stream as `part`s of about
    /// [`RPC_PART_TARGET`] bytes and the `ok` (`docs/protocol/v1.md` §4.3),
    /// elsewhere as one `ok`.
    pub(crate) async fn paged<T: Paged + Send + 'static>(&self, body: T) -> Result<()> {
        self.paged_sent(body).await.map(|_| ())
    }

    /// [`paged`](Self::paged), and whether all of it was written: false when
    /// the stream closed before its last part, or an item was too large to
    /// send (agents §8: a phone's review counts a file read only then).
    pub(crate) async fn paged_sent<T: Paged + Send + 'static>(&self, body: T) -> Result<bool> {
        let ReplyTo::Rpc { send } = &self.to else {
            return self.send(Envelope::new(types::OK, &body)?).await.map(|()| true);
        };
        if self.answered.swap(true, Ordering::Relaxed) {
            return Ok(false);
        }
        let frames = tokio::task::spawn_blocking(move || {
            let parts = body.split(RPC_PART_TARGET);
            let last = parts.len().saturating_sub(1);
            parts
                .into_iter()
                .enumerate()
                .map(|(i, part)| {
                    let t = if i == last { types::OK } else { types::PART };
                    Envelope::new(t, &part).map(|env| env.to_cbor())
                })
                .collect::<std::result::Result<Vec<_>, _>>()
        })
        .await
        .map_err(|e| Error::Internal(e.to_string()))??;
        let whole = !frames.iter().any(|f| f.len() > MAX_FRAME_LEN);
        let frames = if whole {
            frames
        } else {
            // An item too large for any frame: the answer can't be sent whole.
            vec![Envelope::error(ErrorCode::FrameTooLarge, "an item is too large to send").to_cbor()]
        };
        let mut send = send.lock().await;
        Ok(write_rpc_frames(&mut send, types::PART, frames).await && whole)
    }
}

/// Encodes `env` on a blocking thread: an answer may be a large body.
async fn encode_blocking(env: Envelope) -> Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        let bytes = env.to_cbor();
        wire_stats::frame(&env.t, bytes.len(), started.elapsed());
        if bytes.len() > MAX_FRAME_LEN {
            return Envelope::error(ErrorCode::FrameTooLarge, "the answer is too large to send").to_cbor();
        }
        bytes
    })
    .await
    .map_err(|e| Error::Internal(e.to_string()))
}

/// Writes an RPC's answer frames and finishes the stream.
/// Writes `frames` and finishes the stream; whether every frame was written.
async fn write_rpc_frames(send: &mut SendStream, t: &str, frames: impl IntoIterator<Item = Vec<u8>>) -> bool {
    for bytes in frames {
        if let Err(e) = write_frame(&mut *send, &bytes).await {
            tracing::debug!(t, error = %e, "the RPC stream closed before its answer");
            return false;
        }
    }
    send.finish().is_ok()
}

/// The service a message type belongs to, if it's one a lane or an RPC carries.
pub(crate) fn service_of(t: &str) -> Option<&'static str> {
    match lanes::lane_of(t)? {
        Lane::Service(service) | Lane::Rpc { svc: service, .. } => Some(service),
        _ => None,
    }
}

/// Runs the handler of `env`'s service. `Ok(false)`: no handler takes it.
pub(crate) async fn dispatch(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<bool> {
    let Some(service) = service_of(&env.t) else { return Ok(false) };
    let t = env.t.as_str();
    match service {
        svc::NOTIFY => crate::notifications::handle(shared, session, env, rsp).await,
        svc::CLIP => crate::clipboard::handle(shared, session, env, rsp).await,
        svc::MEDIA if t == types::MEDIA_BT => {
            match env.body() {
                Ok(body) => {
                    let peer = session.peer;
                    crate::session::store_blocking(shared, move |shared| {
                        shared.bt_media_from_peer(peer, body)
                    })
                    .await?
                }
                Err(_) => tracing::debug!(peer = %session.peer.short(), "ignored a malformed media.bt"),
            }
            Ok(true)
        }
        svc::MEDIA => crate::media::handle(shared, session, env, rsp).await,
        svc::CALL => crate::calls::handle(shared, session, env, rsp).await,
        svc::CONTACTS => crate::contacts::handle(shared, session, env, rsp).await,
        svc::SMS => crate::sms::handle(shared, session, env, rsp).await,
        svc::PHOTOS => crate::photos::handle(shared, session, env, rsp).await,
        svc::STORAGE => crate::storage::handle(shared, session, env, rsp).await,
        svc::FILES => crate::transfer::handle_control(shared, session.peer, env).await,
        svc::PC if t.starts_with("pc.capture.") => crate::pc_capture::handle(shared, session, env, rsp).await,
        svc::PC if t == types::PC_WAKE_RELAY => crate::actions::wake_relay(shared, session, env, rsp).await,
        svc::PC | svc::TASK | svc::LINK => crate::actions::handle(shared, session, env, rsp).await,
        svc::TOGGLES => crate::toggles::handle(shared, session, env, rsp).await,
        svc::DECK => crate::deck::handle(shared, session, env, rsp).await,
        svc::INPUT => crate::remote::handle(shared, session, env, rsp).await,
        svc::MIRROR => crate::mirror::handle(shared, session, env, rsp).await,
        svc::WEBCAM => crate::webcam::handle(shared, session, env, rsp).await,
        svc::MIC => crate::mic::handle(shared, session, env, rsp).await,
        svc::CAMERA => crate::continuity_camera::handle(shared, session, env, rsp).await,
        svc::AGENT => crate::agents::handle(shared, session, env, rsp).await,
        svc::ELEVATED => crate::elevated_usb::handle(shared, session, env, rsp).await,
        _ => Ok(false),
    }
}

/// Runs `env`'s handler and, for a request, makes sure it's answered: a
/// handler that failed or took no answer gets an `error` in its place.
pub(crate) async fn run_handler(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    env: &Envelope,
    rsp: &Responder<'_>,
    lane: wire_stats::LaneKind,
) {
    let started = Instant::now();
    let handled = dispatch(shared, session, env, rsp).await;
    let service = service_of(&env.t).unwrap_or("");
    wire_stats::handler(service, &env.t, lane, started.elapsed());
    let refusal = match handled {
        Ok(true) => None,
        Ok(false) => Some(Envelope::error(ErrorCode::Unsupported, format!("unknown message type {}", env.t))),
        Err(e) => {
            tracing::warn!(peer = %session.peer.short(), t = %env.t, error = %e, "failed to handle message");
            Some(error_reply(&e))
        }
    };
    if let Some(refusal) = refusal
        && !rsp.answered()
    {
        let _ = rsp.send(refusal).await;
    }
}

/// The `error` that tells the peer why its request failed here.
pub(crate) fn error_reply(e: &Error) -> Envelope {
    let code = match e {
        Error::NotPaired => ErrorCode::Unpaired,
        Error::Denied | Error::Declined => ErrorCode::Denied,
        Error::Unsupported => ErrorCode::Unsupported,
        Error::NotFound => ErrorCode::NotFound,
        Error::Busy => ErrorCode::Busy,
        Error::TooLarge => ErrorCode::FrameTooLarge,
        Error::Protocol(_) => ErrorCode::BadMessage,
        _ => ErrorCode::Internal,
    };
    let msg = code.as_str().to_ascii_lowercase();
    Envelope::error(code, msg)
}
