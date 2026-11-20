// SPDX-License-Identifier: MPL-2.0
//! Text messages (docs/protocol/sms.md): a PC lists a phone's
//! conversations and their messages, fetches pictures in them, and sends
//! texts through the phone; the phone says when something changed.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, LazyLock, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use g7link_protocol::{
    DeviceId, Envelope, ErrorCode,
    messages::{
        SmsAttachment, SmsChanged, SmsMessage, SmsMessages, SmsMessagesGet, SmsPartData, SmsPartGet, SmsSend,
        SmsThread, SmsThreads, SmsThreadsGet, sms, types,
    },
};

use crate::{Error, Result, dispatch::Responder, events::NodeEvent, node::Shared, session::Session};

/// The device toggle that allows messages for a device.
pub(crate) const TOGGLE: &str = "messages";
/// Requests from one PC the phone answers at once; more are turned away.
const MAX_RUNNING: usize = 4;
/// Texts one PC may send through the phone in [`SEND_WINDOW`].
const MAX_SENDS: usize = 10;
const SEND_WINDOW: Duration = Duration::from_secs(60);

/// What each PC has running on the phone, and when it last sent texts.
static LIMITS: LazyLock<Mutex<Limits>> = LazyLock::new(|| Mutex::new(Limits::default()));

fn limits() -> MutexGuard<'static, Limits> {
    LIMITS.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Default)]
struct Limits {
    peers: HashMap<DeviceId, PeerLoad>,
}

#[derive(Debug, Default)]
struct PeerLoad {
    running: usize,
    sends: VecDeque<Instant>,
}

impl Limits {
    /// Starts a request from `peer`, or says why it's turned away: a paired
    /// PC mustn't tie up the phone or send texts without end.
    fn start(
        &mut self,
        peer: DeviceId,
        sending: bool,
        now: Instant,
    ) -> std::result::Result<(), &'static str> {
        let load = self.peers.entry(peer).or_default();
        if load.running >= MAX_RUNNING {
            return Err("too many messages requests at once");
        }
        if sending {
            while load.sends.front().is_some_and(|at| now.saturating_duration_since(*at) >= SEND_WINDOW) {
                load.sends.pop_front();
            }
            if load.sends.len() >= MAX_SENDS {
                return Err("too many texts sent in a minute");
            }
            load.sends.push_back(now);
        }
        load.running += 1;
        Ok(())
    }

    fn finish(&mut self, peer: &DeviceId) {
        if let Some(load) = self.peers.get_mut(peer) {
            load.running = load.running.saturating_sub(1);
            if load.running == 0 && load.sends.is_empty() {
                self.peers.remove(peer);
            }
        }
    }
}

/// A request's place in [`LIMITS`], given back when it's answered.
struct Running(DeviceId);

impl Drop for Running {
    fn drop(&mut self) {
        limits().finish(&self.0);
    }
}

impl Shared {
    fn sms_target(&self, session: &Session) -> bool {
        self.is_desktop(&session.peer)
            && self.toggle_on(&session.peer, TOGGLE)
            && self.store.get_peer(&session.peer).ok().flatten().is_some_and(|p| p.caps.contains(sms::SHOW))
    }

    pub(crate) async fn sms_changed(&self, thread: Option<String>) {
        let Ok(env) = Envelope::new(types::SMS_CHANGED, &SmsChanged { thread }) else { return };
        for session in self.live_sessions().into_iter().filter(|s| self.sms_target(s)) {
            let _ = session.send(env.clone()).await;
        }
    }

    fn offers_locally(&self, capability: &str) -> bool {
        self.we_offer(capability)
    }
}

// ---- A PC asking ----

fn allowed(shared: &Shared, peer: &DeviceId) -> Result<()> {
    if shared.toggle_on(peer, TOGGLE) { Ok(()) } else { Err(Error::Denied) }
}

/// The phone's latest conversations, and its default SMS app (a package name) when it says.
pub(crate) async fn threads(
    shared: &Shared,
    session: &Session,
    limit: u32,
) -> Result<(Vec<SmsThread>, Option<String>)> {
    allowed(shared, &session.peer)?;
    let limit = limit.clamp(1, sms::MAX_PAGE);
    let env = Envelope::new(types::SMS_THREADS, &SmsThreadsGet { limit })?;
    let SmsThreads { mut threads, app } = session.request_paged(env, crate::session::REQUEST_TIMEOUT).await?;
    threads.truncate(limit as usize);
    for t in &mut threads {
        if t.photo.as_ref().is_some_and(|p| p.len() > sms::MAX_PHOTO_BYTES) {
            t.photo = None;
        }
    }
    Ok((threads, app.filter(|a| g7link_protocol::messages::is_package_name(a))))
}

pub(crate) async fn messages(
    shared: &Shared,
    session: &Session,
    thread: String,
    before: Option<i64>,
    limit: u32,
) -> Result<Vec<SmsMessage>> {
    allowed(shared, &session.peer)?;
    let limit = limit.clamp(1, sms::MAX_PAGE);
    let env = Envelope::new(types::SMS_MESSAGES, &SmsMessagesGet { thread, before, limit })?;
    let SmsMessages { mut messages } = session.request_paged(env, crate::session::REQUEST_TIMEOUT).await?;
    messages.truncate(limit as usize);
    Ok(messages)
}

pub(crate) async fn send(
    shared: &Shared,
    session: &Session,
    to: Vec<String>,
    body: String,
    attachments: Vec<SmsAttachment>,
) -> Result<()> {
    allowed(shared, &session.peer)?;
    let send = SmsSend { to, body, attachments };
    if !send.is_valid() {
        let any_oversized = send.attachments.iter().any(|a| a.data.len() > sms::MAX_ATTACHMENT_BYTES);
        return Err(
            if send.body.len() > sms::MAX_SEND_BYTES
                || any_oversized
                || send.attachments.len() > sms::MAX_ATTACHMENTS
            {
                Error::TooLarge
            } else {
                Error::Protocol("invalid text".into())
            },
        );
    }
    let env = Envelope::new(types::SMS_SEND, &send)?;
    session.request(env, crate::session::REQUEST_TIMEOUT).await?.expect(types::OK)?;
    Ok(())
}

pub(crate) async fn part(shared: &Shared, session: &Session, id: String) -> Result<(String, Vec<u8>)> {
    allowed(shared, &session.peer)?;
    let env = Envelope::new(types::SMS_PART, &SmsPartGet { id })?;
    let SmsPartData { mime, data } =
        session.request(env, crate::session::REQUEST_TIMEOUT).await?.expect_body(types::OK)?;
    if data.len() > sms::MAX_PART_BYTES {
        return Err(Error::TooLarge);
    }
    let mime = mime.trim().to_owned();
    if mime.is_empty() || mime.chars().any(char::is_control) {
        return Err(Error::Protocol("invalid MMS part mime".into()));
    }
    Ok((mime, data))
}

// ---- The phone answering ----

/// Handles `sms.*`. Returns false for other types.
pub(crate) async fn handle(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<bool> {
    if env.t == types::SMS_CHANGED {
        let SmsChanged { thread } = env.body()?;
        if shared.offers_locally(sms::SHOW) && shared.toggle_on(&session.peer, TOGGLE) {
            shared.emit(NodeEvent::SmsChanged { device: session.peer, thread });
        }
        return Ok(true);
    }
    if ![types::SMS_THREADS, types::SMS_MESSAGES, types::SMS_SEND, types::SMS_PART].contains(&env.t.as_str())
    {
        return Ok(false);
    }
    let started = limits().start(session.peer, env.t == types::SMS_SEND, Instant::now());
    if let Err(why) = started {
        tracing::warn!(peer = %session.peer.short(), t = %env.t, why, "turned a messages request away");
        let _ = rsp.send(Envelope::error(ErrorCode::Busy, why)).await;
        return Ok(true);
    }
    let _running = Running(session.peer);
    match answer(shared, session, env).await {
        Ok(Answer::Threads(threads)) => rsp.paged(threads).await?,
        Ok(Answer::Messages(messages)) => rsp.paged(messages).await?,
        Ok(Answer::Plain(reply)) => rsp.send(reply).await?,
        Err(e) => {
            tracing::debug!(error = %e, "bad messages request");
            rsp.error(ErrorCode::BadMessage, "invalid request").await?;
        }
    }
    Ok(true)
}

/// The phone's answer to a messages request.
enum Answer {
    Threads(SmsThreads),
    Messages(SmsMessages),
    Plain(Envelope),
}

async fn answer(shared: &Arc<Shared>, session: &Session, env: &Envelope) -> Result<Answer> {
    if !shared.toggle_on(&session.peer, TOGGLE) {
        return Ok(Answer::Plain(Envelope::error(ErrorCode::Denied, "messages are off for this device")));
    }
    let needs = if env.t == types::SMS_SEND { sms::SEND } else { sms::READ };
    if !shared.offers_locally(needs) {
        return Ok(Answer::Plain(Envelope::error(
            ErrorCode::Unsupported,
            "this phone doesn't share messages",
        )));
    }
    let platform = shared.platform.clone();
    let blocking = |f: Box<dyn FnOnce() -> std::result::Result<Answer, String> + Send>| async move {
        tokio::task::spawn_blocking(f).await.unwrap_or_else(|e| Err(e.to_string()))
    };
    let result = match env.t.as_str() {
        types::SMS_THREADS => {
            let SmsThreadsGet { limit } = env.body()?;
            blocking(Box::new(move || {
                let mut threads = platform.sms_threads(limit.min(sms::MAX_PAGE))?;
                for t in &mut threads {
                    if t.photo.as_ref().is_some_and(|p| p.len() > sms::MAX_PHOTO_BYTES) {
                        t.photo = None;
                    }
                }
                let app = platform.sms_app().filter(|a| g7link_protocol::messages::is_package_name(a));
                Ok(Answer::Threads(SmsThreads { threads, app }))
            }))
            .await
        }
        types::SMS_MESSAGES => {
            let SmsMessagesGet { thread, before, limit } = env.body()?;
            blocking(Box::new(move || {
                let messages = platform.sms_messages(&thread, before, limit.min(sms::MAX_PAGE))?;
                Ok(Answer::Messages(SmsMessages { messages }))
            }))
            .await
        }
        types::SMS_SEND => {
            let send: SmsSend = env.body()?;
            if !send.is_valid() {
                return Ok(Answer::Plain(Envelope::error(ErrorCode::BadMessage, "invalid text")));
            }
            blocking(Box::new(move || {
                platform
                    .send_sms_with_attachments(&send.to, &send.body, &send.attachments)
                    .map(|()| Answer::Plain(Envelope::empty(types::OK)))
            }))
            .await
        }
        _ => {
            let SmsPartGet { id } = env.body()?;
            blocking(Box::new(move || {
                let (mime, data) = platform.sms_part(&id)?;
                if data.len() > sms::MAX_PART_BYTES {
                    return Ok(Answer::Plain(Envelope::error(ErrorCode::FrameTooLarge, "too large")));
                }
                Envelope::new(types::OK, &SmsPartData { mime, data })
                    .map(Answer::Plain)
                    .map_err(|e| e.to_string())
            }))
            .await
        }
    };
    Ok(result.unwrap_or_else(|reason| {
        tracing::warn!(reason, t = env.t, "a messages request failed");
        Answer::Plain(Envelope::error(ErrorCode::Internal, "the phone couldn't do it"))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pc_has_a_few_requests_running_and_ten_texts_a_minute() {
        let (pc, other) = (DeviceId([1; 32]), DeviceId([2; 32]));
        let start = Instant::now();
        let mut limits = Limits::default();
        for _ in 0..MAX_RUNNING {
            assert!(limits.start(pc, false, start).is_ok());
        }
        assert!(limits.start(pc, false, start).is_err());
        assert!(limits.start(other, false, start).is_ok(), "each PC has its own");
        limits.finish(&pc);
        assert!(limits.start(pc, false, start).is_ok());
        for _ in 0..MAX_RUNNING {
            limits.finish(&pc);
        }
        limits.finish(&other);
        assert!(limits.peers.is_empty(), "nothing is kept for idle PCs");

        for i in 0..MAX_SENDS {
            assert!(limits.start(pc, true, start).is_ok(), "text {i}");
            limits.finish(&pc);
        }
        assert!(limits.start(pc, true, start + Duration::from_secs(30)).is_err());
        // Reading goes on meanwhile, and texts again once a minute has passed.
        assert!(limits.start(pc, false, start + Duration::from_secs(30)).is_ok());
        limits.finish(&pc);
        assert!(limits.start(pc, true, start + SEND_WINDOW).is_ok());
    }
}
