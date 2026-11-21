// SPDX-License-Identifier: GPL-3.0-or-later
//! A photo from a phone's camera for an agent (phase 5 §V8.4), through the
//! Continuity Camera the PC already has, with its own destination: the
//! picture is never pasted into a window or put on the clipboard.
//!
//! - **The composer**: "Take a photo with Pixel" in Add image. The strip
//!   says "Waiting for Pixel…" with Cancel; the photo joins the thread's next
//!   message. A thread closed meanwhile gets it when it opens again. After
//!   2 minutes: "No photo arrived."
//! - **The launcher**: the host's `ui.photo` event. G7 Link asks the first
//!   phone that can take one and answers `ui.photo.done` with the image or
//!   why none came.
//!
//! The bytes reach the host through a file only this user can read, removed
//! once the host has read them; the host decodes and encodes every picture
//! again (metadata removed), as it does a pasted one.

use std::time::Duration;

use g7_agents::{
    api::{self, ErrorCode, ImageRef, PhotoError, UiPhotoDoneParams},
    client::Client,
};
use g7link_core::{DeviceId, FeatureState, LinkState};

use super::{OpenKind, changed, control, lock};
use crate::{
    continuity_camera::{self, CaptureDest, Ended},
    core_host,
};

/// A photo is waited for this long.
pub(crate) const WAIT: Duration = Duration::from_secs(120);
/// The feature a phone must offer to take one.
const CAMERA: &str = "camera.continuity";

/// The composer's photo on its way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Waiting {
    /// The thread it goes to.
    pub key: String,
    pub request_id: String,
    /// The phone, by name.
    pub phone: String,
}

/// The launcher's photo on its way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForLauncher {
    /// The host's request.
    pub request: String,
    pub request_id: String,
}

/// Photos on their way, and one that came for a thread that wasn't open.
#[derive(Debug, Default)]
pub(crate) struct Photos {
    pub composer: Option<Waiting>,
    pub parked: Option<(String, ImageRef)>,
    pub launcher: Option<ForLauncher>,
    /// A capture that ended before its start was recorded here (the phone
    /// refused at once), by its request.
    early: Option<(String, Ended)>,
}

impl Photos {
    /// The composer's capture started: it's waited for, unless it ended
    /// already (the phone refused before this was recorded).
    pub(crate) fn wait_composer(
        &mut self,
        key: String,
        request_id: String,
        phone: String,
    ) -> Result<(), Ended> {
        if let Some((_, ended)) = self.early.take_if(|(id, _)| *id == request_id) {
            return Err(ended);
        }
        self.composer = Some(Waiting { key, request_id, phone });
        Ok(())
    }

    /// The launcher's capture started, as [`Self::wait_composer`].
    pub(crate) fn wait_launcher(&mut self, request: String, request_id: String) -> Result<(), Ended> {
        if let Some((_, ended)) = self.early.take_if(|(id, _)| *id == request_id) {
            return Err(ended);
        }
        self.launcher = Some(ForLauncher { request, request_id });
        Ok(())
    }

    /// The picture of capture `request_id` came for `dest`: true when it's
    /// the one waited for there (no longer waited for). Anything else is
    /// dropped by the caller.
    pub(crate) fn claim(&mut self, dest: &CaptureDest, request_id: &str) -> bool {
        match dest {
            CaptureDest::AgentComposer { key } => {
                self.composer.take_if(|w| w.request_id == request_id && w.key == *key).is_some()
            }
            CaptureDest::Launcher { request } => {
                self.launcher.take_if(|l| l.request_id == request_id && l.request == *request).is_some()
            }
            CaptureDest::Paste => false,
        }
    }

    /// Capture `request_id` ended without a picture: true when it was
    /// waited for; otherwise it's remembered for a start not recorded yet.
    pub(crate) fn end(&mut self, dest: &CaptureDest, request_id: &str, ended: &Ended) -> bool {
        let claimed = self.claim(dest, request_id);
        if !claimed {
            self.early = Some((request_id.to_owned(), ended.clone()));
        }
        claimed
    }

    /// Capture `request_id` waited [`WAIT`]: whose it was.
    pub(crate) fn expire(&mut self, request_id: &str) -> (Option<Waiting>, Option<ForLauncher>) {
        (
            self.composer.take_if(|w| w.request_id == request_id),
            self.launcher.take_if(|l| l.request_id == request_id),
        )
    }
}

// ---- Rules (pure) ----

/// Why the launcher got no photo, in the host's words.
pub(crate) fn launcher_error(ended: &Ended) -> PhotoError {
    match ended {
        Ended::NoPhone | Ended::Offline | Ended::Failed(_) => PhotoError::NoPhone,
        Ended::Cancelled | Ended::Replaced => PhotoError::Cancelled,
    }
}

/// What the composer's strip says when a photo didn't come ("" for
/// nothing): the phone went away, or it can't be reached.
pub(crate) fn strip_error(ended: &Ended) -> &'static str {
    match ended {
        Ended::NoPhone | Ended::Offline => "nophoto",
        Ended::Cancelled | Ended::Replaced | Ended::Failed(_) => "",
    }
}

/// The file's extension for the phone's picture type.
pub(crate) fn extension(mime: &str) -> &'static str {
    if mime == "image/png" { "png" } else { "jpg" }
}

/// The paired devices that can take a photo now, by their place in the
/// list `start_for` takes: connected, with Continuity Camera available.
pub(crate) fn cameras() -> Vec<(usize, DeviceId, String)> {
    let Some(host) = core_host::try_host() else { return Vec::new() };
    host.hub.read(|s| {
        s.devices
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                matches!(d.link, LinkState::Online { .. })
                    && s.matrices.get(&d.id).and_then(|m| m.state(CAMERA)) == Some(FeatureState::Available)
            })
            .map(|(i, d)| (i, d.id, s.name_of(&d.id).unwrap_or_else(|| d.info.name.clone())))
            .collect()
    })
}

// ---- The composer ----

/// "Take a photo with <phone>": the photo goes with the open thread's next
/// message. `device` is the phone's id, as the menu listed it.
pub fn take(device: String) {
    let (generation, key) = {
        let s = lock();
        let Ok((generation, room)) = super::image_room(&s) else { return };
        let Some(key) = s.open.as_ref().map(|o| o.id.clone()) else { return };
        if room == 0 {
            drop(s);
            super::image_failed(generation, "count");
            return;
        }
        (generation, key)
    };
    let Some((index, _, _)) = cameras().into_iter().find(|(_, id, _)| id.to_string() == device) else {
        super::image_failed(generation, "nophoto");
        return;
    };
    // Not under the bridge's lock: a photo it replaces is told at once.
    let started = continuity_camera::start_for(index, CaptureDest::AgentComposer { key: key.clone() });
    let started = started.and_then(|started| {
        let mut s = lock();
        let request_id = started.request_id.clone();
        s.photo.wait_composer(key, started.request_id, started.device_name)?;
        if let Some(open) = s.open.as_mut().filter(|o| o.generation == generation) {
            open.composer.image_error = "";
        }
        Ok(request_id)
    });
    match started {
        Ok(request_id) => {
            changed();
            core_host::spawn(async move {
                tokio::time::sleep(WAIT).await;
                timed_out(&request_id);
            });
        }
        Err(Ended::Failed(message)) => crate::bridge::app::show_message(message),
        Err(ended) => super::image_failed(generation, strip_error(&ended)),
    }
}

/// The strip's Cancel: the phone closes its camera.
pub fn cancel() {
    let Some(waiting) = lock().photo.composer.take() else { return };
    continuity_camera::cancel_request(&waiting.request_id, "cancelled");
    changed();
}

/// No photo after [`WAIT`]: the phone closes its camera, the strip says so
/// (or the launcher hears it).
fn timed_out(request_id: &str) {
    let (composer, launcher) = {
        let mut s = lock();
        let (composer, launcher) = s.photo.expire(request_id);
        if let Some(w) = &composer
            && let Some(open) = s.open.as_mut().filter(|o| o.is(OpenKind::Thread, &w.key))
        {
            open.composer.image_error = "nophoto";
        }
        (composer, launcher)
    };
    if composer.is_none() && launcher.is_none() {
        return;
    }
    continuity_camera::cancel_request(request_id, "timeout");
    if let Some(l) = launcher {
        done(l.request, Err(PhotoError::Timeout));
    }
    changed();
}

/// What the strip shows for the open thread: the phone it waits for.
pub(crate) fn waiting_for<'a>(photos: &'a Photos, key: &str) -> Option<&'a str> {
    photos.composer.as_ref().filter(|w| w.key == key).map(|w| w.phone.as_str())
}

/// A thread opens: the photo that came for it meanwhile joins its composer.
pub(crate) fn unpark(s: &mut super::BridgeState, generation: u64) {
    let Some(open) = s.open.as_mut().filter(|o| o.generation == generation && o.kind == OpenKind::Thread)
    else {
        return;
    };
    if s.photo.parked.as_ref().is_none_or(|(key, _)| *key != open.id) {
        return;
    }
    let Some((_, image)) = s.photo.parked.take() else { return };
    if open.composer.images.len() < api::MAX_IMAGES {
        let id = image.id.clone();
        open.composer.images.push(image);
        core_host::spawn(super::read_thumb(id));
    } else if let Some(client) = s.client.clone() {
        // No room: the host needn't keep it (not under the lock it's called with).
        core_host::spawn(async move {
            let _ = client.image_drop(image.id).await;
        });
    }
}

// ---- The launcher ----

/// The host's `ui.photo`: one photo from the first phone that can take one.
pub(crate) fn launcher(request: String) {
    if !api::is_valid_photo_request(&request) {
        return;
    }
    let previous = lock().photo.launcher.take();
    if let Some(previous) = previous {
        continuity_camera::cancel_request(&previous.request_id, "cancelled");
    }
    let Some((index, _, _)) = cameras().into_iter().next() else {
        done(request, Err(PhotoError::NoPhone));
        return;
    };
    let started = continuity_camera::start_for(index, CaptureDest::Launcher { request: request.clone() });
    let started = started.and_then(|started| {
        let request_id = started.request_id.clone();
        lock().photo.wait_launcher(request.clone(), started.request_id)?;
        Ok(request_id)
    });
    match started {
        Ok(request_id) => core_host::spawn(async move {
            tokio::time::sleep(WAIT).await;
            timed_out(&request_id);
        }),
        Err(ended) => done(request, Err(launcher_error(&ended))),
    }
}

/// The host's `ui.photo.cancel`: the launcher stopped waiting.
pub(crate) fn launcher_cancel(request: &str) {
    let taken = {
        let mut s = lock();
        if s.photo.launcher.as_ref().is_some_and(|l| l.request == request) {
            s.photo.launcher.take()
        } else {
            None
        }
    };
    if let Some(l) = taken {
        continuity_camera::cancel_request(&l.request_id, "cancelled");
    }
}

/// The connection the launcher's request came on closed.
pub(crate) fn disconnected() {
    if let Some(l) = lock().photo.launcher.take() {
        continuity_camera::cancel_request(&l.request_id, "cancelled");
    }
}

/// Answers the launcher's request on the host. A photo the launcher no
/// longer waits for is dropped there.
fn done(request: String, result: Result<String, PhotoError>) {
    let Some(client) = lock().client.clone() else { return };
    let (image, error) = match result {
        Ok(id) => (Some(id), None),
        Err(e) => (None, Some(e)),
    };
    core_host::spawn(async move {
        let params = UiPhotoDoneParams { request, image: image.clone(), error };
        if let Err(e) = client.ui_photo_done(params).await {
            tracing::debug!(error = %e, "the launcher's photo wasn't answered");
            if let Some(id) = image.filter(|_| e.code() == Some(ErrorCode::NotFound)) {
                let _ = client.image_drop(id).await;
            }
        }
    });
}

// ---- From the phone ----

/// The phone's picture for `dest` (continuity camera's capture `request_id`).
pub(crate) fn arrived(dest: &CaptureDest, request_id: &str, mime: &str, data: &[u8]) {
    match dest {
        CaptureDest::AgentComposer { key } => {
            let target = {
                let mut s = lock();
                // Late, cancelled or another's: dropped, never pasted.
                if !s.photo.claim(dest, request_id) {
                    return;
                }
                match s.open.as_mut().filter(|o| o.is(OpenKind::Thread, key)) {
                    Some(open) => {
                        open.composer.adding += 1;
                        open.composer.image_error = "";
                        Some(open.generation)
                    }
                    None => None,
                }
            };
            changed();
            let (key, ext, bytes) = (key.clone(), extension(mime), data.to_vec());
            core_host::spawn(async move {
                let Ok(file) = write(bytes, ext).await else {
                    if let Some(generation) = target {
                        super::finish_adding(generation, Err("failed"));
                    }
                    return;
                };
                let path = file.path().to_string_lossy().into_owned();
                match target {
                    Some(generation) => super::add_image(generation, path).await,
                    None => park(key, path).await,
                }
                // The file goes now that the host has read it (or refused it).
                drop(file);
            });
        }
        CaptureDest::Launcher { request } => {
            let client = {
                let mut s = lock();
                if !s.photo.claim(dest, request_id) {
                    return;
                }
                s.client.clone()
            };
            let Some(client) = client else { return };
            let request = request.clone();
            let (ext, bytes) = (extension(mime), data.to_vec());
            core_host::spawn(async move {
                let added = match write(bytes, ext).await {
                    Ok(file) => {
                        let added = add(&client, file.path().to_string_lossy().into_owned()).await;
                        drop(file);
                        added
                    }
                    Err(_) => Err("failed"),
                };
                done(request, added.map(|image| image.id).map_err(|_| PhotoError::Cancelled));
            });
        }
        CaptureDest::Paste => {}
    }
}

/// The capture for `dest` ended without a picture.
pub(crate) fn ended(dest: &CaptureDest, request_id: &str, ended: Ended) {
    match dest {
        CaptureDest::AgentComposer { key } => {
            {
                let mut s = lock();
                if !s.photo.end(dest, request_id, &ended) {
                    return;
                }
                let word = strip_error(&ended);
                if let Some(open) =
                    s.open.as_mut().filter(|o| o.is(OpenKind::Thread, key) && !word.is_empty())
                {
                    open.composer.image_error = word;
                }
            }
            if let Ended::Failed(message) = ended {
                crate::bridge::app::show_message(message);
            }
            changed();
        }
        CaptureDest::Launcher { request } => {
            if lock().photo.end(dest, request_id, &ended) {
                done(request.clone(), Err(launcher_error(&ended)));
            }
        }
        CaptureDest::Paste => {}
    }
}

/// The picture in a file only this user can read, for `image.add_path`.
async fn write(bytes: Vec<u8>, ext: &'static str) -> std::io::Result<control::PastedImage> {
    if bytes.len() as u64 > api::MAX_IMAGE_FILE {
        return Err(std::io::Error::other("too large"));
    }
    let written = tokio::task::spawn_blocking(move || {
        let dir = control::paste_dir().ok_or_else(|| std::io::Error::other("no runtime directory"))?;
        control::PastedImage::write_as(&dir, "agent-photo", ext, &bytes)
    })
    .await
    .unwrap_or_else(|e| Err(std::io::Error::other(e)));
    if let Err(error) = &written {
        tracing::warn!(%error, "a phone's photo couldn't be passed on");
    }
    written
}

async fn add(client: &Client, path: String) -> Result<ImageRef, &'static str> {
    client.image_add_path(path).await.map_err(|e| match e.code() {
        Some(ErrorCode::BadRequest) => "type",
        Some(ErrorCode::Limit) => "size",
        _ => "failed",
    })
}

/// A photo for thread `key` that isn't open: kept for when it opens.
async fn park(key: String, path: String) {
    let Some(client) = lock().client.clone() else { return };
    let Ok(image) = add(&client, path).await else { return };
    let (dropped, generation) = {
        let mut s = lock();
        let dropped = s.photo.parked.replace((key, image)).map(|(_, old)| old.id);
        // It may have opened meanwhile.
        (dropped, s.open.as_ref().map(|o| o.generation))
    };
    if let Some(id) = dropped {
        super::drop_image_on_host(id);
    }
    if let Some(generation) = generation {
        unpark(&mut lock(), generation);
        changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launcher_hears_why_no_photo_came_in_the_hosts_words() {
        assert_eq!(launcher_error(&Ended::NoPhone), PhotoError::NoPhone);
        assert_eq!(launcher_error(&Ended::Offline), PhotoError::NoPhone);
        assert_eq!(launcher_error(&Ended::Failed("x".into())), PhotoError::NoPhone);
        assert_eq!(launcher_error(&Ended::Cancelled), PhotoError::Cancelled);
        assert_eq!(launcher_error(&Ended::Replaced), PhotoError::Cancelled);
    }

    #[test]
    fn the_strip_says_no_photo_arrived_only_when_the_phone_went_away() {
        assert_eq!(strip_error(&Ended::Offline), "nophoto");
        assert_eq!(strip_error(&Ended::NoPhone), "nophoto");
        // Cancelled on the phone, another capture, or a message of its own: nothing here.
        assert_eq!(strip_error(&Ended::Cancelled), "");
        assert_eq!(strip_error(&Ended::Replaced), "");
        assert_eq!(strip_error(&Ended::Failed("Allow Photos access".into())), "");
    }

    #[test]
    fn a_photo_keeps_its_type_in_its_file_name() {
        assert_eq!(extension("image/png"), "png");
        assert_eq!(extension("image/jpeg"), "jpg");
        assert_eq!(extension(""), "jpg");
    }

    fn composer(key: &str) -> CaptureDest {
        CaptureDest::AgentComposer { key: key.into() }
    }

    #[test]
    fn a_photo_reaches_the_thread_it_was_taken_for_once() {
        let (a, b) = ("a".repeat(32), "b".repeat(32));
        let mut photos = Photos::default();
        photos.wait_composer(a.clone(), "cc-1-1".into(), "Pixel".into()).unwrap();
        assert!(!photos.claim(&composer(&b), "cc-1-1"), "not another thread's");
        assert!(!photos.claim(&composer(&a), "cc-1-2"), "not another capture's");
        assert!(!photos.claim(&CaptureDest::Paste, "cc-1-1"));
        assert!(photos.claim(&composer(&a), "cc-1-1"));
        assert!(!photos.claim(&composer(&a), "cc-1-1"), "once");
        assert_eq!(waiting_for(&photos, &a), None, "the strip stops waiting");
    }

    #[test]
    fn after_two_minutes_a_photo_reaches_nothing() {
        let a = "a".repeat(32);
        let mut photos = Photos::default();
        photos.wait_composer(a.clone(), "cc-1-1".into(), "Pixel".into()).unwrap();
        photos.wait_launcher("0123456789abcdef".into(), "cc-1-2".into()).unwrap();
        let (waited, launcher) = photos.expire("cc-1-1");
        assert_eq!(waited.map(|w| w.key), Some(a.clone()));
        assert_eq!(launcher, None, "only that capture's");
        // It arrives late: dropped by the caller (an agent's capture never reaches the clipboard).
        assert!(!photos.claim(&composer(&a), "cc-1-1"));
        let (_, launcher) = photos.expire("cc-1-2");
        assert_eq!(launcher.map(|l| l.request), Some("0123456789abcdef".into()));
        assert_eq!(photos.expire("cc-1-2"), (None, None));
    }

    #[test]
    fn a_capture_that_ends_before_it_is_recorded_is_not_waited_for() {
        let a = "a".repeat(32);
        let mut photos = Photos::default();
        // The phone refused at once, before the start was recorded.
        assert!(!photos.end(&composer(&a), "cc-1-1", &Ended::Offline));
        assert_eq!(photos.wait_composer(a.clone(), "cc-1-1".into(), "Pixel".into()), Err(Ended::Offline));
        assert_eq!(waiting_for(&photos, &a), None);
        // A later one waits as usual.
        photos.wait_composer(a.clone(), "cc-1-2".into(), "Pixel".into()).unwrap();
        assert!(photos.end(&composer(&a), "cc-1-2", &Ended::Cancelled));
        assert_eq!(waiting_for(&photos, &a), None);
    }

    #[test]
    fn the_launchers_photo_answers_only_its_own_request() {
        let launcher = |request: &str| CaptureDest::Launcher { request: request.into() };
        let mut photos = Photos::default();
        photos.wait_launcher("0123456789abcdef".into(), "cc-1-1".into()).unwrap();
        assert!(!photos.claim(&launcher("fedcba9876543210"), "cc-1-1"));
        assert!(!photos.claim(&composer(&"a".repeat(32)), "cc-1-1"));
        assert!(photos.claim(&launcher("0123456789abcdef"), "cc-1-1"));
        assert!(photos.launcher.is_none());
    }

    #[test]
    fn the_strip_waits_only_in_the_thread_it_is_for() {
        let a = "a".repeat(32);
        let photos = Photos {
            composer: Some(Waiting { key: a.clone(), request_id: "cc-1-1".into(), phone: "Pixel".into() }),
            ..Photos::default()
        };
        assert_eq!(waiting_for(&photos, &a), Some("Pixel"));
        assert_eq!(waiting_for(&photos, &"b".repeat(32)), None);
        assert_eq!(waiting_for(&Photos::default(), &a), None);
    }
}
