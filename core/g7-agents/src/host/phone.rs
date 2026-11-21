// SPDX-License-Identifier: MPL-2.0
//! Phones, through G7 Link (phase 5, §V8.4, §V8.5 and §V9.1). `ui.handoff`
//! asks G7 Link to continue a thread on a phone. `ui.photo` asks it for one
//! photo from a phone's camera, which comes back with `ui.photo.done` as one
//! of the launcher's own pending images. `ui.beside` asks it to switch what
//! a phone that shows a thread beside this PC shows.
//!
//! The host only passes these on: it never learns which phones there are,
//! and nothing here sends a message, starts a thread or answers a request.

use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use crate::api::*;
use crate::host::attachments::{ImageError, Place};
use crate::host::threads::random_hex;
use crate::host::{Host, client_owner, image_error};

type Answer = Result<UiPhotoResult, RpcError>;

fn link_not_open() -> RpcError {
    RpcError::new(ErrorCode::Unavailable, "G7 Link isn't open.")
}

/// The one photo on its way.
#[derive(Debug)]
struct Pending {
    request: String,
    /// The launcher's connection that asked.
    asker: u64,
    /// The G7 Link connection the request went to.
    link: u64,
    reply: oneshot::Sender<Answer>,
}

/// `ui.photo`: at most one request at a time, all launchers together.
#[derive(Debug)]
pub struct Photos {
    pending: Option<Pending>,
    /// How long a request waits for `ui.photo.done`.
    wait: Duration,
}

impl Default for Photos {
    fn default() -> Self {
        Self { pending: None, wait: Duration::from_millis(PHOTO_WAIT_MS) }
    }
}

impl Photos {
    /// Takes the pending request if it is `request`.
    fn take(&mut self, request: &str) -> Option<Pending> {
        self.pending.take_if(|p| p.request == request)
    }

    /// `request` is waiting and went to connection `link`.
    fn waits_on(&self, request: &str, link: u64) -> bool {
        self.pending.as_ref().is_some_and(|p| p.request == request && p.link == link)
    }

    /// Takes the pending request if it is `request` and went to connection `link`.
    fn take_for(&mut self, request: &str, link: u64) -> Option<Pending> {
        self.pending.take_if(|p| p.request == request && p.link == link)
    }

    /// Connection `client` closed: the request it asked or was sent goes.
    fn closed(&mut self, client: u64) -> Option<Pending> {
        self.pending.take_if(|p| p.asker == client || p.link == client)
    }

    /// `ui.photo.stop` from connection `client`: the request it asked, if one
    /// waits; `Err` when the one waiting is another connection's.
    fn stop(&mut self, client: u64) -> Result<Option<Pending>, ()> {
        match &self.pending {
            Some(p) if p.asker != client => Err(()),
            _ => Ok(self.pending.take()),
        }
    }

    #[cfg(test)]
    pub(crate) fn set_wait(&mut self, wait: Duration) {
        self.wait = wait;
    }
}

impl Host {
    pub(crate) fn lock_photos(&self) -> std::sync::MutexGuard<'_, Photos> {
        self.photos.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `ui.handoff`: the `ui.handoff` event to G7 Link's connections that take it.
    pub(crate) fn ui_handoff(
        &self,
        kind: ClientKind,
        p: UiHandoffParams,
    ) -> Result<UiHandoffResult, RpcError> {
        if !matches!(kind, ClientKind::Launcher | ClientKind::Link) {
            return Err(RpcError::new(
                ErrorCode::Denied,
                "Only the launcher and G7 Link hand a thread over.",
            ));
        }
        if p.device_id.as_deref().is_some_and(|id| id.is_empty() || id.len() > MAX_DEVICE_ID) {
            return Err(RpcError::new(ErrorCode::BadRequest, "Bad device id."));
        }
        if !p.by.is_valid() {
            return Err(RpcError::new(ErrorCode::BadRequest, "Bad origin."));
        }
        if !self.lock_reg().threads.contains_key(&p.thread_id) {
            return Err(RpcError::new(ErrorCode::NotFound, "No such thread."));
        }
        let event = HandoffEvent { thread_id: p.thread_id, device_id: p.device_id, seq: p.seq };
        let sent = self.hub.send_handoff(&event);
        Ok(UiHandoffResult { sent: u32::try_from(sent).unwrap_or(u32::MAX) })
    }

    /// `ui.beside`: the `ui.beside` event to G7 Link's connections that take
    /// it, for a phone that shows that thread now (G7 Link's fresh `ui.presence`).
    pub(crate) fn ui_beside(&self, kind: ClientKind, p: UiBesideParams) -> Result<(), RpcError> {
        if kind != ClientKind::Link {
            return Err(RpcError::new(ErrorCode::Denied, "Only G7 Link switches what a phone shows."));
        }
        if p.device_id.is_empty() || p.device_id.len() > MAX_DEVICE_ID {
            return Err(RpcError::new(ErrorCode::BadRequest, "Bad device id."));
        }
        if !p.by.is_valid() {
            return Err(RpcError::new(ErrorCode::BadRequest, "Bad origin."));
        }
        if !self.lock_reg().threads.contains_key(&p.thread_id) {
            return Err(RpcError::new(ErrorCode::NotFound, "No such thread."));
        }
        let shows = self.hub.lock_presence().fresh(Instant::now()).is_some_and(|presence| {
            presence
                .remote
                .iter()
                .any(|v| v.device_id == p.device_id && v.thread_id.as_deref() == Some(p.thread_id.as_str()))
        });
        if !shows {
            return Err(RpcError::new(ErrorCode::NotFound, "That phone doesn't show this thread."));
        }
        self.hub.send_beside(&BesideEvent { thread_id: p.thread_id, device_id: p.device_id, pane: p.pane });
        Ok(())
    }

    /// `ui.photo`: asks one G7 Link connection for a photo and waits for its
    /// `ui.photo.done`, at most [`Photos::wait`].
    pub(crate) async fn ui_photo(
        &self,
        kind: ClientKind,
        client: u64,
        p: UiPhotoParams,
    ) -> Result<UiPhotoResult, RpcError> {
        if kind != ClientKind::Launcher {
            return Err(RpcError::new(ErrorCode::Denied, "Only the launcher asks G7 Link for a photo."));
        }
        if !p.by.is_valid() {
            return Err(RpcError::new(ErrorCode::BadRequest, "Bad origin."));
        }
        self.image_store()?;
        let link = self.hub.photo_link().ok_or_else(link_not_open)?;
        let (reply, mut answer) = oneshot::channel();
        let (request, wait) = {
            let mut photos = self.lock_photos();
            if photos.pending.is_some() {
                return Err(RpcError::new(ErrorCode::Busy, "A photo is already on its way."));
            }
            let request = random_hex(PHOTO_REQUEST_LEN / 2);
            photos.pending = Some(Pending { request: request.clone(), asker: client, link, reply });
            (request, photos.wait)
        };
        // G7 Link may have gone since it was chosen; its close found nothing to drop then.
        if !self.hub.send_to(link, &Event::UiPhoto(PhotoEvent { request: request.clone() })) {
            self.lock_photos().take(&request);
            return Err(link_not_open());
        }
        match tokio::time::timeout(wait, &mut answer).await {
            Ok(answered) => answered.unwrap_or_else(|_| Err(link_not_open())),
            Err(_) => {
                let gone = self.lock_photos().take(&request);
                match gone {
                    Some(pending) => {
                        self.hub.send_to(pending.link, &Event::UiPhotoCancel(PhotoEvent { request }));
                        Ok(UiPhotoResult { image: None, error: Some(PhotoError::Timeout) })
                    }
                    // `ui.photo.done` took it just now: its answer is on the way.
                    None => answer.await.unwrap_or_else(|_| Err(link_not_open())),
                }
            }
        }
    }

    /// `ui.photo.done`: G7 Link's answer, on the connection the request went to.
    pub(crate) async fn ui_photo_done(
        &self,
        kind: ClientKind,
        client: u64,
        p: UiPhotoDoneParams,
    ) -> Result<(), RpcError> {
        if kind != ClientKind::Link {
            return Err(RpcError::new(ErrorCode::Denied, "Only G7 Link answers for a photo."));
        }
        if p.image.is_some() == p.error.is_some() {
            return Err(RpcError::new(ErrorCode::BadRequest, "Either an image or why none came."));
        }
        let not_asked = || RpcError::new(ErrorCode::NotFound, "No photo was asked for.");
        if !self.lock_photos().waits_on(&p.request, client) {
            return Err(not_asked());
        }
        let link_owner = client_owner(ClientKind::Link);
        let launcher_owner = client_owner(ClientKind::Launcher);
        let images = self.image_store()?;
        // A wrong image leaves the request waiting: G7 Link may answer again.
        if let Some(id) = &p.image {
            let ours =
                images.info(id).is_some_and(|i| i.place == Place::Pending { owner: link_owner.into() });
            if !ours {
                return Err(image_error(ImageError::NotFound));
            }
        }
        let pending = self.lock_photos().take_for(&p.request, client).ok_or_else(not_asked)?;
        let answer = match (p.image, p.error) {
            (Some(id), _) => images
                .give(&id, link_owner, launcher_owner)
                .await
                .map(|info| UiPhotoResult { image: Some(info.image_ref()), error: None })
                .map_err(image_error),
            (None, error) => Ok(UiPhotoResult { image: None, error }),
        };
        let told = answer.as_ref().map(|_| ()).map_err(Clone::clone);
        // The launcher stopped listening in the meantime: the photo goes.
        if let Err(Ok(UiPhotoResult { image: Some(image), .. })) = pending.reply.send(answer) {
            let _ = images.drop_pending(&image.id, Some(launcher_owner)).await;
        }
        told
    }

    /// `ui.photo.stop`: the launcher no longer wants the photo it asked for.
    /// Its `ui.photo` answers `cancelled` and G7 Link stops waiting. Nothing
    /// waiting (never asked, or already answered): nothing to do.
    pub(crate) fn ui_photo_stop(&self, kind: ClientKind, client: u64) -> Result<(), RpcError> {
        let refused = || RpcError::new(ErrorCode::Denied, "Only the launcher that asked stops a photo.");
        if kind != ClientKind::Launcher {
            return Err(refused());
        }
        let stopped = self.lock_photos().stop(client).map_err(|()| refused())?;
        if let Some(pending) = stopped {
            let request = pending.request.clone();
            let _ = pending.reply.send(Ok(UiPhotoResult { image: None, error: Some(PhotoError::Cancelled) }));
            self.hub.send_to(pending.link, &Event::UiPhotoCancel(PhotoEvent { request }));
        }
        Ok(())
    }

    /// Connection `client` closed. The launcher that asked: G7 Link stops
    /// waiting (`ui.photo.cancel`). The G7 Link it went to: the launcher hears so.
    pub(crate) fn photo_closed(&self, client: u64) {
        let Some(pending) = self.lock_photos().closed(client) else { return };
        if pending.link == client {
            let _ = pending.reply.send(Err(link_not_open()));
        } else {
            self.hub.send_to(pending.link, &Event::UiPhotoCancel(PhotoEvent { request: pending.request }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(lines: Vec<String>) -> Vec<Event> {
        lines
            .iter()
            .map(|line| {
                let ServerLine::Event(line) = ServerLine::parse(line).unwrap() else {
                    panic!("not an event")
                };
                Event::from_line(&line).unwrap()
            })
            .collect()
    }

    #[tokio::test]
    async fn a_hand_off_goes_to_the_g7_link_connections_that_take_it() {
        let dir = tempfile::tempdir().unwrap();
        let host = crate::host::test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let thread_id = "a".repeat(32);
        let info = ThreadInfo { id: thread_id.clone(), ..crate::host::threads::test_info(AgentId::Claude) };
        host.lock_reg().threads.insert(thread_id.clone(), crate::host::threads::test_entry(info));
        let launcher = host.hub.test_viewer(ClientKind::Launcher, None);
        let takes = host.hub.test_link(true, false);
        let watch = host.hub.test_link(false, false);
        let params = |device_id: Option<&str>| UiHandoffParams {
            thread_id: thread_id.clone(),
            device_id: device_id.map(str::to_owned),
            seq: Some(12),
            by: Origin::launcher(),
        };
        let sent = host.ui_handoff(ClientKind::Launcher, params(Some("dev1"))).unwrap();
        assert_eq!(sent, UiHandoffResult { sent: 1 });
        let handed =
            HandoffEvent { thread_id: thread_id.clone(), device_id: Some("dev1".into()), seq: Some(12) };
        assert_eq!(events(host.hub.test_lines(takes)), [Event::UiHandoff(handed)]);
        assert!(host.hub.test_lines(watch).is_empty(), "a connection that didn't say handoff");
        assert!(host.hub.test_lines(launcher).is_empty());
        assert_eq!(host.ui_handoff(ClientKind::Link, params(None)).unwrap().sent, 1, "the PC app's own");

        for kind in [ClientKind::Test, ClientKind::Cli, ClientKind::Bridge] {
            assert_eq!(host.ui_handoff(kind, params(None)).unwrap_err().code, ErrorCode::Denied);
        }
        let long = "d".repeat(MAX_DEVICE_ID + 1);
        for bad in [Some(""), Some(long.as_str())] {
            assert_eq!(
                host.ui_handoff(ClientKind::Launcher, params(bad)).unwrap_err().code,
                ErrorCode::BadRequest
            );
        }
        let unknown = UiHandoffParams { thread_id: "b".repeat(32), ..params(None) };
        assert_eq!(host.ui_handoff(ClientKind::Launcher, unknown).unwrap_err().code, ErrorCode::NotFound);
        host.hub.test_close(takes);
        assert_eq!(
            host.ui_handoff(ClientKind::Launcher, params(None)).unwrap().sent,
            0,
            "G7 Link isn't open"
        );
    }

    #[tokio::test]
    async fn a_pane_goes_to_g7_link_for_a_phone_that_shows_the_thread() {
        let dir = tempfile::tempdir().unwrap();
        let host = crate::host::test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let thread_id = "a".repeat(32);
        let other = "b".repeat(32);
        for id in [&thread_id, &other] {
            let info = ThreadInfo { id: id.clone(), ..crate::host::threads::test_info(AgentId::Claude) };
            host.lock_reg().threads.insert(id.clone(), crate::host::threads::test_entry(info));
        }
        let takes = host.hub.test_link(false, false);
        host.hub.set(takes, |slot| slot.beside = true);
        let watch = host.hub.test_link(true, true);
        let launcher = host.hub.test_viewer(ClientKind::Launcher, None);
        let params = |thread_id: &str, device_id: &str| UiBesideParams {
            thread_id: thread_id.to_owned(),
            device_id: device_id.to_owned(),
            pane: Pane::Plan,
            by: Origin::launcher(),
        };
        // Nobody has said what the phone shows yet.
        let err = host.ui_beside(ClientKind::Link, params(&thread_id, "dev1")).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        let phone = RemoteView {
            device_id: "dev1".into(),
            device_name: "Pixel".into(),
            thread_id: Some(thread_id.clone()),
            session_id: None,
            pane: Some(Pane::Changes),
            dnd: false,
        };
        let presence = UiPresenceParams { at_pc: true, remote: vec![phone], ..Default::default() };
        host.hub.lock_presence().set(watch, presence, Instant::now());

        host.ui_beside(ClientKind::Link, params(&thread_id, "dev1")).unwrap();
        let beside = BesideEvent { thread_id: thread_id.clone(), device_id: "dev1".into(), pane: Pane::Plan };
        assert_eq!(events(host.hub.test_lines(takes)), [Event::UiBeside(beside)]);
        assert!(host.hub.test_lines(watch).is_empty(), "a connection that didn't say beside");
        assert!(host.hub.test_lines(launcher).is_empty());

        for kind in [ClientKind::Launcher, ClientKind::Test, ClientKind::Cli, ClientKind::Bridge] {
            let err = host.ui_beside(kind, params(&thread_id, "dev1")).unwrap_err();
            assert_eq!(err.code, ErrorCode::Denied, "{kind:?}");
        }
        let long = "d".repeat(MAX_DEVICE_ID + 1);
        for bad in ["", long.as_str()] {
            let err = host.ui_beside(ClientKind::Link, params(&thread_id, bad)).unwrap_err();
            assert_eq!(err.code, ErrorCode::BadRequest);
        }
        for (thread, device) in [(other.as_str(), "dev1"), (thread_id.as_str(), "dev2")] {
            let err = host.ui_beside(ClientKind::Link, params(thread, device)).unwrap_err();
            assert_eq!(err.code, ErrorCode::NotFound, "not on that phone: {thread} {device}");
        }
        let gone = params(&"c".repeat(32), "dev1");
        assert_eq!(host.ui_beside(ClientKind::Link, gone).unwrap_err().code, ErrorCode::NotFound);
        assert!(host.hub.test_lines(takes).is_empty(), "the refused ones sent nothing");
        let unknown = serde_json::json!({"thread_id": thread_id, "device_id": "dev1", "pane": "diff", "by": {"kind": "launcher"}});
        assert!(serde_json::from_value::<UiBesideParams>(unknown).is_err(), "a pane this host doesn't know");

        // The presence went with its connection: the phone shows nothing any more.
        assert!(host.hub.lock_presence().closed(watch));
        let err = host.ui_beside(ClientKind::Link, params(&thread_id, "dev1")).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_photo_nobody_answers_times_out_and_g7_link_stops_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let host = crate::host::test_host(dir.path(), APPROVAL_LIFETIME_MS);
        host.lock_photos().set_wait(Duration::from_millis(200));
        let launcher = host.hub.test_viewer(ClientKind::Launcher, None);
        let link = host.hub.test_link(false, true);
        let by = || UiPhotoParams { by: Origin::launcher() };
        let asked = {
            let host = host.clone();
            tokio::spawn(async move { host.ui_photo(ClientKind::Launcher, launcher, by()).await })
        };
        let mut sent = Vec::new();
        for _ in 0..2000 {
            sent = events(host.hub.test_lines(link));
            if !sent.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let [Event::UiPhoto(PhotoEvent { request })] = sent.as_slice() else { panic!("asked {sent:?}") };
        let request = request.clone();
        assert!(is_valid_photo_request(&request));
        assert_eq!(
            host.ui_photo(ClientKind::Launcher, launcher, by()).await.unwrap_err().code,
            ErrorCode::Busy
        );
        let answer = asked.await.unwrap().unwrap();
        assert_eq!(answer, UiPhotoResult { image: None, error: Some(PhotoError::Timeout) });
        assert_eq!(
            events(host.hub.test_lines(link)),
            [Event::UiPhotoCancel(PhotoEvent { request: request.clone() })]
        );
        let late = UiPhotoDoneParams { request, image: None, error: Some(PhotoError::Cancelled) };
        let err = host.ui_photo_done(ClientKind::Link, link, late).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound, "too late");

        // The next one isn't busy; with G7 Link gone there is nobody to ask.
        host.hub.test_close(link);
        let err = host.ui_photo(ClientKind::Launcher, launcher, by()).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(host.lock_photos().pending.is_none());
    }
}
