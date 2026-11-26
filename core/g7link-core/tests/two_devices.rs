// SPDX-License-Identifier: MPL-2.0
//! End-to-end tests: two real nodes talking over localhost.

use std::{
    future::Future,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use g7link_core::{
    Battery, DeviceInfo, DeviceKind, Direction, Error, FeatureState, FileSource, LinkState, MediaAction,
    MediaError, MediaPlayer, Node, NodeConfig, NodeEvent, Notification, NotificationAction,
    NotificationError, OutgoingFile, PairingEvent, PairingFailure, PairingUri, PcCaptureError,
    PlainKeyProtector, Platform, PowerAction, PowerLevel, Transfer, TransferFailure, TransferState,
    features::{Effort, Upgrade, UpgradeAction},
};
use tempfile::TempDir;
use tokio::sync::broadcast::Receiver;

const WAIT: Duration = Duration::from_secs(20);

type MmsRecord = (Vec<String>, String, Vec<g7link_core::SmsAttachment>);
type RichClipboardRecord = (String, Option<String>, Option<String>);

/// Holds platform calls until opened, counting the ones it holds.
#[derive(Debug, Default)]
struct Gate {
    open: Mutex<bool>,
    opened: std::sync::Condvar,
    held: std::sync::atomic::AtomicUsize,
}

impl Gate {
    fn pass(&self) {
        self.held.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.opened.wait(open).unwrap();
        }
    }

    fn held(&self) -> usize {
        self.held.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
}

#[derive(Debug, Default)]
struct RecordingPlatform {
    rings: Mutex<Vec<bool>>,
    dismissed: Mutex<Vec<String>>,
    actions: Mutex<Vec<(String, String, Option<String>)>>,
    clipboard: Mutex<Vec<String>>,
    clipboard_private: Mutex<Vec<String>>,
    clipboard_cleared: Mutex<Vec<String>>,
    clipboard_rich: Mutex<Vec<RichClipboardRecord>>,
    images: Mutex<Vec<(String, Vec<u8>)>>,
    media: Mutex<Vec<(String, MediaAction, Option<u64>)>>,
    power: Mutex<Vec<PowerAction>>,
    links: Mutex<Vec<String>>,
    /// Whether each link in `links` came as shared from the sender's share sheet.
    links_shared: Mutex<Vec<bool>>,
    calls: Mutex<Vec<(String, g7link_core::CallCommand)>>,
    dialed: Mutex<Vec<String>>,
    texts: Mutex<Vec<(Vec<String>, String)>>,
    mms_sent: Mutex<Vec<MmsRecord>>,
    mirror_asks: Mutex<Vec<String>>,
    mirror_reject: std::sync::atomic::AtomicBool,
    /// The phone's lock screen is up.
    phone_locked: std::sync::atomic::AtomicBool,
    watch_offers: Mutex<Vec<String>>,
    mic_asks: Mutex<Vec<String>>,
    mic: Arc<TestMicSink>,
    /// What a PC got of a phone's screen.
    screen: Arc<ScreenSink>,
    webcam_asks: Mutex<Vec<String>>,
    webcam: Arc<TestWebcamSink>,
    camera_asks: Mutex<Vec<String>>,
    /// Photos `open_photo` finds, by ID.
    photos: Mutex<std::collections::HashMap<String, std::path::PathBuf>>,
    /// Optional barrier `open_photo` waits on to simulate a slow platform call.
    photo_barrier: Mutex<Option<Arc<std::sync::Barrier>>>,
    /// While set, `contacts` waits for it to open.
    contacts_gate: Mutex<Option<Arc<Gate>>>,
    /// While set, `dismiss_notification` waits for it to open (after recording the key).
    dismiss_gate: Mutex<Option<Arc<Gate>>>,
    remote_inputs: Mutex<Vec<g7link_core::RemoteInput>>,
    phone_toggles: Mutex<Vec<(String, g7link_core::PhoneToggleValue)>>,
    deck_presses: Mutex<Vec<String>>,
    pc_audio_changes: Mutex<Vec<(Option<u8>, Option<bool>)>>,
    pc_audio_outputs: Mutex<Vec<String>>,
    tasks: Mutex<Vec<g7link_core::TaskNotify>>,
    storage: Mutex<Option<g7link_core::FolderStorage>>,
    /// What a phone asked of this PC's screen: `shot`, `record`, `stop`.
    capture_asks: Mutex<Vec<String>>,
    /// The file a screenshot or a stopped recording is saved to; without one
    /// the PC answers that it's locked.
    capture_file: Mutex<Option<std::path::PathBuf>>,
    /// Whether taking one shows the screen-sharing dialog first.
    capture_prompts: std::sync::atomic::AtomicBool,
    /// What phones asked of this PC's agents, and by whom.
    agent_requests: Mutex<Vec<(g7link_core::DeviceId, g7link_core::AgentRequest)>>,
    /// Ask tokens already answered.
    agent_answered: Mutex<std::collections::HashSet<String>>,
    /// The sessions this PC lists (`agent.sessions`).
    agent_sessions: Mutex<Vec<g7link_core::AgentSession>>,
    /// Drafts this PC keeps (`agent.draft.*`), by key.
    agent_drafts: Mutex<std::collections::HashMap<String, g7link_core::AgentDraft>>,
    /// While set, an image a phone sent waits for it to open.
    agent_image_gate: Mutex<Option<Arc<Gate>>>,
    /// Values sent with media commands (shuffle, loop).
    media_values: Mutex<Vec<(String, MediaAction, String)>>,
    /// The large picture `media_art` serves, and how often it was asked.
    media_art: Mutex<Option<(Vec<u8>, g7link_core::ArtFormat)>>,
    media_art_asks: AtomicUsize,
    /// The icon `media_icon` serves.
    media_icon: Mutex<Option<Vec<u8>>>,
    /// How long a platform call takes, by hook name: a slow platform, held
    /// with `std::thread::sleep` inside the call.
    delays: Mutex<std::collections::HashMap<&'static str, Duration>>,
    /// When set, what `contacts` lists instead of its fixed few.
    contacts_list: Mutex<Option<Vec<g7link_core::Contact>>>,
}

impl RecordingPlatform {
    /// Takes as long as `hook` was told to.
    fn slow(&self, hook: &'static str) {
        let delay = self.delays.lock().unwrap().get(hook).copied();
        if let Some(delay) = delay {
            std::thread::sleep(delay);
        }
    }

    fn set_delay(&self, hook: &'static str, delay: Duration) {
        self.delays.lock().unwrap().insert(hook, delay);
    }
}

impl Platform for RecordingPlatform {
    fn start_ringing(&self) {
        self.rings.lock().unwrap().push(true);
    }
    fn stop_ringing(&self) {
        self.rings.lock().unwrap().push(false);
    }
    fn dismiss_notification(&self, key: &str) -> Result<(), NotificationError> {
        self.dismissed.lock().unwrap().push(key.to_owned());
        let gate = self.dismiss_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.pass();
        }
        Ok(())
    }
    fn run_notification_action(
        &self,
        key: &str,
        action: &str,
        reply: Option<&str>,
    ) -> Result<(), NotificationError> {
        if key == "gone" {
            return Err(NotificationError::NotFound);
        }
        self.actions.lock().unwrap().push((key.to_owned(), action.to_owned(), reply.map(str::to_owned)));
        Ok(())
    }
    fn set_clipboard(&self, text: &str) -> Result<(), String> {
        self.clipboard.lock().unwrap().push(text.to_owned());
        Ok(())
    }
    fn set_clipboard_rich(&self, text: &str, html: Option<&str>, rtf: Option<&str>) -> Result<(), String> {
        self.slow("set_clipboard");
        self.clipboard.lock().unwrap().push(text.to_owned());
        self.clipboard_rich.lock().unwrap().push((
            text.to_owned(),
            html.map(str::to_owned),
            rtf.map(str::to_owned),
        ));
        Ok(())
    }
    fn set_clipboard_private(&self, text: &str) -> Result<(), String> {
        self.clipboard_private.lock().unwrap().push(text.to_owned());
        Ok(())
    }
    fn clear_clipboard_private(&self, text: &str) {
        self.clipboard_cleared.lock().unwrap().push(text.to_owned());
    }
    fn set_clipboard_image(&self, mime: &str, bytes: &[u8]) -> Result<(), String> {
        self.images.lock().unwrap().push((mime.to_owned(), bytes.to_vec()));
        Ok(())
    }
    fn power(&self, action: PowerAction) -> Result<(), String> {
        self.power.lock().unwrap().push(action);
        Ok(())
    }
    fn open_link(&self, _from: &g7link_core::DeviceId, url: &str, user_shared: bool) -> Result<(), String> {
        self.links.lock().unwrap().push(url.to_owned());
        self.links_shared.lock().unwrap().push(user_shared);
        Ok(())
    }
    fn task_notify(
        &self,
        _from: &g7link_core::DeviceId,
        task: &g7link_core::TaskNotify,
    ) -> Result<(), String> {
        self.tasks.lock().unwrap().push(task.clone());
        Ok(())
    }
    fn mirror_sink(
        &self,
        _peer: &g7link_core::DeviceId,
        session: u32,
    ) -> Option<Arc<dyn g7link_core::MirrorSink>> {
        self.mirror_asks.lock().unwrap().push(format!("sink {session}"));
        Some(self.screen.clone())
    }
    fn phone_locked(&self) -> bool {
        self.phone_locked.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn mirror_requested(
        &self,
        _peer: &g7link_core::DeviceId,
        options: &g7link_core::MirrorStart,
    ) -> Result<(), String> {
        if self.mirror_reject.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("consent declined".into());
        }
        let sound = if options.audio { " with sound" } else { "" };
        let app = options.app.as_deref().map(|a| format!(" {a} as {}", options.session)).unwrap_or_default();
        let watch = options.watch.as_deref().map(|p| format!(" watching {p}")).unwrap_or_default();
        let only = if options.sound_only { ", sound only" } else { "" };
        self.mirror_asks.lock().unwrap().push(format!("start {}{sound}{only}{app}{watch}", options.max_size));
        Ok(())
    }
    fn watch_offered(
        &self,
        _from: &g7link_core::DeviceId,
        player: &str,
        title: Option<&str>,
        file: Option<&g7link_core::WatchFile>,
    ) -> Result<(), String> {
        let file = file.map(|f| format!(" [{} {} at {}]", f.name, f.size, f.position)).unwrap_or_default();
        self.watch_offers.lock().unwrap().push(format!("{player}: {}{file}", title.unwrap_or("")));
        Ok(())
    }
    fn mirror_stop_requested(&self, _peer: &g7link_core::DeviceId, session: u32) {
        self.mirror_asks.lock().unwrap().push(format!("stop {session}"));
    }
    fn mirror_keyframe_requested(&self, _peer: &g7link_core::DeviceId, session: u32) {
        self.mirror_asks.lock().unwrap().push(format!("keyframe {session}"));
    }
    fn mirror_resize_requested(&self, _peer: &g7link_core::DeviceId, session: u32, width: u32, height: u32) {
        self.mirror_asks.lock().unwrap().push(format!("resize {session} {width}x{height}"));
    }
    fn mirror_sound_requested(&self, _peer: &g7link_core::DeviceId, on: bool) {
        self.mirror_asks.lock().unwrap().push(format!("sound on={on}"));
    }
    fn mirror_power_requested(&self, _peer: &g7link_core::DeviceId, stay_awake: bool, screen_off: bool) {
        self.mirror_asks.lock().unwrap().push(format!("power awake={stay_awake} off={screen_off}"));
    }
    fn mirror_input(&self, _peer: &g7link_core::DeviceId, session: u32, input: g7link_core::MirrorInput) {
        self.mirror_asks.lock().unwrap().push(format!("{input:?} on {session}"));
    }
    fn mic_sink(&self, _peer: &g7link_core::DeviceId) -> Option<Arc<dyn g7link_core::MicSink>> {
        Some(self.mic.clone())
    }
    fn mic_requested(&self, _peer: &g7link_core::DeviceId) -> Result<(), String> {
        self.mic_asks.lock().unwrap().push("start".into());
        Ok(())
    }
    fn mic_stop_requested(&self, _peer: &g7link_core::DeviceId) {
        self.mic_asks.lock().unwrap().push("stop".into());
    }
    fn webcam_sink(&self, _peer: &g7link_core::DeviceId) -> Option<Arc<dyn g7link_core::WebcamSink>> {
        self.webcam_asks.lock().unwrap().push("sink".into());
        Some(self.webcam.clone())
    }
    fn webcam_requested(
        &self,
        _peer: &g7link_core::DeviceId,
        options: &g7link_core::WebcamStart,
    ) -> Result<(), String> {
        self.webcam_asks
            .lock()
            .unwrap()
            .push(format!("start {} {}x{} @ {}", options.camera, options.width, options.height, options.fps));
        Ok(())
    }
    fn webcam_stop_requested(&self, _peer: &g7link_core::DeviceId) {
        self.webcam_asks.lock().unwrap().push("stop".into());
    }
    fn webcam_keyframe_requested(&self, _peer: &g7link_core::DeviceId) {
        self.webcam_asks.lock().unwrap().push("keyframe".into());
    }
    fn camera_capture_requested(
        &self,
        _peer: &g7link_core::DeviceId,
        request: &g7link_core::CameraCaptureRequest,
    ) -> Result<(), String> {
        self.camera_asks.lock().unwrap().push(format!("request {} {:?}", request.request_id, request.mode));
        Ok(())
    }
    fn camera_capture_cancelled(&self, _peer: &g7link_core::DeviceId, request_id: &str) {
        self.camera_asks.lock().unwrap().push(format!("cancel {request_id}"));
    }
    fn phone_apps(&self) -> Result<Vec<g7link_core::PhoneApp>, String> {
        let app = |pkg: &str, label: &str, icon: usize| g7link_core::PhoneApp {
            pkg: pkg.into(),
            label: label.into(),
            icon: Some(vec![1; icon]),
        };
        Ok(vec![
            app("com.example.notes", "notes", 100),
            app("com.example.chat", "Chat", 100),
            // Not a package name: never offered.
            app("not a package", "Bad", 10),
            // Too big an icon: shown without.
            app("com.example.maps", "Maps", 9 * 1024),
        ])
    }
    fn sms_threads(&self, limit: u32) -> Result<Vec<g7link_core::SmsThread>, String> {
        let thread = |i: u32| g7link_core::SmsThread {
            id: i.to_string(),
            addresses: vec![format!("+1555010{i}")],
            names: vec![],
            snippet: format!("text {i}"),
            date: 1_790_000_000_000 - i64::from(i),
            unread: i % 2,
            photo: None,
        };
        Ok((0..5).map(thread).take(limit as usize).collect())
    }
    fn sms_app(&self) -> Option<String> {
        Some("com.example.messaging".into())
    }
    fn sms_messages(
        &self,
        thread: &str,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<g7link_core::SmsMessage>, String> {
        let message = |date: i64| g7link_core::SmsMessage {
            id: date.to_string(),
            thread: thread.to_owned(),
            address: "+15550100".into(),
            body: format!("at {date}"),
            date,
            outgoing: date % 2 == 0,
            status: None,
            parts: vec![g7link_core::SmsPart { id: "p1".into(), mime: "image/jpeg".into(), size: 3 }],
        };
        let newest = before.unwrap_or(100);
        Ok((0..newest).rev().take(limit as usize).map(message).collect())
    }
    fn sms_send(&self, to: &[String], body: &str) -> Result<(), String> {
        self.texts.lock().unwrap().push((to.to_vec(), body.to_owned()));
        Ok(())
    }
    fn send_sms_with_attachments(
        &self,
        to: &[String],
        body: &str,
        attachments: &[g7link_core::SmsAttachment],
    ) -> Result<(), String> {
        if attachments.is_empty() {
            return self.sms_send(to, body);
        }
        self.mms_sent.lock().unwrap().push((to.to_vec(), body.to_owned(), attachments.to_vec()));
        Ok(())
    }
    fn sms_part(&self, id: &str) -> Result<(String, Vec<u8>), String> {
        self.slow("sms_part");
        if id == "big" {
            return Ok(("image/jpeg".into(), vec![7; 900 * 1024]));
        }
        if id == "p1" { Ok(("image/jpeg".into(), vec![1, 2, 3])) } else { Err("gone".into()) }
    }
    fn call_command(&self, id: &str, command: g7link_core::CallCommand) -> Result<(), String> {
        self.slow("call_command");
        self.calls.lock().unwrap().push((id.to_owned(), command));
        Ok(())
    }
    fn call_log(&self, before: Option<i64>, limit: u32) -> Result<Vec<g7link_core::CallLogEntry>, String> {
        let entry = |date: i64| g7link_core::CallLogEntry {
            id: format!("log:{date}"),
            number: format!("+1555010{date}"),
            name: (date % 2 == 0).then(|| format!("Caller {date}")),
            direction: match date % 4 {
                0 => "incoming",
                1 => "outgoing",
                2 => "missed",
                _ => "rejected",
            }
            .into(),
            date,
            duration: if date % 2 == 0 { 42 } else { 0 },
            // Entry 3 has an oversized photo that the core should drop.
            photo: match date {
                4 => Some(vec![0xff, 0xd8]),
                3 => Some(vec![0; 20 * 1024]),
                _ => None,
            },
        };
        let newest = before.unwrap_or(5);
        Ok((0..newest).rev().take(limit as usize).map(entry).collect())
    }
    fn call_dial(&self, number: &str) -> Result<(), String> {
        self.dialed.lock().unwrap().push(number.to_owned());
        Ok(())
    }
    fn call_sims(&self) -> Result<g7link_core::CallSims, String> {
        Ok(g7link_core::CallSims {
            sims: vec![
                g7link_core::CallSim { id: "1".into(), label: "SIM 1 · Airtel".into() },
                g7link_core::CallSim { id: "2".into(), label: "SIM 2 · Jio".into() },
            ],
            default: None,
        })
    }
    fn call_dial_on(&self, number: &str, sim: &str) -> Result<(), String> {
        self.dialed.lock().unwrap().push(format!("{number} on {sim}"));
        Ok(())
    }
    fn contacts(
        &self,
        query: Option<&str>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<g7link_core::Contact>, String> {
        use g7link_core::{Contact, ContactNumber};
        let gate = self.contacts_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.pass();
        }
        self.slow("contacts");
        if let Some(list) = self.contacts_list.lock().unwrap().clone() {
            return Ok(list);
        }
        let all = vec![
            Contact {
                id: "c1".into(),
                name: "Sam Rivera".into(),
                numbers: vec![ContactNumber { number: "+15550100".into(), label: Some("Mobile".into()) }],
                starred: true,
                photo: Some(vec![0xff, 0xd8]),
            },
            Contact {
                id: "c2".into(),
                name: "Alex Chen".into(),
                numbers: vec![ContactNumber { number: "+15550188".into(), label: Some("Mobile".into()) }],
                starred: true,
                // Oversized photo: should be dropped by core rather than dropping the contact.
                photo: Some(vec![0; 20 * 1024]),
            },
            Contact {
                id: "c3".into(),
                name: "Jordan Patel".into(),
                numbers: vec![ContactNumber { number: "+15550155".into(), label: Some("Work".into()) }],
                starred: false,
                photo: None,
            },
            Contact {
                id: "c4".into(),
                name: "Priya Nair".into(),
                numbers: vec![ContactNumber { number: "+15550172".into(), label: None }],
                starred: false,
                photo: None,
            },
        ];
        let q = query.map(str::to_lowercase);
        Ok(all
            .into_iter()
            .filter(|c| match &q {
                None => true,
                Some(q) => {
                    c.name.to_lowercase().contains(q)
                        || c.numbers.iter().any(|n| n.number.to_lowercase().contains(q))
                }
            })
            .skip(offset as usize)
            .take(limit as usize)
            .collect())
    }
    fn open_photo(&self, id: &str) -> Result<OutgoingFile, String> {
        let barrier = self.photo_barrier.lock().unwrap().clone();
        if let Some(barrier) = barrier {
            barrier.wait();
            barrier.wait();
        }
        let path = self.photos.lock().unwrap().get(id).cloned().ok_or("gone")?;
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        Ok(OutgoingFile { name, folder: None, source: FileSource::Path(path) })
    }
    fn photo_albums(&self) -> Result<Vec<g7link_core::PhotoAlbum>, String> {
        use g7link_core::PhotoAlbum;
        Ok(vec![
            PhotoAlbum { id: "cam".into(), name: "Camera".into(), count: 3, cover: Some("media:4".into()) },
            PhotoAlbum {
                id: "shots".into(),
                name: "Screenshots".into(),
                count: 2,
                cover: Some("media:3".into()),
            },
        ])
    }
    fn photo_list(
        &self,
        album: Option<&str>,
        before: Option<(i64, &str)>,
        limit: u32,
    ) -> Result<Vec<g7link_core::PhotoItem>, String> {
        use g7link_core::PhotoItem;
        let all = vec![
            PhotoItem {
                id: "media:4".into(),
                name: "IMG_4.jpg".into(),
                date: 4_000,
                size: 100,
                width: 1920,
                height: 1080,
                duration: None,
                album: Some("cam".into()),
            },
            PhotoItem {
                id: "media:3".into(),
                name: "Screenshot_3.png".into(),
                date: 3_000,
                size: 80,
                width: 1080,
                height: 2400,
                duration: None,
                album: Some("shots".into()),
            },
            PhotoItem {
                id: "media:2".into(),
                name: "VID_2.mp4".into(),
                // Taken the same millisecond as media:3, across a page break.
                date: 3_000,
                size: 5_000,
                width: 1920,
                height: 1080,
                duration: Some(14),
                album: Some("cam".into()),
            },
            PhotoItem {
                id: "media:1".into(),
                name: "IMG_1.jpg".into(),
                date: 1_000,
                size: 90,
                width: 1920,
                height: 1080,
                duration: None,
                album: Some("cam".into()),
            },
            PhotoItem {
                id: "media:0".into(),
                name: "Screenshot_0.png".into(),
                date: 500,
                size: 70,
                width: 1080,
                height: 2400,
                duration: None,
                album: Some("shots".into()),
            },
        ];
        Ok(all
            .into_iter()
            .filter(|i| album.is_none_or(|a| i.album.as_deref() == Some(a)))
            .filter(|i| before.is_none_or(|(date, id)| i.date < date || (i.date == date && *i.id < *id)))
            .take(limit as usize)
            .collect())
    }
    fn photo_delete(&self, ids: &[String]) -> Result<u32, String> {
        Ok(ids.iter().filter(|id| id.as_str() != "gone").count() as u32)
    }
    fn photo_thumbs(&self, ids: &[String]) -> Result<Vec<g7link_core::PhotoThumb>, String> {
        self.slow("photo_thumbs");
        use g7link_core::PhotoThumb;
        Ok(ids
            .iter()
            .filter_map(|id| match id.as_str() {
                "media:4" | "media:3" | "media:2" | "media:1" | "media:0" => Some(PhotoThumb {
                    id: id.clone(),
                    data: vec![0xff, 0xd8, id.as_bytes().last().copied().unwrap()],
                }),
                "oversized" => {
                    Some(PhotoThumb { id: id.clone(), data: vec![0; g7link_core::PHOTO_MAX_THUMB_BYTES + 1] })
                }
                _ => None,
            })
            .collect())
    }
    fn media_command(
        &self,
        player: &str,
        action: MediaAction,
        position: Option<u64>,
        value: Option<&str>,
    ) -> Result<(), MediaError> {
        if player == "gone" {
            return Err(MediaError::NotFound);
        }
        self.slow("media_command");
        self.media.lock().unwrap().push((player.to_owned(), action, position));
        if let Some(value) = value {
            self.media_values.lock().unwrap().push((player.to_owned(), action, value.to_owned()));
        }
        Ok(())
    }
    fn media_art(&self, _key: &str) -> Option<(Vec<u8>, g7link_core::ArtFormat)> {
        self.media_art_asks.fetch_add(1, Ordering::SeqCst);
        self.media_art.lock().unwrap().clone()
    }
    fn media_icon(&self, _key: &str) -> Option<Vec<u8>> {
        self.media_icon.lock().unwrap().clone()
    }
    fn remote_input(
        &self,
        _peer: &g7link_core::DeviceId,
        input: g7link_core::RemoteInput,
    ) -> Result<(), String> {
        self.slow("remote_input");
        self.remote_inputs.lock().unwrap().push(input);
        Ok(())
    }
    fn set_phone_toggle(&self, id: &str, value: &g7link_core::PhoneToggleValue) -> Result<(), String> {
        self.phone_toggles.lock().unwrap().push((id.to_owned(), value.clone()));
        Ok(())
    }
    fn deck_press(&self, _from: &g7link_core::DeviceId, tile: &str) -> Result<(), String> {
        self.deck_presses.lock().unwrap().push(tile.to_owned());
        Ok(())
    }
    fn agent_request(
        &self,
        from: &g7link_core::DeviceId,
        request: g7link_core::AgentRequest,
    ) -> Result<g7link_core::AgentReply, g7link_core::AgentFailure> {
        use g7link_core::{AgentFailure, AgentReply, AgentRequest};
        self.agent_requests.lock().unwrap().push((*from, request.clone()));
        match request {
            AgentRequest::SessionRead { .. } | AgentRequest::Read { .. } => self.slow("agent_read"),
            _ => self.slow("agent_request"),
        }
        match request {
            AgentRequest::Threads => {
                Ok(AgentReply::Threads(vec![agent_thread(AGENT_THREAD, "working", true)]))
            }
            AgentRequest::Start { .. } => Ok(AgentReply::Thread(agent_thread(AGENT_THREAD, "working", true))),
            AgentRequest::Send { thread, .. } if thread == BUSY_THREAD => Err(AgentFailure::Busy),
            AgentRequest::Send { .. } | AgentRequest::Cancel { .. } => Ok(AgentReply::Ok),
            AgentRequest::Read { thread, .. } if thread == THINKING_THREAD => {
                Ok(AgentReply::Items(g7link_core::AgentItems {
                    thread: agent_thread(&thread, "working", false),
                    items: thinking_turn(),
                    ..Default::default()
                }))
            }
            AgentRequest::Read { thread, .. } => Ok(AgentReply::Items(g7link_core::AgentItems {
                thread: agent_thread(&thread, "done", false),
                items: vec![agent_text(1, "Hello")],
                ..Default::default()
            })),
            AgentRequest::Answer { token, .. } => {
                if self.agent_answered.lock().unwrap().insert(token) {
                    Ok(AgentReply::Ok)
                } else {
                    Err(AgentFailure::NotFound)
                }
            }
            AgentRequest::Sessions => Ok(AgentReply::Sessions(self.agent_sessions.lock().unwrap().clone())),
            AgentRequest::SessionRead { session, .. } => {
                let found = self.agent_sessions.lock().unwrap().iter().find(|s| s.id == session).cloned();
                let session = found.ok_or(AgentFailure::NotFound)?;
                Ok(AgentReply::SessionItems(g7link_core::AgentSessionItems {
                    session,
                    items: vec![agent_text_at(0, "Hello"), agent_text_at(5 << 6, "Working on it.")],
                    more_before: false,
                }))
            }
            AgentRequest::SessionWatch { .. } => Ok(AgentReply::Ok),
            AgentRequest::SessionContinue { session } if session == OPEN_SESSION => Err(AgentFailure::Busy),
            AgentRequest::SessionContinue { .. } => {
                Ok(AgentReply::Thread(agent_thread(AGENT_THREAD, "done", true)))
            }
            AgentRequest::Export { thread: Some(thread), session: None } if thread == AGENT_THREAD => {
                Ok(AgentReply::Export(g7link_core::AgentExport {
                    text: big_export("List the files here."),
                    truncated: false,
                    file_name: "claude-list-the-files-here-2026-11-19.md".into(),
                }))
            }
            AgentRequest::Export { thread: Some(thread), session: None } if thread == BUSY_THREAD => {
                // Over the limit, with controls and a path for a name: the PC cuts and cleans it.
                Ok(AgentReply::Export(g7link_core::AgentExport {
                    text: format!("# Big\u{1b}\n{}", "y".repeat(g7link_core::agent::MAX_EXPORT_BYTES)),
                    truncated: false,
                    file_name: "/home/me/big.md".into(),
                }))
            }
            AgentRequest::Export { thread: None, session: Some(session) } => {
                let found = self.agent_sessions.lock().unwrap().iter().find(|s| s.id == session).cloned();
                let session = found.ok_or(AgentFailure::NotFound)?;
                Ok(AgentReply::Export(g7link_core::AgentExport {
                    text: big_export(&session.title),
                    truncated: false,
                    file_name: "claude-fix-the-login-test-2026-11-19.md".into(),
                }))
            }
            AgentRequest::Export { .. } => Err(AgentFailure::NotFound),
            AgentRequest::Configure { thread, model, effort, mode, .. } => {
                let mut configured = agent_thread(&thread, "done", false);
                configured.mode = mode.unwrap_or(configured.mode);
                configured.model = model.filter(|m| !m.is_empty());
                configured.effort = effort.filter(|e| !e.is_empty());
                Ok(AgentReply::Thread(configured))
            }
            AgentRequest::Queue { id, .. } if id == SENT_QUEUED => Err(AgentFailure::NotFound),
            AgentRequest::Queue { .. } | AgentRequest::Implement { .. } => Ok(AgentReply::Ok),
            AgentRequest::QuestionAnswer { token, .. } | AgentRequest::QuestionDismiss { token } => {
                if self.agent_answered.lock().unwrap().insert(token) {
                    Ok(AgentReply::Ok)
                } else {
                    Err(AgentFailure::NotFound)
                }
            }
            AgentRequest::Files { .. } => {
                let hit = |path: &str, dir: bool| g7link_core::AgentFileHit { path: path.into(), dir };
                Ok(AgentReply::Files(g7link_core::AgentFiles {
                    files: vec![
                        hit("src/main.rs", false),
                        hit("/home/me/.ssh/id_ed25519", false),
                        hit("../../etc/passwd", false),
                        hit("src/", true),
                    ],
                    more: false,
                }))
            }
            AgentRequest::Commands { .. } => {
                let command = |name: &str, kind: &str| g7link_core::AgentCommand {
                    name: name.into(),
                    note: Some("Does a thing".into()),
                    hint: None,
                    kind: kind.into(),
                };
                Ok(AgentReply::Commands(vec![
                    command("/review", "command"),
                    command("two words", "command"),
                    command("pdf", "skill"),
                ]))
            }
            AgentRequest::DraftGet { key } => {
                Ok(AgentReply::Draft(self.agent_drafts.lock().unwrap().get(&key).cloned()))
            }
            AgentRequest::DraftSet { key, text, images, rev } => {
                let mut drafts = self.agent_drafts.lock().unwrap();
                let stored = drafts.get(&key).cloned();
                let current = stored.as_ref().map_or(0, |d| d.rev);
                if rev != current {
                    return Ok(AgentReply::DraftSaved(g7link_core::AgentDraftSaved {
                        rev: current,
                        conflict: stored,
                    }));
                }
                let images = images
                    .into_iter()
                    .map(|id| g7link_core::AgentImage { id, ..Default::default() })
                    .collect();
                let draft = g7link_core::AgentDraft {
                    key: key.clone(),
                    text,
                    images,
                    rev: current + 1,
                    by_name: Some("Pixel".into()),
                };
                drafts.insert(key, draft);
                Ok(AgentReply::DraftSaved(g7link_core::AgentDraftSaved { rev: current + 1, conflict: None }))
            }
            AgentRequest::ImageAdd { name, bytes } => {
                let gate = self.agent_image_gate.lock().unwrap().clone();
                if let Some(gate) = gate {
                    gate.pass();
                }
                Ok(AgentReply::Image(g7link_core::AgentImage {
                    id: format!("{:032x}", bytes.len()),
                    name,
                    bytes: bytes.len() as u32,
                    w: 640,
                    h: 480,
                }))
            }
            AgentRequest::ImageThumb { id, .. } if id == AGENT_IMAGE => Ok(AgentReply::Thumb(jpeg(9_000))),
            AgentRequest::ImageThumb { id, .. } if id == BIG_IMAGE => Ok(AgentReply::Thumb(jpeg(200 * 1024))),
            AgentRequest::ImageThumb { .. } => Err(AgentFailure::NotFound),
            // History and undo (§6): tests/agent_history.rs has its own host.
            _ => Err(AgentFailure::Unsupported),
        }
    }
    fn set_pc_audio(
        &self,
        _from: &g7link_core::DeviceId,
        volume: Option<u8>,
        muted: Option<bool>,
    ) -> Result<(), String> {
        self.pc_audio_changes.lock().unwrap().push((volume, muted));
        Ok(())
    }
    fn set_pc_audio_output(&self, _from: &g7link_core::DeviceId, id: &str) -> Result<(), String> {
        self.pc_audio_outputs.lock().unwrap().push(id.to_owned());
        Ok(())
    }
    fn storage_list(&self, path: &str) -> Result<Vec<g7link_core::StorageEntry>, g7link_core::StorageError> {
        let guard = self.storage.lock().unwrap();
        let storage = guard.as_ref().ok_or(g7link_core::StorageError::Unsupported)?;
        storage.list(path)
    }
    fn storage_open_read(
        &self,
        path: &str,
    ) -> Result<g7link_core::StorageReadFile, g7link_core::StorageError> {
        let guard = self.storage.lock().unwrap();
        let storage = guard.as_ref().ok_or(g7link_core::StorageError::Unsupported)?;
        storage.open_read(path)
    }
    fn storage_write(
        &self,
        path: &str,
        staged: &std::path::Path,
        modified: Option<i64>,
    ) -> Result<g7link_core::StorageWriteDone, g7link_core::StorageError> {
        let guard = self.storage.lock().unwrap();
        let storage = guard.as_ref().ok_or(g7link_core::StorageError::Unsupported)?;
        storage.write(path, staged, modified)
    }
    fn storage_mkdir(&self, path: &str) -> Result<(), g7link_core::StorageError> {
        let guard = self.storage.lock().unwrap();
        let storage = guard.as_ref().ok_or(g7link_core::StorageError::Unsupported)?;
        storage.mkdir(path)
    }
    fn storage_rename(&self, from: &str, to: &str) -> Result<(), g7link_core::StorageError> {
        let guard = self.storage.lock().unwrap();
        let storage = guard.as_ref().ok_or(g7link_core::StorageError::Unsupported)?;
        storage.rename(from, to)
    }
    fn storage_delete(&self, path: &str, confirmed: bool) -> Result<(), g7link_core::StorageError> {
        let guard = self.storage.lock().unwrap();
        let storage = guard.as_ref().ok_or(g7link_core::StorageError::Unsupported)?;
        storage.delete(path, confirmed)
    }
    fn pc_capture_shot(
        &self,
        _peer: &g7link_core::DeviceId,
        prompt: g7link_core::CapturePrompt,
    ) -> Result<std::path::PathBuf, PcCaptureError> {
        self.capture_asks.lock().unwrap().push("shot".into());
        if self.capture_prompts.load(std::sync::atomic::Ordering::SeqCst) {
            prompt();
        }
        self.capture_file.lock().unwrap().clone().ok_or(PcCaptureError::Locked)
    }
    fn pc_capture_record_start(
        &self,
        _peer: &g7link_core::DeviceId,
        _prompt: g7link_core::CapturePrompt,
    ) -> Result<(), PcCaptureError> {
        let mut asks = self.capture_asks.lock().unwrap();
        if asks.last().is_some_and(|a| a == "record") {
            return Err(PcCaptureError::Busy);
        }
        asks.push("record".into());
        Ok(())
    }
    fn pc_capture_record_stop(
        &self,
        _peer: &g7link_core::DeviceId,
    ) -> Result<std::path::PathBuf, PcCaptureError> {
        let mut asks = self.capture_asks.lock().unwrap();
        if asks.last().is_none_or(|a| a != "record") {
            return Err(PcCaptureError::NotRecording);
        }
        asks.push("stop".into());
        self.capture_file.lock().unwrap().clone().ok_or_else(|| PcCaptureError::Failed("no file".into()))
    }
}

#[derive(Debug, Default)]
struct ScreenSink {
    got: Mutex<Vec<String>>,
}

impl g7link_core::MirrorSink for ScreenSink {
    fn config(&self, config: g7link_core::MirrorConfig) {
        self.got.lock().unwrap().push(format!("config {}x{}", config.width, config.height));
    }
    fn packet(&self, keyframe: bool, time_us: u64, data: Vec<u8>) {
        self.got.lock().unwrap().push(format!(
            "{} {time_us} {}",
            if keyframe { "key" } else { "frame" },
            data.len()
        ));
    }
    fn ended(&self) {
        self.got.lock().unwrap().push("ended".into());
    }
    fn audio_config(&self, config: g7link_core::MirrorAudioConfig) {
        self.got.lock().unwrap().push(format!("sound {} Hz x{}", config.rate, config.channels));
    }
    fn audio(&self, time_us: u64, data: Vec<u8>) {
        self.got.lock().unwrap().push(format!("sound {time_us} {}", data.len()));
    }
    fn audio_ended(&self) {
        self.got.lock().unwrap().push("sound ended".into());
    }
}

#[derive(Debug, Default)]
struct TestWebcamSink {
    got: Mutex<Vec<String>>,
}

impl g7link_core::WebcamSink for TestWebcamSink {
    fn config(&self, config: g7link_core::WebcamConfig) {
        self.got
            .lock()
            .unwrap()
            .push(format!("config {} {}x{} @ {}", config.camera, config.width, config.height, config.fps));
    }
    fn packet(&self, keyframe: bool, time_us: u64, data: Vec<u8>) {
        self.got.lock().unwrap().push(format!(
            "{} {time_us} {}",
            if keyframe { "key" } else { "frame" },
            data.len()
        ));
    }
    fn ended(&self) {
        self.got.lock().unwrap().push("ended".into());
    }
}

#[derive(Debug, Default)]
struct TestMicSink {
    got: Mutex<Vec<String>>,
}

impl g7link_core::MicSink for TestMicSink {
    fn config(&self, config: g7link_core::MirrorAudioConfig) {
        self.got.lock().unwrap().push(format!("config {} Hz x{}", config.rate, config.channels));
    }
    fn audio(&self, _time_us: u64, data: Vec<u8>) {
        self.got.lock().unwrap().push(format!("audio {}", data.len()));
    }
    fn ended(&self) {
        self.got.lock().unwrap().push("ended".into());
    }
}

struct TestDevice {
    node: Node,
    events: Receiver<NodeEvent>,
    platform: Arc<RecordingPlatform>,
    dir: TempDir,
    name: &'static str,
}

fn info(name: &str, kind: DeviceKind) -> DeviceInfo {
    DeviceInfo {
        name: name.into(),
        kind,
        os: if kind == DeviceKind::Phone { "android" } else { "linux" }.into(),
        os_ver: "test".into(),
        model: None,
        accent: None,
        screen: None,
        install_id: None,
    }
}

fn init_logging() {
    // Set RUST_LOG=g7link_core=debug to see what the nodes are doing.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
}

async fn start_in(dir: TempDir, name: &'static str, kind: DeviceKind) -> TestDevice {
    init_logging();
    let mut config = NodeConfig::new(dir.path(), info(name, kind), "0.0.1-test");
    config.private_clip_lifetime = Duration::from_millis(300);
    config.lan_discovery = false;
    config.away_mode = false;
    config.key_protector = Some(Arc::new(PlainKeyProtector));
    config.capabilities = vec!["media.control".into()];
    let platform = Arc::new(RecordingPlatform::default());
    let node = Node::start(config, platform.clone()).await.expect("node starts");
    let events = node.events();
    TestDevice { node, events, platform, dir, name }
}

async fn device(name: &'static str, kind: DeviceKind) -> TestDevice {
    start_in(tempfile::tempdir().unwrap(), name, kind).await
}

/// A device that offers more than the defaults.
async fn device_with(name: &'static str, kind: DeviceKind, offers: &[&str]) -> TestDevice {
    let dev = device(name, kind).await;
    let mut caps = vec!["media.control".to_owned()];
    caps.extend(offers.iter().map(|c| (*c).to_owned()));
    dev.node.update_power(PowerLevel::NotApplicable, caps).await;
    dev
}

/// Waits for the first event matching `pick`, failing the test on timeout.
async fn wait_for<T>(dev: &mut TestDevice, what: &str, mut pick: impl FnMut(&NodeEvent) -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, dev.events.recv()).await {
            Ok(Ok(event)) => {
                if let Some(found) = pick(&event) {
                    return found;
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(e)) => panic!("{}: event stream ended while waiting for {what}: {e}", dev.name),
            Err(_) => panic!("{}: timed out waiting for {what}", dev.name),
        }
    }
}

async fn wait_online(dev: &mut TestDevice, peer: g7link_core::DeviceId) {
    wait_for(dev, "peer online", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Online { .. } } if *device == peer => Some(()),
        _ => None,
    })
    .await
}

async fn with_timeout<T>(what: &str, fut: impl Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, fut).await.unwrap_or_else(|_| panic!("timed out: {what}"))
}

/// Pairs `phone` with `pc` through the QR flow and waits until both are online.
async fn pair_qr(pc: &mut TestDevice, phone: &mut TestDevice) {
    let uri = pc.node.pairing_start_qr().await.unwrap();
    assert!(!uri.addrs.is_empty(), "pairing link should carry direct addresses");
    with_timeout("join", phone.node.pairing_join(&uri.to_uri())).await.expect("pairing succeeds");

    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    wait_for(pc, "paired", |e| match e {
        NodeEvent::Pairing(PairingEvent::Paired(d)) if d.id == phone_id => Some(()),
        _ => None,
    })
    .await;
    wait_online(phone, pc_id).await;
    wait_online(pc, phone_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn qr_pairing_connects_both_devices() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;

    assert!(!pc.node.is_pairing(), "pairing mode ends after a successful pairing");
    let pc_peers = pc.node.paired_devices().unwrap();
    assert_eq!(pc_peers.len(), 1);
    assert_eq!(pc_peers[0].id, phone.node.device_id());
    assert_eq!(pc_peers[0].info.name, "Pixel");
    assert!(matches!(pc_peers[0].link, LinkState::Online { .. }));

    let phone_peers = phone.node.paired_devices().unwrap();
    assert_eq!(phone_peers.len(), 1);
    assert_eq!(phone_peers[0].info.name, "Desktop");
}

/// The Bluetooth beacon keys `dev` has, once it has `count`.
async fn beacon_keys(dev: &TestDevice, count: usize) -> Vec<(g7link_core::DeviceId, [u8; 32])> {
    with_timeout("beacon keys", async {
        loop {
            let keys = dev.node.ble_beacon_keys();
            if keys.len() == count {
                return keys;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_and_a_pc_share_a_beacon_key_until_unpaired() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    let mut other_pc = device("Laptop", DeviceKind::Desktop).await;
    pair_qr(&mut pc, &mut phone).await;
    pair_qr(&mut pc, &mut other_pc).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let on_pc = beacon_keys(&pc, 1).await;
    assert_eq!(on_pc[0].0, phone_id, "a PC shares one with phones only");
    assert_eq!(beacon_keys(&phone, 1).await, vec![(pc_id, on_pc[0].1)]);
    assert!(other_pc.node.ble_beacon_keys().is_empty());

    with_timeout("unpair", pc.node.unpair(phone_id)).await.unwrap();
    beacon_keys(&pc, 0).await;
    beacon_keys(&phone, 0).await;
}

/// Carries values between two Bluetooth link ends, as the platforms do, until
/// the returned task is aborted.
fn wire(a: Arc<g7link_core::BleEndpoint>, b: Arc<g7link_core::BleEndpoint>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let mut moved = false;
            while let Some(value) = a.next_outbound() {
                moved = true;
                if b.receive(&value).is_err() {
                    return;
                }
            }
            while let Some(value) = b.next_outbound() {
                moved = true;
                if a.receive(&value).is_err() {
                    return;
                }
            }
            if !moved {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    })
}

/// A phone and a PC paired, both letting each other use Bluetooth, with the
/// secret shared.
async fn ble_pair() -> (TestDevice, TestDevice) {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &["ble.link"]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &["ble.link"]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    beacon_keys(&phone, 1).await;
    pc.node.set_device_toggle(phone_id, "ble_link", true).unwrap();
    phone.node.set_device_toggle(pc_id, "ble_link", true).unwrap();
    (pc, phone)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bluetooth_link_comes_up_and_carries_a_ring() {
    let (pc, phone) = ble_pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let on_pc = pc.node.ble_accept();
    let on_phone = phone.node.ble_connect(pc_id).unwrap();
    let pump = wire(on_phone.clone(), on_pc.clone());
    with_timeout("link up", async {
        while !(pc.node.ble_linked(phone_id) && phone.node.ble_linked(pc_id)) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert_eq!(on_pc.peer(), Some(phone_id));
    assert_eq!(on_phone.peer(), Some(pc_id));

    with_timeout("ping", pc.node.ble_ping(phone_id)).await.expect("the phone answers over Bluetooth");
    with_timeout("ping back", phone.node.ble_ping(pc_id)).await.expect("the PC answers over Bluetooth");

    with_timeout("ring", pc.node.ble_ring(phone_id, true)).await.expect("the phone rings");
    assert_eq!(*phone.platform.rings.lock().unwrap(), [true]);
    with_timeout("stop", pc.node.ble_ring(phone_id, false)).await.expect("the phone stops");
    assert_eq!(*phone.platform.rings.lock().unwrap(), [true, false]);

    // A disconnect ends both ends; neither has a link after it.
    pump.abort();
    on_phone.close();
    on_pc.close();
    assert!(!pc.node.ble_linked(phone_id) && !phone.node.ble_linked(pc_id));
    assert!(with_timeout("ping after close", phone.node.ble_ping(pc_id)).await.is_err());
}

/// Waits until `dev` has no session with `peer`.
async fn wait_offline(dev: &mut TestDevice, peer: g7link_core::DeviceId) {
    wait_for(dev, "peer offline", |e| match e {
        NodeEvent::LinkChanged { device, link }
            if *device == peer && !matches!(link, LinkState::Online { .. }) =>
        {
            Some(())
        }
        _ => None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn copied_text_and_codes_go_over_bluetooth_without_a_network() {
    let (mut pc, mut phone) = ble_pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    // No network between them: neither reconnects.
    pc.node.set_reconnect(phone_id, false).unwrap();
    phone.node.set_reconnect(pc_id, false).unwrap();
    wait_offline(&mut pc, phone_id).await;
    wait_offline(&mut phone, pc_id).await;

    let on_pc = pc.node.ble_accept();
    let on_phone = phone.node.ble_connect(pc_id).unwrap();
    let pump = wire(on_phone.clone(), on_pc.clone());
    with_timeout("link up", async {
        while !(pc.node.ble_linked(phone_id) && phone.node.ble_linked(pc_id)) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    // Short text goes over the link, both ways; long text waits for the network.
    with_timeout("clip to phone", pc.node.send_clipboard(phone_id, "over the air".into())).await.unwrap();
    assert_eq!(*phone.platform.clipboard.lock().unwrap(), ["over the air"]);
    with_timeout("clip to pc", phone.node.send_clipboard(pc_id, "and back".into())).await.unwrap();
    assert_eq!(*pc.platform.clipboard.lock().unwrap(), ["and back"]);
    with_timeout("long clip", pc.node.send_clipboard(phone_id, "x".repeat(2049))).await.unwrap();
    assert_eq!(phone.platform.clipboard.lock().unwrap().len(), 1, "queued, not sent");

    // A code in a notification reaches the PC; the notification doesn't.
    let mut code = note("otp", "com.bank", "Bank", false);
    code.text = Some("Your verification code is 493021".into());
    phone.node.notification_posted(code).await;
    let got = wait_for(&mut pc, "code", |e| match e {
        NodeEvent::OneTimeCode { device, code, .. } if *device == phone_id => Some(code.clone()),
        _ => None,
    })
    .await;
    assert_eq!(got, "493021");

    // With the clipboard off on the phone for the PC, nothing is written there.
    phone.node.set_device_toggle(pc_id, "clipboard", false).unwrap();
    assert!(with_timeout("refused clip", pc.node.send_clipboard(phone_id, "nope".into())).await.is_ok());
    assert_eq!(*phone.platform.clipboard.lock().unwrap(), ["over the air"]);
    pump.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_without_a_network_asks_for_the_hotspot_over_bluetooth() {
    use g7link_core::{
        PhoneToggleValue, PhoneToggles, TOGGLES_READ, TOGGLES_SHOW, TOGGLES_WIFI, ringer_modes, toggle_ids,
    };

    let (mut pc, mut phone) = ble_pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.update_power(PowerLevel::NotApplicable, vec!["ble.link".into(), TOGGLES_SHOW.into()]).await;
    phone
        .node
        .update_power(PowerLevel::Elevated, vec!["ble.link".into(), TOGGLES_READ.into(), TOGGLES_WIFI.into()])
        .await;
    wait_for(&mut pc, "toggles.wifi available", |e| match e {
        NodeEvent::Capabilities(m)
            if m.device == phone_id && m.state("toggles.wifi") == Some(FeatureState::Available) =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
    pc.node.set_reconnect(phone_id, false).unwrap();
    phone.node.set_reconnect(pc_id, false).unwrap();
    wait_offline(&mut pc, phone_id).await;
    wait_offline(&mut phone, pc_id).await;

    let on_pc = pc.node.ble_accept();
    let on_phone = phone.node.ble_connect(pc_id).unwrap();
    let pump = wire(on_phone.clone(), on_pc.clone());
    with_timeout("link up", async {
        while !(pc.node.ble_linked(phone_id) && phone.node.ble_linked(pc_id)) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    // The hotspot goes over Bluetooth; nothing else does.
    with_timeout(
        "hotspot",
        pc.node.set_phone_toggle(phone_id, toggle_ids::HOTSPOT.into(), PhoneToggleValue::Bool(true)),
    )
    .await
    .unwrap();
    assert_eq!(
        *phone.platform.phone_toggles.lock().unwrap(),
        [(toggle_ids::HOTSPOT.to_owned(), PhoneToggleValue::Bool(true))]
    );
    let wifi = pc.node.set_phone_toggle(phone_id, toggle_ids::WIFI.into(), PhoneToggleValue::Bool(false));
    assert!(with_timeout("wifi", wifi).await.is_err());

    // The phone's toggles reach the PC over the link, so it sees the hotspot come on.
    let state = PhoneToggles {
        dnd: false,
        ringer: ringer_modes::RING.into(),
        flashlight: None,
        volume: 50,
        brightness: 50,
        wifi: false,
        bluetooth: true,
        hotspot: Some(true),
    };
    phone.node.toggles_changed(state).await.unwrap();
    let got = wait_for(&mut pc, "toggles", |e| match e {
        NodeEvent::PhoneToggles { device, toggles } if *device == phone_id => Some(toggles.hotspot),
        _ => None,
    })
    .await;
    assert_eq!(got, Some(true));
    pump.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_touchpad_and_keyboard_work_over_bluetooth() {
    use g7link_core::{INPUT_INJECT, RemoteInput};

    let (mut pc, mut phone) = ble_pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.update_power(PowerLevel::NotApplicable, vec!["ble.link".into(), INPUT_INJECT.into()]).await;
    wait_for(&mut phone, "input.remote available", |e| match e {
        NodeEvent::Capabilities(m)
            if m.device == pc_id && m.state("input.remote") == Some(FeatureState::Available) =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
    pc.node.set_reconnect(phone_id, false).unwrap();
    phone.node.set_reconnect(pc_id, false).unwrap();
    wait_offline(&mut pc, phone_id).await;
    wait_offline(&mut phone, pc_id).await;

    let on_pc = pc.node.ble_accept();
    let on_phone = phone.node.ble_connect(pc_id).unwrap();
    // Not pumped yet: motion adds up while the link is busy.
    let mut pump = None;
    for (a, b) in [(&on_phone, &on_pc), (&on_pc, &on_phone)].into_iter().cycle().take(40) {
        while let Some(value) = a.next_outbound() {
            b.receive(&value).unwrap();
        }
        if pc.node.ble_linked(phone_id) && phone.node.ble_linked(pc_id) {
            break;
        }
    }
    assert!(pc.node.ble_linked(phone_id));

    // Turned off on the PC: asked, refused, and nothing moves.
    let check = phone.node.remote_check(pc_id);
    pump.get_or_insert_with(|| wire(on_phone.clone(), on_pc.clone()));
    assert!(matches!(with_timeout("check", check).await, Err(Error::Denied)));
    pc.node.set_device_toggle(phone_id, "remote_input", true).unwrap();
    with_timeout("check again", phone.node.remote_check(pc_id)).await.unwrap();

    // Motion sent faster than the link takes it arrives added up, in order with keys.
    if let Some(pump) = pump.take() {
        pump.abort();
    }
    for _ in 0..10 {
        phone.node.remote_move(pc_id, 3.0, -2.0).await.unwrap();
    }
    let pump = wire(on_phone.clone(), on_pc.clone());
    let moved = with_timeout("moved", async {
        loop {
            let moved: (f32, f32) = pc
                .platform
                .remote_inputs
                .lock()
                .unwrap()
                .iter()
                .filter_map(|i| match i {
                    RemoteInput::Move { dx, dy } => Some((*dx, *dy)),
                    _ => None,
                })
                .fold((0.0, 0.0), |(x, y), (dx, dy)| (x + dx, y + dy));
            if moved == (30.0, -20.0) {
                return moved;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert_eq!(moved, (30.0, -20.0));
    let moves = pc.platform.remote_inputs.lock().unwrap().len();
    assert!(moves < 10, "added up into fewer moves: {moves}");

    let key = RemoteInput::Key { key: "enter".into(), mods: Vec::new() };
    with_timeout("key", phone.node.remote_input(pc_id, key.clone())).await.unwrap();
    with_timeout("text", phone.node.remote_input(pc_id, RemoteInput::Text { text: "hi".into() }))
        .await
        .unwrap();
    let inputs = pc.platform.remote_inputs.lock().unwrap().clone();
    assert_eq!(inputs[inputs.len() - 2..], [key, RemoteInput::Text { text: "hi".into() }]);
    pump.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_that_hasnt_turned_bluetooth_on_isnt_linked() {
    let (pc, phone) = ble_pair().await;
    let pc_id = pc.node.device_id();
    phone.node.set_device_toggle(pc_id, "ble_link", false).unwrap();
    assert!(phone.node.ble_connect(pc_id).is_err());

    // Turned off on the PC instead: the phone's handshake is refused.
    phone.node.set_device_toggle(pc_id, "ble_link", true).unwrap();
    pc.node.set_device_toggle(phone.node.device_id(), "ble_link", false).unwrap();
    let on_pc = pc.node.ble_accept();
    let on_phone = phone.node.ble_connect(pc_id).unwrap();
    let mut refused = false;
    while let Some(value) = on_phone.next_outbound() {
        refused |= on_pc.receive(&value).is_err();
    }
    assert!(refused && !on_pc.is_up());
}

#[tokio::test(flavor = "multi_thread")]
async fn garbage_before_the_handshake_ends_the_link() {
    let (pc, _phone) = ble_pair().await;
    let on_pc = pc.node.ble_accept();
    // A first fragment announcing 300 bytes: more than a handshake message.
    assert!(on_pc.receive(&[0x80, 0x01, 0x2C, 1, 2, 3]).is_err());
    let on_pc = pc.node.ble_accept();
    assert!(on_pc.receive(&[0x05]).is_err(), "out of sequence");
}

/// A phone and a PC as [`ble_pair`] makes them, with the PC offering
/// `pc_caps` and the phone `phone_caps` (both with `ble.link`), each knowing
/// what the other offers, and the PC taking the phone's input.
async fn remote_pair(pc_caps: &[&str], phone_caps: &[&str]) -> (TestDevice, TestDevice) {
    let (pc, phone) = ble_pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let caps = |extra: &[&str]| -> Vec<String> {
        ["ble.link"].iter().chain(extra).map(|c| (*c).to_owned()).collect()
    };
    pc.node.update_power(PowerLevel::NotApplicable, caps(pc_caps)).await;
    phone.node.update_power(PowerLevel::NotApplicable, caps(phone_caps)).await;
    until("each knows what the other offers", || {
        pc_caps.iter().all(|c| phone.node.peer_offers(pc_id, c))
            && phone_caps.iter().all(|c| pc.node.peer_offers(phone_id, c))
    })
    .await;
    pc.node.set_device_toggle(phone_id, "remote_input", true).unwrap();
    (pc, phone)
}

/// Brings a Bluetooth link up between `pc` and `phone`, at `att_mtu`: both
/// ends, and the pump carrying values between them with a count of those
/// the phone sent.
async fn link_up(
    pc: &TestDevice,
    phone: &TestDevice,
    att_mtu: u16,
) -> (
    Arc<g7link_core::BleEndpoint>,
    Arc<g7link_core::BleEndpoint>,
    tokio::task::JoinHandle<()>,
    Arc<AtomicUsize>,
) {
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let on_pc = pc.node.ble_accept();
    let on_phone = phone.node.ble_connect(pc_id).unwrap();
    on_pc.set_mtu(att_mtu);
    on_phone.set_mtu(att_mtu);
    let sent = Arc::new(AtomicUsize::new(0));
    let pump = tapped_wire(on_phone.clone(), on_pc.clone(), sent.clone());
    until("link up", || pc.node.ble_linked(phone_id) && phone.node.ble_linked(pc_id)).await;
    (on_pc, on_phone, pump, sent)
}

/// [`wire`], counting the values `phone` sends.
fn tapped_wire(
    phone: Arc<g7link_core::BleEndpoint>,
    pc: Arc<g7link_core::BleEndpoint>,
    sent: Arc<AtomicUsize>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let mut moved = false;
            while let Some(value) = phone.next_outbound() {
                moved = true;
                sent.fetch_add(1, Ordering::SeqCst);
                if pc.receive(&value).is_err() {
                    return;
                }
            }
            while let Some(value) = pc.next_outbound() {
                moved = true;
                if phone.receive(&value).is_err() {
                    return;
                }
            }
            if !moved {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    })
}

/// Every event `dev` has had since it last looked.
fn drain(dev: &mut TestDevice) -> Vec<NodeEvent> {
    let mut events = Vec::new();
    loop {
        match dev.events.try_recv() {
            Ok(event) => events.push(event),
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(_) => return events,
        }
    }
}

fn text_state(rev: u32, applied: u32, before: &str, after: &str) -> g7link_core::RemoteTextState {
    g7link_core::RemoteTextState {
        focus: 3,
        field: 9,
        epoch: 0,
        rev,
        applied,
        kind: g7link_core::TextFieldKind::Mirror,
        app: Some("Writer".into()),
        label: Some("Document".into()),
        window: false,
        multiline: true,
        start: 0,
        before: before.into(),
        after: after.into(),
        anchor: None,
        more_before: false,
        more_after: false,
        icon: Some("0123456789abcdef".into()),
    }
}

fn pc_text_state(
    from: g7link_core::DeviceId,
) -> impl FnMut(&NodeEvent) -> Option<g7link_core::RemoteTextState> {
    move |e| match e {
        NodeEvent::PcTextState { device, state } if *device == from => Some(state.clone()),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_typing_box_follows_the_pc_over_bluetooth() {
    use g7link_core::{BLE_REMOTE_CAPABILITY, INPUT_APP_ICON, INPUT_INJECT, INPUT_TEXT_MIRROR, RemoteInput};

    let (mut pc, mut phone) = remote_pair(
        &[BLE_REMOTE_CAPABILITY, INPUT_INJECT, INPUT_TEXT_MIRROR, INPUT_APP_ICON],
        &[BLE_REMOTE_CAPABILITY],
    )
    .await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.set_reconnect(phone_id, false).unwrap();
    phone.node.set_reconnect(pc_id, false).unwrap();
    wait_offline(&mut pc, phone_id).await;
    wait_offline(&mut phone, pc_id).await;
    let (_on_pc, on_phone, pump, _) = link_up(&pc, &phone, 517).await;

    // The box opens: the PC hears it over the link.
    with_timeout("watch", phone.node.remote_text_watch(pc_id, true)).await.unwrap();
    wait_for(&mut pc, "the phone watches", |e| match e {
        NodeEvent::TextWatch { device, on: true } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    // A long field arrives cut to what a message on the link holds.
    let long = text_state(1, 0, &"b".repeat(4000), &"a".repeat(1000));
    with_timeout("state", pc.node.remote_text_state(phone_id, &long)).await.unwrap();
    let heard = wait_for(&mut phone, "the PC's field", pc_text_state(pc_id)).await;
    assert_eq!((heard.before.len(), heard.after.len()), (2048, 512));
    assert_eq!(heard.start, 4000 - 2048);
    assert!(heard.more_before && heard.more_after);
    assert_eq!(heard.caret(), long.caret());
    assert_eq!(heard.label, long.label);

    // An edit runs, and the next state says it did.
    let edit = RemoteInput::Edit { focus: 3, seq: 1, range: None, moves: 0, backspaces: 1, text: "c".into() };
    with_timeout("edit", phone.node.remote_input(pc_id, edit.clone())).await.unwrap();
    assert!(pc.platform.remote_inputs.lock().unwrap().contains(&edit));
    let after = text_state(2, 1, "bc", "");
    with_timeout("state", pc.node.remote_text_state(phone_id, &after)).await.unwrap();
    let heard = wait_for(&mut phone, "the PC's field", pc_text_state(pc_id)).await;
    assert_eq!((heard.applied, heard.before.as_str()), (1, "bc"));
    assert!(phone.node.remote_route_state(pc_id).ble_linked);

    // Icons need the session: nothing goes on the link for one.
    pump.abort();
    let icon = phone.node.remote_app_icon(pc_id, "0123456789abcdef".into()).await;
    assert!(matches!(icon, Err(Error::Offline)), "{icon:?}");
    assert!(on_phone.next_outbound().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn states_dont_queue_behind_a_slow_link() {
    use g7link_core::{BLE_REMOTE_CAPABILITY, INPUT_INJECT, INPUT_TEXT_MIRROR};

    let (mut pc, mut phone) =
        remote_pair(&[BLE_REMOTE_CAPABILITY, INPUT_INJECT, INPUT_TEXT_MIRROR], &[BLE_REMOTE_CAPABILITY])
            .await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.set_reconnect(phone_id, false).unwrap();
    phone.node.set_reconnect(pc_id, false).unwrap();
    wait_offline(&mut pc, phone_id).await;
    wait_offline(&mut phone, pc_id).await;
    let (on_pc, on_phone, pump, _) = link_up(&pc, &phone, 23).await;

    // The link takes nothing for a while; the field changes thirty times.
    pump.abort();
    for rev in 1..=30 {
        let state = text_state(rev, 0, &format!("draft {rev} ").repeat(40), "");
        pc.node.remote_text_state(phone_id, &state).await.unwrap();
    }
    drain(&mut phone);
    let pump = wire(on_phone.clone(), on_pc.clone());
    let heard = wait_for(&mut phone, "the PC's field", pc_text_state(pc_id)).await;
    assert_eq!(heard.rev, 30, "only the newest went");
    assert!(heard.before.len() <= 512, "cut for a small MTU: {}", heard.before.len());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!drain(&mut phone).iter().any(|e| matches!(e, NodeEvent::PcTextState { .. })));
    pump.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_asks_to_use_bluetooth() {
    use g7link_core::{BLE_REMOTE_CAPABILITY, BleUseState};

    let (mut pc, phone) = remote_pair(&[BLE_REMOTE_CAPABILITY], &[BLE_REMOTE_CAPABILITY]).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.set_device_toggle(phone_id, "ble_link", false).unwrap();
    drain(&mut pc);
    let requested =
        |e: &NodeEvent| matches!(e, NodeEvent::BleLinkRequested { device } if *device == phone_id);

    // Off on the PC: its user is asked, once.
    let ask = phone.node.ble_use(pc_id, true, true);
    assert_eq!(with_timeout("ask", ask).await.unwrap(), BleUseState::Asking);
    wait_for(&mut pc, "the prompt", |e| requested(e).then_some(())).await;
    let again = phone.node.ble_use(pc_id, true, true);
    assert_eq!(with_timeout("ask again", again).await.unwrap(), BleUseState::Asking, "still showing");
    // "Not now": the phone is told, and asking again within a minute doesn't prompt.
    pc.node.ble_prompt_closed(phone_id);
    let poll = phone.node.ble_use(pc_id, true, false);
    assert_eq!(with_timeout("poll", poll).await.unwrap(), BleUseState::Off);
    let again = phone.node.ble_use(pc_id, true, true);
    assert_eq!(with_timeout("ask again", again).await.unwrap(), BleUseState::Off);
    assert!(!drain(&mut pc).iter().any(requested), "one prompt a minute");
    assert!(!pc.node.ble_held(phone_id));

    // Allowed: held, and the PC can be connected to.
    pc.node.set_device_toggle(phone_id, "ble_link", true).unwrap();
    let poll = phone.node.ble_use(pc_id, true, false);
    assert_eq!(with_timeout("poll", poll).await.unwrap(), BleUseState::Open);
    wait_for(&mut pc, "a window", |e| match e {
        NodeEvent::BleWanted { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert!(pc.node.ble_held(phone_id));
    let (_on_pc, _on_phone, pump, _) = link_up(&pc, &phone, 517).await;
    let renew = phone.node.ble_use(pc_id, true, false);
    assert_eq!(with_timeout("renew", renew).await.unwrap(), BleUseState::Linked);

    // Let go of.
    drain(&mut pc);
    let stop = phone.node.ble_use(pc_id, false, false);
    assert_eq!(with_timeout("stop", stop).await.unwrap(), BleUseState::Off);
    wait_for(&mut pc, "the hold ends", |e| match e {
        NodeEvent::BleHoldChanged { device, held: false } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert!(!pc.node.ble_held(phone_id));
    pump.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_phone_finds_the_pc_busy() {
    use g7link_core::{BLE_REMOTE_CAPABILITY, BleUseState};

    let (mut pc, phone) = remote_pair(&[BLE_REMOTE_CAPABILITY], &[BLE_REMOTE_CAPABILITY]).await;
    let pc_id = pc.node.device_id();
    let mut other = device_with("Galaxy", DeviceKind::Phone, &["ble.link", BLE_REMOTE_CAPABILITY]).await;
    pair_qr(&mut pc, &mut other).await;
    let other_id = other.node.device_id();
    beacon_keys(&other, 1).await;
    pc.node.set_device_toggle(other_id, "ble_link", true).unwrap();
    other.node.set_device_toggle(pc_id, "ble_link", true).unwrap();
    until("the PC knows the other phone", || pc.node.peer_offers(other_id, BLE_REMOTE_CAPABILITY)).await;

    // The PC serves one link: while the first phone has it, the other waits.
    let (_on_pc, _on_phone, pump, _) = link_up(&pc, &phone, 517).await;
    assert_eq!(with_timeout("ask", other.node.ble_use(pc_id, true, true)).await.unwrap(), BleUseState::Busy);
    assert!(!pc.node.ble_held(other_id));
    pump.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pinned_route_is_used_and_falls_back() {
    use g7link_core::{BLE_REMOTE_CAPABILITY, INPUT_INJECT, RemoteInput, RemotePath, RemoteRoute};

    let (pc, mut phone) = remote_pair(&[BLE_REMOTE_CAPABILITY, INPUT_INJECT], &[BLE_REMOTE_CAPABILITY]).await;
    let pc_id = pc.node.device_id();
    let (on_pc, on_phone, pump, sent) = link_up(&pc, &phone, 517).await;
    let key = |n: usize| RemoteInput::Key { key: format!("f{n}"), mods: Vec::new() };

    // Pinned to Bluetooth with both up: the key goes on the link.
    phone.node.set_remote_route(pc_id, RemoteRoute::Bluetooth).unwrap();
    let state = wait_for(&mut phone, "the route", |e| match e {
        NodeEvent::RemoteRouteChanged { device, state } if *device == pc_id => Some(*state),
        _ => None,
    })
    .await;
    assert_eq!(
        (state.route, state.active, state.fallback),
        (RemoteRoute::Bluetooth, Some(RemotePath::Ble), false)
    );
    assert!(state.ble_remote && state.ble_linked);
    let before = sent.load(Ordering::SeqCst);
    with_timeout("key", phone.node.remote_input(pc_id, key(1))).await.unwrap();
    assert!(sent.load(Ordering::SeqCst) > before, "it went on the link");

    // Network: the session.
    phone.node.set_remote_route(pc_id, RemoteRoute::Network).unwrap();
    let before = sent.load(Ordering::SeqCst);
    with_timeout("key", phone.node.remote_input(pc_id, key(2))).await.unwrap();
    assert_eq!(sent.load(Ordering::SeqCst), before, "it went on the session");

    // Pinned to Bluetooth and the link goes: the session.
    phone.node.set_remote_route(pc_id, RemoteRoute::Bluetooth).unwrap();
    pump.abort();
    on_phone.close();
    on_pc.close();
    with_timeout("key", phone.node.remote_input(pc_id, key(3))).await.unwrap();
    let state = phone.node.remote_route_state(pc_id);
    assert_eq!((state.active, state.fallback, state.ble_linked), (Some(RemotePath::Session), true, false));
    assert_eq!(pc.platform.remote_inputs.lock().unwrap()[..], [key(1), key(2), key(3)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn input_keeps_its_order_across_a_switch() {
    use g7link_core::{BLE_REMOTE_CAPABILITY, INPUT_INJECT, RemoteInput, RemoteRoute};

    let (pc, phone) = remote_pair(&[BLE_REMOTE_CAPABILITY, INPUT_INJECT], &[BLE_REMOTE_CAPABILITY]).await;
    let pc_id = pc.node.device_id();
    let (_on_pc, _on_phone, pump, sent) = link_up(&pc, &phone, 185).await;
    let handshake = sent.load(Ordering::SeqCst);
    let typed: Vec<RemoteInput> = (0..50).map(|n| RemoteInput::Text { text: format!("{n} ") }).collect();
    for (n, input) in typed.iter().enumerate() {
        let route = if n % 2 == 0 { RemoteRoute::Bluetooth } else { RemoteRoute::Network };
        phone.node.set_remote_route(pc_id, route).unwrap();
        with_timeout("key", phone.node.remote_input(pc_id, input.clone())).await.unwrap();
    }
    assert!(sent.load(Ordering::SeqCst) > handshake, "some went on the link");
    assert_eq!(*pc.platform.remote_inputs.lock().unwrap(), typed, "in order, none twice");
    pump.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_old_pc_gets_no_ble_use() {
    use g7link_core::{
        BLE_REMOTE_CAPABILITY, INPUT_INJECT, INPUT_TEXT_MIRROR, RemoteInput, RemotePath, RemoteRoute,
    };

    // A PC whose link carries input but that doesn't offer `ble.remote`.
    let (pc, phone) = remote_pair(&[INPUT_INJECT, INPUT_TEXT_MIRROR], &[BLE_REMOTE_CAPABILITY]).await;
    let pc_id = pc.node.device_id();
    assert!(matches!(phone.node.ble_use(pc_id, true, true).await, Err(Error::Unsupported)));
    let (on_pc, on_phone, pump, sent) = link_up(&pc, &phone, 517).await;
    let handshake = sent.load(Ordering::SeqCst);

    // With a session, input takes it whatever the route: as before the picker.
    phone.node.set_remote_route(pc_id, RemoteRoute::Bluetooth).unwrap();
    let state = phone.node.remote_route_state(pc_id);
    assert!(!state.ble_remote && !state.fallback);
    assert_eq!(state.active, Some(RemotePath::Session));
    let key = RemoteInput::Key { key: "enter".into(), mods: Vec::new() };
    with_timeout("key", phone.node.remote_input(pc_id, key.clone())).await.unwrap();
    assert_eq!(sent.load(Ordering::SeqCst), handshake, "nothing on the link");
    // The typing box's watch doesn't go on the link either.
    let mut phone = phone;
    pc.node.set_reconnect(phone.node.device_id(), false).unwrap();
    phone.node.set_reconnect(pc_id, false).unwrap();
    wait_offline(&mut phone, pc_id).await;
    assert!(matches!(phone.node.remote_text_watch(pc_id, true).await, Err(Error::Offline)));
    // Without one, the link.
    with_timeout("key", phone.node.remote_input(pc_id, key.clone())).await.unwrap();
    assert!(sent.load(Ordering::SeqCst) > handshake);
    assert_eq!(*pc.platform.remote_inputs.lock().unwrap(), [key.clone(), key]);
    pump.abort();
    drop((on_pc, on_phone));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_state_from_the_old_path_is_dropped() {
    use g7link_core::{BLE_REMOTE_CAPABILITY, INPUT_INJECT, INPUT_TEXT_MIRROR, RemoteRoute};

    let (mut pc, mut phone) =
        remote_pair(&[BLE_REMOTE_CAPABILITY, INPUT_INJECT, INPUT_TEXT_MIRROR], &[BLE_REMOTE_CAPABILITY])
            .await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let (_on_pc, _on_phone, pump, _) = link_up(&pc, &phone, 517).await;
    let watching = |e: &NodeEvent| match e {
        NodeEvent::TextWatch { device, on: true } if *device == phone_id => Some(()),
        _ => None,
    };

    // Watching over the session: rev 5 comes that way.
    phone.node.set_remote_route(pc_id, RemoteRoute::Network).unwrap();
    phone.node.remote_text_watch(pc_id, true).await.unwrap();
    wait_for(&mut pc, "the phone watches", watching).await;
    pc.node.remote_text_state(phone_id, &text_state(5, 0, "five", "")).await.unwrap();
    assert_eq!(wait_for(&mut phone, "rev 5", pc_text_state(pc_id)).await.rev, 5);

    // The route moves to the link and the watch with it: rev 4, sent before
    // the move, lands after; it's dropped. Rev 6 isn't.
    phone.node.set_remote_route(pc_id, RemoteRoute::Bluetooth).unwrap();
    phone.node.remote_text_watch(pc_id, true).await.unwrap();
    wait_for(&mut pc, "the phone watches over the link", watching).await;
    pc.node.remote_text_state(phone_id, &text_state(4, 0, "four", "")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    pc.node.remote_text_state(phone_id, &text_state(6, 0, "six", "")).await.unwrap();
    assert_eq!(wait_for(&mut phone, "rev 6", pc_text_state(pc_id)).await.rev, 6, "rev 4 was dropped");
    pump.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn update_hints_reach_peers_now_and_on_connect() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    // Heard before the phone connects: it gets it once they're connected.
    pc.node.announce_update("stable".into(), "0.1.5".into()).await;
    pair_qr(&mut pc, &mut phone).await;
    let pc_id = pc.node.device_id();
    let hint = |e: &NodeEvent| match e {
        NodeEvent::UpdateHint { device, channel, version } if *device == pc_id => {
            Some((channel.clone(), version.clone()))
        }
        _ => None,
    };
    assert_eq!(wait_for(&mut phone, "stored hint", hint).await, ("stable".to_owned(), "0.1.5".to_owned()));

    // Heard while connected: sent right away. A malformed one isn't sent.
    pc.node.announce_update("beta".into(), "0.1.6".into()).await;
    pc.node.announce_update("nightly".into(), "0.1.6-nightly.3".into()).await;
    assert_eq!(
        wait_for(&mut phone, "live hint", hint).await,
        ("nightly".to_owned(), "0.1.6-nightly.3".to_owned())
    );

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn local_stop_clears_the_initiators_ring_state() {
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut tablet = device("Tablet", DeviceKind::Phone).await;
    pair_qr(&mut phone, &mut pc).await;
    pair_qr(&mut phone, &mut tablet).await;

    for target in [&pc, &tablet] {
        let target_id = target.node.device_id();
        phone.node.ring(target_id, true).await.unwrap();
        wait_for(&mut phone, "remote ring started", |event| match event {
            NodeEvent::RemoteRing { device, on: true } if *device == target_id => Some(()),
            _ => None,
        })
        .await;

        target.node.stop_ringing().await;
        wait_for(&mut phone, "remote ring stopped", |event| match event {
            NodeEvent::RemoteRing { device, on: false } if *device == target_id => Some(()),
            _ => None,
        })
        .await;
        assert_eq!(*target.platform.rings.lock().unwrap(), [true, false]);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ring_and_battery_flow_between_devices() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    with_timeout("ring", pc.node.ring(phone_id, true)).await.expect("ring succeeds");
    with_timeout("stop ring", pc.node.ring(phone_id, false)).await.expect("stop succeeds");
    assert_eq!(*phone.platform.rings.lock().unwrap(), vec![true, false]);

    let battery = Battery {
        level: 82,
        charging: true,
        plugged: Some("usb".into()),
        full_in: Some(28),
        cell_bars: Some(4),
        cell_type: Some("5G".into()),
        wifi_bars: Some(3),
    };
    phone.node.update_battery(battery.clone()).await;
    let received = wait_for(&mut pc, "battery", |e| match e {
        NodeEvent::Battery { device, battery } if *device == phone_id => Some(battery.clone()),
        _ => None,
    })
    .await;
    assert_eq!(received, battery);

    let mut renamed = info("Pixel 9 Pro", DeviceKind::Phone);
    renamed.accent = Some(0xFF65558F);
    renamed.screen = Some(g7link_core::ScreenShape {
        v: 1,
        aspect: 0.45,
        corners: Some(g7link_core::ScreenCorners { tl: 0.088, tr: 0.088, br: 0.088, bl: 0.088 }),
        cutouts: vec![g7link_core::ScreenRect { x: 0.46, y: 0.018, w: 0.08, h: 0.036 }],
        cutout_path: None,
    });
    phone.node.update_device_info(renamed.clone()).await;
    let info = wait_for(&mut pc, "rename", |e| match e {
        NodeEvent::PeerInfoChanged { device, info } if *device == phone_id && info.name == "Pixel 9 Pro" => {
            Some(info.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(info, renamed);
    let stored = &pc.node.paired_devices().unwrap()[0].info;
    assert_eq!(stored.name, "Pixel 9 Pro", "rename is persisted");
    assert_eq!(stored.screen, renamed.screen, "screen geometry is persisted");
    let _ = pc_id;
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_pairing_secret_is_rejected() {
    let pc = device("Desktop", DeviceKind::Desktop).await;
    let phone = device("Pixel", DeviceKind::Phone).await;

    let mut uri = pc.node.pairing_start_qr().await.unwrap();
    uri.secret[0] ^= 0xFF; // as if an attacker guessed or altered the code
    let result = with_timeout("join", phone.node.pairing_join(&uri.to_uri())).await;
    assert!(matches!(result, Err(Error::Denied)), "got {result:?}");
    assert!(pc.node.paired_devices().unwrap().is_empty());
    assert!(phone.node.paired_devices().unwrap().is_empty());
    assert!(pc.node.is_pairing(), "a single wrong attempt doesn't end pairing mode");
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_requires_pairing_mode() {
    let pc = device("Desktop", DeviceKind::Desktop).await;
    let phone = device("Pixel", DeviceKind::Phone).await;

    // A link for the right device, but the PC isn't showing a code.
    let uri = PairingUri {
        id: pc.node.device_id(),
        secret: [1; 16],
        name: "Desktop".into(),
        addrs: pc.node.direct_addrs().await,
    };
    let result = with_timeout("join", phone.node.pairing_join(&uri.to_uri())).await;
    assert!(result.is_err(), "pairing without pairing mode must fail");
    assert!(pc.node.paired_devices().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn too_many_wrong_attempts_end_pairing_mode() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let phone = device("Pixel", DeviceKind::Phone).await;
    let mut uri = pc.node.pairing_start_qr().await.unwrap();
    uri.secret[3] ^= 1;
    for _ in 0..5 {
        let _ = with_timeout("join", phone.node.pairing_join(&uri.to_uri())).await;
    }
    wait_for(&mut pc, "pairing failure", |e| match e {
        NodeEvent::Pairing(PairingEvent::Failed(PairingFailure::Rejected)) => Some(()),
        _ => None,
    })
    .await;
    assert!(!pc.node.is_pairing());
}

async fn sas_code(dev: &mut TestDevice) -> String {
    wait_for(dev, "6-digit code", |e| match e {
        NodeEvent::Pairing(PairingEvent::SasCode { code, .. }) => Some(code.clone()),
        _ => None,
    })
    .await
}

async fn start_nearby_setup() -> (TestDevice, TestDevice, Vec<SocketAddr>) {
    let pc = device("Desktop", DeviceKind::Desktop).await;
    let phone = device("Pixel", DeviceKind::Phone).await;
    // The PC shows its pairing screen; the phone picks it from the nearby list.
    pc.node.pairing_start_qr().await.unwrap();
    let addrs = pc.node.direct_addrs().await;
    phone.node.add_known_addrs(pc.node.device_id(), &addrs);
    (pc, phone, addrs)
}

#[tokio::test(flavor = "multi_thread")]
async fn nearby_pairing_with_matching_codes() {
    let (mut pc, mut phone, _) = start_nearby_setup().await;
    let pc_id = pc.node.device_id();
    let initiator = phone.node.clone();
    let task = tokio::spawn(async move { initiator.pairing_start_nearby(pc_id).await });

    let (phone_code, pc_code) = (sas_code(&mut phone).await, sas_code(&mut pc).await);
    assert_eq!(phone_code, pc_code, "both screens show the same code");
    assert_eq!(phone_code.len(), 6);

    phone.node.pairing_confirm(true).unwrap();
    pc.node.pairing_confirm(true).unwrap();
    with_timeout("nearby pairing", task).await.unwrap().expect("pairing succeeds");

    wait_online(&mut phone, pc_id).await;
    assert_eq!(pc.node.paired_devices().unwrap().len(), 1);
    assert_eq!(phone.node.paired_devices().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn nearby_pairing_fails_if_either_user_declines() {
    let (mut pc, mut phone, _) = start_nearby_setup().await;
    let pc_id = pc.node.device_id();
    let initiator = phone.node.clone();
    let task = tokio::spawn(async move { initiator.pairing_start_nearby(pc_id).await });

    sas_code(&mut phone).await;
    sas_code(&mut pc).await;
    phone.node.pairing_confirm(true).unwrap();
    pc.node.pairing_confirm(false).unwrap(); // codes didn't match on the PC

    let result = with_timeout("nearby pairing", task).await.unwrap();
    assert!(matches!(result, Err(Error::Denied)), "the phone learns the PC rejected: {result:?}");
    wait_for(&mut pc, "declined", |e| match e {
        NodeEvent::Pairing(PairingEvent::Failed(PairingFailure::Declined)) => Some(()),
        _ => None,
    })
    .await;
    assert!(pc.node.paired_devices().unwrap().is_empty());
    assert!(phone.node.paired_devices().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn second_nearby_pairing_is_refused_while_one_awaits_confirmation() {
    let (mut pc, mut phone_a, addrs) = start_nearby_setup().await;
    let pc_id = pc.node.device_id();
    let phone_b = device("Other", DeviceKind::Phone).await;
    phone_b.node.add_known_addrs(pc_id, &addrs);

    let initiator_a = phone_a.node.clone();
    let task_a = tokio::spawn(async move { initiator_a.pairing_start_nearby(pc_id).await });
    let (code_a, code_pc) = (sas_code(&mut phone_a).await, sas_code(&mut pc).await);
    assert_eq!(code_a, code_pc);

    // A second phone tries to pair while the PC is waiting for the user to
    // confirm the first phone's 6-digit code.
    let second = with_timeout("second nearby", phone_b.node.pairing_start_nearby(pc_id)).await;
    assert!(matches!(second, Err(Error::Denied)), "second ceremony is refused: {second:?}");

    // The first ceremony's decision channel is untouched and still completes.
    phone_a.node.pairing_confirm(true).unwrap();
    pc.node.pairing_confirm(true).unwrap();
    with_timeout("first nearby", task_a).await.unwrap().expect("first pairing succeeds");
    assert_eq!(pc.node.paired_devices().unwrap().len(), 1);
    assert_eq!(pc.node.paired_devices().unwrap()[0].id, phone_a.node.device_id());
    assert!(phone_b.node.paired_devices().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn too_many_failed_nearby_attempts_end_pairing_mode() {
    let (mut pc, mut phone, _) = start_nearby_setup().await;
    let pc_id = pc.node.device_id();
    for _ in 0..5 {
        let initiator = phone.node.clone();
        let task = tokio::spawn(async move { initiator.pairing_start_nearby(pc_id).await });
        sas_code(&mut phone).await;
        sas_code(&mut pc).await;
        phone.node.pairing_confirm(false).unwrap();
        let _ = with_timeout("nearby attempt", task).await.unwrap();
        wait_for(&mut pc, "attempt failed", |e| match e {
            NodeEvent::Pairing(PairingEvent::Failed(_)) => Some(()),
            _ => None,
        })
        .await;
    }
    assert!(!pc.node.is_pairing(), "pairing mode ends after 5 failed nearby attempts");
}

/// A pairing link another app opened isn't trusted for its secret: both devices
/// show a fresh code, and pairing waits for both users.
#[tokio::test(flavor = "multi_thread")]
async fn link_from_another_app_compares_a_code_on_both_devices() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    // Only the link tells the phone where the PC is.
    let link = pc.node.pairing_start_qr().await.unwrap().to_uri();
    let initiator = phone.node.clone();
    let task = tokio::spawn(async move { initiator.pairing_join_link(&link).await });

    let phone_code = wait_for(&mut phone, "the phone's code", |e| match e {
        NodeEvent::Pairing(PairingEvent::SasCode { peer, code }) if *peer == pc_id => Some(code.clone()),
        _ => None,
    })
    .await;
    let pc_code = wait_for(&mut pc, "the PC's code", |e| match e {
        NodeEvent::Pairing(PairingEvent::SasCode { peer, code }) if *peer == phone_id => Some(code.clone()),
        _ => None,
    })
    .await;
    assert_eq!(phone_code, pc_code, "both screens show the same code");

    // The phone's yes alone pairs nothing.
    phone.node.pairing_confirm(true).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!task.is_finished(), "pairing waits for the PC's user too");
    assert!(pc.node.paired_devices().unwrap().is_empty());
    assert!(phone.node.paired_devices().unwrap().is_empty());

    pc.node.pairing_confirm(true).unwrap();
    with_timeout("link pairing", task).await.unwrap().expect("pairing succeeds");
    wait_online(&mut phone, pc_id).await;
    assert_eq!(pc.node.paired_devices().unwrap()[0].id, phone_id);
    assert_eq!(phone.node.paired_devices().unwrap()[0].id, pc_id);
}

/// A link swapped for one to another device in pairing mode, under the PC's
/// name: the user's PC never shows a code, so the user says no.
#[tokio::test(flavor = "multi_thread")]
async fn substituted_link_from_another_app_shows_no_code_on_the_real_pc() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    let mut other = device("Desktop", DeviceKind::Desktop).await;
    pc.node.pairing_start_qr().await.unwrap();
    let other_id = other.node.device_id();
    let link = other.node.pairing_start_qr().await.unwrap().to_uri();
    let initiator = phone.node.clone();
    let task = tokio::spawn(async move { initiator.pairing_join_link(&link).await });

    let shown_to = wait_for(&mut phone, "the phone's code", |e| match e {
        NodeEvent::Pairing(PairingEvent::SasCode { peer, .. }) => Some(*peer),
        _ => None,
    })
    .await;
    assert_eq!(shown_to, other_id, "the code is compared with the device the link names");
    sas_code(&mut other).await;
    while let Ok(event) = pc.events.try_recv() {
        assert!(!matches!(event, NodeEvent::Pairing(PairingEvent::SasCode { .. })), "{event:?}");
    }

    phone.node.pairing_confirm(false).unwrap();
    let result = with_timeout("link pairing", task).await.unwrap();
    assert!(matches!(result, Err(Error::Declined)), "got {result:?}");
    assert!(phone.node.paired_devices().unwrap().is_empty());
    assert!(other.node.paired_devices().unwrap().is_empty());
    assert!(pc.node.paired_devices().unwrap().is_empty());
}

/// A link read by the app's own scanner still pairs on its secret alone.
#[tokio::test(flavor = "multi_thread")]
async fn scanned_link_pairs_without_a_code() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    let link = pc.node.pairing_start_qr().await.unwrap().to_uri();
    with_timeout("join", phone.node.pairing_join(&link)).await.expect("pairing succeeds");
    assert_eq!(phone.node.paired_devices().unwrap().len(), 1);
    for dev in [&mut pc, &mut phone] {
        while let Ok(event) = dev.events.try_recv() {
            assert!(!matches!(event, NodeEvent::Pairing(PairingEvent::SasCode { .. })), "{event:?}");
        }
    }
}

/// With nobody to compare a code on the other end, a link from another app
/// fails; it never falls back to the link's secret.
#[tokio::test(flavor = "multi_thread")]
async fn link_from_another_app_fails_without_a_code_check() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    let link = pc.node.pairing_start_qr().await.unwrap().to_uri();
    // The PC left its pairing screen: it takes neither a code nor the secret.
    pc.node.pairing_cancel();

    let result = with_timeout("link pairing", phone.node.pairing_join_link(&link)).await;
    assert!(result.is_err(), "pairing without the PC's code check must fail");
    wait_for(&mut phone, "pairing failure", |e| match e {
        NodeEvent::Pairing(PairingEvent::Failed(_)) => Some(()),
        _ => None,
    })
    .await;
    for dev in [&mut pc, &mut phone] {
        while let Ok(event) = dev.events.try_recv() {
            assert!(!matches!(event, NodeEvent::Pairing(PairingEvent::SasCode { .. })), "{event:?}");
        }
    }
    assert!(pc.node.paired_devices().unwrap().is_empty());
    assert!(phone.node.paired_devices().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn unpairing_is_mirrored_on_the_other_device() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let pc_id = pc.node.device_id();

    with_timeout("ring before unpair", phone.node.ring(pc_id, true)).await.unwrap();
    with_timeout("unpair", phone.node.unpair(pc_id)).await.unwrap();
    let phone_id = phone.node.device_id();
    wait_for(&mut pc, "device removed", |e| match e {
        NodeEvent::DeviceRemoved(id) if *id == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert_eq!(*pc.platform.rings.lock().unwrap(), [true, false]);
    assert!(pc.node.paired_devices().unwrap().is_empty());
    assert!(phone.node.paired_devices().unwrap().is_empty());
    assert!(matches!(pc.node.ring(phone_id, true).await, Err(Error::NotPaired)));
}

/// Polls `check` until it gives a value: for what's stored without an event.
async fn poll_until<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    with_timeout(what, async {
        loop {
            if let Some(found) = check() {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_says_how_it_is_reached_and_a_typed_address_lasts() {
    use g7link_core::reach::PeerAddressOutcome;

    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let pc_id = pc.node.device_id();

    // On connect the PC says how it's reached: Away mode off, and only its
    // tailnet addresses (this machine may have some; CI has none).
    let reach = poll_until("the PC's reach", || phone.node.peer_reach(pc_id).unwrap()).await;
    assert!(!reach.away);
    assert!(reach.tailnet.iter().all(|a| g7link_core::is_tailnet_ip(a.ip())), "only tailnet addresses");

    // A change of its Away mode is heard while connected.
    pc.node.set_away_mode(true);
    poll_until("the PC's Away mode on", || phone.node.peer_reach(pc_id).unwrap().filter(|r| r.away)).await;
    pc.node.set_away_mode(false);
    poll_until("the PC's Away mode off", || phone.node.peer_reach(pc_id).unwrap().filter(|r| !r.away)).await;

    // An address typed for the PC is saved and dialed, and both last a restart.
    let typed: SocketAddr = "100.64.0.9:5000".parse().unwrap();
    assert_eq!(
        phone.node.set_peer_address(pc_id, "100.64.0.9:5000").await.unwrap(),
        PeerAddressOutcome::Ready
    );
    assert!(phone.node.dial_addrs(pc_id).contains(&typed));
    phone.node.shutdown().await;
    let TestDevice { node, dir, .. } = phone;
    drop(node);
    let phone = start_in(dir, "Pixel", DeviceKind::Phone).await;
    assert_eq!(phone.node.peer_address(pc_id).unwrap().as_deref(), Some("100.64.0.9:5000"));
    assert!(phone.node.dial_addrs(pc_id).contains(&typed), "dialed after the restart");
    assert!(phone.node.peer_reach(pc_id).unwrap().is_some(), "what the PC said is kept");

    assert_eq!(phone.node.set_peer_address(pc_id, "").await.unwrap(), PeerAddressOutcome::Cleared);
    assert_eq!(phone.node.peer_address(pc_id).unwrap(), None);
    assert!(!phone.node.dial_addrs(pc_id).contains(&typed));
    assert_eq!(phone.node.set_peer_address(pc_id, "http://x").await.unwrap(), PeerAddressOutcome::Invalid);
    assert_eq!(phone.node.peer_address(pc_id).unwrap(), None, "nothing saved");

    // A public address with Away mode off: saved, never dialed.
    let public: std::net::IpAddr = "203.0.113.7".parse().unwrap();
    assert_eq!(
        phone.node.set_peer_address(pc_id, "203.0.113.7").await.unwrap(),
        PeerAddressOutcome::NotAllowed
    );
    assert_eq!(phone.node.peer_address(pc_id).unwrap().as_deref(), Some("203.0.113.7"));
    assert!(phone.node.dial_addrs(pc_id).iter().all(|a| a.ip() != public));
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnects_after_a_restart() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let old_ports: std::collections::BTreeSet<u16> =
        phone.node.direct_addrs().await.iter().map(SocketAddr::port).collect();
    // The phone restarts (app update, reboot...).
    phone.node.shutdown().await;
    wait_for(&mut pc, "peer offline", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    let ports = |addrs: Vec<SocketAddr>| {
        addrs.iter().map(SocketAddr::port).collect::<std::collections::BTreeSet<_>>()
    };
    // A restart is a new process: the old node and its sockets are gone.
    let TestDevice { node: old_node, dir, .. } = phone;
    drop(old_node);
    let mut phone = start_in(dir, "Pixel", DeviceKind::Phone).await;
    // The port is kept, unless something else took it in the meantime
    // (other tests run in parallel); then any free port is fine.
    let new_ports = ports(phone.node.direct_addrs().await);
    if new_ports != old_ports {
        let taken = old_ports.iter().any(|p| std::net::UdpSocket::bind(("0.0.0.0", *p)).is_err());
        assert!(taken, "the port is kept across restarts when it's free: {old_ports:?} -> {new_ports:?}");
    }
    assert_eq!(phone.node.device_id(), phone_id, "identity survives the restart");
    assert_eq!(phone.node.paired_devices().unwrap().len(), 1, "pairing survives the restart");

    wait_online(&mut phone, pc_id).await;
    wait_online(&mut pc, phone_id).await;
    with_timeout("ring after reconnect", pc.node.ring(phone_id, true)).await.expect("ring works again");
}

/// "Stop reconnecting" (GitHub #48): the device disconnects and stays
/// disconnected, though the other side keeps dialing, across a restart too,
/// until it's turned back on.
#[tokio::test(flavor = "multi_thread")]
async fn stop_reconnecting_keeps_a_device_disconnected_until_turned_back_on() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let offline = |dev: &TestDevice| {
        dev.node.paired_devices().unwrap().iter().all(|d| !matches!(d.link, LinkState::Online { .. }))
    };

    phone.node.set_reconnect(pc_id, false).unwrap();
    wait_for(&mut phone, "reconnect off", |e| match e {
        NodeEvent::ReconnectChanged { device, on: false } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    wait_for(&mut pc, "peer offline", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert!(!phone.node.paired_devices().unwrap()[0].reconnect);
    assert!(pc.node.paired_devices().unwrap()[0].reconnect, "only the phone stopped");
    // The PC dials again with its backoff (1 s, then 2 s); the phone refuses.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(offline(&pc) && offline(&phone), "still disconnected while the PC keeps dialing");

    // Kept across a restart.
    phone.node.shutdown().await;
    let TestDevice { node: old_node, dir, .. } = phone;
    drop(old_node);
    let mut phone = start_in(dir, "Pixel", DeviceKind::Phone).await;
    assert!(!phone.node.paired_devices().unwrap()[0].reconnect, "the setting survives a restart");
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(offline(&pc) && offline(&phone), "still disconnected after the restart");

    // Reconnect: it dials right away.
    phone.node.set_reconnect(pc_id, true).unwrap();
    wait_online(&mut phone, pc_id).await;
    wait_online(&mut pc, phone_id).await;
    assert!(phone.node.paired_devices().unwrap()[0].reconnect);
    // Both sides dial at once after the switch, so the first link can still be replaced by the
    // second just after "online": the ring has to work within a moment, not on the first try.
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        match with_timeout("ring after reconnecting", pc.node.ring(phone_id, true)).await {
            Ok(_) => break,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(e) => panic!("ring works again: {e:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phone_stopped_and_started_again_in_the_same_process_reconnects() {
    // With its background connection off, the phone app stops the node when it goes to the
    // background and starts a new one when it's opened, in the same process, while the old
    // handle may still be around. Stopping has to let go of everything a new node binds.
    let usb_base = loop {
        let base = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let Some(last) = base.checked_add(g7link_core::USB_PORT_COUNT) else { continue };
        if (base..=last).all(|p| std::net::TcpListener::bind(("127.0.0.1", p)).is_ok()) {
            break base;
        }
    };
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = start_usb_device("Pixel", DeviceKind::Phone, Some(usb_base), &[]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let usb_port = phone.node.usb_port().expect("the USB listener is up");

    for round in 0..3 {
        phone.node.shutdown().await;
        wait_for(&mut pc, "the phone goes offline", |e| match e {
            NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if *device == phone_id => {
                Some(())
            }
            _ => None,
        })
        .await;
        assert!(phone.node.paired_devices().is_ok(), "a stopped node still answers what it stores");
        assert!(matches!(phone.node.ring(pc_id, true).await, Err(Error::Offline)), "and has no connections");

        let TestDevice { node: stopped, dir, .. } = phone;
        phone = start_usb_device_in(dir, "Pixel", DeviceKind::Phone, Some(usb_base), &[]).await;
        drop(stopped);
        assert_eq!(phone.node.device_id(), phone_id, "round {round}: same identity");
        assert_eq!(phone.node.usb_port(), Some(usb_port), "round {round}: the USB port was let go");

        wait_online(&mut phone, pc_id).await;
        wait_online(&mut pc, phone_id).await;
        with_timeout("ring after starting again", pc.node.ring(phone_id, round % 2 == 0))
            .await
            .expect("connected again");
    }
}

/// A PC tells a phone that may control it when a text box gets the focus,
/// and tells no phone that may not.
#[tokio::test(flavor = "multi_thread")]
async fn a_pc_says_when_a_text_box_has_the_focus() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &["input.inject"]).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Remote input is off for the phone: nothing goes.
    with_timeout("focus while off", pc.node.remote_text_focus(phone_id, true)).await.unwrap();
    pc.node.set_device_toggle(phone_id, "remote_input", true).unwrap();
    with_timeout("focus", pc.node.remote_text_focus(phone_id, true)).await.unwrap();
    let first = wait_for(&mut phone, "text focus", |e| match e {
        NodeEvent::PcTextFocus { device, editable } => Some((*device, *editable)),
        _ => None,
    })
    .await;
    // The first the phone hears is the one sent while remote input was on.
    assert_eq!(first, (pc_id, true));
    with_timeout("blur", pc.node.remote_text_focus(phone_id, false)).await.unwrap();
    let second = wait_for(&mut phone, "text blur", |e| match e {
        NodeEvent::PcTextFocus { device, editable } => Some((*device, *editable)),
        _ => None,
    })
    .await;
    assert_eq!(second, (pc_id, false));
}

/// A PC tells a phone what its clipboard got, for the phone's Paste chip: only
/// with the clipboard on for it, and a preview only with text.
#[tokio::test(flavor = "multi_thread")]
async fn a_pc_says_what_its_clipboard_got() {
    use g7link_core::PcClipKind;

    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let pick = |e: &NodeEvent| match e {
        NodeEvent::PcClip { device, kind, preview } => Some((*device, *kind, preview.clone())),
        _ => None,
    };

    // The clipboard is off for the phone: nothing goes.
    pc.node.set_device_toggle(phone_id, "clipboard", false).unwrap();
    with_timeout("clip while off", pc.node.announce_clip(phone_id, PcClipKind::Image, None)).await.unwrap();
    pc.node.set_device_toggle(phone_id, "clipboard", true).unwrap();

    // Text the PC doesn't send the phone by itself (automatic sending is off): no preview.
    with_timeout("text", pc.node.announce_clip(phone_id, PcClipKind::Text, None)).await.unwrap();
    // The first the phone hears is the one sent with the clipboard on.
    assert_eq!(wait_for(&mut phone, "text", pick).await, (pc_id, PcClipKind::Text, None));

    let preview = Some("Meeting at 3".to_owned());
    with_timeout("previewed", pc.node.announce_clip(phone_id, PcClipKind::Text, preview.clone()))
        .await
        .unwrap();
    assert_eq!(wait_for(&mut phone, "previewed", pick).await, (pc_id, PcClipKind::Text, preview));

    // Nothing of a private clip is told, whatever the caller passes.
    let leaked = Some("hunter2".to_owned());
    with_timeout("private", pc.node.announce_clip(phone_id, PcClipKind::Private, leaked)).await.unwrap();
    assert_eq!(wait_for(&mut phone, "private", pick).await, (pc_id, PcClipKind::Private, None));
}

/// A phone that refuses the PC's taps (Xiaomi without USB debugging (Security
/// settings)) says so, and the PC hears which window it's about.
#[tokio::test(flavor = "multi_thread")]
async fn a_phone_that_refuses_taps_tells_the_pc() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::MIRROR_VIEW]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::MIRROR_CAPTURE]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    with_timeout("notice", phone.node.mirror_notice(pc_id, 2, g7link_core::MIRROR_NOTICE_INPUT_BLOCKED))
        .await
        .unwrap();
    let (device, session, code) = wait_for(&mut pc, "the notice", |e| match e {
        NodeEvent::MirrorNotice { device, session, code } => Some((*device, *session, code.clone())),
        _ => None,
    })
    .await;
    assert_eq!((device, session, code.as_str()), (phone_id, 2, g7link_core::MIRROR_NOTICE_INPUT_BLOCKED));
}

/// Each packet is reported back to the sender, and well past what a device
/// from before reports reads (64 bytes): the stream carries on.
#[tokio::test(flavor = "multi_thread")]
async fn a_video_stream_outlasts_its_reports() {
    use g7link_core::{MirrorSend, PacketKind};

    let pc_offers = [g7link_core::MIRROR_VIEW];
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &pc_offers).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::MIRROR_CAPTURE]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let options = g7link_core::MirrorStart {
        max_size: 1280,
        fps: 30,
        bitrate: 4_000_000,
        audio: false,
        session: 0,
        app: None,
        stay_awake: false,
        screen_off: false,
        watch: None,
        sound_only: false,
    };
    with_timeout("start", pc.node.mirror_start(phone_id, options)).await.unwrap();
    let stream = Arc::new(with_timeout("open", phone.node.mirror_open(pc_id)).await.unwrap());
    let config = g7link_core::MirrorConfig { codec: "h264".into(), width: 1280, height: 720, session: 0 };
    let sender = stream.clone();
    tokio::task::spawn_blocking(move || {
        assert_eq!(sender.send(PacketKind::Config, 0, config.to_cbor()), MirrorSend::Queued);
        assert_eq!(sender.send(PacketKind::Keyframe, 0, vec![0; 2_000]), MirrorSend::Queued);
    })
    .await
    .unwrap();
    wait_for(&mut pc, "showing", |e| matches!(e, NodeEvent::Mirroring { on: true, .. }).then_some(())).await;

    let screen = pc.platform.screen.clone();
    let packets = 300u64;
    let sender = stream.clone();
    tokio::task::spawn_blocking(move || {
        for time in 1..=packets {
            while sender.send(PacketKind::Frame, time, vec![0; 500]) != MirrorSend::Queued {
                assert!(!sender.is_closed(), "the stream ended at {time}");
                sender.send(PacketKind::Keyframe, time, vec![0; 500]);
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    })
    .await
    .unwrap();
    let deadline = tokio::time::Instant::now() + WAIT;
    let last = format!("frame {packets} 500");
    while !screen.got.lock().unwrap().contains(&last) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(screen.got.lock().unwrap().contains(&last), "the last frame arrives");
    assert!(!stream.is_closed());
    assert_eq!(stream.link_measure(), None, "a clear link never makes it wait");
}

#[tokio::test(flavor = "multi_thread")]
async fn mirror_open_waits_for_a_connection_that_is_coming_back() {
    let pc_offers = [g7link_core::MIRROR_VIEW, g7link_core::MIRROR_LISTEN];
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &pc_offers).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::MIRROR_CAPTURE]).await;
    pair_qr(&mut pc, &mut phone).await;
    let pc_id = pc.node.device_id();

    // The PC drops off right after asking for the screen.
    pc.node.shutdown().await;
    wait_for(&mut phone, "pc offline", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;

    // If it stays away, opening gives up after a while instead of hanging.
    let started = std::time::Instant::now();
    let gone = with_timeout("open while away", phone.node.mirror_open(pc_id)).await;
    assert!(matches!(gone, Err(Error::Offline)), "{:?}", gone.err());
    assert!(started.elapsed() >= Duration::from_secs(5), "waits for the PC first");

    // If it comes back meanwhile, the stream opens on the new connection.
    let TestDevice { node: old_pc, dir, .. } = pc;
    drop(old_pc);
    let (opened, _pc) =
        tokio::join!(with_timeout("open while reconnecting", phone.node.mirror_open(pc_id)), async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let pc = start_in(dir, "Desktop", DeviceKind::Desktop).await;
            let mut caps = vec!["media.control".to_owned()];
            caps.extend(pc_offers.iter().map(|c| (*c).to_owned()));
            pc.node.update_power(PowerLevel::NotApplicable, caps).await;
            pc
        });
    assert!(opened.is_ok(), "opens once the PC is back: {:?}", opened.err());
}

/// Waits for a capability matrix of `peer` whose `feature` is in a state
/// matching `want`.
async fn wait_feature(
    dev: &mut TestDevice,
    peer: g7link_core::DeviceId,
    feature: &str,
    want: impl Fn(&FeatureState) -> bool,
) -> FeatureState {
    wait_for(dev, feature, |e| match e {
        NodeEvent::Capabilities(m) if m.device == peer => m.state(feature).filter(|s| want(s)),
        _ => None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn capabilities_follow_power_and_toggles() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Right after connecting, the PC knows what the phone offers.
    let matrix = pc.node.capabilities(phone_id).unwrap();
    assert_eq!(matrix.state("device.find_phone"), Some(FeatureState::Available));
    let elevated =
        Upgrade { action: UpgradeAction::RaisePower(PowerLevel::Elevated), effort: Effort::Minutes(2) };
    assert_eq!(matrix.state("clipboard.auto_phone_to_pc"), Some(FeatureState::Locked { upgrade: elevated }));
    // Both devices compute the same matrix for the pair.
    assert_eq!(phone.node.capabilities(pc_id).unwrap().features, matrix.features);

    // The phone is raised to Elevated and now offers automatic clipboard.
    phone.node.update_power(PowerLevel::Elevated, vec!["clip.read.auto".into(), "clip.write".into()]).await;
    wait_feature(&mut pc, phone_id, "clipboard.auto_phone_to_pc", |s| *s == FeatureState::Available).await;

    // The PC user turns clipboard off for this phone.
    pc.node.set_device_toggle(phone_id, "clipboard", false).unwrap();
    let state =
        wait_feature(&mut pc, phone_id, "clipboard.pc_to_phone", |s| *s != FeatureState::Available).await;
    assert_eq!(
        state,
        FeatureState::Locked {
            upgrade: Upgrade {
                action: UpgradeAction::EnableDeviceToggle("clipboard"),
                effort: Effort::Instant
            }
        }
    );
    let toggles = pc.node.device_toggles(phone_id).unwrap();
    assert!(toggles.contains(&("clipboard", false)) && toggles.contains(&("photos", true)));
    assert!(matches!(pc.node.set_device_toggle(phone_id, "no-such-toggle", true), Err(Error::Unsupported)));

    // What the phone offered is remembered while it's offline.
    phone.node.shutdown().await;
    wait_for(&mut pc, "peer offline", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    pc.node.set_device_toggle(phone_id, "clipboard", true).unwrap();
    let matrix = pc.node.capabilities(phone_id).unwrap();
    assert_eq!(matrix.state("clipboard.auto_phone_to_pc"), Some(FeatureState::Available));
}

fn note(key: &str, app: &str, title: &str, icon: bool) -> Notification {
    Notification {
        key: key.into(),
        app: app.into(),
        app_name: "Chat".into(),
        title: Some(title.into()),
        text: Some("Are you coming?".into()),
        sub: None,
        when: 1_760_000_000_000,
        actions: vec![
            NotificationAction { id: "0".into(), title: "Reply".into(), reply: true },
            NotificationAction { id: "1".into(), title: "Mark as read".into(), reply: false },
        ],
        silent: false,
        ongoing: false,
        icon: icon.then(|| vec![0x89, b'P', b'N', b'G', 1, 2, 3]),
        image: None,
        live: None,
        conversation: None,
    }
}

/// A phone that mirrors notifications (notification access granted).
async fn grant_notification_access(phone: &TestDevice) {
    phone.node.update_power(PowerLevel::Basic, vec!["notify.mirror".into(), "notify.reply".into()]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn notifications_mirror_to_the_pc_and_actions_reach_the_phone() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    grant_notification_access(&phone).await;

    // The listener connects: everything showing goes over as a snapshot,
    // with each app's icon once.
    phone
        .node
        .notifications_reset(vec![note("a", "com.chat", "Sam", true), note("b", "com.chat", "Alex", true)])
        .await;
    let items = wait_for(&mut pc, "snapshot", |e| match e {
        NodeEvent::NotificationsReset { device, items } if *device == phone_id => Some(items.clone()),
        _ => None,
    })
    .await;
    assert_eq!(items.len(), 2);
    assert_eq!(items.iter().filter(|n| n.icon.is_some()).count(), 1, "one icon per app and session");

    // A new one: the PC already has this app's icon.
    phone.node.notification_posted(note("c", "com.chat", "Kim", true)).await;
    let posted = wait_for(&mut pc, "posted", |e| match e {
        NodeEvent::NotificationPosted { device, notification } if *device == phone_id => {
            Some(notification.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(posted.key, "c");
    assert_eq!(posted.title.as_deref(), Some("Kim"));
    assert_eq!(posted.icon, None);

    phone.node.notification_removed("a".into()).await;
    let removed = wait_for(&mut pc, "removed", |e| match e {
        NodeEvent::NotificationRemoved { device, key } if *device == phone_id => Some(key.clone()),
        _ => None,
    })
    .await;
    assert_eq!(removed, "a");

    // The PC dismisses one and replies to another.
    with_timeout("dismiss", pc.node.dismiss_notification(phone_id, "b".into())).await.expect("dismissed");
    with_timeout(
        "reply",
        pc.node.run_notification_action(phone_id, "c".into(), "0".into(), Some("Yes!".into())),
    )
    .await
    .expect("replied");
    assert_eq!(*phone.platform.dismissed.lock().unwrap(), vec!["b".to_owned()]);
    assert_eq!(*phone.platform.actions.lock().unwrap(), vec![("c".into(), "0".into(), Some("Yes!".into()))]);

    let gone =
        with_timeout("gone", pc.node.run_notification_action(phone_id, "gone".into(), "1".into(), None))
            .await;
    assert!(matches!(gone, Err(Error::NotFound)), "{gone:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_pc_gets_no_notifications_and_catches_up_afterwards() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    grant_notification_access(&phone).await;
    phone.node.notifications_reset(vec![note("a", "com.chat", "Sam", false)]).await;
    wait_for(&mut pc, "first snapshot", |e| match e {
        NodeEvent::NotificationsReset { device, items } if *device == phone_id && items.len() == 1 => {
            Some(())
        }
        _ => None,
    })
    .await;

    pc.node.hold_notifications(true).await;
    // The hold travels on the notify lane: wait until the phone has it before posting.
    let pc_id = pc.node.device_id();
    until("the phone holds", || phone.node.debug_notifications_held(pc_id)).await;
    phone.node.notification_posted(note("b", "com.chat", "Alex", false)).await;
    phone
        .node
        .notifications_reset(vec![note("a", "com.chat", "Sam", false), note("b", "com.chat", "Alex", false)])
        .await;

    pc.node.hold_notifications(false).await;
    // Lists from before the hold may still be waiting here (they repeat), so wait for
    // the one with both. Nothing posted while held got through ahead of it.
    wait_for(&mut pc, "catch-up", |e| match e {
        NodeEvent::NotificationPosted { device, .. } if *device == phone_id => {
            panic!("a notification came through while held")
        }
        NodeEvent::NotificationsReset { device, items } if *device == phone_id && items.len() == 2 => {
            Some(())
        }
        _ => None,
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn notification_toggles_are_honored_on_both_devices() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    grant_notification_access(&phone).await;
    phone.node.notifications_reset(vec![note("a", "com.chat", "Sam", false)]).await;
    // Snapshots are idempotent and may repeat (one on connect, one when
    // the phone's list is reset), so wait for the one with the expected size.
    let snapshot = |expected: usize| {
        move |e: &NodeEvent| match e {
            NodeEvent::NotificationsReset { device, items }
                if *device == phone_id && items.len() == expected =>
            {
                Some(())
            }
            _ => None,
        }
    };
    wait_for(&mut pc, "first snapshot", snapshot(1)).await;

    // The phone stops sharing with this PC: the PC is cleared, and actions
    // from it are refused.
    phone.node.set_device_toggle(pc_id, "notifications", false).unwrap();
    wait_for(&mut pc, "cleared by the phone", snapshot(0)).await;
    let denied = with_timeout("denied", pc.node.dismiss_notification(phone_id, "a".into())).await;
    assert!(matches!(denied, Err(Error::Denied)), "{denied:?}");
    phone.node.set_device_toggle(pc_id, "notifications", true).unwrap();
    wait_for(&mut pc, "shared again", snapshot(1)).await;

    // The PC hides this phone's notifications, then shows them again: it
    // asks the phone for what's showing.
    pc.node.set_device_toggle(phone_id, "notifications", false).unwrap();
    wait_for(&mut pc, "hidden on the PC", snapshot(0)).await;
    pc.node.set_device_toggle(phone_id, "notifications", true).unwrap();
    wait_for(&mut pc, "synced again", snapshot(1)).await;
}

/// Unlike other toggles, `bt_media` is kept the same on both devices: a
/// change on either reaches the other, which doesn't send it back.
#[tokio::test(flavor = "multi_thread")]
async fn bt_media_is_kept_the_same_on_both_devices() {
    use g7link_core::BT_MEDIA_TOGGLE;
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let allowed = |dev: &TestDevice, peer: g7link_core::DeviceId| {
        dev.node.device_toggles(peer).unwrap().contains(&(BT_MEDIA_TOGGLE, true))
    };
    let changed_by = |from: g7link_core::DeviceId, to: bool| {
        move |e: &NodeEvent| match e {
            NodeEvent::DeviceToggleChanged { device, toggle, on }
                if *device == from && toggle == BT_MEDIA_TOGGLE && *on == to =>
            {
                Some(())
            }
            _ => None,
        }
    };
    assert!(!allowed(&pc, phone_id) && !allowed(&phone, pc_id), "off by default on both");

    // Turned on and straight off again on the PC: the phone ends up off.
    pc.node.set_device_toggle(phone_id, BT_MEDIA_TOGGLE, true).unwrap();
    pc.node.set_device_toggle(phone_id, BT_MEDIA_TOGGLE, false).unwrap();
    wait_for(&mut phone, "on from the PC", changed_by(pc_id, true)).await;
    wait_for(&mut phone, "off from the PC", changed_by(pc_id, false)).await;
    assert!(!allowed(&phone, pc_id));
    // No echo: an "on" sent back would turn the PC's on again.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!allowed(&pc, phone_id), "the phone didn't send the change back");
    while let Ok(event) = pc.events.try_recv() {
        assert!(!matches!(event, NodeEvent::DeviceToggleChanged { .. }), "echoed: {event:?}");
    }

    // On the phone: the PC follows.
    phone.node.set_device_toggle(pc_id, BT_MEDIA_TOGGLE, true).unwrap();
    wait_for(&mut pc, "on from the phone", changed_by(phone_id, true)).await;
    assert!(allowed(&pc, phone_id) && allowed(&phone, pc_id));
    tokio::time::sleep(Duration::from_secs(1)).await;
    while let Ok(event) = phone.events.try_recv() {
        assert!(!matches!(event, NodeEvent::DeviceToggleChanged { .. }), "echoed: {event:?}");
    }
}

fn bt_media_on(dev: &TestDevice, peer: g7link_core::DeviceId) -> bool {
    dev.node.device_toggles(peer).unwrap().contains(&(g7link_core::BT_MEDIA_TOGGLE, true))
}

fn bt_media_from(from: g7link_core::DeviceId, to: bool) -> impl FnMut(&NodeEvent) -> Option<()> {
    move |e| match e {
        NodeEvent::DeviceToggleChanged { device, toggle, on }
            if *device == from && toggle == g7link_core::BT_MEDIA_TOGGLE && *on == to =>
        {
            Some(())
        }
        _ => None,
    }
}

/// Waits a second, then fails if `dev` reported a `bt_media` change since
/// its events were last read.
async fn no_bt_media_change(dev: &mut TestDevice) {
    tokio::time::sleep(Duration::from_secs(1)).await;
    while let Ok(event) = dev.events.try_recv() {
        assert!(!matches!(event, NodeEvent::DeviceToggleChanged { .. }), "{}: {event:?}", dev.name);
    }
}

fn online(dev: &TestDevice) -> bool {
    dev.node.paired_devices().unwrap().iter().any(|d| matches!(d.link, LinkState::Online { .. }))
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Disconnects the phone from the PC ("Stop reconnecting") until [`together`].
async fn apart(pc: &TestDevice, phone: &TestDevice) {
    phone.node.set_reconnect(pc.node.device_id(), false).unwrap();
    until("both offline", || !online(pc) && !online(phone)).await;
}

/// Connects the two again. Events aren't read, so a change sent on connect
/// is still there to wait for.
async fn together(pc: &TestDevice, phone: &TestDevice) {
    phone.node.set_reconnect(pc.node.device_id(), true).unwrap();
    until("both online", || online(pc) && online(phone)).await;
}

/// A `bt_media` change made while the other device is offline reaches it when
/// they connect again.
#[tokio::test(flavor = "multi_thread")]
async fn bt_media_changed_while_apart_reaches_the_other_device() {
    use g7link_core::BT_MEDIA_TOGGLE;
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.set_device_toggle(phone_id, BT_MEDIA_TOGGLE, true).unwrap();
    wait_for(&mut phone, "on from the PC", bt_media_from(pc_id, true)).await;

    apart(&pc, &phone).await;
    phone.node.set_device_toggle(pc_id, BT_MEDIA_TOGGLE, false).unwrap();
    assert!(bt_media_on(&pc, phone_id), "the PC doesn't know yet");
    no_bt_media_change(&mut phone).await;

    together(&pc, &phone).await;
    wait_for(&mut pc, "off from the phone", bt_media_from(phone_id, false)).await;
    assert!(!bt_media_on(&pc, phone_id) && !bt_media_on(&phone, pc_id));
    // The PC's older "on", sent as they connected, didn't undo it.
    no_bt_media_change(&mut phone).await;
    assert!(!bt_media_on(&phone, pc_id));
}

/// Changed on both while apart: the later change wins on both.
#[tokio::test(flavor = "multi_thread")]
async fn bt_media_changed_on_both_while_apart_keeps_the_later_change() {
    use g7link_core::BT_MEDIA_TOGGLE;
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    apart(&pc, &phone).await;
    phone.node.set_device_toggle(pc_id, BT_MEDIA_TOGGLE, true).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    pc.node.set_device_toggle(phone_id, BT_MEDIA_TOGGLE, false).unwrap();
    pc.node.set_device_toggle(phone_id, BT_MEDIA_TOGGLE, true).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    phone.node.set_device_toggle(pc_id, BT_MEDIA_TOGGLE, false).unwrap();
    no_bt_media_change(&mut pc).await;
    no_bt_media_change(&mut phone).await;

    // The phone turned it off last.
    together(&pc, &phone).await;
    wait_for(&mut pc, "off from the phone", bt_media_from(phone_id, false)).await;
    no_bt_media_change(&mut phone).await;
    assert!(!bt_media_on(&pc, phone_id) && !bt_media_on(&phone, pc_id));

    // And the other way round: the PC last.
    apart(&pc, &phone).await;
    phone.node.set_device_toggle(pc_id, BT_MEDIA_TOGGLE, false).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    pc.node.set_device_toggle(phone_id, BT_MEDIA_TOGGLE, true).unwrap();
    no_bt_media_change(&mut pc).await;
    no_bt_media_change(&mut phone).await;
    together(&pc, &phone).await;
    wait_for(&mut phone, "on from the PC", bt_media_from(pc_id, true)).await;
    no_bt_media_change(&mut pc).await;
    assert!(bt_media_on(&pc, phone_id) && bt_media_on(&phone, pc_id));
}

/// A device that never changed `bt_media` sends nothing on connect, so its
/// default doesn't replace a choice made on the other one.
#[tokio::test(flavor = "multi_thread")]
async fn a_bt_media_never_changed_doesnt_override_the_other_device() {
    use g7link_core::BT_MEDIA_TOGGLE;
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Never changed on either: nothing happens on connect.
    apart(&pc, &phone).await;
    together(&pc, &phone).await;
    no_bt_media_change(&mut pc).await;
    no_bt_media_change(&mut phone).await;
    assert!(!bt_media_on(&pc, phone_id) && !bt_media_on(&phone, pc_id), "both still off");

    // Turned on on the PC while apart; the phone's default "off" stays home.
    apart(&pc, &phone).await;
    pc.node.set_device_toggle(phone_id, BT_MEDIA_TOGGLE, true).unwrap();
    together(&pc, &phone).await;
    wait_for(&mut phone, "on from the PC", bt_media_from(pc_id, true)).await;
    no_bt_media_change(&mut pc).await;
    assert!(bt_media_on(&pc, phone_id) && bt_media_on(&phone, pc_id));
}

#[tokio::test(flavor = "multi_thread")]
async fn clipboard_text_goes_both_ways_with_consent() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    with_timeout("pc to phone", pc.node.send_clipboard(phone_id, "from the PC".into())).await.expect("sent");
    assert_eq!(*phone.platform.clipboard.lock().unwrap(), ["from the PC"]);
    wait_for(&mut phone, "clipboard received", |e| match e {
        NodeEvent::ClipboardReceived { device } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;

    with_timeout("phone to pc", phone.node.send_clipboard(pc_id, "from the phone ✓".into()))
        .await
        .expect("sent");
    assert_eq!(*pc.platform.clipboard.lock().unwrap(), ["from the phone ✓"]);

    // Rich text (HTML + RTF) accompanies plain text both ways.
    with_timeout(
        "rich pc to phone",
        pc.node.send_clipboard_rich(
            phone_id,
            "Hello world".into(),
            Some("<b>Hello</b> world".into()),
            Some("{\\rtf1\\ansi\\b Hello\\b0  world}".into()),
        ),
    )
    .await
    .expect("rich sent");
    assert_eq!(
        phone.platform.clipboard_rich.lock().unwrap().last(),
        Some(&(
            "Hello world".to_owned(),
            Some("<b>Hello</b> world".to_owned()),
            Some("{\\rtf1\\ansi\\b Hello\\b0  world}".to_owned()),
        )),
    );

    // Oversized rich text markup is dropped so the plain text still arrives.
    let big_rtf = "r".repeat(g7link_core::CLIP_MAX_BYTES);
    with_timeout(
        "oversized rtf dropped",
        pc.node.send_clipboard_rich(
            phone_id,
            "Plain survives".into(),
            Some("<i>Plain</i> survives".into()),
            Some(big_rtf),
        ),
    )
    .await
    .expect("plain + html sent");
    assert_eq!(
        phone.platform.clipboard_rich.lock().unwrap().last(),
        Some(&("Plain survives".to_owned(), Some("<i>Plain</i> survives".to_owned()), None)),
    );

    // The receiver turned the clipboard off for the sender.
    phone.node.set_device_toggle(pc_id, "clipboard", false).unwrap();
    let denied = with_timeout("denied", pc.node.send_clipboard(phone_id, "nope".into())).await;
    assert!(matches!(denied, Err(Error::Denied)), "{denied:?}");
    // The sender turned it off: nothing leaves.
    let not_sent = with_timeout("not sent", phone.node.send_clipboard(pc_id, "nope".into())).await;
    assert!(matches!(not_sent, Err(Error::Denied)), "{not_sent:?}");
    assert_eq!(phone.platform.clipboard.lock().unwrap().len(), 3);

    let huge = "x".repeat(g7link_core::CLIP_MAX_BYTES + 1);
    assert!(matches!(pc.node.send_clipboard(phone_id, huge).await, Err(Error::TooLarge)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_password_arrives_marked_private_and_leaves_no_record() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    phone.node.set_device_toggle(pc_id, "clipboard", true).unwrap();
    pc.node.set_device_toggle(phone_id, "clipboard", true).unwrap();
    with_timeout("send", phone.node.send_clipboard_private(pc_id, "hunter2".into())).await.expect("sent");

    assert_eq!(pc.platform.clipboard_private.lock().unwrap().as_slice(), ["hunter2"]);
    assert!(pc.platform.clipboard.lock().unwrap().is_empty(), "not the normal clipboard call");
    assert!(pc.node.clipboard_history(None).is_empty(), "nothing in the PC's history");
    assert!(phone.node.clipboard_history(None).is_empty(), "nothing in the phone's history");

    // It doesn't stay: the receiver is told to take it off the clipboard.
    let cleared = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if !pc.platform.clipboard_cleared.lock().unwrap().is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(cleared.is_ok(), "the password was left on the clipboard");
    assert_eq!(pc.platform.clipboard_cleared.lock().unwrap().as_slice(), ["hunter2"]);
    assert!(phone.platform.clipboard_cleared.lock().unwrap().is_empty(), "only the receiver clears");

    // Never queued for later.
    let stranger = pc.node.device_id();
    pc.node.shutdown().await;
    let later = with_timeout("offline", phone.node.send_clipboard_private(stranger, "hunter2".into())).await;
    assert!(later.is_err(), "{later:?}");
    assert!(phone.node.send_queue(None).unwrap().is_empty(), "nothing left in the queue");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_newer_password_replaces_the_one_waiting_to_be_cleared() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    phone.node.set_device_toggle(pc_id, "clipboard", true).unwrap();
    pc.node.set_device_toggle(phone_id, "clipboard", true).unwrap();
    with_timeout("first", phone.node.send_clipboard_private(pc_id, "hunter2".into())).await.expect("sent");
    with_timeout("second", phone.node.send_clipboard_private(pc_id, "swordfish".into())).await.expect("sent");

    until("the clipboard cleared", || !pc.platform.clipboard_cleared.lock().unwrap().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    // Only the one still there is cleared; the replaced one was let go, not kept for its timer.
    assert_eq!(pc.platform.clipboard_cleared.lock().unwrap().as_slice(), ["swordfish"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn clipboard_images_go_both_ways_with_consent() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // A large screenshot, and a small photo the other way.
    let screenshot = data(9 * 1024 * 1024 + 17, 3);
    with_timeout(
        "pc to phone",
        pc.node.send_clipboard_image(phone_id, "image/png".into(), screenshot.clone()),
    )
    .await
    .expect("sent");
    assert_eq!(*phone.platform.images.lock().unwrap(), [("image/png".to_owned(), screenshot)]);
    wait_for(&mut phone, "clipboard received", |e| match e {
        NodeEvent::ClipboardReceived { device } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    let photo = data(40_000, 4);
    with_timeout("phone to pc", phone.node.send_clipboard_image(pc_id, "image/jpeg".into(), photo.clone()))
        .await
        .expect("sent");
    assert_eq!(*pc.platform.images.lock().unwrap(), [("image/jpeg".to_owned(), photo)]);

    // Only PNG and JPEG, and not too large.
    let gif = pc.node.send_clipboard_image(phone_id, "image/gif".into(), vec![1; 10]).await;
    assert!(matches!(gif, Err(Error::Internal(_))), "{gif:?}");
    let max = usize::try_from(g7link_core::CLIP_MAX_IMAGE_BYTES).unwrap();
    let huge = pc.node.send_clipboard_image(phone_id, "image/png".into(), vec![0; max + 1]).await;
    assert!(matches!(huge, Err(Error::TooLarge)), "{huge:?}");

    // The receiver turned the clipboard off for the sender; the sender's
    // stream is refused and nothing lands.
    phone.node.set_device_toggle(pc_id, "clipboard", false).unwrap();
    let denied = with_timeout(
        "denied",
        pc.node.send_clipboard_image(phone_id, "image/png".into(), data(2_000_000, 5)),
    )
    .await;
    assert!(matches!(denied, Err(Error::Denied)), "{denied:?}");
    assert_eq!(phone.platform.images.lock().unwrap().len(), 1);
    // And the connection still works.
    with_timeout("still connected", pc.node.send_clipboard(phone_id, "x".into()))
        .await
        .expect_err("still off");
}

#[tokio::test(flavor = "multi_thread")]
async fn clipboard_history_tracks_text_and_images_and_excludes_otps() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Text clip sent PC -> phone is recorded on both sides (outgoing on PC, incoming on phone).
    with_timeout("pc to phone text", pc.node.send_clipboard(phone_id, "https://g7.astraque.io/docs".into()))
        .await
        .expect("sent");
    wait_for(&mut phone, "clipboard received", |e| match e {
        NodeEvent::ClipboardReceived { device } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;

    // OTP clip is never recorded in clipboard history on either side.
    with_timeout("otp clip", phone.node.send_clipboard(pc_id, "482913".into())).await.expect("sent");
    wait_for(&mut pc, "otp received", |e| match e {
        NodeEvent::ClipboardReceived { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    // Image clip sent phone -> PC is recorded on both sides.
    let img = vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4];
    with_timeout(
        "phone to pc image",
        phone.node.send_clipboard_image(pc_id, "image/png".into(), img.clone()),
    )
    .await
    .expect("sent");
    wait_for(&mut pc, "image received", |e| match e {
        NodeEvent::ClipboardReceived { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    let pc_hist = pc.node.clipboard_history(None);
    assert_eq!(pc_hist.len(), 2, "OTP clip was excluded from PC history");
    assert_eq!(pc_hist[0].kind, g7link_core::ClipboardItemKind::Image);
    assert!(pc_hist[0].incoming);
    assert_eq!(pc_hist[0].device_name, "Pixel");
    assert_eq!(pc.node.clipboard_history_image(&pc_hist[0].id), Some(("image/png".to_owned(), img.clone())));
    assert_eq!(pc_hist[1].kind, g7link_core::ClipboardItemKind::Text);
    assert_eq!(pc_hist[1].text, "https://g7.astraque.io/docs");
    assert!(!pc_hist[1].incoming);

    let phone_hist = phone.node.clipboard_history(None);
    assert_eq!(phone_hist.len(), 2, "OTP clip was excluded from phone history");
    assert!(!phone_hist[0].incoming);
    assert!(phone_hist[1].incoming);

    // Search, pin, copy again, delete, and clear all.
    assert_eq!(pc.node.clipboard_history(Some("astraque")).len(), 1);
    let text_id = pc_hist[1].id.clone();
    let img_id = pc_hist[0].id.clone();
    assert!(pc.node.pin_clipboard_history(&text_id, true));
    assert_eq!(pc.node.clipboard_history(None)[0].id, text_id);

    pc.node.copy_clipboard_history(&text_id).unwrap();
    assert_eq!(
        pc.platform.clipboard.lock().unwrap().last().map(String::as_str),
        Some("https://g7.astraque.io/docs")
    );

    assert!(pc.node.delete_clipboard_history(&img_id));
    assert_eq!(pc.node.clipboard_history_image(&img_id), None);
    pc.node.clear_clipboard_history().unwrap();
    assert!(pc.node.clipboard_history(None).is_empty());
}

fn player(id: &str, title: &str, art: Option<(&str, Vec<u8>)>) -> MediaPlayer {
    MediaPlayer {
        id: id.into(),
        app: "Music".into(),
        title: Some(title.into()),
        artist: Some("Artist".into()),
        album: None,
        playing: true,
        duration: Some(200_000),
        position: Some(12_000),
        actions: vec!["play".into(), "pause".into(), "next".into(), "seek".into(), "bogus".into()],
        art_key: art.as_ref().map(|(key, _)| (*key).into()),
        art: art.map(|(_, bytes)| bytes),
        rate_milli: None,
        age: None,
        art_full: None,
        icon: None,
        shuffle: None,
        repeat: None,
    }
}

fn media_of(device: g7link_core::DeviceId) -> impl FnMut(&NodeEvent) -> Option<Vec<MediaPlayer>> {
    move |e| match e {
        NodeEvent::MediaChanged { device: d, players } if *d == device => Some(players.clone()),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn media_is_shared_and_controlled_with_consent() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // The phone plays something; the PC gets it with its artwork.
    let art = data(30_000, 7);
    phone.node.media_changed(vec![player("com.music", "Song", Some(("a1", art.clone())))]).await;
    let players = wait_for(&mut pc, "media", media_of(phone_id)).await;
    assert_eq!(players.len(), 1);
    assert_eq!(players[0].title.as_deref(), Some("Song"));
    assert_eq!(players[0].art.as_deref(), Some(&art[..]));
    assert_eq!(players[0].actions, ["play", "pause", "next", "seek"], "unknown actions are dropped");

    // Artwork goes once per session; a new picture goes again.
    phone.node.media_changed(vec![player("com.music", "Song", Some(("a1", art.clone())))]).await;
    let players = wait_for(&mut pc, "same art", media_of(phone_id)).await;
    assert_eq!((players[0].art_key.as_deref(), players[0].art.as_ref()), (Some("a1"), None));
    phone.node.media_changed(vec![player("com.music", "Next song", Some(("a2", vec![9; 10])))]).await;
    // (A state sent as the session started may arrive in between.)
    let players = wait_for(&mut pc, "new art", |e| {
        media_of(phone_id)(e).filter(|p| p.first().is_some_and(|p| p.title.as_deref() == Some("Next song")))
    })
    .await;
    assert_eq!(players[0].art.as_deref(), Some(&[9; 10][..]));

    // The PC controls it.
    with_timeout("pause", pc.node.media_command(phone_id, "com.music".into(), MediaAction::Pause, None))
        .await
        .expect("paused");
    with_timeout(
        "seek",
        pc.node.media_command(phone_id, "com.music".into(), MediaAction::Seek, Some(60_000)),
    )
    .await
    .expect("sought");
    assert_eq!(
        *phone.platform.media.lock().unwrap(),
        [
            ("com.music".to_owned(), MediaAction::Pause, None),
            ("com.music".to_owned(), MediaAction::Seek, Some(60_000))
        ]
    );
    let gone = pc.node.media_command(phone_id, "gone".into(), MediaAction::Play, None).await;
    assert!(matches!(gone, Err(Error::NotFound)), "{gone:?}");
    let no_position = pc.node.media_command(phone_id, "com.music".into(), MediaAction::Seek, None).await;
    assert!(no_position.is_err(), "{no_position:?}");

    // The PC turns media off for the phone: it's cleared there at once,
    // and the phone stops taking its commands only if the phone turns it off.
    pc.node.set_device_toggle(phone_id, "media", false).unwrap();
    assert!(wait_for(&mut pc, "cleared", media_of(phone_id)).await.is_empty());
    let off = pc.node.media_command(phone_id, "com.music".into(), MediaAction::Play, None).await;
    assert!(matches!(off, Err(Error::Denied)), "{off:?}");
    // On again: the PC asks for what plays now.
    pc.node.set_device_toggle(phone_id, "media", true).unwrap();
    let players = wait_for(&mut pc, "synced", media_of(phone_id)).await;
    assert_eq!(players[0].title.as_deref(), Some("Next song"));

    // The phone turns it off for the PC: the PC is told nothing plays, and
    // commands are refused.
    phone.node.set_device_toggle(pc_id, "media", false).unwrap();
    assert!(wait_for(&mut pc, "cleared by the phone", media_of(phone_id)).await.is_empty());
    let denied = pc.node.media_command(phone_id, "com.music".into(), MediaAction::Play, None).await;
    assert!(matches!(denied, Err(Error::Denied)), "{denied:?}");

    // And the other way: the PC's own players reach the phone once the
    // phone allows media again.
    phone.node.set_device_toggle(pc_id, "media", true).unwrap();
    pc.node.media_changed(vec![player("spotify", "On the PC", None)]).await;
    let players = wait_for(&mut phone, "pc media", |e| match e {
        NodeEvent::MediaChanged { device, players } if *device == pc_id && !players.is_empty() => {
            Some(players.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(players[0].title.as_deref(), Some("On the PC"));
    with_timeout(
        "phone controls the pc",
        phone.node.media_command(pc_id, "spotify".into(), MediaAction::Next, None),
    )
    .await
    .expect("next");
    assert_eq!(pc.platform.media.lock().unwrap()[0], ("spotify".to_owned(), MediaAction::Next, None));
}

#[tokio::test(flavor = "multi_thread")]
async fn pc_playback_reaches_both_phones_with_large_artwork() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut first = device("Pixel", DeviceKind::Phone).await;
    let mut second = device("Tablet", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut first).await;
    let pc_id = pc.node.device_id();

    let players = (0..4)
        .map(|i| {
            let mut p = player(
                &format!("player-{i}"),
                &format!("Video {i}"),
                Some((&format!("art-{i}"), vec![i; 256 * 1024])),
            );
            p.playing = i == 3;
            p
        })
        .collect::<Vec<_>>();
    pc.node.media_changed(players.clone()).await;
    let state = wait_for(&mut first, "first phone sees video", media_of(pc_id)).await;
    assert_eq!(state.len(), 4);
    assert!(state[3].playing && state[3].art.is_some());

    // The second phone joins after playback started. Use a separate event
    // subscription so waiting for pairing can't consume its first snapshot.
    let mut events = second.node.events();
    pair_qr(&mut pc, &mut second).await;
    let state = with_timeout("second phone sees the already playing video", async {
        loop {
            if let Some(players) = media_of(pc_id)(&events.recv().await.unwrap()) {
                break players;
            }
        }
    })
    .await;
    assert_eq!(state.len(), 4);
    assert!(state[3].playing && state[3].art.is_some());

    let mut paused = players;
    paused[3].playing = false;
    pc.node.media_changed(paused).await;
    for phone in [&mut first, &mut second] {
        let state = wait_for(phone, "phone sees the pause", |event| {
            media_of(pc_id)(event).filter(|players| players.len() == 4 && !players[3].playing)
        })
        .await;
        assert_eq!(state[3].title.as_deref(), Some("Video 3"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn phones_lock_pcs_and_links_open_on_either_side() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &["pc.power"]).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    with_timeout("lock", phone.node.pc_power(pc_id, PowerAction::Lock)).await.expect("locked");
    with_timeout("sleep", phone.node.pc_power(pc_id, PowerAction::Sleep)).await.expect("asleep");
    // The PC answers first and acts a moment later.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(*pc.platform.power.lock().unwrap(), [PowerAction::Lock, PowerAction::Sleep]);

    // A phone doesn't lock or sleep on request.
    let phone_power = pc.node.pc_power(phone_id, PowerAction::Lock).await;
    assert!(matches!(phone_power, Err(Error::Unsupported)), "{phone_power:?}");

    // The PC's user turned PC actions off for the phone.
    pc.node.set_device_toggle(phone_id, "pc_actions", false).unwrap();
    let denied = phone.node.pc_power(pc_id, PowerAction::Lock).await;
    assert!(matches!(denied, Err(Error::Denied)), "{denied:?}");

    // Links both ways; only web links.
    with_timeout("to pc", phone.node.open_link(pc_id, "https://example.com/a".into())).await.expect("opened");
    with_timeout("to phone", pc.node.open_link(phone_id, "http://example.org".into())).await.expect("opened");
    assert_eq!(*pc.platform.links.lock().unwrap(), ["https://example.com/a"]);
    assert_eq!(*phone.platform.links.lock().unwrap(), ["http://example.org"]);
    let script = pc.node.open_link(phone_id, "javascript:alert(1)".into()).await;
    assert!(script.is_err());
    assert_eq!(phone.platform.links.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_links_shared_from_the_share_sheet_skip_the_question() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &["pc.power"]).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let pc_id = pc.node.device_id();

    with_timeout("asked", phone.node.open_link(pc_id, "https://example.com/ask".into())).await.unwrap();
    with_timeout("shared", phone.node.open_shared_link(pc_id, "https://example.com/shared".into()))
        .await
        .unwrap();
    assert_eq!(*pc.platform.links.lock().unwrap(), ["https://example.com/ask", "https://example.com/shared"]);
    // The PC hears which one the user shared; any other link is for it to ask about.
    assert_eq!(*pc.platform.links_shared.lock().unwrap(), [false, true]);

    phone.node.shutdown().await;
    pc.node.shutdown().await;
}

/// Deterministic, incompressible-looking test data.
fn data(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn outgoing(dir: &std::path::Path, name: &str, contents: &[u8]) -> OutgoingFile {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    OutgoingFile { name: name.into(), folder: None, source: FileSource::Path(path) }
}

/// Waits for a transfer to reach a state matching `pick`.
async fn wait_transfer<T>(
    dev: &mut TestDevice,
    id: &str,
    what: &str,
    mut pick: impl FnMut(&Transfer) -> Option<T>,
) -> T {
    wait_for(dev, what, |e| match e {
        NodeEvent::Transfer(t) if t.id == id => pick(t),
        _ => None,
    })
    .await
}

fn saved(t: &Transfer) -> Option<Vec<std::path::PathBuf>> {
    match &t.state {
        TransferState::Done { saved } => Some(saved.clone()),
        TransferState::Failed(f) => panic!("transfer failed: {f:?}"),
        TransferState::Cancelled => panic!("transfer cancelled"),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn files_arrive_intact_under_free_names() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    let src = tempfile::tempdir().unwrap();
    let big = data(3 * 1024 * 1024 + 17, 1);
    let small = b"hello".to_vec();

    // The phone already has a "notes.txt".
    let downloads = phone.dir.path().join("received");
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::write(downloads.join("notes.txt"), "old").unwrap();

    let files = vec![outgoing(src.path(), "video.bin", &big), outgoing(src.path(), "notes.txt", &small)];
    let id = with_timeout("send", pc.node.send_files(phone_id, files)).await.expect("send starts");

    let received = wait_transfer(&mut phone, &id, "received", |t| {
        assert_eq!(t.direction, Direction::Incoming);
        assert_eq!(t.names, ["video.bin", "notes.txt"]);
        saved(t)
    })
    .await;
    assert_eq!(received, [downloads.join("video.bin"), downloads.join("notes (2).txt")]);
    assert_eq!(std::fs::read(&received[0]).unwrap(), big);
    assert_eq!(std::fs::read(&received[1]).unwrap(), small);
    assert_eq!(std::fs::read(downloads.join("notes.txt")).unwrap(), b"old", "existing files are kept");

    let sent = wait_transfer(&mut pc, &id, "sent", |t| {
        assert_eq!(t.direction, Direction::Outgoing);
        saved(t).map(|_| t.done)
    })
    .await;
    assert_eq!(sent, big.len() as u64 + 5);
    assert!(
        std::fs::read_dir(phone.dir.path().join("incoming")).unwrap().next().is_none(),
        "no partial files are left"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn folders_arrive_with_their_layout() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    let src = tempfile::tempdir().unwrap();
    let trip = src.path().join("Trip");
    std::fs::create_dir_all(trip.join("Day 1")).unwrap();
    std::fs::write(trip.join("Day 1").join("beach.jpg"), b"sand").unwrap();
    std::fs::write(trip.join("notes.txt"), b"fun").unwrap();
    std::fs::write(src.path().join("ticket.pdf"), b"pdf").unwrap();

    // The phone already has a "Trip" folder: this one becomes "Trip (2)".
    let downloads = phone.dir.path().join("received");
    std::fs::create_dir_all(downloads.join("Trip")).unwrap();

    let files = g7link_core::outgoing_paths(&[trip, src.path().join("ticket.pdf")]).unwrap();
    let id = with_timeout("send", pc.node.send_files(phone_id, files)).await.expect("send starts");
    let received = wait_transfer(&mut phone, &id, "received", |t| {
        assert_eq!(t.names, ["Trip", "ticket.pdf"]);
        assert_eq!(t.files, 3);
        saved(t)
    })
    .await;
    let folder = downloads.join("Trip (2)");
    assert_eq!(received, [folder.clone(), downloads.join("ticket.pdf")]);
    assert_eq!(std::fs::read(folder.join("Day 1").join("beach.jpg")).unwrap(), b"sand");
    assert_eq!(std::fs::read(folder.join("notes.txt")).unwrap(), b"fun");
    assert!(
        std::fs::read_dir(downloads.join("Trip")).unwrap().next().is_none(),
        "the old folder is untouched"
    );
}

/// The phone's sound on its own (#94): the PC asks for the sound alone, the
/// phone gets that request, and its sound stream plays without any video. A
/// phone that doesn't offer it is never asked to share its screen instead.
#[tokio::test(flavor = "multi_thread")]
async fn the_phone_sound_plays_on_the_pc_without_its_screen() {
    use g7link_core::{MirrorSend, PacketKind};
    let sound_only = g7link_core::MirrorStart {
        max_size: 0,
        fps: 0,
        bitrate: 0,
        audio: true,
        session: 0,
        app: None,
        stay_awake: false,
        screen_off: false,
        watch: None,
        sound_only: true,
    };

    // An older phone: no mirror.sound, so the request is refused, not
    // turned into screen sharing.
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::MIRROR_LISTEN]).await;
    let mut old_phone = device_with(
        "Old phone",
        DeviceKind::Phone,
        &[g7link_core::MIRROR_CAPTURE, g7link_core::MIRROR_AUDIO_PLAYBACK],
    )
    .await;
    pair_qr(&mut pc, &mut old_phone).await;
    let refused = with_timeout(
        "start on an old phone",
        pc.node.mirror_start(old_phone.node.device_id(), sound_only.clone()),
    )
    .await;
    assert!(matches!(refused, Err(Error::Unsupported)), "{refused:?}");
    assert!(old_phone.platform.mirror_asks.lock().unwrap().is_empty(), "the old phone was never asked");

    let mut phone = device_with(
        "Pixel",
        DeviceKind::Phone,
        &[g7link_core::MIRROR_CAPTURE, g7link_core::MIRROR_AUDIO_PLAYBACK, g7link_core::MIRROR_SOUND],
    )
    .await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    with_timeout("start", pc.node.mirror_start(phone_id, sound_only)).await.unwrap();
    assert_eq!(*phone.platform.mirror_asks.lock().unwrap(), ["start 0 with sound, sound only"]);

    // The phone sends only its sound.
    let sound = Arc::new(with_timeout("open sound", phone.node.mirror_open_audio(pc_id)).await.unwrap());
    let format =
        g7link_core::MirrorAudioConfig { codec: g7link_core::MIRROR_PCM.into(), rate: 48_000, channels: 2 };
    let sender = sound.clone();
    let sent = tokio::task::spawn_blocking(move || {
        [
            sender.send(PacketKind::Config, 0, format.to_cbor()),
            sender.send(PacketKind::Frame, 10_000, vec![0; 1_920]),
        ]
    })
    .await
    .unwrap();
    assert!(sent.iter().all(|s| *s == MirrorSend::Queued), "{sent:?}");
    let screen = pc.platform.screen.clone();
    let deadline = tokio::time::Instant::now() + WAIT;
    while screen.got.lock().unwrap().len() < 2 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        *screen.got.lock().unwrap(),
        ["sound 48000 Hz x2", "sound 10000 1920"],
        "sound, and no picture"
    );

    // Turning it off: the phone hears it, and the sound stream ends.
    pc.node.mirror_stop(phone_id, 0).await;
    let deadline = tokio::time::Instant::now() + WAIT;
    while !phone.platform.mirror_asks.lock().unwrap().iter().any(|a| a == "stop 0")
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(phone.platform.mirror_asks.lock().unwrap().iter().any(|a| a == "stop 0"));
    let deadline = tokio::time::Instant::now() + WAIT;
    while !screen.got.lock().unwrap().iter().any(|g| g == "sound ended")
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(screen.got.lock().unwrap().iter().any(|g| g == "sound ended"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_phone_screen_streams_to_the_pc() {
    use g7link_core::{MirrorSend, PacketKind};
    let mut pc =
        device_with("Desktop", DeviceKind::Desktop, &[g7link_core::MIRROR_VIEW, g7link_core::MIRROR_LISTEN])
            .await;
    let mut phone = device_with(
        "Pixel",
        DeviceKind::Phone,
        &[g7link_core::MIRROR_CAPTURE, g7link_core::MIRROR_INPUT, g7link_core::MIRROR_AUDIO_PLAYBACK],
    )
    .await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let options = g7link_core::MirrorStart {
        max_size: 1920,
        fps: 60,
        bitrate: 8_000_000,
        audio: true,
        session: 0,
        app: None,
        stay_awake: false,
        screen_off: false,
        watch: None,
        sound_only: false,
    };
    with_timeout("start", pc.node.mirror_start(phone_id, options)).await.unwrap();
    assert_eq!(*phone.platform.mirror_asks.lock().unwrap(), ["start 1920 with sound"]);

    // The user agreed: the phone streams.
    let stream = Arc::new(with_timeout("open", phone.node.mirror_open(pc_id)).await.unwrap());
    let config = g7link_core::MirrorConfig { codec: "h264".into(), width: 1080, height: 2400, session: 0 };
    let sender = stream.clone();
    let sent = tokio::task::spawn_blocking(move || {
        [
            sender.send(PacketKind::Config, 0, config.to_cbor()),
            sender.send(PacketKind::Keyframe, 1, vec![0; 90_000]),
            sender.send(PacketKind::Frame, 16_667, vec![0; 4_000]),
        ]
    })
    .await
    .unwrap();
    assert!(sent.iter().all(|s| *s == MirrorSend::Queued), "{sent:?}");
    wait_for(&mut pc, "showing", |e| matches!(e, NodeEvent::Mirroring { on: true, .. }).then_some(())).await;
    let screen = pc.platform.screen.clone();
    let deadline = tokio::time::Instant::now() + WAIT;
    while screen.got.lock().unwrap().len() < 3 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(*screen.got.lock().unwrap(), ["config 1080x2400", "key 1 90000", "frame 16667 4000"]);

    // And its sound, on a stream of its own.
    let sound = Arc::new(with_timeout("open sound", phone.node.mirror_open_audio(pc_id)).await.unwrap());
    let format =
        g7link_core::MirrorAudioConfig { codec: g7link_core::MIRROR_PCM.into(), rate: 48_000, channels: 2 };
    let sender = sound.clone();
    let sent = tokio::task::spawn_blocking(move || {
        [
            sender.send(PacketKind::Config, 0, format.to_cbor()),
            sender.send(PacketKind::Frame, 10_000, vec![0; 1_920]),
        ]
    })
    .await
    .unwrap();
    assert!(sent.iter().all(|s| *s == MirrorSend::Queued), "{sent:?}");
    let deadline = tokio::time::Instant::now() + WAIT;
    while screen.got.lock().unwrap().len() < 5 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(screen.got.lock().unwrap()[3..], ["sound 48000 Hz x2", "sound 10000 1920"]);

    pc.node.mirror_keyframe(phone_id, 0).await;
    pc.node.mirror_power(phone_id, true, true).await.unwrap();
    // Muted on the PC, the sound goes back to the phone.
    pc.node.mirror_sound(phone_id, false).await.unwrap();
    // The PC's mouse on the phone's screen; nonsense never leaves the PC.
    use g7link_core::{MirrorInput, TouchAction};
    pc.node
        .mirror_input(phone_id, 0, MirrorInput::Touch { action: TouchAction::Down, x: 0.5, y: 0.25 })
        .await
        .unwrap();
    pc.node.mirror_input(phone_id, 0, MirrorInput::Key { key: "back".into() }).await.unwrap();
    pc.node.mirror_input(phone_id, 0, MirrorInput::Key { key: "lock".into() }).await.unwrap();
    assert!(pc.node.mirror_input(phone_id, 0, MirrorInput::Key { key: "power".into() }).await.is_err());
    // The PC stops watching: the phone hears it, and its stream closes.
    pc.node.mirror_stop(phone_id, 0).await;
    wait_for(&mut pc, "stopped", |e| matches!(e, NodeEvent::Mirroring { on: false, .. }).then_some(())).await;
    let deadline = tokio::time::Instant::now() + WAIT;
    while !screen.got.lock().unwrap().iter().any(|g| g == "sound ended")
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let got = screen.got.lock().unwrap().clone();
    assert!(got.iter().any(|g| g == "ended") && got.iter().any(|g| g == "sound ended"), "{got:?}");
    let deadline = tokio::time::Instant::now() + WAIT;
    while !stream.is_closed() && tokio::time::Instant::now() < deadline {
        let s = stream.clone();
        tokio::task::spawn_blocking(move || s.send(PacketKind::Frame, 0, vec![0])).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(stream.is_closed(), "the phone's stream closes");
    let deadline = tokio::time::Instant::now() + WAIT;
    while !sound.is_closed() && tokio::time::Instant::now() < deadline {
        let s = sound.clone();
        tokio::task::spawn_blocking(move || s.send(PacketKind::Frame, 0, vec![0])).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(sound.is_closed(), "and its sound stream");
    // The phone records the stop after the streams close; under load that can lag a little.
    let wanted = [
        "keyframe 0",
        "power awake=true off=true",
        "sound on=false",
        "Touch(Down) on 0",
        "Key(back) on 0",
        "Key(lock) on 0",
        "stop 0",
    ];
    let deadline = tokio::time::Instant::now() + WAIT;
    let asks = loop {
        let asks = phone.platform.mirror_asks.lock().unwrap().clone();
        let complete = wanted.iter().all(|w| asks.iter().any(|a| a == w));
        if complete || tokio::time::Instant::now() >= deadline {
            break asks;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    for wanted in wanted {
        assert!(asks.iter().any(|a| a == wanted), "{wanted}: {asks:?}");
    }
    let shown = pc.platform.mirror_asks.lock().unwrap().clone();
    assert_eq!(shown, ["sink 0", "sink 0"], "the screen and its sound");
}

#[tokio::test(flavor = "multi_thread")]
async fn phone_apps_open_in_windows_of_their_own() {
    use g7link_core::{MirrorInput, MirrorSend, PacketKind};
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::MIRROR_VIEW]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::MIRROR_CAPTURE]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let app = |session: u32| g7link_core::MirrorStart {
        max_size: 1280,
        fps: 30,
        bitrate: 4_000_000,
        audio: false,
        session,
        app: Some("com.example.chat".into()),
        stay_awake: false,
        screen_off: false,
        watch: None,
        sound_only: false,
    };

    // Not Elevated yet: no apps, no app windows.
    assert!(pc.node.mirror_apps(phone_id).await.is_err());
    assert!(pc.node.mirror_start(phone_id, app(7)).await.is_err());
    phone
        .node
        .update_power(
            PowerLevel::Elevated,
            vec![g7link_core::MIRROR_CAPTURE.into(), g7link_core::MIRROR_VIRTUAL_DISPLAY.into()],
        )
        .await;
    let deadline = tokio::time::Instant::now() + WAIT;
    let apps = loop {
        match pc.node.mirror_apps(phone_id).await {
            Ok(apps) => break apps,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await
            }
            Err(e) => panic!("no apps: {e}"),
        }
    };
    let listed: Vec<_> = apps.iter().map(|a| (a.label.as_str(), a.icon.is_some())).collect();
    assert_eq!(listed, [("Chat", true), ("Maps", false), ("notes", true)], "by name; big icons left out");

    // Nothing opens while the phone is locked, and the PC hears why.
    phone.platform.phone_locked.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(pc.node.mirror_start(phone_id, app(7)).await, Err(Error::Declined)));
    phone.platform.phone_locked.store(false, std::sync::atomic::Ordering::SeqCst);

    // An app window is a session of its own; the screen's (0) can't have an app.
    assert!(pc.node.mirror_start(phone_id, app(0)).await.is_err());
    with_timeout("start", pc.node.mirror_start(phone_id, app(7))).await.unwrap();
    let stream = Arc::new(with_timeout("open", phone.node.mirror_open(pc_id)).await.unwrap());
    let config = g7link_core::MirrorConfig { codec: "h264".into(), width: 720, height: 1280, session: 7 };
    let sender = stream.clone();
    let sent = tokio::task::spawn_blocking(move || {
        [
            sender.send(PacketKind::Config, 0, config.to_cbor()),
            sender.send(PacketKind::Keyframe, 1, vec![0; 10]),
        ]
    })
    .await
    .unwrap();
    assert!(sent.iter().all(|s| *s == MirrorSend::Queued), "{sent:?}");
    let session = wait_for(&mut pc, "showing", |e| match e {
        NodeEvent::Mirroring { session, on: true, .. } => Some(*session),
        _ => None,
    })
    .await;
    assert_eq!(session, 7);
    assert!(matches!(pc.node.mirror_resize(phone_id, 0, 800, 600).await, Err(Error::Protocol(_))));
    assert!(matches!(pc.node.mirror_resize(phone_id, 7, 10, 600).await, Err(Error::Protocol(_))));
    pc.node.mirror_resize(phone_id, 7, 1024, 768).await.unwrap();
    pc.node.mirror_input(phone_id, 7, MirrorInput::Key { key: "back".into() }).await.unwrap();
    pc.node.mirror_keyframe(phone_id, 7).await;
    pc.node.mirror_stop(phone_id, 7).await;
    let ended = wait_for(&mut pc, "stopped", |e| match e {
        NodeEvent::Mirroring { session, on: false, .. } => Some(*session),
        _ => None,
    })
    .await;
    assert_eq!(ended, 7);
    // The phone handles some of these off the control stream, so give it a
    // moment to get to all of them.
    for wanted in
        ["start 1280 com.example.chat as 7", "resize 7 1024x768", "Key(back) on 7", "keyframe 7", "stop 7"]
    {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !phone.platform.mirror_asks.lock().unwrap().iter().any(|a| a == wanted) {
            let asks = phone.platform.mirror_asks.lock().unwrap().clone();
            assert!(std::time::Instant::now() < deadline, "{wanted}: {asks:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    assert_eq!(*pc.platform.mirror_asks.lock().unwrap(), ["sink 7"]);

    // The app closes on the phone: its window's stream ends at once, even
    // with no more pictures to send.
    with_timeout("start again", pc.node.mirror_start(phone_id, app(8))).await.unwrap();
    let stream = Arc::new(with_timeout("open again", phone.node.mirror_open(pc_id)).await.unwrap());
    let config = g7link_core::MirrorConfig { codec: "h264".into(), width: 720, height: 1280, session: 8 };
    let sender = stream.clone();
    tokio::task::spawn_blocking(move || sender.send(PacketKind::Config, 0, config.to_cbor())).await.unwrap();
    wait_for(&mut pc, "showing 8", |e| {
        matches!(e, NodeEvent::Mirroring { session: 8, on: true, .. }).then_some(())
    })
    .await;
    stream.close();
    wait_for(&mut pc, "ended 8", |e| {
        matches!(e, NodeEvent::Mirroring { session: 8, on: false, .. }).then_some(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_pc_reads_and_sends_texts_through_the_phone() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::SMS_SHOW]).await;
    let mut phone =
        device_with("Pixel", DeviceKind::Phone, &[g7link_core::SMS_READ, g7link_core::SMS_SEND]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let threads = with_timeout("threads", pc.node.sms_threads(phone_id, 3)).await.unwrap();
    assert_eq!(threads.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["0", "1", "2"]);
    // With the phone's SMS app, for its icon.
    let (_, app) = with_timeout("threads", pc.node.sms_threads_and_app(phone_id, 1)).await.unwrap();
    assert_eq!(app.as_deref(), Some("com.example.messaging"));

    // Paging back through a conversation.
    let latest = pc.node.sms_messages(phone_id, "1".into(), None, 10).await.unwrap();
    assert_eq!((latest[0].date, latest.len()), (99, 10));
    let older = pc.node.sms_messages(phone_id, "1".into(), Some(latest[9].date), 10).await.unwrap();
    assert_eq!(older[0].date, 89);

    let (mime, data) = pc.node.sms_part(phone_id, "p1".into()).await.unwrap();
    assert_eq!((mime.as_str(), data.as_slice()), ("image/jpeg", [1u8, 2, 3].as_slice()));

    pc.node.sms_send(phone_id, vec!["+15550100".into()], "On my way".into()).await.unwrap();
    assert_eq!(
        *phone.platform.texts.lock().unwrap(),
        [(vec!["+15550100".to_owned()], "On my way".to_owned())]
    );
    assert!(matches!(pc.node.sms_send(phone_id, vec![], "x".into()).await, Err(Error::Protocol(_))));

    // The phone says what changed.
    phone.node.sms_changed(Some("1".into())).await;
    let changed = wait_for(&mut pc, "a change", |e| match e {
        NodeEvent::SmsChanged { device, thread } if *device == phone_id => Some(thread.clone()),
        _ => None,
    })
    .await;
    assert_eq!(changed.as_deref(), Some("1"));

    // The phone's user turned messages off for this PC.
    phone.node.set_device_toggle(pc_id, "messages", false).unwrap();
    assert!(matches!(pc.node.sms_threads(phone_id, 3).await, Err(Error::Denied)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sender_name_takes_no_reply() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::SMS_SHOW]).await;
    let mut phone =
        device_with("Pixel", DeviceKind::Phone, &[g7link_core::SMS_READ, g7link_core::SMS_SEND]).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    let sent = with_timeout("send", pc.node.sms_send(phone_id, vec!["MPESA".into()], "Hi".into())).await;
    assert!(matches!(sent, Err(Error::Protocol(_))), "{sent:?}");
    assert!(phone.platform.texts.lock().unwrap().is_empty(), "nothing reached the phone's radio");

    // A short code still takes a text.
    with_timeout("send", pc.node.sms_send(phone_id, vec!["22141".into()], "STOP".into())).await.unwrap();
    assert_eq!(*phone.platform.texts.lock().unwrap(), [(vec!["22141".to_owned()], "STOP".to_owned())]);
}

#[tokio::test(flavor = "multi_thread")]
async fn calls_show_on_the_pc_which_can_answer_them() {
    use g7link_core::{CallCommand, CallState};
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::CALLS_SHOW]).await;
    let mut phone =
        device_with("Pixel", DeviceKind::Phone, &[g7link_core::CALLS_STATE, g7link_core::CALLS_CONTROL])
            .await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let ringing = CallState {
        id: "c1".into(),
        state: "ringing".into(),
        incoming: true,
        number: Some("+15550100".into()),
        name: Some("Sam".into()),
        photo: Some(vec![0xff, 0xd8]),
        missed: false,
        since: None,
        controls: None,
    };
    // The call in `state` (a PC that just connected may hear the earlier
    // one again).
    async fn next_call(pc: &mut TestDevice, phone_id: g7link_core::DeviceId, state: &str) -> CallState {
        wait_for(pc, state, |e| match e {
            NodeEvent::Call { device, call } if *device == phone_id && call.state == state => {
                Some(call.clone())
            }
            _ => None,
        })
        .await
    }

    phone.node.call_changed(ringing.clone()).await.unwrap();
    assert_eq!(next_call(&mut pc, phone_id, "ringing").await, ringing);
    with_timeout("answer", pc.node.call_command(phone_id, "c1".into(), CallCommand::Answer)).await.unwrap();
    assert_eq!(*phone.platform.calls.lock().unwrap(), [("c1".to_owned(), CallCommand::Answer)]);
    // Only the call in progress.
    assert!(matches!(
        pc.node.call_command(phone_id, "c0".into(), CallCommand::Decline).await,
        Err(Error::NotFound)
    ));

    let active = CallState { state: "active".into(), photo: None, ..ringing.clone() };
    phone.node.call_changed(active.clone()).await.unwrap();
    next_call(&mut pc, phone_id, "active").await;
    // An answered call doesn't ring anymore, but can be hung up.
    assert!(matches!(
        pc.node.call_command(phone_id, "c1".into(), CallCommand::Silence).await,
        Err(Error::NotFound)
    ));
    // The volume works on any phone; mute needs one that controls the call.
    pc.node.call_command(phone_id, "c1".into(), CallCommand::Volume(true)).await.unwrap();
    assert!(matches!(
        pc.node.call_command(phone_id, "c1".into(), CallCommand::Mute(true)).await,
        Err(Error::Unsupported)
    ));
    phone
        .node
        .update_power(
            PowerLevel::NotApplicable,
            vec![
                "media.control".into(),
                g7link_core::CALLS_STATE.into(),
                g7link_core::CALLS_CONTROL.into(),
                g7link_core::CALLS_IN_CALL.into(),
            ],
        )
        .await;
    pc.node.call_command(phone_id, "c1".into(), CallCommand::Mute(true)).await.unwrap();
    pc.node.call_command(phone_id, "c1".into(), CallCommand::Dtmf('5')).await.unwrap();
    let seen: Vec<CallCommand> = phone.platform.calls.lock().unwrap().iter().map(|(_, c)| *c).collect();
    assert_eq!(seen[1..], [CallCommand::Volume(true), CallCommand::Mute(true), CallCommand::Dtmf('5')]);
    pc.node.call_command(phone_id, "c1".into(), CallCommand::Decline).await.unwrap();

    let ended = CallState { state: "ended".into(), ..active };
    phone.node.call_changed(ended).await.unwrap();
    next_call(&mut pc, phone_id, "ended").await;
    assert!(matches!(
        pc.node.call_command(phone_id, "c1".into(), CallCommand::Decline).await,
        Err(Error::NotFound)
    ));

    // The phone's user turned calls off for this PC.
    phone.node.set_device_toggle(pc_id, "calls", false).unwrap();
    phone.node.call_changed(CallState { id: "c2".into(), ..ringing }).await.unwrap();
    assert!(matches!(
        pc.node.call_command(phone_id, "c2".into(), CallCommand::Answer).await,
        Err(Error::Denied)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn new_photos_reach_the_pc_and_come_when_asked() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::PHOTOS_SHOW]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::PHOTOS_READ]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let shot = phone.dir.path().join("Screenshot_1.png");
    let pixels = data(300_000, 7);
    std::fs::write(&shot, &pixels).unwrap();
    phone.platform.photos.lock().unwrap().insert("media:1".into(), shot);
    let photo = g7link_core::Photo {
        id: "media:1".into(),
        name: "Screenshot_1.png".into(),
        size: pixels.len() as u64,
        taken: 1_790_000_000,
        screenshot: true,
        thumb: vec![0xff, 0xd8, 0xff],
    };

    phone.node.photo_taken(photo.clone()).await.unwrap();
    let got = wait_for(&mut pc, "the photo", |e| match e {
        NodeEvent::PhotoAdded { device, photo } if *device == phone_id => Some(photo.clone()),
        _ => None,
    })
    .await;
    assert_eq!(got, photo);

    // The PC asks for it: it comes as a transfer.
    let id = with_timeout("fetch", pc.node.fetch_photo(phone_id, "media:1".into())).await.unwrap();
    let saved = wait_transfer(&mut pc, &id, "the photo", saved).await;
    assert_eq!(std::fs::read(&saved[0]).unwrap(), pixels);

    // Only announced photos can be asked for.
    assert!(matches!(pc.node.fetch_photo(phone_id, "media:2".into()).await, Err(Error::NotFound)));
    // The phone's user turned photos off for this PC: nothing more comes.
    phone.node.set_device_toggle(pc_id, "photos", false).unwrap();
    assert!(matches!(pc.node.fetch_photo(phone_id, "media:1".into()).await, Err(Error::Denied)));
    // An oversized preview isn't sent.
    let big = g7link_core::Photo { thumb: vec![0; g7link_core::PHOTO_MAX_THUMB_BYTES + 1], ..photo };
    assert!(matches!(phone.node.photo_taken(big).await, Err(Error::TooLarge)));
}

#[tokio::test(flavor = "multi_thread")]
async fn files_need_the_toggle_and_can_be_cancelled() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let src = tempfile::tempdir().unwrap();

    // The phone doesn't take files from this PC.
    phone.node.set_device_toggle(pc_id, "files", false).unwrap();
    let id = pc.node.send_files(phone_id, vec![outgoing(src.path(), "a.txt", b"a")]).await.unwrap();
    let state =
        wait_transfer(&mut pc, &id, "refused", |t| t.state.is_finished().then(|| t.state.clone())).await;
    assert_eq!(state, TransferState::Failed(TransferFailure::Denied));
    phone.node.set_device_toggle(pc_id, "files", true).unwrap();
    // This PC doesn't send files to the phone.
    pc.node.set_device_toggle(phone_id, "files", false).unwrap();
    assert!(matches!(
        pc.node.send_files(phone_id, vec![outgoing(src.path(), "b.txt", b"b")]).await,
        Err(Error::Denied)
    ));
    pc.node.set_device_toggle(phone_id, "files", true).unwrap();

    // Cancelled by the receiver as soon as bytes flow.
    let big = data(64 * 1024 * 1024, 2);
    let id = pc.node.send_files(phone_id, vec![outgoing(src.path(), "big.bin", &big)]).await.unwrap();
    wait_transfer(&mut phone, &id, "receiving", |t| {
        (t.state == TransferState::Running && t.done > 0).then_some(())
    })
    .await;
    phone.node.cancel_transfer(&id);
    let on_phone =
        wait_transfer(&mut phone, &id, "cancelled here", |t| t.state.is_finished().then(|| t.state.clone()))
            .await;
    let on_pc =
        wait_transfer(&mut pc, &id, "cancelled there", |t| t.state.is_finished().then(|| t.state.clone()))
            .await;
    assert_eq!((on_phone, on_pc), (TransferState::Cancelled, TransferState::Cancelled));
    assert!(!phone.dir.path().join("received").join("big.bin").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn transfers_resume_after_the_receiver_restarts() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let src = tempfile::tempdir().unwrap();
    let big = data(96 * 1024 * 1024, 3);
    let id = pc.node.send_files(phone_id, vec![outgoing(src.path(), "big.bin", &big)]).await.unwrap();

    // The phone goes away part way through.
    let before = wait_transfer(&mut phone, &id, "receiving", |t| (t.done > 0).then_some(t.done)).await;
    phone.node.shutdown().await;
    let TestDevice { node, dir, .. } = phone;
    drop(node);
    let interrupted =
        std::fs::metadata(dir.path().join("incoming").join(pc_id.to_string()).join(&id).join("0.part"))
            .unwrap()
            .len();
    assert!(interrupted >= before && interrupted < big.len() as u64, "{interrupted} of {}", big.len());
    wait_transfer(&mut pc, &id, "waiting for the phone", |t| {
        (t.state == TransferState::Waiting).then_some(())
    })
    .await;

    // Back again: the transfer continues where it stopped.
    let mut phone = start_in(dir, "Pixel", DeviceKind::Phone).await;
    let resumed_at =
        wait_transfer(&mut phone, &id, "resumed", |t| (t.state == TransferState::Running).then_some(t.done))
            .await;
    assert!(resumed_at >= interrupted, "resumed at {resumed_at}, had {interrupted}");
    let received = wait_transfer(&mut phone, &id, "received", saved).await;
    assert_eq!(std::fs::read(&received[0]).unwrap(), big);
    wait_transfer(&mut pc, &id, "sent", saved).await;
    let _ = pc_id;
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_rpc_does_not_block_control_stream() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::PHOTOS_SHOW]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::PHOTOS_READ]).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    let file = phone.dir.path().join("IMG_1.jpg");
    let pixels = b"jpeg-bytes";
    std::fs::write(&file, pixels).unwrap();
    phone.platform.photos.lock().unwrap().insert("media:1".into(), file);
    let photo = g7link_core::Photo {
        id: "media:1".into(),
        name: "IMG_1.jpg".into(),
        size: pixels.len() as u64,
        taken: 1_790_000_000,
        screenshot: true,
        thumb: vec![0xff, 0xd8, 0xff],
    };
    phone.node.photo_taken(photo).await.unwrap();
    wait_for(&mut pc, "the photo", |e| match e {
        NodeEvent::PhotoAdded { device, .. } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    let barrier = Arc::new(std::sync::Barrier::new(2));
    *phone.platform.photo_barrier.lock().unwrap() = Some(barrier.clone());

    let pc_node = pc.node.clone();
    let get_task = tokio::spawn(async move { pc_node.fetch_photo(phone_id, "media:1".into()).await });

    // Wait until the phone's `open_photo` is blocked inside `photos.get`.
    tokio::task::spawn_blocking({
        let barrier = barrier.clone();
        move || barrier.wait()
    })
    .await
    .unwrap();

    // While `photos.get` is stuck, another control-stream RPC still completes.
    with_timeout("ring while photos.get is blocked", pc.node.ring(phone_id, true))
        .await
        .expect("ring succeeds without waiting for photos.get");

    // Release `open_photo` so `photos.get` finishes cleanly.
    tokio::task::spawn_blocking(move || barrier.wait()).await.unwrap();
    let transfer_id = with_timeout("photos.get finishes", get_task).await.unwrap().expect("photo sent");
    wait_transfer(&mut pc, &transfer_id, "photo received", saved).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn call_log_and_dial_work_and_respect_permissions_and_toggles() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::CALLS_SHOW]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::CALLS_LOG]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let page1 = with_timeout("call log page 1", pc.node.call_log(phone_id, None, 3)).await.unwrap();
    assert_eq!(page1.len(), 3);
    assert_eq!((&*page1[0].id, &*page1[0].direction, page1[0].date), ("log:4", "incoming", 4));
    assert_eq!(page1[0].photo.as_deref(), Some(&[0xff, 0xd8][..]));
    // Entry 3 had an oversized photo: kept without its photo.
    assert_eq!((&*page1[1].id, &*page1[1].direction, page1[1].photo.as_ref()), ("log:3", "rejected", None));
    assert_eq!((&*page1[2].id, &*page1[2].direction), ("log:2", "missed"));

    let page2 = with_timeout("call log page 2", pc.node.call_log(phone_id, Some(2), 3)).await.unwrap();
    assert_eq!(page2.iter().map(|e| e.date).collect::<Vec<_>>(), [1, 0]);

    phone.node.call_log_changed().await;
    wait_for(&mut pc, "call log changed", |e| match e {
        NodeEvent::CallLogChanged { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    // Without `call.dial` on the phone, dialing from the PC is refused as unsupported.
    assert!(matches!(pc.node.call_dial(phone_id, "+15550100".into(), None).await, Err(Error::Unsupported)));

    phone
        .node
        .update_power(
            PowerLevel::NotApplicable,
            vec!["media.control".into(), g7link_core::CALLS_LOG.into(), g7link_core::CALLS_DIAL.into()],
        )
        .await;
    wait_for(&mut pc, "call.dial unlocked", |e| match e {
        NodeEvent::Capabilities(m)
            if m.device == phone_id && m.state("calls.dial") == Some(FeatureState::Available) =>
        {
            Some(())
        }
        _ => None,
    })
    .await;

    with_timeout("dial", pc.node.call_dial(phone_id, "+15550100".into(), None)).await.unwrap();
    assert_eq!(*phone.platform.dialed.lock().unwrap(), ["+15550100"]);

    // A phone that doesn't list its SIMs has none to pick, and a SIM the PC
    // names anyway isn't sent: it dials on its own default.
    assert_eq!(with_timeout("no sims", pc.node.call_sims(phone_id)).await.unwrap(), Default::default());
    with_timeout("dial on a sim", pc.node.call_dial(phone_id, "+15550101".into(), Some("2".into())))
        .await
        .unwrap();
    assert_eq!(phone.platform.dialed.lock().unwrap().last().map(String::as_str), Some("+15550101"));

    // With `call.sims`, the PC sees the SIMs and the phone dials on the one picked.
    phone
        .node
        .update_power(
            PowerLevel::NotApplicable,
            vec![
                "media.control".into(),
                g7link_core::CALLS_LOG.into(),
                g7link_core::CALLS_DIAL.into(),
                g7link_core::CALLS_SIMS.into(),
            ],
        )
        .await;
    let sims = loop {
        let sims = with_timeout("sims", pc.node.call_sims(phone_id)).await.unwrap();
        if !sims.sims.is_empty() {
            break sims;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert_eq!(
        sims.sims.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(),
        ["SIM 1 · Airtel", "SIM 2 · Jio"]
    );
    with_timeout("dial on sim 2", pc.node.call_dial(phone_id, "+15550102".into(), Some("2".into())))
        .await
        .unwrap();
    assert_eq!(phone.platform.dialed.lock().unwrap().last().map(String::as_str), Some("+15550102 on 2"));

    // Turning `calls` off for this PC blocks both call log and dialing.
    phone.node.set_device_toggle(pc_id, "calls", false).unwrap();
    assert!(matches!(pc.node.call_log(phone_id, None, 3).await, Err(Error::Denied)));
    assert!(matches!(pc.node.call_dial(phone_id, "+15550100".into(), None).await, Err(Error::Denied)));
}

#[tokio::test(flavor = "multi_thread")]
async fn contacts_list_search_page_and_respect_permissions_and_toggles() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::CONTACTS_SHOW]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Without `contacts.read` on the phone, listing contacts is unsupported.
    assert!(matches!(pc.node.contacts(phone_id, None, 0, 10).await, Err(Error::Unsupported)));

    phone
        .node
        .update_power(
            PowerLevel::NotApplicable,
            vec!["media.control".into(), g7link_core::CONTACTS_READ.into()],
        )
        .await;
    wait_for(&mut pc, "contacts.read unlocked", |e| match e {
        NodeEvent::Capabilities(m)
            if m.device == phone_id && m.state("contacts.read") == Some(FeatureState::Available) =>
        {
            Some(())
        }
        _ => None,
    })
    .await;

    let first_two = with_timeout("contacts page 1", pc.node.contacts(phone_id, None, 0, 2)).await.unwrap();
    assert_eq!(first_two.len(), 2);
    assert_eq!((&*first_two[0].name, first_two[0].starred), ("Sam Rivera", true));
    assert_eq!(first_two[0].photo.as_deref(), Some(&[0xff, 0xd8][..]));
    // Oversized photo on c2 was stripped rather than dropping the contact.
    assert_eq!((&*first_two[1].name, first_two[1].photo.as_ref()), ("Alex Chen", None));

    let rest = with_timeout("contacts page 2", pc.node.contacts(phone_id, None, 2, 10)).await.unwrap();
    assert_eq!(rest.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Jordan Patel", "Priya Nair"]);

    let searched = with_timeout("contacts search", pc.node.contacts(phone_id, Some("patel".into()), 0, 10))
        .await
        .unwrap();
    assert_eq!(searched.len(), 1);
    assert_eq!(searched[0].name, "Jordan Patel");

    phone.node.contacts_changed().await;
    wait_for(&mut pc, "contacts changed", |e| match e {
        NodeEvent::ContactsChanged { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    // Turning `contacts` off for this PC refuses contact queries.
    phone.node.set_device_toggle(pc_id, "contacts", false).unwrap();
    assert!(matches!(pc.node.contacts(phone_id, None, 0, 10).await, Err(Error::Denied)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_cant_keep_more_contact_lookups_running_than_the_background_limit() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::CONTACTS_SHOW]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::CONTACTS_READ]).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    let gate = Arc::new(Gate::default());
    *phone.platform.contacts_gate.lock().unwrap() = Some(gate.clone());
    let asks: Vec<_> = (0..40)
        .map(|_| {
            let pc = pc.node.clone();
            tokio::spawn(async move { pc.contacts(phone_id, None, 0, 10).await })
        })
        .collect();
    // The PC has at most 16 RPCs in flight to a phone; the rest wait their turn.
    until("the first lookups running", || gate.held() >= 16).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let running = gate.held();
    assert!(running <= 16, "{running} lookups running at once");
    assert!(asks.iter().all(|a| !a.is_finished()), "none is turned away");

    gate.open();
    for ask in asks {
        if let Err(e) = with_timeout("contacts lookup", ask).await.unwrap() {
            panic!("unexpected {e:?}");
        }
    }
    let counters = pc.node.wire_counters(phone_id).unwrap();
    assert_eq!(counters.busy_received, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn gallery_albums_paging_thumbs_download_and_toggles() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::PHOTOS_SHOW]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Without `photos.read` on the phone, gallery queries are unsupported.
    assert!(matches!(pc.node.photo_albums(phone_id).await, Err(Error::Unsupported)));
    assert!(matches!(pc.node.photo_list(phone_id, None, None, 10).await, Err(Error::Unsupported)));
    assert!(matches!(pc.node.photo_thumbs(phone_id, vec!["media:4".into()]).await, Err(Error::Unsupported)));

    phone
        .node
        .update_power(
            PowerLevel::NotApplicable,
            vec!["media.control".into(), g7link_core::PHOTOS_READ.into()],
        )
        .await;
    wait_for(&mut pc, "photos.read unlocked", |e| match e {
        NodeEvent::Capabilities(m)
            if m.device == phone_id && m.state("files.recent_photos") == Some(FeatureState::Available) =>
        {
            Some(())
        }
        _ => None,
    })
    .await;

    // Albums list.
    let albums = with_timeout("albums", pc.node.photo_albums(phone_id)).await.unwrap();
    assert_eq!(albums.len(), 2);
    assert_eq!((&*albums[0].id, &*albums[0].name, albums[0].count), ("cam", "Camera", 3));
    assert_eq!(albums[0].cover.as_deref(), Some("media:4"));
    assert_eq!((&*albums[1].id, &*albums[1].name, albums[1].count), ("shots", "Screenshots", 2));

    // Paging all items newest-first; an item sharing the last one's date
    // still comes on the next page.
    let page1 = with_timeout("list page 1", pc.node.photo_list(phone_id, None, None, 2)).await.unwrap();
    assert_eq!(page1.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["media:4", "media:3"]);
    let last = Some((page1[1].date, page1[1].id.clone()));
    let page2 = with_timeout("list page 2", pc.node.photo_list(phone_id, None, last, 2)).await.unwrap();
    assert_eq!(page2.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["media:2", "media:1"]);
    assert_eq!(page2[0].duration, Some(14));

    // Filtering by album.
    let shots = with_timeout("list album", pc.node.photo_list(phone_id, Some("shots".into()), None, 10))
        .await
        .unwrap();
    assert_eq!(shots.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["media:3", "media:0"]);

    // Deleting asks the phone and reports how many went; nonsense IDs never leave the PC.
    let ids = vec!["media:4".to_owned(), "gone".to_owned(), "../bad id".to_owned()];
    assert_eq!(pc.node.photos_delete(phone_id, ids).await.unwrap(), 1);
    // Batch thumbnails (oversized or missing ones are skipped without failing the batch).
    let thumbs = with_timeout(
        "thumbs",
        pc.node.photo_thumbs(
            phone_id,
            vec!["media:4".into(), "oversized".into(), "gone".into(), "media:2".into()],
        ),
    )
    .await
    .unwrap();
    assert_eq!(thumbs.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["media:4", "media:2"]);
    assert_eq!(thumbs[0].data, vec![0xff, 0xd8, b'4']);
    assert_eq!(thumbs[1].data, vec![0xff, 0xd8, b'2']);

    // Multi-file download via `fetch_photos`.
    let img4 = phone.dir.path().join("IMG_4.jpg");
    let vid2 = phone.dir.path().join("VID_2.mp4");
    let bytes4 = data(50_000, 11);
    let bytes2 = data(120_000, 12);
    std::fs::write(&img4, &bytes4).unwrap();
    std::fs::write(&vid2, &bytes2).unwrap();
    {
        let mut map = phone.platform.photos.lock().unwrap();
        map.insert("media:4".into(), img4);
        map.insert("media:2".into(), vid2);
    }
    let tid = with_timeout(
        "fetch_photos",
        pc.node.fetch_photos(phone_id, vec!["media:4".into(), "media:2".into()]),
    )
    .await
    .unwrap();
    let files = wait_transfer(&mut pc, &tid, "gallery files", saved).await;
    assert_eq!(files.len(), 2);
    assert_eq!(std::fs::read(&files[0]).unwrap(), bytes4);
    assert_eq!(std::fs::read(&files[1]).unwrap(), bytes2);

    // A backup's download is saved the same way but leaves nothing in the Timeline.
    let quiet =
        with_timeout("fetch_photos_quietly", pc.node.fetch_photos_quietly(phone_id, vec!["media:4".into()]))
            .await
            .unwrap();
    let quiet_files = wait_transfer(&mut pc, &quiet, "backup file", saved).await;
    assert_eq!(std::fs::read(&quiet_files[0]).unwrap(), bytes4);
    let timeline =
        pc.node.timeline_page(&g7link_core::TimelineQuery { limit: 50, ..Default::default() }).unwrap();
    let refs: Vec<_> = timeline.entries.iter().filter_map(|e| e.ref_id.as_deref()).collect();
    assert!(refs.contains(&tid.as_str()), "the gallery download is in the Timeline");
    assert!(!refs.contains(&quiet.as_str()), "the backup download isn't");

    // `photos.changed` notification.
    phone.node.photos_changed().await;
    wait_for(&mut pc, "photos changed", |e| match e {
        NodeEvent::PhotosChanged { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    // Turning `photos` toggle off on the phone blocks albums, list, thumbs, and download.
    phone.node.set_device_toggle(pc_id, "photos", false).unwrap();
    assert!(matches!(pc.node.photo_albums(phone_id).await, Err(Error::Denied)));
    assert!(matches!(pc.node.photo_list(phone_id, None, None, 10).await, Err(Error::Denied)));
    assert!(matches!(pc.node.photo_thumbs(phone_id, vec!["media:4".into()]).await, Err(Error::Denied)));
    assert!(matches!(pc.node.fetch_photos(phone_id, vec!["media:4".into()]).await, Err(Error::Denied)));
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_input_needs_toggle_and_delivers_motion_keys_and_slides() {
    use g7link_core::{
        ButtonAction, INPUT_INJECT, KeyMod, MouseButton, RemoteInput, SlideAction, remote_keys,
    };

    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Without `input.inject` on the PC, remote input is unsupported.
    assert!(matches!(phone.node.remote_check(pc_id).await, Err(Error::Unsupported)));
    assert!(matches!(
        phone
            .node
            .remote_input(
                pc_id,
                RemoteInput::Button { button: MouseButton::Left, action: ButtonAction::Click }
            )
            .await,
        Err(Error::Unsupported)
    ));

    // The PC announces `input.inject`.
    pc.node.update_power(PowerLevel::NotApplicable, vec!["media.control".into(), INPUT_INJECT.into()]).await;
    wait_for(&mut phone, "input.remote available on phone", |e| match e {
        NodeEvent::Capabilities(m)
            if m.device == pc_id && m.state("input.remote") == Some(FeatureState::Available) =>
        {
            Some(())
        }
        _ => None,
    })
    .await;

    // On the PC, `remote_input` is off by default: the first ask is refused and
    // triggers the one-time prompt event on the PC.
    let toggles = pc.node.device_toggles(phone_id).unwrap();
    assert!(toggles.contains(&("remote_input", false)));
    assert!(matches!(phone.node.remote_check(pc_id).await, Err(Error::Denied)));
    wait_for(&mut pc, "remote input prompt", |e| match e {
        NodeEvent::RemoteInputRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    // While the toggle is off, discrete and datagram inputs are both refused.
    assert!(matches!(
        phone
            .node
            .remote_input(
                pc_id,
                RemoteInput::Button { button: MouseButton::Left, action: ButtonAction::Click }
            )
            .await,
        Err(Error::Denied)
    ));
    assert!(matches!(
        phone.node.remote_input(pc_id, RemoteInput::Move { dx: 10.0, dy: 20.0 }).await,
        Err(Error::Denied)
    ));
    phone.node.remote_move(pc_id, 10.0, 20.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(pc.platform.remote_inputs.lock().unwrap().is_empty(), "nothing injected while toggle is off");

    // The PC user enables `remote_input` for this phone.
    pc.node.set_device_toggle(phone_id, "remote_input", true).unwrap();
    with_timeout("remote_check allowed", phone.node.remote_check(pc_id)).await.unwrap();

    // Pointer motion (datagram), buttons, drag, scroll, text, keys, slides, and laser.
    phone.node.remote_move(pc_id, 18.5, -12.0).await.unwrap();
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if pc.platform.remote_inputs.lock().unwrap().contains(&RemoteInput::Move { dx: 18.5, dy: -12.0 }) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for move datagram");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let events = vec![
        RemoteInput::Button { button: MouseButton::Left, action: ButtonAction::Click },
        RemoteInput::Button { button: MouseButton::Right, action: ButtonAction::Click },
        RemoteInput::Button { button: MouseButton::Left, action: ButtonAction::Down },
        RemoteInput::Button { button: MouseButton::Left, action: ButtonAction::Up },
        RemoteInput::Scroll { dx: 0.0, dy: -3.0 },
        RemoteInput::Text { text: "Hello G7 Link 🐝".into() },
        RemoteInput::Key { key: remote_keys::ENTER.into(), mods: vec![] },
        RemoteInput::Key { key: "c".into(), mods: vec![KeyMod::Ctrl] },
        RemoteInput::Key { key: remote_keys::COPY.into(), mods: vec![] },
        RemoteInput::Slide { action: SlideAction::Next },
        RemoteInput::Slide { action: SlideAction::Previous },
        RemoteInput::Slide { action: SlideAction::Black },
    ];
    for ev in &events {
        with_timeout("remote_input", phone.node.remote_input(pc_id, ev.clone())).await.unwrap();
    }
    with_timeout("laser off", phone.node.remote_laser(pc_id, false, 0.0, 0.0)).await.unwrap();

    // Scrolls are injected by the PC's motion worker, on its own thread, so
    // they may land just after the answer.
    let deadline = tokio::time::Instant::now() + WAIT;
    let recorded = loop {
        let recorded = pc.platform.remote_inputs.lock().unwrap().clone();
        if events.iter().all(|ev| recorded.contains(ev)) || tokio::time::Instant::now() >= deadline {
            break recorded;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    for ev in &events {
        assert!(recorded.contains(ev), "missing {ev:?} in {recorded:?}");
    }
    assert!(recorded.contains(&RemoteInput::Laser { on: false, x: 0.0, y: 0.0 }));

    // Voice typing rewrites what it typed: the phone sends a backspace for each
    // character it replaces, then the new text, each as soon as the last is
    // answered. None of it may be refused (GitHub #52).
    pc.platform.remote_inputs.lock().unwrap().clear();
    let backspace = RemoteInput::Key { key: remote_keys::BACKSPACE.into(), mods: vec![] };
    let rewrite = RemoteInput::Text { text: "नमस्ते, how are you doing today".into() };
    for _ in 0..80 {
        with_timeout("backspace", phone.node.remote_input(pc_id, backspace.clone())).await.unwrap();
    }
    with_timeout("rewritten text", phone.node.remote_input(pc_id, rewrite.clone())).await.unwrap();
    // Pointer and laser events from the first half can still land after the clear
    // (they go through the motion worker); only the keys and text are checked here.
    let recorded: Vec<RemoteInput> = pc
        .platform
        .remote_inputs
        .lock()
        .unwrap()
        .iter()
        .filter(|e| {
            !matches!(e, RemoteInput::Laser { .. } | RemoteInput::Move { .. } | RemoteInput::Scroll { .. })
        })
        .cloned()
        .collect();
    let mut expected = vec![backspace; 80];
    expected.push(rewrite);
    assert_eq!(recorded, expected, "every backspace and the text arrive, in order");

    // Invalid inputs are rejected.
    assert!(
        phone
            .node
            .remote_input(pc_id, RemoteInput::Key { key: "unknown_key".into(), mods: vec![] })
            .await
            .is_err()
    );
    assert!(phone.node.remote_move(pc_id, 99999.0, 0.0).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watching_phone_hears_the_pcs_text_field_and_edits_it() {
    use g7link_core::{
        INPUT_APP_ICON, INPUT_INJECT, INPUT_TEXT_MIRROR, RemoteInput, RemoteTextState, TextFieldKind,
    };

    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // A PC without the mirror: the phone can't watch.
    pc.node.update_power(PowerLevel::NotApplicable, vec![INPUT_INJECT.into()]).await;
    wait_for(&mut phone, "input.remote available", |e| match e {
        NodeEvent::Capabilities(m)
            if m.device == pc_id && m.state("input.remote") == Some(FeatureState::Available) =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
    assert!(!phone.node.remote_mirrors_text(&pc_id));
    assert!(matches!(phone.node.remote_text_watch(pc_id, true).await, Err(Error::Unsupported)));
    let icon_key = "0123456789abcdef".to_owned();
    assert!(matches!(phone.node.remote_app_icon(pc_id, icon_key.clone()).await, Err(Error::Unsupported)));

    pc.node
        .update_power(
            PowerLevel::NotApplicable,
            vec![INPUT_INJECT.into(), INPUT_TEXT_MIRROR.into(), INPUT_APP_ICON.into()],
        )
        .await;
    let deadline = tokio::time::Instant::now() + WAIT;
    while !phone.node.remote_mirrors_text(&pc_id) {
        assert!(tokio::time::Instant::now() < deadline, "the phone learns the PC mirrors text");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Remote input is off on the PC: the watch isn't heard.
    phone.node.remote_text_watch(pc_id, true).await.unwrap();
    pc.node.set_device_toggle(phone_id, "remote_input", true).unwrap();
    phone.node.remote_text_watch(pc_id, true).await.unwrap();
    wait_for(&mut pc, "the phone watches", |e| match e {
        NodeEvent::TextWatch { device, on: true } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    let state = RemoteTextState {
        focus: 4,
        field: 77,
        epoch: 0,
        rev: 1,
        applied: 0,
        kind: TextFieldKind::Mirror,
        app: Some("Firefox".into()),
        label: Some("Search".into()),
        window: false,
        multiline: false,
        start: 0,
        before: "hello".into(),
        after: String::new(),
        anchor: None,
        more_before: false,
        more_after: false,
        icon: Some(icon_key.clone()),
    };
    pc.node.remote_text_state(phone_id, &state).await.unwrap();
    let heard = wait_for(&mut phone, "the PC's field", |e| match e {
        NodeEvent::PcTextState { device, state } if *device == pc_id => Some(state.clone()),
        _ => None,
    })
    .await;
    assert_eq!(heard, state);

    // The icon the state names: not kept yet, then kept and fetched.
    assert!(phone.node.remote_app_icon(pc_id, icon_key.clone()).await.is_err());
    let png = b"\x89PNG\r\n\x1a\nicon".to_vec();
    assert!(pc.node.keep_app_icon("not a key".into(), png.clone()).is_err());
    assert!(pc.node.keep_app_icon(icon_key.clone(), b"GIF89a".to_vec()).is_err());
    pc.node.keep_app_icon(icon_key.clone(), png.clone()).unwrap();
    assert_eq!(phone.node.remote_app_icon(pc_id, icon_key.clone()).await.unwrap(), png);
    assert!(matches!(phone.node.remote_app_icon(pc_id, "../etc".into()).await, Err(Error::Protocol(_))));

    let edit = RemoteInput::Edit {
        focus: 4,
        seq: 1,
        range: Some((0, 5)),
        moves: 0,
        backspaces: 0,
        text: "hi".into(),
    };
    phone.node.remote_input(pc_id, edit.clone()).await.unwrap();
    assert!(pc.platform.remote_inputs.lock().unwrap().contains(&edit));

    phone.node.remote_text_watch(pc_id, false).await.unwrap();
    wait_for(&mut pc, "the phone stops watching", |e| match e {
        NodeEvent::TextWatch { device, on: false } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn smooth_scroll_reaches_the_pc_in_fractions_of_a_click() {
    use g7link_core::{INPUT_INJECT, RemoteInput};

    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    pc.node.update_power(PowerLevel::NotApplicable, vec!["media.control".into(), INPUT_INJECT.into()]).await;
    wait_for(&mut phone, "input.remote available on phone", |e| match e {
        NodeEvent::Capabilities(m)
            if m.device == pc_id && m.state("input.remote") == Some(FeatureState::Available) =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
    pc.node.set_device_toggle(phone_id, "remote_input", true).unwrap();
    with_timeout("remote_check allowed", phone.node.remote_check(pc_id)).await.unwrap();

    // A two-finger scroll sends a little each frame, up, down and sideways.
    let steps = [(0.0, 0.125), (0.0, -0.75), (0.25, 0.0), (-1.5, 2.5)];
    for (dx, dy) in steps {
        with_timeout("scroll", phone.node.remote_scroll_fast(pc_id, dx, dy)).await.unwrap();
    }
    // The PC adds up scrolls that arrive together (remote.rs `coalesce`), so what
    // reaches it is the same total in fractions of a click, maybe in fewer events.
    // These steps are exact binary fractions, so any grouping adds up exactly.
    let want = steps.iter().fold((0.0f32, 0.0f32), |(x, y), &(dx, dy)| (x + dx, y + dy));
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let recorded = pc.platform.remote_inputs.lock().unwrap().clone();
        let total = recorded.iter().fold((0.0f32, 0.0f32), |(x, y), event| match event {
            RemoteInput::Scroll { dx, dy } => (x + dx, y + dy),
            _ => (x, y),
        });
        if total == want {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for scrolls: {recorded:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(want, (-1.25, 1.875), "the steps themselves are fractions of a click");

    // More than the protocol allows in one event never leaves the phone.
    assert!(phone.node.remote_scroll_fast(pc_id, 0.0, 500.0).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn voice_recordings_arrive_with_markers_and_respect_capability_and_toggle() {
    use g7link_core::{RECORDER, RecordingMarker, TransferFailure};

    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let src = tempfile::tempdir().unwrap();
    let audio = data(64 * 1024 + 31, 42);
    let markers = vec![
        RecordingMarker { at_ms: 1_250, label: Some("Intro".into()) },
        RecordingMarker { at_ms: 4_800, label: None },
    ];

    // Without the `recorder` capability on the PC, a recording transfer is refused as unsupported.
    let tid = phone
        .node
        .send_recording(pc_id, outgoing(src.path(), "rec1.m4a", &audio), markers.clone())
        .await
        .unwrap();
    let failure = wait_transfer(&mut phone, &tid, "refused without capability", |t| match &t.state {
        TransferState::Failed(f) => Some(f.clone()),
        _ => None,
    })
    .await;
    assert!(
        matches!(failure, TransferFailure::Other(ref s) if s.to_ascii_lowercase().contains("unsupported")),
        "{failure:?}"
    );

    // The PC announces `recorder`.
    pc.node.update_power(PowerLevel::NotApplicable, vec![RECORDER.into()]).await;
    wait_for(&mut phone, "files.recordings available", |e| match e {
        NodeEvent::Capabilities(m)
            if m.device == pc_id && m.state("files.recordings") == Some(FeatureState::Available) =>
        {
            Some(())
        }
        _ => None,
    })
    .await;

    // The recording arrives on the PC and is reported with `recording == true` and its markers.
    let tid = phone
        .node
        .send_recording(pc_id, outgoing(src.path(), "Recording.m4a", &audio), markers.clone())
        .await
        .unwrap();
    let (received_paths, got_recording, got_markers) =
        wait_transfer(&mut pc, &tid, "recording received", |t| {
            saved(t).map(|paths| (paths, t.recording, t.markers.clone()))
        })
        .await;
    assert!(got_recording);
    assert_eq!(got_markers, markers);
    assert_eq!(received_paths.len(), 1);
    assert_eq!(std::fs::read(&received_paths[0]).unwrap(), audio);
    wait_transfer(&mut phone, &tid, "recording sent", saved).await;

    // Turning off the `recordings` toggle on the PC denies incoming recordings while regular files still work.
    pc.node.set_device_toggle(phone_id, "recordings", false).unwrap();
    let tid =
        phone.node.send_recording(pc_id, outgoing(src.path(), "rec2.m4a", &audio), Vec::new()).await.unwrap();
    let denied = wait_transfer(&mut phone, &tid, "denied by toggle", |t| match &t.state {
        TransferState::Failed(f) => Some(f.clone()),
        _ => None,
    })
    .await;
    assert_eq!(denied, TransferFailure::Denied);
}

#[tokio::test(flavor = "multi_thread")]
async fn phone_toggles_arrive_on_connect_change_and_respect_capabilities_and_toggle() {
    use g7link_core::{
        PhoneToggleValue, PhoneToggles, TOGGLES_BLUETOOTH, TOGGLES_BRIGHTNESS, TOGGLES_DND,
        TOGGLES_FLASHLIGHT, TOGGLES_READ, TOGGLES_RINGER, TOGGLES_SHOW, TOGGLES_VOLUME, TOGGLES_WIFI,
        ringer_modes, toggle_ids,
    };

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[TOGGLES_SHOW]).await;
    let mut phone = device_with(
        "Pixel",
        DeviceKind::Phone,
        &[TOGGLES_READ, TOGGLES_RINGER, TOGGLES_VOLUME, TOGGLES_FLASHLIGHT],
    )
    .await;

    let initial = PhoneToggles {
        dnd: false,
        ringer: ringer_modes::RING.into(),
        flashlight: Some(false),
        volume: 65,
        brightness: 40,
        wifi: true,
        bluetooth: true,
        hotspot: None,
    };
    phone.node.toggles_changed(initial.clone()).await.unwrap();

    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Initial state arrives on connect.
    let got = wait_for(&mut pc, "initial toggles", |e| match e {
        NodeEvent::PhoneToggles { device, toggles } if *device == phone_id => Some(toggles.clone()),
        _ => None,
    })
    .await;
    assert_eq!(got, initial);

    // Volume, ringer (vibrate), and flashlight work at Basic without extra permissions.
    with_timeout(
        "set volume",
        pc.node.set_phone_toggle(phone_id, toggle_ids::VOLUME.into(), PhoneToggleValue::Level(80)),
    )
    .await
    .unwrap();
    with_timeout(
        "set ringer vibrate",
        pc.node.set_phone_toggle(
            phone_id,
            toggle_ids::RINGER.into(),
            PhoneToggleValue::Mode(ringer_modes::VIBRATE.into()),
        ),
    )
    .await
    .unwrap();
    with_timeout(
        "set flashlight on",
        pc.node.set_phone_toggle(phone_id, toggle_ids::FLASHLIGHT.into(), PhoneToggleValue::Bool(true)),
    )
    .await
    .unwrap();

    // Silent ringer and DND require `toggles.dnd`; brightness requires `toggles.brightness`;
    // Wi-Fi requires `toggles.wifi` (Elevated).
    assert!(matches!(
        pc.node
            .set_phone_toggle(
                phone_id,
                toggle_ids::RINGER.into(),
                PhoneToggleValue::Mode(ringer_modes::SILENT.into()),
            )
            .await,
        Err(Error::Unsupported)
    ));
    assert!(matches!(
        pc.node.set_phone_toggle(phone_id, toggle_ids::DND.into(), PhoneToggleValue::Bool(true)).await,
        Err(Error::Unsupported)
    ));
    assert!(matches!(
        pc.node.set_phone_toggle(phone_id, toggle_ids::BRIGHTNESS.into(), PhoneToggleValue::Level(50)).await,
        Err(Error::Unsupported)
    ));
    assert!(matches!(
        pc.node.set_phone_toggle(phone_id, toggle_ids::WIFI.into(), PhoneToggleValue::Bool(false)).await,
        Err(Error::Unsupported)
    ));

    // Invalid IDs or out-of-range values are rejected as protocol errors.
    assert!(matches!(
        pc.node.set_phone_toggle(phone_id, "unknown".into(), PhoneToggleValue::Bool(true)).await,
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        pc.node.set_phone_toggle(phone_id, toggle_ids::VOLUME.into(), PhoneToggleValue::Level(101)).await,
        Err(Error::Protocol(_))
    ));

    // Phone unlocks DND, brightness, and Elevated toggles.
    phone
        .node
        .update_power(
            PowerLevel::Elevated,
            vec![
                "media.control".into(),
                TOGGLES_READ.into(),
                TOGGLES_RINGER.into(),
                TOGGLES_VOLUME.into(),
                TOGGLES_FLASHLIGHT.into(),
                TOGGLES_DND.into(),
                TOGGLES_BRIGHTNESS.into(),
                TOGGLES_WIFI.into(),
                TOGGLES_BLUETOOTH.into(),
            ],
        )
        .await;
    wait_for(&mut pc, "toggles.wifi available", |e| match e {
        NodeEvent::Capabilities(m)
            if m.device == phone_id && m.state("toggles.wifi") == Some(FeatureState::Available) =>
        {
            Some(())
        }
        _ => None,
    })
    .await;

    with_timeout(
        "set ringer silent",
        pc.node.set_phone_toggle(
            phone_id,
            toggle_ids::RINGER.into(),
            PhoneToggleValue::Mode(ringer_modes::SILENT.into()),
        ),
    )
    .await
    .unwrap();
    with_timeout(
        "set dnd on",
        pc.node.set_phone_toggle(phone_id, toggle_ids::DND.into(), PhoneToggleValue::Bool(true)),
    )
    .await
    .unwrap();
    with_timeout(
        "set brightness",
        pc.node.set_phone_toggle(phone_id, toggle_ids::BRIGHTNESS.into(), PhoneToggleValue::Level(90)),
    )
    .await
    .unwrap();
    with_timeout(
        "set wifi off",
        pc.node.set_phone_toggle(phone_id, toggle_ids::WIFI.into(), PhoneToggleValue::Bool(false)),
    )
    .await
    .unwrap();

    let recorded = phone.platform.phone_toggles.lock().unwrap().clone();
    assert_eq!(
        recorded,
        vec![
            (toggle_ids::VOLUME.into(), PhoneToggleValue::Level(80)),
            (toggle_ids::RINGER.into(), PhoneToggleValue::Mode(ringer_modes::VIBRATE.into())),
            (toggle_ids::FLASHLIGHT.into(), PhoneToggleValue::Bool(true)),
            (toggle_ids::RINGER.into(), PhoneToggleValue::Mode(ringer_modes::SILENT.into())),
            (toggle_ids::DND.into(), PhoneToggleValue::Bool(true)),
            (toggle_ids::BRIGHTNESS.into(), PhoneToggleValue::Level(90)),
            (toggle_ids::WIFI.into(), PhoneToggleValue::Bool(false)),
        ]
    );

    // Phone pushes updated toggles state; PC receives it.
    let updated = PhoneToggles {
        dnd: true,
        ringer: ringer_modes::SILENT.into(),
        flashlight: Some(true),
        volume: 80,
        brightness: 90,
        wifi: false,
        bluetooth: true,
        hotspot: Some(true),
    };
    phone.node.toggles_changed(updated.clone()).await.unwrap();
    let got_updated = wait_for(&mut pc, "updated toggles", |e| match e {
        NodeEvent::PhoneToggles { device, toggles } if *device == phone_id && *toggles == updated => {
            Some(toggles.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(got_updated, updated);

    // Turning off the `toggles` device toggle on the phone refuses `phone.toggle.set`.
    phone.node.set_device_toggle(pc_id, "toggles", false).unwrap();
    assert!(matches!(
        pc.node.set_phone_toggle(phone_id, toggle_ids::VOLUME.into(), PhoneToggleValue::Level(50)).await,
        Err(Error::Denied)
    ));
}

/// A device relays Wake-on-LAN only for an address one of its paired PCs
/// gave as its own (#74 audit), never for any MAC another device names.
#[tokio::test(flavor = "multi_thread")]
async fn wake_relay_is_only_for_a_paired_pcs_own_address() {
    use g7link_core::{PC_WAKE, PcWakeInfo};
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[PC_WAKE]).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pc.node
        .set_wake_info(PcWakeInfo {
            macs: vec!["00:00:5E:00:53:01".into()],
            broadcasts: vec!["127.255.255.255".into()],
        })
        .await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    wait_for(&mut phone, "wake info", |e| match e {
        NodeEvent::WakeInfoChanged { device, can_wake: true } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    let loopback = Some(std::net::Ipv4Addr::new(127, 255, 255, 255));

    // The PC's own address: the phone relays it.
    with_timeout(
        "relay own",
        pc.node.wake_via_relay(&phone_id, [0x00, 0x00, 0x5e, 0x00, 0x53, 0x01], loopback),
    )
    .await
    .expect("a paired PC's own address is relayed");
    // Any other address: refused.
    let other = with_timeout(
        "relay other",
        pc.node.wake_via_relay(&phone_id, [0x02, 0x11, 0x22, 0x33, 0x44, 0x55], loopback),
    )
    .await;
    assert!(matches!(other, Err(Error::Denied)), "{other:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn wake_info_syncs_persists_respects_toggle_and_sends_magic_packet() {
    use g7link_core::{PC_WAKE, PcWakeInfo, magic_packet};
    use std::net::Ipv4Addr;

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[PC_WAKE]).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;

    pc.node
        .set_wake_info(PcWakeInfo {
            macs: vec!["00:00:5E:00:53:01".into()],
            broadcasts: vec!["192.168.1.255".into()],
        })
        .await;

    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Initial wake info arrives on connect and is stored on the phone.
    wait_for(&mut phone, "initial wake info", |e| match e {
        NodeEvent::WakeInfoChanged { device, can_wake: true } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    assert_eq!(
        phone.node.wake_info(pc_id).unwrap(),
        Some(PcWakeInfo { macs: vec!["00:00:5e:00:53:01".into()], broadcasts: vec!["192.168.1.255".into()] })
    );
    assert!(phone.node.paired_devices().unwrap().iter().any(|d| d.id == pc_id && d.can_wake));

    // Adapter change on the PC updates connected phones immediately.
    pc.node
        .set_wake_info(PcWakeInfo {
            macs: vec!["00:00:5E:00:53:01".into(), "aa-bb-cc-dd-ee-ff".into()],
            broadcasts: vec!["192.168.1.255".into(), "10.0.0.255".into()],
        })
        .await;
    let phone_node = phone.node.clone();
    wait_for(&mut phone, "updated wake info", |e| match e {
        NodeEvent::WakeInfoChanged { device, can_wake: true }
            if *device == pc_id
                && phone_node
                    .wake_info(pc_id)
                    .ok()
                    .flatten()
                    .is_some_and(|w| w.macs.len() == 2 && w.broadcasts.len() == 2) =>
        {
            Some(())
        }
        _ => None,
    })
    .await;

    // Turning `pc_actions` off on the PC clears the phone's stored wake info; turning it back on restores it.
    pc.node.set_device_toggle(phone_id, "pc_actions", false).unwrap();
    wait_for(&mut phone, "wake info cleared", |e| match e {
        NodeEvent::WakeInfoChanged { device, can_wake: false } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    assert_eq!(phone.node.wake_info(pc_id).unwrap(), None);
    assert!(matches!(phone.node.wake(pc_id).await, Err(Error::Unsupported)));

    pc.node.set_device_toggle(phone_id, "pc_actions", true).unwrap();
    wait_for(&mut phone, "wake info restored", |e| match e {
        NodeEvent::WakeInfoChanged { device, can_wake: true } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;

    // The PC goes offline and the phone restarts: stored wake info survives across restarts.
    pc.node.shutdown().await;
    phone.node.shutdown().await;
    let TestDevice { node, dir, .. } = phone;
    drop(node);
    let phone = start_in(dir, "Pixel", DeviceKind::Phone).await;
    assert!(phone.node.paired_devices().unwrap().iter().any(|d| d.id == pc_id && d.can_wake));

    // Waking the offline PC sends the 102-byte magic packet over UDP.
    let listener = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    phone.node.wake_on_ports(pc_id, &[port], &[Ipv4Addr::LOCALHOST]).await.unwrap();

    let mut buf = [0u8; 256];
    let (n, _) = with_timeout("receive magic packet", listener.recv_from(&mut buf)).await.unwrap();
    assert_eq!(n, 102);
    assert_eq!(&buf[..102], &magic_packet(&[0x00, 0x00, 0x5E, 0x00, 0x53, 0x01])[..]);
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deck_layout_and_state_arrive_press_runs_action_and_respects_toggles_and_unknown_tile() {
    use g7link_core::{
        DECK_ACTIONS, DeckAction, DeckConfig, DeckLayout, DeckState, DeckTileConfig, deck_colors, deck_icons,
    };

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[DECK_ACTIONS]).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;

    let mut cfg = DeckConfig::default_deck();
    cfg.pages[0].tiles.push(DeckTileConfig {
        id: "run_build".into(),
        label: "Build".into(),
        icon: deck_icons::TERMINAL.into(),
        color: deck_colors::CORAL.into(),
        action: DeckAction::RunCommand { command: "cargo check".into() },
    });
    let expected_layout = cfg.to_wire_layout();
    pc.node.set_deck_layout(expected_layout.clone()).await.unwrap();
    let initial_state = DeckState {
        playing: true,
        volume: 60,
        muted: false,
        mic_muted: Some(false),
        output_devices: Vec::new(),
    };
    pc.node.set_deck_state(initial_state.clone()).await.unwrap();

    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Initial layout and live state arrive on connect.
    let got_layout = wait_for(&mut phone, "initial deck layout", |e| match e {
        NodeEvent::DeckLayout { device, layout } if *device == pc_id => Some(layout.clone()),
        _ => None,
    })
    .await;
    assert_eq!(got_layout, expected_layout);
    assert_eq!(phone.node.deck_layout(pc_id), Some(expected_layout));

    let got_state = wait_for(&mut phone, "initial deck state", |e| match e {
        NodeEvent::DeckState { device, state } if *device == pc_id && *state == initial_state => {
            Some(state.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(got_state, initial_state);
    assert_eq!(phone.node.deck_state(pc_id), Some(initial_state));

    // Both `remote_input` and `commands` are off by default on the PC.
    let toggles = pc.node.device_toggles(phone_id).unwrap();
    assert!(toggles.contains(&("remote_input", false)));
    assert!(toggles.contains(&("commands", false)));

    // Pressing any tile while `remote_input` is off is denied and emits the one-time prompt.
    assert!(matches!(phone.node.deck_press(pc_id, "play_pause".into()).await, Err(Error::Denied)));
    wait_for(&mut pc, "remote input prompt from deck", |e| match e {
        NodeEvent::RemoteInputRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert!(pc.platform.deck_presses.lock().unwrap().is_empty());

    // Enable `remote_input`: built-in tiles work, unknown tiles return NotFound,
    // and `run_command` tiles are still denied while `commands` is off.
    pc.node.set_device_toggle(phone_id, "remote_input", true).unwrap();
    with_timeout("press play_pause", phone.node.deck_press(pc_id, "play_pause".into())).await.unwrap();
    with_timeout("press mic_mute", phone.node.deck_press(pc_id, "mic_mute".into())).await.unwrap();
    assert!(matches!(phone.node.deck_press(pc_id, "no_such_tile".into()).await, Err(Error::NotFound)));
    assert!(matches!(phone.node.deck_press(pc_id, "bad tile!".into()).await, Err(Error::Protocol(_))));
    assert!(matches!(phone.node.deck_press(pc_id, "run_build".into()).await, Err(Error::Denied)));

    // Enable `commands`: the `run_build` tile now runs.
    pc.node.set_device_toggle(phone_id, "commands", true).unwrap();
    with_timeout("press run_build", phone.node.deck_press(pc_id, "run_build".into())).await.unwrap();

    assert_eq!(
        *pc.platform.deck_presses.lock().unwrap(),
        vec!["play_pause".to_owned(), "mic_mute".to_owned(), "run_build".to_owned()]
    );

    // Live state updates push to the phone.
    let updated_state = DeckState {
        playing: false,
        volume: 45,
        muted: true,
        mic_muted: Some(true),
        output_devices: Vec::new(),
    };
    pc.node.set_deck_state(updated_state.clone()).await.unwrap();
    let got_updated = wait_for(&mut phone, "updated deck state", |e| match e {
        NodeEvent::DeckState { device, state } if *device == pc_id && *state == updated_state => {
            Some(state.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(got_updated, updated_state);

    // Layout edits push to the phone; removed tiles can no longer be pressed.
    let default_only = DeckLayout::default_layout();
    pc.node.set_deck_layout(default_only.clone()).await.unwrap();
    wait_for(&mut phone, "updated deck layout", |e| match e {
        NodeEvent::DeckLayout { device, layout } if *device == pc_id && *layout == default_only => Some(()),
        _ => None,
    })
    .await;
    assert!(matches!(phone.node.deck_press(pc_id, "run_build".into()).await, Err(Error::NotFound)));
}

/// A phone browses a PC's shared folders: the toggles named `storage` (on
/// the PC: browse that phone; on the phone: let that PC browse it) don't
/// stand in the way, whichever way they are set.
#[tokio::test(flavor = "multi_thread")]
async fn a_phone_browses_a_pcs_shared_folders() {
    use g7link_core::{FolderStorage, STORAGE_MOUNT, STORAGE_READ, STORAGE_WRITE};

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[STORAGE_MOUNT, STORAGE_READ]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[STORAGE_READ, STORAGE_WRITE]).await;
    let shared = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(shared.path().join("Downloads")).unwrap();
    std::fs::write(shared.path().join("Downloads/report.pdf"), b"pdf").unwrap();
    *pc.platform.storage.lock().unwrap() = Some(FolderStorage::new(shared.path()));

    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    for (pc_toggle, phone_toggle) in [(None, None), (Some(false), Some(false)), (Some(true), Some(true))] {
        if let Some(on) = pc_toggle {
            pc.node.set_device_toggle(phone_id, "storage", on).unwrap();
        }
        if let Some(on) = phone_toggle {
            phone.node.set_device_toggle(pc_id, "storage", on).unwrap();
        }
        let root = with_timeout("list the PC", phone.node.storage_list(pc_id, "")).await.unwrap();
        assert_eq!(root.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Downloads"], "{pc_toggle:?}");
        let files =
            with_timeout("list Downloads", phone.node.storage_list(pc_id, "Downloads")).await.unwrap();
        assert_eq!(files.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["report.pdf"]);
    }

    // A PC that doesn't share its folders says no.
    *pc.platform.storage.lock().unwrap() = None;
    assert!(phone.node.storage_list(pc_id, "").await.is_err());
}

/// A folder whose entries don't fit one answer comes back whole, page by
/// page (a camera folder of thousands of photos, say).
#[tokio::test(flavor = "multi_thread")]
async fn a_folder_too_big_for_one_answer_is_listed_whole() {
    use g7link_core::{FolderStorage, STORAGE_MOUNT, STORAGE_READ};

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[STORAGE_MOUNT, STORAGE_READ]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[STORAGE_READ]).await;
    let shared = tempfile::tempdir().unwrap();
    let camera = shared.path().join("Camera");
    std::fs::create_dir_all(&camera).unwrap();
    // Long names, so a few thousand entries pass one frame (1 MiB).
    let count = 8_000;
    for i in 0..count {
        std::fs::write(camera.join(format!("{i:05}-{}.jpg", "x".repeat(140))), b"").unwrap();
    }
    *pc.platform.storage.lock().unwrap() = Some(FolderStorage::new(shared.path()));
    pair_qr(&mut pc, &mut phone).await;
    let pc_id = pc.node.device_id();

    let listed = with_timeout("list a big folder", phone.node.storage_list(pc_id, "Camera")).await.unwrap();
    assert_eq!(listed.len(), count, "every entry, across pages");
    let mut names: Vec<_> = listed.iter().map(|e| e.name.clone()).collect();
    names.dedup();
    assert_eq!(names.len(), count, "no entry twice");
}

#[tokio::test(flavor = "multi_thread")]
async fn phone_storage_lists_reads_ranges_writes_resumes_mutates_and_enforces_security() {
    use g7link_core::{FolderStorage, STORAGE_MOUNT, STORAGE_READ, STORAGE_WRITE};

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[STORAGE_MOUNT]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[STORAGE_READ, STORAGE_WRITE]).await;

    let storage_root = tempfile::tempdir().unwrap();
    let trash_root = tempfile::tempdir().unwrap();
    let outside_root = tempfile::tempdir().unwrap();
    std::fs::write(outside_root.path().join("outside.txt"), b"do not expose").unwrap();

    std::fs::create_dir_all(storage_root.path().join("Documents")).unwrap();
    std::fs::create_dir_all(storage_root.path().join("DCIM")).unwrap();
    std::fs::write(storage_root.path().join("Documents/readme.txt"), b"0123456789abcdef").unwrap();
    std::fs::write(storage_root.path().join("DCIM/photo.jpg"), b"fake-jpeg-bytes").unwrap();

    // Create a directory link inside storage_root pointing to outside_root to verify symlink/junction rejection.
    {
        let _ = std::os::unix::fs::symlink(outside_root.path(), storage_root.path().join("escape_link"));
    }

    *phone.platform.storage.lock().unwrap() =
        Some(FolderStorage::new(storage_root.path()).with_trash_dir(trash_root.path()));

    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // `storage` toggle is OFF by default on the phone.
    let toggles = phone.node.device_toggles(pc_id).unwrap();
    assert!(toggles.contains(&("storage", false)));

    // Requests while `storage` is off are denied and emit `StorageRequested` once on the phone.
    assert!(matches!(pc.node.storage_list(phone_id, "").await, Err(Error::Denied)));
    wait_for(&mut phone, "storage requested prompt", |e| match e {
        NodeEvent::StorageRequested { device } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    assert!(matches!(
        pc.node.storage_read(phone_id, "Documents/readme.txt", 0, None).await,
        Err(Error::Denied)
    ));
    assert!(matches!(pc.node.storage_mkdir(phone_id, "NewFolder").await, Err(Error::Denied)));

    // Enable `storage` on the phone for this PC.
    phone.node.set_device_toggle(pc_id, "storage", true).unwrap();

    // Root listing returns DCIM and Documents (sorted), and omits `escape_link` pointing outside root.
    let root_entries = with_timeout("list root", pc.node.storage_list(phone_id, "")).await.unwrap();
    let names: Vec<&str> = root_entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["DCIM", "Documents"]);
    assert!(root_entries.iter().all(|e| e.is_dir));
    assert!(phone.node.storage_open_folders().contains(&String::new()));

    // Listing `Documents` returns `readme.txt` with its size.
    let doc_entries =
        with_timeout("list Documents", pc.node.storage_list(phone_id, "Documents")).await.unwrap();
    assert_eq!(doc_entries.len(), 1);
    assert_eq!(doc_entries[0].name, "readme.txt");
    assert_eq!(doc_entries[0].size, 16);
    assert!(!doc_entries[0].is_dir);

    // Full read and ranged read (`offset` + `length`).
    let (meta_full, bytes_full) =
        with_timeout("read full", pc.node.storage_read(phone_id, "Documents/readme.txt", 0, None))
            .await
            .unwrap();
    assert_eq!(meta_full.size, 16);
    assert_eq!(meta_full.length, 16);
    assert_eq!(bytes_full, b"0123456789abcdef");

    let (meta_range, bytes_range) =
        with_timeout("read range", pc.node.storage_read(phone_id, "Documents/readme.txt", 6, Some(5)))
            .await
            .unwrap();
    assert_eq!(meta_range.size, 16);
    assert_eq!(meta_range.length, 5);
    assert_eq!(bytes_range, b"6789a");

    let (meta_eof, bytes_eof) =
        with_timeout("read past eof", pc.node.storage_read(phone_id, "Documents/readme.txt", 100, Some(10)))
            .await
            .unwrap();
    assert_eq!(meta_eof.size, 16);
    assert_eq!(meta_eof.length, 0);
    assert!(bytes_eof.is_empty());

    // `mkdir` creates a directory and triggers `storage.changed` for watched parent `""`.
    with_timeout("mkdir Projects", pc.node.storage_mkdir(phone_id, "Projects")).await.unwrap();
    assert!(storage_root.path().join("Projects").is_dir());
    let changed_dir = wait_for(&mut pc, "storage changed on root", |e| match e {
        NodeEvent::StorageChanged { device, path } if *device == phone_id => Some(path.clone()),
        _ => None,
    })
    .await;
    assert_eq!(changed_dir, "");

    // `write` with interruption and resume using the same upload ID.
    let upload_src = tempfile::tempdir().unwrap();
    let payload = data(250 * 1024, 9);
    let local_file = upload_src.path().join("upload.bin");
    std::fs::write(&local_file, &payload).unwrap();

    // First attempt stops after 100 KiB.
    let interrupted = pc
        .node
        .storage_write_with_id(
            phone_id,
            "Documents/upload.bin",
            &local_file,
            Some("resume-upload-01".into()),
            Some(100 * 1024),
        )
        .await;
    assert!(interrupted.is_err());
    assert!(!storage_root.path().join("Documents/upload.bin").exists());

    // Wait briefly for the phone to flush the partial staging file, then resume with the same ID.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let done = with_timeout(
        "resumed write",
        pc.node.storage_write_with_id(
            phone_id,
            "Documents/upload.bin",
            &local_file,
            Some("resume-upload-01".into()),
            None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(done.size, payload.len() as u64);
    assert_eq!(std::fs::read(storage_root.path().join("Documents/upload.bin")).unwrap(), payload);

    // If a partial upload is interrupted and the source file's content changes (same size and ID),
    // the second upload does not splice the stale prefix into the new content.
    let payload_v1 = data(200 * 1024, 11);
    let payload_v2 = data(200 * 1024, 22);
    let changed_file = upload_src.path().join("changed.bin");
    std::fs::write(&changed_file, &payload_v1).unwrap();
    let _ = pc
        .node
        .storage_write_with_id(
            phone_id,
            "Documents/changed.bin",
            &changed_file,
            Some("resume-upload-changed".into()),
            Some(80 * 1024),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    std::fs::write(&changed_file, &payload_v2).unwrap();
    with_timeout(
        "write after content change",
        pc.node.storage_write_with_id(
            phone_id,
            "Documents/changed.bin",
            &changed_file,
            Some("resume-upload-changed".into()),
            None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(storage_root.path().join("Documents/changed.bin")).unwrap(), payload_v2);
    pc.node.storage_delete(phone_id, "Documents/changed.bin", true).await.unwrap();

    // `rename` moves the uploaded file into `Projects/renamed.bin`.
    with_timeout(
        "rename file",
        pc.node.storage_rename(phone_id, "Documents/upload.bin", "Projects/renamed.bin"),
    )
    .await
    .unwrap();
    assert!(!storage_root.path().join("Documents/upload.bin").exists());
    assert_eq!(std::fs::read(storage_root.path().join("Projects/renamed.bin")).unwrap(), payload);

    // `delete`: media file moves to trash even with `confirmed = false`; non-media requires `confirmed = true`.
    with_timeout("trash photo", pc.node.storage_delete(phone_id, "DCIM/photo.jpg", false)).await.unwrap();
    assert!(!storage_root.path().join("DCIM/photo.jpg").exists());
    assert_eq!(std::fs::read(trash_root.path().join("photo.jpg")).unwrap(), b"fake-jpeg-bytes");

    assert!(matches!(
        pc.node.storage_delete(phone_id, "Projects/renamed.bin", false).await,
        Err(Error::Denied)
    ));
    with_timeout("confirmed delete", pc.node.storage_delete(phone_id, "Projects/renamed.bin", true))
        .await
        .unwrap();
    assert!(!storage_root.path().join("Projects/renamed.bin").exists());

    // Path traversal, absolute paths, `/data`, and symlinks/junctions outside root are refused.
    for bad in ["..", "../outside.txt", "Documents/../../outside.txt", "/etc/passwd", "/data/data"] {
        assert!(pc.node.storage_list(phone_id, bad).await.is_err(), "list {bad}");
        assert!(pc.node.storage_read(phone_id, bad, 0, None).await.is_err(), "read {bad}");
        assert!(pc.node.storage_mkdir(phone_id, bad).await.is_err(), "mkdir {bad}");
        assert!(pc.node.storage_delete(phone_id, bad, true).await.is_err(), "delete {bad}");
    }
    if storage_root.path().join("escape_link").exists() {
        assert!(matches!(
            pc.node.storage_list(phone_id, "escape_link").await,
            Err(Error::Denied | Error::NotFound)
        ));
        assert!(matches!(
            pc.node.storage_read(phone_id, "escape_link/outside.txt", 0, None).await,
            Err(Error::Denied | Error::NotFound)
        ));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_phone_camera_streams_to_the_pc_as_a_webcam() {
    webcam_round_trip(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_phone_negotiates_hevc_only_with_an_advertising_receiver() {
    webcam_round_trip(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn screen_mirroring_prefers_hevc_only_when_the_receiver_advertises_it() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::MIRROR_HEVC]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[]).await;
    pair_qr(&mut pc, &mut phone).await;
    let pc_id = pc.node.device_id();

    assert_eq!(phone.node.mirror_codecs(pc_id).unwrap(), ["h265", "h264"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn screen_mirroring_uses_h264_for_receivers_without_hevc_support() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[]).await;
    pair_qr(&mut pc, &mut phone).await;

    assert_eq!(phone.node.mirror_codecs(pc.node.device_id()).unwrap(), ["h264"]);
}

async fn webcam_round_trip(hevc: bool) {
    use g7link_core::{MirrorSend, PacketKind};
    let mut caps = vec![g7link_core::WEBCAM_VIRTUAL];
    if hevc {
        caps.push(g7link_core::WEBCAM_HEVC);
    }
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &caps).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::WEBCAM_STREAM]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let codecs = phone.node.webcam_codecs(pc_id).unwrap();
    assert_eq!(codecs, if hevc { vec!["h265", "h264"] } else { vec!["h264"] });

    let options = g7link_core::WebcamStart {
        camera: g7link_core::WEBCAM_CAMERA_BACK.into(),
        width: 1920,
        height: 1080,
        fps: 30,
        bitrate: 6_000_000,
    };
    with_timeout("webcam_start", pc.node.webcam_start(phone_id, options.clone())).await.unwrap();
    assert_eq!(*phone.platform.webcam_asks.lock().unwrap(), ["start back 1920x1080 @ 30"]);

    let stream = Arc::new(with_timeout("webcam_open", phone.node.webcam_open(pc_id)).await.unwrap());
    let config = g7link_core::WebcamConfig {
        codec: codecs[0].clone(),
        width: 1920,
        height: 1080,
        camera: g7link_core::WEBCAM_CAMERA_BACK.into(),
        fps: 30,
        max_height: 2160,
    };
    let sender = stream.clone();
    let sent = tokio::task::spawn_blocking(move || {
        [
            sender.send(PacketKind::Config, 0, config.to_cbor()),
            sender.send(PacketKind::Keyframe, 1, vec![0; 48_000]),
            sender.send(PacketKind::Frame, 33_333, vec![0; 3_200]),
        ]
    })
    .await
    .unwrap();
    assert!(sent.iter().all(|s| *s == MirrorSend::Queued), "{sent:?}");
    wait_for(&mut pc, "webcam on", |e| matches!(e, NodeEvent::Webcam { on: true, .. }).then_some(())).await;

    let sink = pc.platform.webcam.clone();
    let deadline = tokio::time::Instant::now() + WAIT;
    while sink.got.lock().unwrap().len() < 3 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(*sink.got.lock().unwrap(), ["config back 1920x1080 @ 30", "key 1 48000", "frame 33333 3200"]);

    pc.node.webcam_keyframe(phone_id).await;
    pc.node.webcam_stop(phone_id).await;
    wait_for(&mut pc, "webcam off", |e| matches!(e, NodeEvent::Webcam { on: false, .. }).then_some(())).await;

    let deadline = tokio::time::Instant::now() + WAIT;
    while !stream.is_closed() && tokio::time::Instant::now() < deadline {
        let s = stream.clone();
        tokio::task::spawn_blocking(move || s.send(PacketKind::Frame, 0, vec![0])).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(stream.is_closed(), "the phone's webcam stream closes");

    for wanted in ["keyframe", "stop"] {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !phone.platform.webcam_asks.lock().unwrap().iter().any(|a| a == wanted) {
            let asks = phone.platform.webcam_asks.lock().unwrap().clone();
            assert!(std::time::Instant::now() < deadline, "{wanted}: {asks:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    // Turning off the webcam device toggle refuses new requests.
    phone.node.set_device_toggle(pc_id, "webcam", false).unwrap();
    let refused = pc.node.webcam_start(phone_id, options).await;
    assert!(matches!(refused, Err(Error::Denied)), "{refused:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_webcam_turned_off_before_its_format_arrives_doesnt_start() {
    use g7link_core::PacketKind;
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::WEBCAM_VIRTUAL]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::WEBCAM_STREAM]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // The PC has the stream's header and waits for its format when the user turns the webcam off.
    let stream = Arc::new(with_timeout("webcam_open", phone.node.webcam_open(pc_id)).await.unwrap());
    tokio::time::sleep(Duration::from_millis(300)).await;
    pc.node.set_device_toggle(phone_id, "webcam", false).unwrap();

    let config = g7link_core::WebcamConfig {
        codec: "h264".into(),
        width: 1920,
        height: 1080,
        camera: g7link_core::WEBCAM_CAMERA_BACK.into(),
        fps: 30,
        max_height: 2160,
    };
    let sender = stream.clone();
    tokio::task::spawn_blocking(move || sender.send(PacketKind::Config, 0, config.to_cbor())).await.unwrap();
    let deadline = tokio::time::Instant::now() + WAIT;
    while !stream.is_closed() && tokio::time::Instant::now() < deadline {
        let s = stream.clone();
        tokio::task::spawn_blocking(move || s.send(PacketKind::Frame, 0, vec![0])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(stream.is_closed(), "the PC refuses the phone's webcam stream");
    assert!(!pc.platform.webcam_asks.lock().unwrap().iter().any(|a| a == "sink"), "no webcam started");
}

#[tokio::test(flavor = "multi_thread")]
async fn pc_audio_volume_mute_output_devices_and_smart_clipboard_context_chips_flow_end_to_end() {
    use g7link_core::{AudioOutputDevice, ClipKind, DECK_ACTIONS, DeckState, PC_AUDIO};

    let mut pc =
        device_with("Desktop", DeviceKind::Desktop, &[DECK_ACTIONS, PC_AUDIO, "clipboard.sync"]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &["clipboard.sync"]).await;

    let initial_state = DeckState {
        playing: false,
        volume: 75,
        muted: false,
        mic_muted: Some(false),
        output_devices: vec![
            AudioOutputDevice {
                id: "{0.0.0.00000000}.{speakers-id}".into(),
                name: "Speakers (Realtek Audio)".into(),
                is_default: true,
            },
            AudioOutputDevice {
                id: "{0.0.0.00000000}.{headphones-id}".into(),
                name: "Headphones (USB DAC)".into(),
                is_default: false,
            },
        ],
    };
    pc.node.set_deck_state(initial_state.clone()).await.unwrap();

    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Initial DeckState with active output devices arrives on connect.
    let got_state = wait_for(&mut phone, "initial deck state with output devices", |e| match e {
        NodeEvent::DeckState { device, state } if *device == pc_id && *state == initial_state => {
            Some(state.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(got_state.default_output_name(), Some("Speakers (Realtek Audio)"));
    assert_eq!(got_state.output_devices.len(), 2);

    // Phone sets PC volume and mute (`pc_actions` is ON by default).
    with_timeout("set pc volume 40", phone.node.set_pc_audio(pc_id, Some(40), None)).await.unwrap();
    let after_vol = wait_for(&mut phone, "deck state volume 40", |e| match e {
        NodeEvent::DeckState { device, state } if *device == pc_id && state.volume == 40 => {
            Some(state.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(after_vol.volume, 40);
    assert!(!after_vol.muted);

    with_timeout("set pc mute true", phone.node.set_pc_audio(pc_id, None, Some(true))).await.unwrap();
    let after_mute = wait_for(&mut phone, "deck state muted true", |e| match e {
        NodeEvent::DeckState { device, state } if *device == pc_id && state.muted => Some(state.clone()),
        _ => None,
    })
    .await;
    assert!(after_mute.muted);
    assert_eq!(*pc.platform.pc_audio_changes.lock().unwrap(), vec![(Some(40), None), (None, Some(true))]);

    // Out-of-range volume or empty request is rejected as a protocol error.
    assert!(matches!(phone.node.set_pc_audio(pc_id, Some(101), None).await, Err(Error::Protocol(_))));
    assert!(matches!(phone.node.set_pc_audio(pc_id, None, None).await, Err(Error::Protocol(_))));

    // When both `pc_actions` and `remote_input` are off, `set_pc_audio` is denied.
    pc.node.set_device_toggle(phone_id, "pc_actions", false).unwrap();
    assert!(matches!(phone.node.set_pc_audio(pc_id, Some(50), None).await, Err(Error::Denied)));

    // Switching the output is a PC action too.
    let headphones = "{0.0.0.00000000}.{headphones-id}".to_owned();
    assert!(matches!(phone.node.set_pc_audio_output(pc_id, headphones.clone()).await, Err(Error::Denied)));
    assert!(pc.platform.pc_audio_outputs.lock().unwrap().is_empty());

    // Turning `pc_actions` back on re-enables `set_pc_audio`.
    pc.node.set_device_toggle(phone_id, "pc_actions", true).unwrap();

    // The phone switches the PC to the headphones; its list then shows them as the default.
    with_timeout("switch pc output", phone.node.set_pc_audio_output(pc_id, headphones.clone()))
        .await
        .unwrap();
    let switched = wait_for(&mut phone, "deck state with headphones default", |e| match e {
        NodeEvent::DeckState { device, state }
            if *device == pc_id && state.default_output_name() == Some("Headphones (USB DAC)") =>
        {
            Some(state.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(switched.output_devices.iter().filter(|d| d.is_default).count(), 1);
    assert_eq!(*pc.platform.pc_audio_outputs.lock().unwrap(), vec![headphones]);
    assert!(matches!(phone.node.set_pc_audio_output(pc_id, String::new()).await, Err(Error::Protocol(_))));
    with_timeout("set pc audio unmute + 65", phone.node.set_pc_audio(pc_id, Some(65), Some(false)))
        .await
        .unwrap();

    // Phone as speaker needs the phone's `media` permission to start; stopping is always allowed.
    pc.node.set_device_toggle(phone_id, "media", false).unwrap();
    assert!(matches!(
        phone.node.set_pc_audio(pc_id, Some(g7link_core::deck::PHONE_SPEAKER_ON), None).await,
        Err(Error::Denied)
    ));
    with_timeout(
        "phone speaker off while media is off",
        phone.node.set_pc_audio(pc_id, Some(g7link_core::deck::PHONE_SPEAKER_OFF), None),
    )
    .await
    .unwrap();
    pc.node.set_device_toggle(phone_id, "media", true).unwrap();
    with_timeout(
        "phone speaker on",
        phone.node.set_pc_audio(pc_id, Some(g7link_core::deck::PHONE_SPEAKER_ON), None),
    )
    .await
    .unwrap();

    // Smart clipboard context chips: phone sends different text clips to PC,
    // and PC classifies each into at most 1 context chip (and None for OTP).
    let cases = [
        ("https://g7.astraque.io/docs?ref=clip", Some((ClipKind::WebLink, "Open"))),
        (
            "1600 Amphitheatre Parkway, Mountain View, CA 94043",
            Some((ClipKind::StreetAddress, "Open in Maps")),
        ),
        ("+1 (415) 555-0199", Some((ClipKind::PhoneNumber, "Call"))),
        ("1Z999AA10123456784", Some((ClipKind::TrackingNumber, "Track"))),
        ("hello@g7link.app", Some((ClipKind::Email, "Email"))),
        ("Your verification code is 482910", None),
        ("Just a regular sentence with no special action.", None),
    ];

    for (text, expected) in cases {
        with_timeout("send clipboard", phone.node.send_clipboard(pc_id, text.into())).await.unwrap();
        wait_for(&mut pc, "clipboard received", |e| match e {
            NodeEvent::ClipboardReceived { device } if *device == phone_id => Some(()),
            _ => None,
        })
        .await;
        let suggestion = pc.node.last_clip_suggestion();
        match expected {
            Some((kind, label)) => {
                let (from, s) = suggestion.unwrap_or_else(|| panic!("expected chip for {text:?}"));
                assert_eq!(from, phone_id);
                assert_eq!(s.kind, kind, "wrong kind for {text:?}");
                assert_eq!(s.action_label(), label, "wrong label for {text:?}");
                assert!(!format!("{s:?}").contains(&s.target), "Debug must not leak target");
            }
            None => {
                assert!(suggestion.is_none(), "expected no chip for {text:?}, got {suggestion:?}");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn live_notifications_and_task_notify_flow_end_to_end() {
    use g7link_core::{LivePoint, LiveSegment, NotificationLive, TaskNotify};

    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    grant_notification_access(&phone).await;

    // Post a regular notification and an older live notification: snapshot puts live first.
    let mut regular = note("n-regular", "com.chat", "Sam", false);
    regular.when = 2_000_000_000_000;
    let mut live_note = note("n-live", "com.rides", "Driver arriving", false);
    live_note.when = 1_500_000_000_000;
    live_note.live = Some(NotificationLive {
        v: 1,
        progress: Some(40),
        max: Some(100),
        indeterminate: false,
        chip: Some("4 min".into()),
        segments: vec![
            LiveSegment { length: 50, color: Some(0xFF4C_AF50) },
            LiveSegment { length: 50, color: None },
        ],
        points: vec![LivePoint { position: 50, color: Some(0xFFFF_C107) }],
        chronometer: false,
        countdown: false,
    });
    phone.node.notifications_reset(vec![regular, live_note.clone()]).await;

    let items = wait_for(&mut pc, "snapshot with live first", |e| match e {
        NodeEvent::NotificationsReset { device, items } if *device == phone_id && items.len() == 2 => {
            Some(items.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(items[0].key, "n-live");
    assert_eq!(items[0].live.as_ref().and_then(|l| l.chip.as_deref()), Some("4 min"));
    assert_eq!(items[1].key, "n-regular");

    // Updating the live notification in place delivers the updated progress and chip.
    live_note.live.as_mut().unwrap().progress = Some(85);
    live_note.live.as_mut().unwrap().chip = Some("1 min".into());
    phone.node.notification_posted(live_note).await;
    let updated = wait_for(&mut pc, "live update posted", |e| match e {
        NodeEvent::NotificationPosted { device, notification }
            if *device == phone_id && notification.key == "n-live" =>
        {
            Some(notification.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(updated.live.as_ref().and_then(|l| l.progress), Some(85));
    assert_eq!(updated.live.as_ref().and_then(|l| l.chip.as_deref()), Some("1 min"));

    // PC sends an active task notification and then a completion task notification to the phone.
    let running = TaskNotify {
        id: "task-build-1".into(),
        title: "cargo build --release".into(),
        active: true,
        elapsed_ms: 1_500,
        exit_code: None,
    };
    with_timeout("task_notify running", pc.node.task_notify(Some(phone_id), running.clone()))
        .await
        .expect("task_notify running succeeds");

    let done = TaskNotify {
        id: "task-build-1".into(),
        title: "cargo build --release".into(),
        active: false,
        elapsed_ms: 42_000,
        exit_code: Some(0),
    };
    with_timeout("task_notify done", pc.node.task_notify(None, done.clone()))
        .await
        .expect("task_notify done succeeds");

    // A new session or a refresh re-sends the running task with its elapsed time moved on, so
    // the phone may see it more than once before the completion.
    let recorded = phone.platform.tasks.lock().unwrap().clone();
    let (last, updates) = recorded.split_last().expect("task updates recorded");
    assert_eq!(last, &done);
    assert_eq!(updates.first(), Some(&running));
    for update in updates {
        assert_eq!(update.id, running.id);
        assert!(update.active && update.exit_code.is_none());
        assert!(update.elapsed_ms >= running.elapsed_ms);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn material_you_accent_syncs_updates_live_and_persists_offline() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;

    // Set initial Material You seed on the phone before pairing.
    phone.node.update_accent(Some(0xFF8A_5100)).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    let paired = pc.node.paired_devices().unwrap();
    assert_eq!(paired.len(), 1);
    assert_eq!(paired[0].info.accent, Some(0xFF8A_5100));

    // Live wallpaper change on the phone sends HelloUpdate via update_accent.
    phone.node.update_accent(Some(0xFF00_FF00)).await;
    let info1 = wait_for(&mut pc, "accent updated to green", |e| match e {
        NodeEvent::PeerInfoChanged { device, info }
            if *device == phone_id && info.accent == Some(0xFF00_FF00) =>
        {
            Some(info.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(info1.accent, Some(0xFF00_FF00));
    assert_eq!(pc.node.paired_devices().unwrap()[0].info.accent, Some(0xFF00_FF00));

    // When the phone later updates DeviceInfo with accent = None (e.g. name change),
    // the PC preserves the phone's last stored seed in SQLite and in PeerInfoChanged.
    let mut renamed = info("Pixel Pro", DeviceKind::Phone);
    renamed.accent = None;
    phone.node.update_device_info(renamed).await;
    let info2 = wait_for(&mut pc, "renamed with preserved accent", |e| match e {
        NodeEvent::PeerInfoChanged { device, info } if *device == phone_id && info.name == "Pixel Pro" => {
            Some(info.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(info2.accent, Some(0xFF00_FF00));
    assert_eq!(pc.node.paired_devices().unwrap()[0].info.accent, Some(0xFF00_FF00));
}

#[tokio::test(flavor = "multi_thread")]
async fn timeline_records_files_clips_links_photos_recordings_and_sessions_without_duplicating_clips() {
    use g7link_core::{
        MIRROR_CAPTURE, MIRROR_VIEW, MirrorConfig, PHOTOS_READ, PHOTOS_SHOW, PacketKind, RECORDER,
        RecordingMarker, TimelineKind, TimelineQuery, WEBCAM_STREAM, WEBCAM_VIRTUAL, WebcamConfig,
    };

    let mut pc =
        device_with("Desktop", DeviceKind::Desktop, &[PHOTOS_SHOW, RECORDER, MIRROR_VIEW, WEBCAM_VIRTUAL])
            .await;
    let mut phone =
        device_with("Pixel", DeviceKind::Phone, &[PHOTOS_READ, MIRROR_CAPTURE, WEBCAM_STREAM]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // 1. Outgoing & incoming file transfer.
    let file_path = phone.dir.path().join("report.pdf");
    std::fs::write(&file_path, b"quarterly report").unwrap();
    let tid = phone
        .node
        .send_files(
            pc_id,
            vec![OutgoingFile {
                name: "report.pdf".into(),
                folder: None,
                source: FileSource::Path(file_path),
            }],
        )
        .await
        .unwrap();
    let _ = wait_transfer(&mut pc, &tid, "report.pdf", saved).await;

    // 2. Text clip (stored by ref_id only, never duplicated in SQLite) + OTP clip (excluded).
    let secret_clip = "super-unique-clipboard-payload-998877";
    phone.node.send_clipboard(pc_id, secret_clip.into()).await.unwrap();
    wait_for(&mut pc, "clip received", |e| match e {
        NodeEvent::ClipboardReceived { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    phone
        .node
        .send_clipboard(pc_id, "Your verification code is 482910. Do not share this OTP.".into())
        .await
        .unwrap();
    wait_for(&mut pc, "otp clip received", |e| match e {
        NodeEvent::ClipboardReceived { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    // 3. Web link and map handoff opened.
    phone.node.open_link(pc_id, "https://g7.astraque.io/timeline".into()).await.unwrap();
    phone.node.open_link(pc_id, "geo:37.7749,-122.4194?q=San+Francisco".into()).await.unwrap();

    // 4. Photo saved from phone.
    let photo_path = phone.dir.path().join("IMG_100.jpg");
    std::fs::write(&photo_path, b"\xff\xd8\xff\xd9").unwrap();
    phone.platform.photos.lock().unwrap().insert("media:100".into(), photo_path);
    let ptid = pc.node.fetch_photo(phone_id, "media:100".into()).await.unwrap();
    let _ = wait_transfer(&mut pc, &ptid, "IMG_100.jpg", saved).await;

    // 5. Voice recording with markers.
    let rec_path = phone.dir.path().join("Voice_01.m4a");
    std::fs::write(&rec_path, b"fake-m4a-audio").unwrap();
    let rtid = phone
        .node
        .send_recording(
            pc_id,
            OutgoingFile { name: "Voice_01.m4a".into(), folder: None, source: FileSource::Path(rec_path) },
            vec![RecordingMarker { at_ms: 1000, label: Some("Intro".into()) }],
        )
        .await
        .unwrap();
    let _ = wait_transfer(&mut pc, &rtid, "Voice_01.m4a", saved).await;

    // 6. Screen mirroring session & webcam session (start/end recorded as one line with duration).
    let mstream = phone.node.mirror_open(pc_id).await.unwrap();
    let mcfg = MirrorConfig { codec: "h264".into(), width: 1080, height: 2400, session: 0 }.to_cbor();
    let _ = tokio::task::block_in_place(|| mstream.send(PacketKind::Config, 0, mcfg));
    wait_for(&mut pc, "mirroring started", |e| match e {
        NodeEvent::Mirroring { device, on: true, .. } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    mstream.close();
    wait_for(&mut pc, "mirroring ended", |e| match e {
        NodeEvent::Mirroring { device, on: false, .. } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    let wstream = phone.node.webcam_open(pc_id).await.unwrap();
    let wcfg = WebcamConfig {
        codec: "h264".into(),
        width: 1280,
        height: 720,
        camera: "back".into(),
        fps: 30,
        max_height: 1080,
    }
    .to_cbor();
    let _ = tokio::task::block_in_place(|| wstream.send(PacketKind::Config, 0, wcfg));
    wait_for(&mut pc, "webcam started", |e| match e {
        NodeEvent::Webcam { device, on: true } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    wstream.close();
    wait_for(&mut pc, "webcam ended", |e| match e {
        NodeEvent::Webcam { device, on: false } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    // Query the PC's timeline.
    let page = pc.node.timeline_page(&TimelineQuery::default()).unwrap();
    let kinds: Vec<TimelineKind> = page.entries.iter().map(|i| i.kind).collect();
    assert!(kinds.contains(&TimelineKind::File), "expected File in {kinds:?}");
    assert!(kinds.contains(&TimelineKind::Clip), "expected Clip in {kinds:?}");
    assert!(kinds.contains(&TimelineKind::Link), "expected Link in {kinds:?}");
    let geo_item = page
        .entries
        .iter()
        .find(|i| i.kind == TimelineKind::Link && i.detail == "geo:37.7749,-122.4194?q=San+Francisco")
        .expect("geo link recorded with raw URI in detail");
    assert_eq!(geo_item.title, "San Francisco on a map");
    assert!(kinds.contains(&TimelineKind::Photo), "expected Photo in {kinds:?}");
    assert!(kinds.contains(&TimelineKind::Recording), "expected Recording in {kinds:?}");
    assert!(
        kinds.iter().filter(|&&k| k == TimelineKind::Session).count() >= 2,
        "expected Mirror and Webcam sessions in {kinds:?}"
    );
    for session_item in page.entries.iter().filter(|i| i.kind == TimelineKind::Session) {
        assert!(session_item.duration_secs >= 1, "session must record duration >= 1s");
    }

    // Verify OTP was never recorded in the timeline.
    let otp_search = pc
        .node
        .timeline_page(&TimelineQuery { search: Some("482910".into()), ..TimelineQuery::default() })
        .unwrap();
    assert!(otp_search.entries.is_empty(), "OTP must never appear in timeline");

    // Verify clip content is resolved in memory for search and display, but NEVER stored in SQLite!
    let clip_search = pc
        .node
        .timeline_page(&TimelineQuery {
            search: Some("super-unique-clipboard".into()),
            ..TimelineQuery::default()
        })
        .unwrap();
    assert_eq!(clip_search.entries.len(), 1);
    assert_eq!(clip_search.entries[0].kind, TimelineKind::Clip);
    assert_eq!(clip_search.entries[0].title, secret_clip);
    let clip_ref = clip_search.entries[0].ref_id.clone().expect("clip has ref_id");

    let sqlite_bytes = std::fs::read(pc.dir.path().join("g7link.db")).unwrap();
    let wal_bytes = std::fs::read(pc.dir.path().join("g7link.db-wal")).unwrap_or_default();
    assert!(
        !sqlite_bytes.windows(secret_clip.len()).any(|w| w == secret_clip.as_bytes())
            && !wal_bytes.windows(secret_clip.len()).any(|w| w == secret_clip.as_bytes()),
        "SQLite database must never duplicate clip content"
    );

    // Deleting the clip from clipboard history also removes its timeline entry.
    assert!(pc.node.delete_clipboard_history(&clip_ref));
    let after_del = pc
        .node
        .timeline_page(&TimelineQuery { kind: Some(TimelineKind::Clip), ..TimelineQuery::default() })
        .unwrap();
    assert!(after_del.entries.is_empty(), "deleting clip removes linked timeline entry");
}

#[tokio::test(flavor = "multi_thread")]
async fn messaging_style_notifications_populate_unified_chat_threads_and_reply() {
    use g7link_core::{NotificationChatMessage, NotificationConversation};

    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    grant_notification_access(&phone).await;

    let now_ms = 1_760_000_000_000i64;
    let mut wa_note = note("wa|maya", "com.whatsapp", "Maya Lin", true);
    wa_note.app_name = "WhatsApp".into();
    wa_note.when = now_ms - 60_000;
    wa_note.conversation = Some(NotificationConversation {
        v: 1,
        id: None,
        title: "Maya Lin".into(),
        group: false,
        avatar: Some(vec![0xff, 0xd8, 1, 2]),
        messages: vec![
            NotificationChatMessage {
                sender: Some("Maya Lin".into()),
                text: "Are you free for a quick sync?".into(),
                time: now_ms - 120_000,
                self_sent: false,
                avatar: None,
            },
            NotificationChatMessage {
                sender: Some("Maya Lin".into()),
                text: "Sent over the deck too".into(),
                time: now_ms - 60_000,
                self_sent: false,
                avatar: None,
            },
        ],
    });

    phone.node.notification_posted(wa_note).await;
    let changed_thread = wait_for(&mut pc, "chat thread updated from notification", |e| match e {
        NodeEvent::SmsChanged { device, thread: Some(tid) }
            if *device == phone_id && tid.starts_with("chat:") =>
        {
            Some(tid.clone())
        }
        _ => None,
    })
    .await;

    let threads = pc.node.chat_threads(phone_id);
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].thread_id, changed_thread);
    assert_eq!(threads[0].app, "com.whatsapp");
    assert_eq!(threads[0].app_name, "WhatsApp");
    assert_eq!(threads[0].title, "Maya Lin");
    assert!(!threads[0].is_group);
    assert_eq!(threads[0].unread, 2);
    assert!(threads[0].can_reply());
    assert!(threads[0].app_icon.is_some());
    assert!(threads[0].avatar.is_some());

    let msgs = pc.node.chat_messages(phone_id, &changed_thread, 20);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].text, "Sent over the deck too");
    assert_eq!(msgs[1].text, "Are you free for a quick sync?");

    // Reply directly from the unified conversation thread.
    with_timeout(
        "reply_chat_thread",
        pc.node.reply_chat_thread(phone_id, &changed_thread, "Give me 5 minutes!".into()),
    )
    .await
    .unwrap();
    assert_eq!(
        *phone.platform.actions.lock().unwrap(),
        vec![("wa|maya".into(), "0".into(), Some("Give me 5 minutes!".into()))]
    );

    let after_reply = pc.node.chat_thread(phone_id, &changed_thread).unwrap();
    assert_eq!(after_reply.unread, 0);
    assert_eq!(after_reply.snippet, "You: Give me 5 minutes!");
    let msgs = pc.node.chat_messages(phone_id, &changed_thread, 20);
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].text, "Give me 5 minutes!");
    assert!(msgs[0].self_sent);

    // A second chat, never replied to from the PC.
    let mut sam_note = note("wa|sam", "com.whatsapp", "Sam", false);
    sam_note.app_name = "WhatsApp".into();
    sam_note.conversation = Some(NotificationConversation {
        v: 1,
        id: None,
        title: "Sam".into(),
        group: false,
        avatar: None,
        messages: vec![NotificationChatMessage {
            sender: Some("Sam".into()),
            text: "Lunch?".into(),
            time: now_ms,
            self_sent: false,
            avatar: None,
        }],
    });
    phone.node.notification_posted(sam_note).await;
    wait_for(&mut pc, "the second chat", |e| match e {
        NodeEvent::SmsChanged { device, thread: Some(tid) }
            if *device == phone_id && *tid != changed_thread =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
    assert_eq!(pc.node.chat_threads(phone_id).len(), 2);

    // Their notifications leave the phone: the replied-to chat is kept, without
    // a reply box; the other leaves the list.
    for key in ["wa|maya", "wa|sam"] {
        phone.node.notification_removed(key.into()).await;
        wait_for(&mut pc, "notification removed", |e| match e {
            NodeEvent::NotificationRemoved { device, key: removed }
                if *device == phone_id && removed == key =>
            {
                Some(())
            }
            _ => None,
        })
        .await;
    }
    let listed: Vec<String> = pc.node.chat_threads(phone_id).into_iter().map(|t| t.thread_id).collect();
    assert_eq!(listed, [changed_thread.as_str()]);
    let after_removed = pc.node.chat_thread(phone_id, &changed_thread).unwrap();
    assert!(after_removed.kept && !after_removed.active);
    assert!(!after_removed.can_reply());
    assert!(matches!(
        pc.node.reply_chat_thread(phone_id, &changed_thread, "Still there?".into()).await,
        Err(Error::NotFound)
    ));
}

/// A WhatsApp chat notification, as the phone's app wrote it.
fn chat_note(key: &str, id: Option<&str>, title: &str, messages: &[(&str, &str, i64)]) -> Notification {
    let mut n = note(key, "com.whatsapp", title, false);
    n.app_name = "WhatsApp".into();
    n.when = messages.last().map_or(n.when, |m| m.2);
    n.conversation = Some(g7link_core::NotificationConversation {
        v: 1,
        id: id.map(Into::into),
        title: title.into(),
        group: true,
        avatar: None,
        messages: messages
            .iter()
            .map(|(sender, text, time)| g7link_core::NotificationChatMessage {
                sender: Some((*sender).into()),
                text: (*text).into(),
                time: *time,
                self_sent: false,
                avatar: None,
            })
            .collect(),
    });
    n
}

fn listed_chats(dev: &TestDevice, phone: g7link_core::DeviceId) -> Vec<String> {
    let mut ids: Vec<String> = dev.node.chat_threads(phone).into_iter().map(|t| t.thread_id).collect();
    ids.sort();
    ids
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_cleared_while_the_pc_was_away_is_gone_after_reconnect() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    grant_notification_access(&phone).await;
    let t0 = 1_760_000_000_000i64;
    phone
        .node
        .notifications_reset(vec![
            chat_note(
                "wa|club",
                Some("g1"),
                "Book Club (2 messages)",
                &[("Ana", "Hi", t0), ("Ben", "Yo", t0 + 1)],
            ),
            chat_note("wa|maya", None, "Maya", &[("Maya", "Lunch?", t0)]),
        ])
        .await;
    wait_for(&mut pc, "the snapshot", |e| match e {
        NodeEvent::NotificationsReset { device, items } if *device == phone_id && items.len() == 2 => {
            Some(())
        }
        _ => None,
    })
    .await;
    assert_eq!(listed_chats(&pc, phone_id), ["chat:com.whatsapp:i:g1", "chat:com.whatsapp:t:maya"]);

    apart(&pc, &phone).await;
    phone.node.notification_removed("wa|club".into()).await;
    together(&pc, &phone).await;
    wait_for(&mut pc, "the snapshot after reconnecting", |e| match e {
        NodeEvent::NotificationsReset { device, items } if *device == phone_id && items.len() == 1 => {
            Some(())
        }
        _ => None,
    })
    .await;
    assert_eq!(listed_chats(&pc, phone_id), ["chat:com.whatsapp:t:maya"]);
    let club = pc.node.chat_thread(phone_id, "chat:com.whatsapp:i:g1").expect("stored for the grace");
    assert!(!club.active && club.retired_ms.is_some() && club.unread == 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_whose_title_counts_up_stays_one_thread_end_to_end() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    grant_notification_access(&phone).await;
    let t0 = 1_760_000_000_000i64;
    let said = [("Ana", "Hi", t0), ("Ben", "Yo", t0 + 1), ("Cy", "Late", t0 + 2), ("Ana", "Go", t0 + 3)];
    for (n, id) in [(2, Some("g1")), (3, Some("g1")), (4, Some("g1")), (2, None), (3, None)] {
        let (key, title) = if id.is_some() { ("wa|club", "Book Club") } else { ("wa|hike", "Hiking") };
        phone
            .node
            .notification_posted(chat_note(key, id, &format!("{title} ({n} messages)"), &said[..n]))
            .await;
        wait_for(&mut pc, "posted", |e| match e {
            NodeEvent::NotificationPosted { device, notification }
                if *device == phone_id && notification.key == key =>
            {
                Some(())
            }
            _ => None,
        })
        .await;
    }
    let threads = pc.node.chat_threads(phone_id);
    let mut seen: Vec<(String, String, u32)> =
        threads.into_iter().map(|t| (t.thread_id, t.title, t.unread)).collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("chat:com.whatsapp:i:g1".to_owned(), "Book Club".to_owned(), 4),
            ("chat:com.whatsapp:t:hiking".to_owned(), "Hiking".to_owned(), 3),
        ]
    );
    assert_eq!(pc.node.chat_messages(phone_id, "chat:com.whatsapp:i:g1", 50).len(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_chat_on_the_pc_dismisses_it_on_the_phone() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    grant_notification_access(&phone).await;
    let t0 = 1_760_000_000_000i64;
    let (club, maya, sam) = ("chat:com.whatsapp:i:g1", "chat:com.whatsapp:t:maya", "chat:com.whatsapp:i:s1");
    phone.node.notification_posted(chat_note("wa|club", Some("g1"), "Book Club", &[("Ana", "Hi", t0)])).await;
    phone.node.notification_posted(chat_note("wa|maya", None, "Maya", &[("Maya", "Lunch?", t0)])).await;
    until("both chats on the PC", || pc.node.chat_threads(phone_id).len() == 2).await;

    // A live chat: gone here, and its notification dismissed on the phone.
    with_timeout("remove", pc.node.remove_chat_thread(phone_id, club)).await.expect("removed");
    assert!(pc.node.chat_thread(phone_id, club).is_none());
    assert!(pc.node.chat_messages(phone_id, club, 10).is_empty());
    assert_eq!(*phone.platform.dismissed.lock().unwrap(), ["wa|club"]);

    // A kept chat whose notification already left: removed here only.
    assert!(pc.node.set_chat_kept(phone_id, maya, true));
    phone.node.notification_removed("wa|maya".into()).await;
    wait_for(&mut pc, "removed", |e| match e {
        NodeEvent::NotificationRemoved { device, key } if *device == phone_id && key == "wa|maya" => Some(()),
        _ => None,
    })
    .await;
    assert_eq!(listed_chats(&pc, phone_id), [maya]);
    with_timeout("remove kept", pc.node.remove_chat_thread(phone_id, maya)).await.expect("removed");
    assert!(pc.node.chat_threads(phone_id).is_empty());
    with_timeout("remove again", pc.node.remove_chat_thread(phone_id, maya))
        .await
        .expect("already gone is fine");
    assert_eq!(*phone.platform.dismissed.lock().unwrap(), ["wa|club"], "nothing more sent");

    // Cancelled while the phone dismisses, and the PC closes: the delete stands.
    phone.node.notification_posted(chat_note("wa|sam", Some("s1"), "Sam", &[("Sam", "Hey", t0)])).await;
    until("the third chat on the PC", || pc.node.chat_thread(phone_id, sam).is_some()).await;
    let gate = Arc::new(Gate::default());
    *phone.platform.dismiss_gate.lock().unwrap() = Some(gate.clone());
    let remove = {
        let node = pc.node.clone();
        tokio::spawn(async move { node.remove_chat_thread(phone_id, sam).await })
    };
    until("the dismiss on the phone", || gate.held() == 1).await;
    remove.abort();
    let pc_dir = pc.dir;
    pc.node.shutdown().await;
    drop(pc.node);
    gate.open();
    let pc = start_in(pc_dir, "Desktop", DeviceKind::Desktop).await;
    assert!(pc.node.chat_thread(phone_id, sam).is_none(), "deleted before the dismiss was awaited");
    assert!(pc.node.chat_messages(phone_id, sam, 10).is_empty());
    assert_eq!(*phone.platform.dismissed.lock().unwrap(), ["wa|club", "wa|sam"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn mms_image_attachments_send_over_sms_service() {
    use g7link_core::{SMS_MAX_ATTACHMENT_BYTES, SmsAttachment};

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::SMS_SHOW]).await;
    let mut phone =
        device_with("Pixel", DeviceKind::Phone, &[g7link_core::SMS_READ, g7link_core::SMS_SEND]).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    let jpeg = SmsAttachment { mime: "image/jpeg".into(), data: vec![0xff, 0xd8, 1, 2, 3, 4] };
    let png = SmsAttachment { mime: "image/png".into(), data: vec![0x89, b'P', b'N', b'G', 5, 6] };

    // Send MMS with both caption and a JPEG attachment.
    with_timeout(
        "send mms with caption",
        pc.node.send_sms_with_attachments(
            phone_id,
            vec!["+15550100".into()],
            "Check this out".into(),
            vec![jpeg.clone()],
        ),
    )
    .await
    .unwrap();

    // Send image-only MMS (empty body, PNG attachment).
    with_timeout(
        "send image-only mms",
        pc.node.send_sms_with_attachments(
            phone_id,
            vec!["+15550188".into()],
            String::new(),
            vec![png.clone()],
        ),
    )
    .await
    .unwrap();

    let sent = phone.platform.mms_sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].0, vec!["+15550100".to_owned()]);
    assert_eq!(sent[0].1, "Check this out");
    assert_eq!(sent[0].2, vec![jpeg.clone()]);
    assert_eq!(sent[1].0, vec!["+15550188".to_owned()]);
    assert_eq!(sent[1].1, "");
    assert_eq!(sent[1].2, vec![png.clone()]);

    // Unsupported MIME type is rejected as a protocol error; oversized or too many attachments return TooLarge.
    let bad_mime = SmsAttachment { mime: "application/x-executable".into(), data: vec![1, 2, 3] };
    assert!(matches!(
        pc.node
            .send_sms_with_attachments(phone_id, vec!["+15550100".into()], String::new(), vec![bad_mime])
            .await,
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        pc.node
            .send_sms_with_attachments(phone_id, vec!["+15550100".into()], String::new(), vec![jpeg, png])
            .await,
        Err(Error::TooLarge)
    ));
    let oversized =
        SmsAttachment { mime: "image/jpeg".into(), data: vec![0u8; SMS_MAX_ATTACHMENT_BYTES + 1] };
    assert!(matches!(
        pc.node
            .send_sms_with_attachments(phone_id, vec!["+15550100".into()], String::new(), vec![oversized])
            .await,
        Err(Error::TooLarge)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuity_camera_photo_and_scan_flow() {
    use g7link_core::{CameraCaptureMode, CameraCaptureResultMeta};

    let mut pc = device_with("pc", DeviceKind::Desktop, &["camera.continuity"]).await;
    let mut phone = device_with("phone", DeviceKind::Phone, &["camera.capture"]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    assert_eq!(
        pc.node.capabilities(phone_id).unwrap().state("camera.continuity"),
        Some(FeatureState::Available)
    );

    // 1. Request a photo capture from PC.
    with_timeout(
        "request photo capture",
        pc.node.request_camera_capture(phone_id, "req-photo-1".into(), CameraCaptureMode::Photo),
    )
    .await
    .unwrap();

    let (req_peer, req_id, req_mode) = wait_for(&mut phone, "photo capture requested", |e| match e {
        NodeEvent::CameraCaptureRequested { device, request_id, mode } => {
            Some((*device, request_id.clone(), *mode))
        }
        _ => None,
    })
    .await;
    assert_eq!(req_peer, pc_id);
    assert_eq!(req_id, "req-photo-1");
    assert_eq!(req_mode, CameraCaptureMode::Photo);

    let fake_jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4, 0xFF, 0xD9];
    let photo_meta = CameraCaptureResultMeta {
        request_id: "req-photo-1".into(),
        mode: CameraCaptureMode::Photo,
        file_name: "IMG_20260409.jpg".into(),
        mime: "image/jpeg".into(),
        size: fake_jpeg.len() as u64,
        width: 1920,
        height: 1080,
    };
    with_timeout(
        "send photo capture result",
        phone.node.send_camera_capture_result(pc_id, photo_meta, fake_jpeg.clone()),
    )
    .await
    .unwrap();

    let (recv_peer, recv_id, recv_mode, recv_name, recv_mime, recv_w, recv_h, recv_data, saved_path) =
        wait_for(&mut pc, "photo capture received", |e| match e {
            NodeEvent::CameraCaptureReceived {
                device,
                request_id,
                mode,
                file_name,
                mime,
                width,
                height,
                data,
                saved_path,
            } => Some((
                *device,
                request_id.clone(),
                *mode,
                file_name.clone(),
                mime.clone(),
                *width,
                *height,
                data.clone(),
                saved_path.clone(),
            )),
            _ => None,
        })
        .await;
    assert_eq!(recv_peer, phone_id);
    assert_eq!(recv_id, "req-photo-1");
    assert_eq!(recv_mode, CameraCaptureMode::Photo);
    assert_eq!(recv_name, "IMG_20260409.jpg");
    assert_eq!(recv_mime, "image/jpeg");
    assert_eq!(recv_w, 1920);
    assert_eq!(recv_h, 1080);
    assert_eq!(recv_data, fake_jpeg);
    let path = saved_path.expect("saved to downloads_dir");
    assert_eq!(std::fs::read(&path).unwrap(), fake_jpeg);

    // 2. Request a document scan from PC.
    with_timeout(
        "request scan capture",
        pc.node.request_camera_capture(phone_id, "req-scan-2".into(), CameraCaptureMode::Scan),
    )
    .await
    .unwrap();

    let (_, scan_id, scan_mode) = wait_for(&mut phone, "scan capture requested", |e| match e {
        NodeEvent::CameraCaptureRequested { device, request_id, mode } => {
            Some((*device, request_id.clone(), *mode))
        }
        _ => None,
    })
    .await;
    assert_eq!(scan_id, "req-scan-2");
    assert_eq!(scan_mode, CameraCaptureMode::Scan);

    let fake_scan = vec![0xFF, 0xD8, 0xFF, 0xDB, 9, 8, 7, 6, 5, 0xFF, 0xD9];
    let scan_meta = CameraCaptureResultMeta {
        request_id: "req-scan-2".into(),
        mode: CameraCaptureMode::Scan,
        // A drive-prefixed name must still land in the downloads folder.
        file_name: "C:Scan_20260409.jpg".into(),
        mime: "image/jpeg".into(),
        size: fake_scan.len() as u64,
        width: 1654,
        height: 2339,
    };
    with_timeout(
        "send scan capture result",
        phone.node.send_camera_capture_result(pc_id, scan_meta, fake_scan.clone()),
    )
    .await
    .unwrap();

    let (_, got_scan_id, got_scan_mode, got_scan_name, _, got_w, got_h, got_scan_data, got_scan_path) =
        wait_for(&mut pc, "scan capture received", |e| match e {
            NodeEvent::CameraCaptureReceived {
                device,
                request_id,
                mode,
                file_name,
                mime,
                width,
                height,
                data,
                saved_path,
            } => Some((
                *device,
                request_id.clone(),
                *mode,
                file_name.clone(),
                mime.clone(),
                *width,
                *height,
                data.clone(),
                saved_path.clone(),
            )),
            _ => None,
        })
        .await;
    assert_eq!(got_scan_id, "req-scan-2");
    assert_eq!(got_scan_mode, CameraCaptureMode::Scan);
    assert_eq!(got_scan_name, "C_Scan_20260409.jpg");
    assert_eq!(got_w, 1654);
    assert_eq!(got_h, 2339);
    assert_eq!(got_scan_data, fake_scan);
    let got_scan_path = got_scan_path.unwrap();
    assert!(got_scan_path.exists());
    assert_eq!(got_scan_path.parent(), path.parent());
    assert_eq!(got_scan_path.file_name().unwrap(), "C_Scan_20260409.jpg");

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

/// Asks `phone` for a photo for `purpose`, has it send `bytes`, and returns
/// the PC's event for that request: its data and where it was saved.
async fn capture_for(
    pc: &mut TestDevice,
    phone: &mut TestDevice,
    request_id: &str,
    purpose: g7link_core::continuity_camera::CaptureFor,
    bytes: &[u8],
) -> (Vec<u8>, Option<std::path::PathBuf>) {
    use g7link_core::{CameraCaptureMode, CameraCaptureResultMeta};
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    with_timeout(
        "request a capture",
        pc.node.request_camera_capture_for(phone_id, request_id.into(), CameraCaptureMode::Photo, purpose),
    )
    .await
    .unwrap();
    wait_for(phone, "capture requested", |e| match e {
        NodeEvent::CameraCaptureRequested { device, request_id: id, .. }
            if *device == pc_id && id == request_id =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
    let meta = CameraCaptureResultMeta {
        request_id: request_id.into(),
        mode: CameraCaptureMode::Photo,
        file_name: format!("{request_id}.jpg"),
        mime: "image/jpeg".into(),
        size: bytes.len() as u64,
        width: 640,
        height: 480,
    };
    with_timeout("send the capture", phone.node.send_camera_capture_result(pc_id, meta, bytes.to_vec()))
        .await
        .unwrap();
    wait_for(pc, "capture received", |e| match e {
        NodeEvent::CameraCaptureReceived { device, request_id: id, data, saved_path, .. }
            if *device == phone_id && id == request_id =>
        {
            Some((data.clone(), saved_path.clone()))
        }
        _ => None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuity_camera_for_an_agent_keeps_no_copy_and_no_timeline_row() {
    use g7link_core::{TimelineKind, TimelineQuery, continuity_camera::CaptureFor};

    let mut pc = device_with("pc", DeviceKind::Desktop, &["camera.continuity"]).await;
    let mut phone = device_with("phone", DeviceKind::Phone, &["camera.capture"]).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    let downloads = pc.dir.path().join("received");
    let files = |dir: &std::path::Path| std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0);
    let photo_rows = |pc: &TestDevice| {
        let query =
            TimelineQuery { kind: Some(TimelineKind::Photo), device: Some(phone_id), ..Default::default() };
        pc.node.timeline_page(&query).unwrap().entries.iter().filter(|e| e.incoming).count()
    };

    // For an agent: the picture reaches the PC's event and nowhere else.
    let for_agent = vec![0xFF, 0xD8, 0xFF, 0xE0, 7, 7, 7, 0xFF, 0xD9];
    let (data, saved) = capture_for(&mut pc, &mut phone, "req-agent-1", CaptureFor::Agent, &for_agent).await;
    assert_eq!(data, for_agent);
    assert_eq!(saved, None, "not saved");
    assert_eq!(files(&downloads), 0, "nothing in the downloads folder");
    assert_eq!(photo_rows(&pc), 0, "no timeline row");

    // For the user, as always: saved, and in the timeline.
    let for_user = vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 0xFF, 0xD9];
    let (data, saved) = capture_for(&mut pc, &mut phone, "req-user-1", CaptureFor::User, &for_user).await;
    assert_eq!(data, for_user);
    let saved = saved.expect("saved to downloads_dir");
    assert_eq!(saved.parent(), Some(downloads.as_path()));
    assert_eq!(std::fs::read(&saved).unwrap(), for_user);
    assert_eq!(files(&downloads), 1);
    assert_eq!(photo_rows(&pc), 1);

    // An agent capture goes only to a phone that's connected: never queued.
    phone.node.set_reconnect(pc.node.device_id(), false).unwrap();
    wait_for(&mut pc, "phone offline", |e| match e {
        NodeEvent::LinkChanged { device, link }
            if *device == phone_id && !matches!(link, LinkState::Online { .. }) =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
    let later = pc
        .node
        .request_camera_capture_for(
            phone_id,
            "req-agent-2".into(),
            g7link_core::CameraCaptureMode::Photo,
            CaptureFor::Agent,
        )
        .await;
    assert!(matches!(later, Err(Error::Offline)), "{later:?}");
    assert!(pc.node.send_queue(Some(phone_id)).unwrap().is_empty(), "nothing queued for later");

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuity_camera_cancel_propagates() {
    use g7link_core::CameraCaptureMode;

    let mut pc = device_with("pc", DeviceKind::Desktop, &["camera.continuity"]).await;
    let mut phone = device_with("phone", DeviceKind::Phone, &["camera.capture"]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Phone cancels an active request -> PC receives CameraCaptureCancelled.
    with_timeout(
        "request capture 1",
        pc.node.request_camera_capture(phone_id, "req-cancel-1".into(), CameraCaptureMode::Photo),
    )
    .await
    .unwrap();
    wait_for(&mut phone, "capture 1 requested", |e| match e {
        NodeEvent::CameraCaptureRequested { request_id, .. } if request_id == "req-cancel-1" => Some(()),
        _ => None,
    })
    .await;

    with_timeout(
        "phone cancels capture 1",
        phone.node.cancel_camera_capture(pc_id, "req-cancel-1".into(), Some("user dismissed".into())),
    )
    .await
    .unwrap();

    let (from, id, reason) = wait_for(&mut pc, "pc sees cancel 1", |e| match e {
        NodeEvent::CameraCaptureCancelled { device, request_id, reason } => {
            Some((*device, request_id.clone(), reason.clone()))
        }
        _ => None,
    })
    .await;
    assert_eq!(from, phone_id);
    assert_eq!(id, "req-cancel-1");
    assert_eq!(reason.as_deref(), Some("user dismissed"));

    // PC cancels an active request -> Phone receives CameraCaptureCancelled.
    with_timeout(
        "request capture 2",
        pc.node.request_camera_capture(phone_id, "req-cancel-2".into(), CameraCaptureMode::Scan),
    )
    .await
    .unwrap();
    wait_for(&mut phone, "capture 2 requested", |e| match e {
        NodeEvent::CameraCaptureRequested { request_id, .. } if request_id == "req-cancel-2" => Some(()),
        _ => None,
    })
    .await;

    with_timeout(
        "pc cancels capture 2",
        pc.node.cancel_camera_capture(phone_id, "req-cancel-2".into(), Some("cancelled on PC".into())),
    )
    .await
    .unwrap();

    let (from2, id2, reason2) = wait_for(&mut phone, "phone sees cancel 2", |e| match e {
        NodeEvent::CameraCaptureCancelled { device, request_id, reason } => {
            Some((*device, request_id.clone(), reason.clone()))
        }
        _ => None,
    })
    .await;
    assert_eq!(from2, pc_id);
    assert_eq!(id2, "req-cancel-2");
    assert_eq!(reason2.as_deref(), Some("cancelled on PC"));

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuity_camera_respects_photos_toggle() {
    use g7link_core::CameraCaptureMode;

    let mut pc = device_with("pc", DeviceKind::Desktop, &["camera.continuity"]).await;
    let mut phone = device_with("phone", DeviceKind::Phone, &["camera.capture"]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (_pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Turning off photos toggle on PC blocks outgoing request_camera_capture with Error::Denied.
    pc.node.set_device_toggle(phone_id, "photos", false).unwrap();
    let res =
        pc.node.request_camera_capture(phone_id, "req-denied-pc".into(), CameraCaptureMode::Photo).await;
    assert!(matches!(res, Err(Error::Denied)));

    // Re-enable on PC, but disable photos toggle on phone -> phone replies with cancel.
    pc.node.set_device_toggle(phone_id, "photos", true).unwrap();
    phone.node.set_device_toggle(pc.node.device_id(), "photos", false).unwrap();
    with_timeout(
        "request with phone photos toggle off",
        pc.node.request_camera_capture(phone_id, "req-denied-phone".into(), CameraCaptureMode::Photo),
    )
    .await
    .unwrap();

    let (from, id, reason) = wait_for(&mut pc, "phone cancels because photos off", |e| match e {
        NodeEvent::CameraCaptureCancelled { device, request_id, reason } => {
            Some((*device, request_id.clone(), reason.clone()))
        }
        _ => None,
    })
    .await;
    assert_eq!(from, phone_id);
    assert_eq!(id, "req-denied-phone");
    assert!(reason.unwrap_or_default().contains("photos"));

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_the_pc_never_asked_for_isnt_saved() {
    use g7link_core::{CameraCaptureMode, CameraCaptureResultMeta};

    let mut pc = device_with("pc", DeviceKind::Desktop, &["camera.continuity"]).await;
    let mut phone = device_with("phone", DeviceKind::Phone, &["camera.capture"]).await;
    pair_qr(&mut pc, &mut phone).await;
    let pc_id = pc.node.device_id();

    let picture = vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4, 0xFF, 0xD9];
    let meta = CameraCaptureResultMeta {
        request_id: "never-asked".into(),
        mode: CameraCaptureMode::Photo,
        file_name: "unasked.jpg".into(),
        mime: "image/jpeg".into(),
        size: picture.len() as u64,
        width: 1,
        height: 1,
    };
    let sent =
        with_timeout("send an unasked capture", phone.node.send_camera_capture_result(pc_id, meta, picture))
            .await;
    assert!(sent.is_err(), "the PC turns it away");
    assert!(!pc.dir.path().join("received").join("unasked.jpg").exists());

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn data_retention_counts_and_clear_all_local_data_preserves_pairing_and_settings() {
    use g7link_core::{Notification, NotificationAction, NotificationChatMessage, NotificationConversation};

    let mut pc = device_with("pc", DeviceKind::Desktop, &["notify.show", "clip.write"]).await;
    let mut phone = device_with("phone", DeviceKind::Phone, &["notify.mirror", "notify.reply"]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Configure non-default retention and a device toggle on PC.
    pc.node
        .set_timeline_retention(g7link_core::timeline::TimelineRetention { max_days: 30, max_entries: 5000 })
        .unwrap();
    pc.node.set_device_toggle(phone_id, "clipboard", true).unwrap();

    // Send a clipboard item from phone -> PC.
    with_timeout("send clip", phone.node.send_clipboard(pc_id, "Secret recipe notes".into())).await.unwrap();
    wait_for(&mut pc, "clip history changed", |e| match e {
        NodeEvent::ClipboardHistoryChanged => Some(()),
        _ => None,
    })
    .await;

    // Post a MessagingStyle chat notification from phone -> PC.
    let note = Notification {
        key: "sig|sam".into(),
        app: "org.thoughtcrime.securesms".into(),
        app_name: "Signal".into(),
        title: Some("Sam Rivera".into()),
        text: Some("Dinner at 7?".into()),
        sub: None,
        when: 1_760_000_000_000,
        actions: vec![NotificationAction { id: "reply".into(), title: "Reply".into(), reply: true }],
        silent: false,
        ongoing: false,
        icon: None,
        image: None,
        live: None,
        conversation: Some(NotificationConversation {
            v: 1,
            id: None,
            title: "Sam Rivera".into(),
            group: false,
            avatar: None,
            messages: vec![NotificationChatMessage {
                sender: Some("Sam Rivera".into()),
                text: "Dinner at 7?".into(),
                time: 1_760_000_000_000,
                self_sent: false,
                avatar: None,
            }],
        }),
    };
    phone.node.notification_posted(note).await;
    wait_for(&mut pc, "chat sms changed", |e| match e {
        NodeEvent::SmsChanged { device, .. } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    let counts = pc.node.data_retention_counts();
    assert!(counts.clipboard_items >= 1);
    assert!(counts.timeline_items >= 1);
    assert_eq!(counts.timeline_retention_days, 30);
    assert_eq!(counts.chat_threads, 1);
    assert_eq!(counts.chat_messages, 1);

    // Clear everything on PC: all history and caches become 0, while pairing and settings remain intact.
    pc.node.clear_all_local_data().unwrap();
    let after = pc.node.data_retention_counts();
    assert_eq!(after.clipboard_items, 0);
    assert_eq!(after.timeline_items, 0);
    assert_eq!(after.chat_threads, 0);
    assert_eq!(after.chat_messages, 0);
    assert_eq!(after.received_file_records, 0);
    assert_eq!(after.timeline_retention_days, 30, "timeline retention setting must be preserved");
    assert_eq!(pc.node.paired_devices().unwrap().len(), 1, "paired devices must never be deleted");
    assert!(
        pc.node.device_toggles(phone_id).unwrap().into_iter().any(|(k, v)| k == "clipboard" && v),
        "per-device toggles must be preserved"
    );

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

fn free_tcp_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

async fn start_localsend_in(dir: TempDir, name: &'static str, kind: DeviceKind, port: u16) -> TestDevice {
    init_logging();
    let mut config = NodeConfig::new(dir.path(), info(name, kind), "0.0.1-test");
    config.lan_discovery = false;
    config.away_mode = false;
    config.localsend_port = port;
    config.key_protector = Some(Arc::new(PlainKeyProtector));
    config.capabilities = vec!["media.control".into()];
    let platform = Arc::new(RecordingPlatform::default());
    let node = Node::start(config, platform.clone()).await.expect("node starts");
    let events = node.events();
    TestDevice { node, events, platform, dir, name }
}

#[tokio::test]
async fn localsend_is_off_by_default_and_persists_setting() {
    let dir = tempfile::tempdir().unwrap();
    let port1 = free_tcp_port();
    let mut dev = start_localsend_in(dir, "PC", DeviceKind::Desktop, port1).await;

    assert!(!dev.node.localsend_enabled());
    assert!(!dev.node.localsend_receiving());
    assert!(dev.node.localsend_peers().is_empty());

    dev.node.set_localsend_enabled(true).unwrap();
    wait_for(&mut dev, "localsend enabled event", |e| match e {
        NodeEvent::LocalSendChanged => Some(()),
        _ => None,
    })
    .await;
    assert!(dev.node.localsend_enabled());
    // Wait up to 2s for the background TCP listener to bind and set receiving=true.
    for _ in 0..40 {
        if dev.node.localsend_receiving() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(dev.node.localsend_receiving());

    let dir = dev.dir;
    dev.node.shutdown().await;

    let port2 = free_tcp_port();
    let dev2 = start_localsend_in(dir, "PC", DeviceKind::Desktop, port2).await;
    assert!(dev2.node.localsend_enabled(), "enabled state must persist across restarts");
    for _ in 0..40 {
        if dev2.node.localsend_receiving() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(dev2.node.localsend_receiving(), "server must start automatically when persisted enabled");
    dev2.node.shutdown().await;
}

/// Apps turn LocalSend on from their UI thread, outside the node's runtime.
#[tokio::test(flavor = "multi_thread")]
async fn localsend_turns_on_from_a_thread_outside_the_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let mut dev = start_localsend_in(dir, "PC", DeviceKind::Desktop, free_tcp_port()).await;
    let node = dev.node.clone();
    std::thread::spawn(move || {
        node.set_localsend_enabled(true).unwrap();
        node.refresh_localsend();
    })
    .join()
    .expect("turning LocalSend on outside the runtime doesn't panic");
    for _ in 0..40 {
        if dev.node.localsend_receiving() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(dev.node.localsend_receiving());
    dev.node.set_localsend_enabled(false).unwrap();
    wait_for(&mut dev, "localsend changed event", |e| match e {
        NodeEvent::LocalSendChanged => Some(()),
        _ => None,
    })
    .await;
    dev.node.shutdown().await;
}

#[tokio::test]
async fn localsend_coexists_when_port_is_already_bound() {
    let occupied = std::net::TcpListener::bind(("0.0.0.0", 0)).expect("bind occupied port");
    let port = occupied.local_addr().unwrap().port();

    let dir = tempfile::tempdir().unwrap();
    let dev = start_localsend_in(dir, "PC", DeviceKind::Desktop, port).await;
    dev.node.set_localsend_enabled(true).unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert!(dev.node.localsend_enabled());
    assert!(
        !dev.node.localsend_receiving(),
        "when port is occupied by standalone LocalSend, G7 Link must leave receiving to it without failing"
    );

    dev.node.shutdown().await;
    drop(occupied);
}

/// A paired phone also answers LocalSend (G7 Link does). It isn't listed
/// as a LocalSend device nearby too, until it's unpaired.
#[tokio::test(flavor = "multi_thread")]
async fn a_paired_device_is_left_out_of_localsend_nearby() {
    let pc_port = free_tcp_port();
    let phone_port = free_tcp_port();
    let mut pc = start_localsend_in(tempfile::tempdir().unwrap(), "PC", DeviceKind::Desktop, pc_port).await;
    let mut phone =
        start_localsend_in(tempfile::tempdir().unwrap(), "Pixel", DeviceKind::Phone, phone_port).await;
    pc.node.set_localsend_enabled(true).unwrap();
    phone.node.set_localsend_enabled(true).unwrap();
    for _ in 0..40 {
        if phone.node.localsend_receiving() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    let found = pc.node.probe_localsend_peer("127.0.0.1", phone_port).await.expect("probe the phone");
    assert_eq!(found.alias, "Pixel");
    assert!(
        !pc.node.localsend_peers().iter().any(|p| p.id == found.id),
        "the paired phone is reached as itself, not listed again as LocalSend"
    );

    pc.node.unpair(phone_id).await.unwrap();
    wait_for(&mut pc, "device removed", |e| matches!(e, NodeEvent::DeviceRemoved(_)).then_some(())).await;
    assert!(
        pc.node.localsend_peers().iter().any(|p| p.id == found.id),
        "once unpaired, it's a LocalSend device like any other"
    );

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test]
async fn localsend_discovers_peer_requires_accept_and_transfers_files_over_https() {
    let sender_port = free_tcp_port();
    let receiver_port = free_tcp_port();
    let mut sender =
        start_localsend_in(tempfile::tempdir().unwrap(), "Phone Sender", DeviceKind::Phone, sender_port)
            .await;
    let mut receiver =
        start_localsend_in(tempfile::tempdir().unwrap(), "PC Receiver", DeviceKind::Desktop, receiver_port)
            .await;

    sender.node.set_localsend_enabled(true).unwrap();
    receiver.node.set_localsend_enabled(true).unwrap();
    // Generous: on a busy runner the server can take a while to start.
    for _ in 0..200 {
        if receiver.node.localsend_receiving() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(receiver.node.localsend_receiving());

    let peer = sender
        .node
        .probe_localsend_peer("127.0.0.1", receiver_port)
        .await
        .expect("probe receiver over HTTPS");
    assert_eq!(peer.alias, "PC Receiver");
    assert_eq!(peer.port, receiver_port);
    assert_eq!(peer.protocol, "https");
    assert_eq!(peer.device_type, "desktop");
    assert!(sender.node.localsend_peers().iter().any(|p| p.id == peer.id));

    let src_dir = sender.dir.path().join("src_files");
    std::fs::create_dir_all(src_dir.join("sub")).unwrap();
    let file_a = src_dir.join("notes.txt");
    let file_b = src_dir.join("sub").join("hello.txt");
    std::fs::write(&file_a, b"LocalSend interop over HTTPS!").unwrap();
    std::fs::write(&file_b, b"Nested folder payload").unwrap();

    let out_id = sender
        .node
        .send_files(
            peer.id,
            vec![
                OutgoingFile { name: "notes.txt".into(), folder: None, source: FileSource::Path(file_a) },
                OutgoingFile {
                    name: "hello.txt".into(),
                    folder: Some("sub".into()),
                    source: FileSource::Path(file_b),
                },
            ],
        )
        .await
        .expect("start LocalSend upload");

    let req: Transfer = wait_for(&mut receiver, "incoming LocalSend transfer request", |e| match e {
        NodeEvent::Transfer(t)
            if t.direction == Direction::Incoming && t.state == TransferState::Requested =>
        {
            Some(t.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(req.files, 2);
    assert!(
        !receiver.dir.path().join("received").join("notes.txt").exists(),
        "unpaired LocalSend transfer must wait for user acceptance before writing files"
    );

    receiver.node.accept_transfer(&req.id);

    wait_for(&mut sender, "sender LocalSend transfer done", |e| match e {
        NodeEvent::Transfer(t) if t.id == out_id && matches!(t.state, TransferState::Done { .. }) => Some(()),
        _ => None,
    })
    .await;
    wait_for(&mut receiver, "receiver LocalSend transfer done", |e| match e {
        NodeEvent::Transfer(t) if t.id == req.id && matches!(t.state, TransferState::Done { .. }) => Some(()),
        _ => None,
    })
    .await;

    let dl_a = receiver.dir.path().join("received").join("notes.txt");
    let dl_b = receiver.dir.path().join("received").join("sub").join("hello.txt");
    assert_eq!(std::fs::read(&dl_a).unwrap(), b"LocalSend interop over HTTPS!");
    assert_eq!(std::fs::read(&dl_b).unwrap(), b"Nested folder payload");

    assert!(
        !sender
            .node
            .timeline_page(&g7link_core::timeline::TimelineQuery::default())
            .unwrap()
            .entries
            .is_empty()
    );
    assert!(
        !receiver
            .node
            .timeline_page(&g7link_core::timeline::TimelineQuery::default())
            .unwrap()
            .entries
            .is_empty()
    );

    sender.node.shutdown().await;
    receiver.node.shutdown().await;
}

/// One HTTP/1.1 request over plain TCP to a LocalSend server, as a LocalSend
/// sender would send it; returns the status and the body.
async fn localsend_request(port: u16, method: &str, target: &str, body: &[u8]) -> (u16, Vec<u8>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let head = format!(
        "{method} {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    tcp.write_all(head.as_bytes()).await.unwrap();
    tcp.write_all(body).await.unwrap();
    let mut out = Vec::new();
    let _ = tcp.read_to_end(&mut out).await;
    let text = String::from_utf8_lossy(&out);
    let status = text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").map_or(out.len(), |i| i + 4);
    (status, out[split..].to_vec())
}

fn localsend_offer(files: &[(&str, &str, &str, u64)]) -> Vec<u8> {
    let files: serde_json::Map<String, serde_json::Value> = files
        .iter()
        .map(|(key, id, name, size)| {
            (
                (*key).to_owned(),
                serde_json::json!({ "id": id, "fileName": name, "size": size, "fileType": "text/plain" }),
            )
        })
        .collect();
    serde_json::to_vec(&serde_json::json!({
        "info": { "alias": "Raw sender", "version": "2.1", "deviceType": "mobile", "fingerprint": "raw-sender-fp", "port": 1, "protocol": "http" },
        "files": files,
    }))
    .unwrap()
}

fn part_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(part_files(&path));
        } else if path.extension().is_some_and(|e| e == "part") {
            found.push(path);
        }
    }
    found
}

/// A sender that offers duplicate file ids is refused; one that sends one of
/// two files and cancels leaves that file, a timeline entry saying it only
/// partly arrived, no staged parts, and no session (#72 audit).
#[tokio::test]
async fn localsend_refuses_duplicate_ids_and_records_a_cancelled_partial_delivery() {
    let port = free_tcp_port();
    let mut receiver =
        start_localsend_in(tempfile::tempdir().unwrap(), "PC Receiver", DeviceKind::Desktop, port).await;
    receiver.node.set_localsend_enabled(true).unwrap();
    for _ in 0..40 {
        if receiver.node.localsend_receiving() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(receiver.node.localsend_receiving());

    let (status, _) = localsend_request(
        port,
        "POST",
        "/api/localsend/v2/prepare-upload",
        &localsend_offer(&[("a", "same", "a.txt", 5), ("b", "same", "b.txt", 5)]),
    )
    .await;
    assert_eq!(status, 400, "two files with one id could never both arrive");

    let offer = localsend_offer(&[("f1", "f1", "first.txt", 5), ("f2", "f2", "second.txt", 6)]);
    let prepare = tokio::spawn(async move {
        localsend_request(port, "POST", "/api/localsend/v2/prepare-upload", &offer).await
    });
    let req: Transfer = wait_for(&mut receiver, "offer", |e| match e {
        NodeEvent::Transfer(t) if t.state == TransferState::Requested => Some(t.clone()),
        _ => None,
    })
    .await;
    receiver.node.accept_transfer(&req.id);
    let (status, body) = with_timeout("prepare-upload", prepare).await.unwrap();
    assert_eq!(status, 200);
    let answer: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let session = answer["sessionId"].as_str().unwrap().to_owned();
    let token = answer["files"]["f1"].as_str().unwrap().to_owned();

    let (status, _) = localsend_request(
        port,
        "POST",
        &format!("/api/localsend/v2/upload?sessionId={session}&fileId=f1&token={token}"),
        b"hello",
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = localsend_request(
        port,
        "POST",
        &format!("/api/localsend/v2/upload?sessionId={session}&fileId=f1&token={token}"),
        b"again",
    )
    .await;
    assert_eq!(status, 403, "a file that arrived isn't uploaded twice");

    let (status, _) =
        localsend_request(port, "POST", &format!("/api/localsend/v2/cancel?sessionId={session}"), b"").await;
    assert_eq!(status, 200);
    wait_for(&mut receiver, "cancelled", |e| match e {
        NodeEvent::Transfer(t) if t.id == req.id && t.state == TransferState::Cancelled => Some(()),
        _ => None,
    })
    .await;

    let saved = receiver.dir.path().join("received").join("first.txt");
    assert_eq!(std::fs::read(&saved).unwrap(), b"hello", "what arrived is kept");
    let entries =
        receiver.node.timeline_page(&g7link_core::timeline::TimelineQuery::default()).unwrap().entries;
    assert!(
        entries.iter().any(|e| e.detail.contains("1 of 2 files") && e.target.contains("first.txt")),
        "{:?}",
        entries.iter().map(|e| &e.detail).collect::<Vec<_>>()
    );
    assert!(part_files(receiver.dir.path()).is_empty(), "no staged parts are left");
    let (status, _) = localsend_request(
        port,
        "POST",
        &format!("/api/localsend/v2/upload?sessionId={session}&fileId=f2&token=x"),
        b"second",
    )
    .await;
    assert_eq!(status, 404, "the session is gone");

    receiver.node.shutdown().await;
}

#[tokio::test]
async fn localsend_declining_transfer_returns_denied_to_sender() {
    let sender_port = free_tcp_port();
    let receiver_port = free_tcp_port();
    let mut sender =
        start_localsend_in(tempfile::tempdir().unwrap(), "Phone Sender", DeviceKind::Phone, sender_port)
            .await;
    let mut receiver =
        start_localsend_in(tempfile::tempdir().unwrap(), "PC Receiver", DeviceKind::Desktop, receiver_port)
            .await;

    sender.node.set_localsend_enabled(true).unwrap();
    receiver.node.set_localsend_enabled(true).unwrap();
    for _ in 0..40 {
        if receiver.node.localsend_receiving() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let peer = sender.node.probe_localsend_peer("127.0.0.1", receiver_port).await.unwrap();

    let file_a = sender.dir.path().join("reject_me.txt");
    std::fs::write(&file_a, b"do not accept").unwrap();

    let out_id = sender
        .node
        .send_files(
            peer.id,
            vec![OutgoingFile {
                name: "reject_me.txt".into(),
                folder: None,
                source: FileSource::Path(file_a),
            }],
        )
        .await
        .unwrap();

    let req: Transfer = wait_for(&mut receiver, "incoming LocalSend transfer request", |e| match e {
        NodeEvent::Transfer(t)
            if t.direction == Direction::Incoming && t.state == TransferState::Requested =>
        {
            Some(t.clone())
        }
        _ => None,
    })
    .await;

    receiver.node.cancel_transfer(&req.id);

    wait_for(&mut sender, "sender sees Denied failure", |e| match e {
        NodeEvent::Transfer(t)
            if t.id == out_id && t.state == TransferState::Failed(TransferFailure::Denied) =>
        {
            Some(())
        }
        _ => None,
    })
    .await;

    assert!(!receiver.dir.path().join("received").join("reject_me.txt").exists());

    sender.node.shutdown().await;
    receiver.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_blake3_integrity_verified_and_mismatch_keeps_partial() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let src = tempfile::tempdir().unwrap();

    // 1. A transfer whose bytes are altered on the sender after the offer hash is computed
    //    must fail with TransferFailure::Integrity on both sides and keep the .part file.
    let payload = data(48 * 1024 * 1024, 42);
    let src_path = src.path().join("corrupt_me.bin");
    std::fs::write(&src_path, &payload).unwrap();

    let id = pc
        .node
        .send_files(
            phone_id,
            vec![OutgoingFile {
                name: "corrupt_me.bin".into(),
                folder: None,
                source: FileSource::Path(src_path.clone()),
            }],
        )
        .await
        .unwrap();

    // Paused as soon as it runs: the offer and its hash are sent by then, and
    // waiting for the first progress report lets an optimized build finish
    // all 48 MB before it comes.
    wait_transfer(&mut pc, &id, "sending before pause", |t| {
        (t.state == TransferState::Running).then_some(())
    })
    .await;
    pc.node.pause_transfer(&id);
    wait_transfer(&mut pc, &id, "sender paused", |t| (t.state == TransferState::Paused).then_some(())).await;
    wait_transfer(&mut phone, &id, "receiver paused", |t| (t.state == TransferState::Paused).then_some(()))
        .await;

    // Flip the last byte of the source file while paused so the remaining stream bytes
    // no longer match the BLAKE3 digest in the offer.
    let mut corrupted = payload.clone();
    let last = corrupted.len() - 1;
    corrupted[last] ^= 0xFF;
    std::fs::write(&src_path, &corrupted).unwrap();

    pc.node.resume_transfer(&id);

    let phone_state = wait_transfer(&mut phone, &id, "receiver integrity failure", |t| {
        t.state.is_finished().then(|| t.state.clone())
    })
    .await;
    let pc_state = wait_transfer(&mut pc, &id, "sender integrity failure", |t| {
        t.state.is_finished().then(|| t.state.clone())
    })
    .await;
    assert_eq!(phone_state, TransferState::Failed(TransferFailure::Integrity));
    assert_eq!(pc_state, TransferState::Failed(TransferFailure::Integrity));

    // The partial file is kept in incoming/<pc_id>/<id>/0.part and NOT moved to received/.
    let part_path = phone.dir.path().join("incoming").join(pc_id.to_string()).join(&id).join("0.part");
    assert!(part_path.exists(), "partial .part file must be kept on integrity failure");
    assert!(!phone.dir.path().join("received").join("corrupt_me.bin").exists());

    // 2. Restore the valid bytes and send again: normal BLAKE3 verification succeeds.
    std::fs::write(&src_path, &payload).unwrap();
    let id2 = pc
        .node
        .send_files(
            phone_id,
            vec![OutgoingFile {
                name: "corrupt_me.bin".into(),
                folder: None,
                source: FileSource::Path(src_path),
            }],
        )
        .await
        .unwrap();
    let received = wait_transfer(&mut phone, &id2, "valid transfer received", saved).await;
    assert_eq!(std::fs::read(&received[0]).unwrap(), payload);
    wait_transfer(&mut pc, &id2, "valid transfer sent", saved).await;

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_manual_pause_and_resume_both_sides() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let src = tempfile::tempdir().unwrap();

    let big = data(64 * 1024 * 1024, 77);
    let id = pc.node.send_files(phone_id, vec![outgoing(src.path(), "movie.bin", &big)]).await.unwrap();

    // 1. Pause from the sender (PC) as soon as it runs: an optimized build can
    //    send all 64 MB before the first progress report.
    wait_transfer(&mut phone, &id, "receiving initial", |t| {
        (t.state == TransferState::Running).then_some(())
    })
    .await;
    pc.node.pause_transfer(&id);

    wait_transfer(&mut pc, &id, "pc paused", |t| (t.state == TransferState::Paused).then_some(())).await;
    wait_transfer(&mut phone, &id, "phone paused", |t| (t.state == TransferState::Paused).then_some(()))
        .await;

    let part_path = phone.dir.path().join("incoming").join(pc_id.to_string()).join(&id).join("0.part");
    assert!(part_path.exists(), "partial file kept while paused");

    // 2. Resume from the sender (PC), then pause from the receiver (phone), then resume from the receiver.
    pc.node.resume_transfer(&id);
    wait_transfer(&mut phone, &id, "phone running after sender resume", |t| {
        (t.state == TransferState::Running).then_some(())
    })
    .await;

    phone.node.pause_transfer(&id);
    wait_transfer(&mut phone, &id, "phone paused by receiver", |t| {
        (t.state == TransferState::Paused).then_some(())
    })
    .await;
    wait_transfer(&mut pc, &id, "pc paused by receiver", |t| {
        (t.state == TransferState::Paused).then_some(())
    })
    .await;

    phone.node.resume_transfer(&id);
    let received = wait_transfer(&mut phone, &id, "received after receiver resume", saved).await;
    assert_eq!(std::fs::read(&received[0]).unwrap(), big);
    wait_transfer(&mut pc, &id, "sent after receiver resume", saved).await;

    // 3. Pause a transfer and cancel it while paused from the receiver: partial files are cleaned up
    //    and both sides transition to Cancelled.
    let id2 =
        pc.node.send_files(phone_id, vec![outgoing(src.path(), "cancel_paused.bin", &big)]).await.unwrap();
    wait_transfer(&mut phone, &id2, "receiving second", |t| {
        (t.state == TransferState::Running).then_some(())
    })
    .await;
    phone.node.pause_transfer(&id2);
    wait_transfer(&mut phone, &id2, "phone paused second", |t| {
        (t.state == TransferState::Paused).then_some(())
    })
    .await;
    wait_transfer(&mut pc, &id2, "pc paused second", |t| (t.state == TransferState::Paused).then_some(()))
        .await;

    phone.node.cancel_transfer(&id2);
    let phone_end = wait_transfer(&mut phone, &id2, "phone cancelled from pause", |t| {
        t.state.is_finished().then(|| t.state.clone())
    })
    .await;
    let pc_end = wait_transfer(&mut pc, &id2, "pc cancelled from pause", |t| {
        t.state.is_finished().then(|| t.state.clone())
    })
    .await;
    assert_eq!((phone_end, pc_end), (TransferState::Cancelled, TransferState::Cancelled));
    let part_dir2 = phone.dir.path().join("incoming").join(pc_id.to_string()).join(&id2);
    assert!(!part_dir2.exists(), "cancelled paused transfer removes incoming directory");

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

/// The receiver's resume travels on the control stream and its pause as the
/// transfer stream's reset, so the resume can reach the sender first. The
/// sender then mustn't stay paused once the reset comes in.
#[tokio::test(flavor = "multi_thread")]
async fn a_resume_that_overtakes_its_pause_still_resumes() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    let src = tempfile::tempdir().unwrap();

    let big = data(64 * 1024 * 1024, 91);
    let id = pc.node.send_files(phone_id, vec![outgoing(src.path(), "overtaken.bin", &big)]).await.unwrap();
    wait_transfer(&mut phone, &id, "receiving", |t| (t.state == TransferState::Running).then_some(())).await;

    // In arrival order at the PC: the resume, then the pause.
    phone.node.resume_transfer(&id);
    tokio::time::sleep(Duration::from_millis(50)).await;
    phone.node.pause_transfer(&id);

    let received = wait_transfer(&mut phone, &id, "received despite the late pause", saved).await;
    assert_eq!(std::fs::read(&received[0]).unwrap(), big);
    wait_transfer(&mut pc, &id, "sent despite the late pause", saved).await;

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn battery_signal_strength_and_peer_diagnostics_flow() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (_pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // 1. Signal strength fields in Battery flow intact and clamp/sanitize on the wire.
    let signal_battery = Battery {
        level: 64,
        charging: false,
        plugged: None,
        full_in: None,
        cell_bars: Some(4),
        cell_type: Some("5G".into()),
        wifi_bars: Some(3),
    };
    phone.node.update_battery(signal_battery.clone()).await;
    let received = wait_for(&mut pc, "battery with signal", |e| match e {
        NodeEvent::Battery { device, battery } if *device == phone_id => Some(battery.clone()),
        _ => None,
    })
    .await;
    assert_eq!(received, signal_battery);

    // 2. PeerDiagnostics starts empty when mDNS is disabled in test harness, and records
    //    LAN sightings and dial failures accurately.
    let initial = pc.node.peer_diagnostics(&phone_id);
    assert_eq!(initial.mdns_seen_ago_secs, None);
    assert!(initial.mdns_addrs.is_empty());
    assert_eq!(initial.dial_failures, 0);

    let lan_addr: SocketAddr = "192.168.1.55:4242".parse().unwrap();
    pc.node.record_lan_sighting(phone_id, vec![lan_addr]);
    pc.node.record_dial_failure(phone_id);
    pc.node.record_dial_failure(phone_id);

    let diag = pc.node.peer_diagnostics(&phone_id);
    assert!(diag.mdns_seen_ago_secs.is_some_and(|s| s < 5));
    assert_eq!(diag.mdns_addrs, vec![lan_addr]);
    assert_eq!(diag.dial_failures, 2);

    // 3. Unpairing clears LAN sighting and dial failure diagnostics for the peer.
    with_timeout("unpair", pc.node.unpair(phone_id)).await.unwrap();
    let cleared = pc.node.peer_diagnostics(&phone_id);
    assert_eq!(cleared.mdns_seen_ago_secs, None);
    assert!(cleared.mdns_addrs.is_empty());
    assert_eq!(cleared.dial_failures, 0);

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lan_sightings_are_capped_and_paired_devices_keep_theirs() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    let addr = |i: u16| SocketAddr::from(([192, 168, 1, 2], 40_000 + i));
    let stranger = |i: u16| {
        let mut id = [0u8; 32];
        id[..2].copy_from_slice(&i.to_le_bytes());
        g7link_core::DeviceId(id)
    };
    pc.node.record_lan_sighting(phone_id, vec![addr(0)]);
    for i in 0..300 {
        pc.node.record_lan_sighting(stranger(i), (0..20).map(addr).collect());
    }

    // Anyone on the LAN can announce identities: the oldest strangers made room, the paired
    // phone kept its sighting, and each keeps a bounded number of addresses.
    assert_eq!(pc.node.peer_diagnostics(&stranger(0)).mdns_seen_ago_secs, None);
    assert_eq!(pc.node.peer_diagnostics(&stranger(299)).mdns_addrs.len(), 16);
    assert_eq!(pc.node.peer_diagnostics(&phone_id).mdns_addrs, vec![addr(0)]);

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

async fn start_usb_device(
    name: &'static str,
    kind: DeviceKind,
    usb_port: Option<u16>,
    offers: &[&str],
) -> TestDevice {
    start_usb_device_in(tempfile::tempdir().unwrap(), name, kind, usb_port, offers).await
}

async fn start_usb_device_in(
    dir: TempDir,
    name: &'static str,
    kind: DeviceKind,
    usb_port: Option<u16>,
    offers: &[&str],
) -> TestDevice {
    init_logging();
    let mut config = NodeConfig::new(dir.path(), info(name, kind), "0.0.1-test");
    config.lan_discovery = false;
    config.away_mode = false;
    config.usb_port = usb_port;
    config.adb_server = None;
    config.key_protector = Some(Arc::new(PlainKeyProtector));
    let mut caps = vec!["media.control".to_owned()];
    caps.extend(offers.iter().map(|c| (*c).to_owned()));
    config.capabilities = caps;
    let platform = Arc::new(RecordingPlatform::default());
    let node = Node::start(config, platform.clone()).await.expect("usb test node starts");
    let events = node.events();
    TestDevice { node, events, platform, dir, name }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_profiles_on_one_phone_each_get_their_own_usb_port() {
    // Android profiles (personal, work, a cloned app) share one loopback: both apps ask
    // for the same port, and the second takes the next one rather than a random port the
    // PC never tries. Each port then reaches its own profile.
    // Other tests take ports all the time, so a range that looked free can be gone
    // by the time the devices start: then try another one.
    let (base, personal, work) = 'range: loop {
        let base = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let Some(last) = base.checked_add(g7link_core::USB_PORT_COUNT) else { continue };
        if !(base..=last).all(|p| std::net::TcpListener::bind(("127.0.0.1", p)).is_ok()) {
            continue;
        }
        let personal = start_usb_device("Pixel", DeviceKind::Phone, Some(base), &[]).await;
        let work = start_usb_device("Pixel (Work)", DeviceKind::Phone, Some(base), &[]).await;
        if personal.node.usb_port().is_some() && work.node.usb_port().is_some() {
            break 'range (base, personal, work);
        }
        personal.node.shutdown().await;
        work.node.shutdown().await;
    };
    let pc = start_usb_device("Desktop", DeviceKind::Desktop, None, &[]).await;

    // Other tests run alongside and take ports too, so which ones are free can vary; what
    // matters is that both stay in the range the PC tries, and apart.
    let range = base..base + g7link_core::USB_PORT_COUNT;
    let personal_port = personal.node.usb_port().expect("personal profile listens");
    let work_port = work.node.usb_port().expect("work profile listens");
    assert!(
        range.contains(&personal_port) && range.contains(&work_port),
        "{personal_port}, {work_port} in {range:?}"
    );
    assert_ne!(personal_port, work_port);

    let at = |port: u16| -> SocketAddr { ("127.0.0.1".parse::<std::net::IpAddr>().unwrap(), port).into() };
    let reached =
        pc.node.connect_usb_tunnel(at(personal_port)).await.expect("tunnel to the personal profile");
    assert_eq!(reached, personal.node.device_id());
    let reached = pc.node.connect_usb_tunnel(at(work_port)).await.expect("tunnel to the work profile");
    assert_eq!(reached, work.node.device_id());

    pc.node.shutdown().await;
    work.node.shutdown().await;
    personal.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usb_auto_pair_requires_consent_on_both_sides_and_connects_over_usb() {
    use g7link_core::ConnectionPath;

    let mut phone = start_usb_device("Pixel USB", DeviceKind::Phone, Some(0), &[]).await;
    let mut pc = start_usb_device("Desktop USB", DeviceKind::Desktop, None, &[]).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let usb_port = phone.node.usb_port().expect("phone bound ephemeral USB TCP listener");
    let tunnel_addr: SocketAddr = ("127.0.0.1".parse::<std::net::IpAddr>().unwrap(), usb_port).into();

    // The phone shows "Pair with PC".
    phone.node.set_discovery_fast(true);
    pc.node.connect_usb_tunnel(tunnel_addr).await.expect("connect USB tunnel");

    // 1. Both PC and Phone emit NodeEvent::Discovered for each other.
    let mut pc_saw_discovered = false;
    let mut pc_code: Option<String> = None;
    while !pc_saw_discovered || pc_code.is_none() {
        let ev = wait_for(&mut pc, "pc discovered + sas code", |e| Some(e.clone())).await;
        match ev {
            NodeEvent::Discovered(d) if d.id == phone_id => pc_saw_discovered = true,
            NodeEvent::Pairing(PairingEvent::SasCode { peer, code }) if peer == phone_id => {
                pc_code = Some(code);
            }
            _ => {}
        }
    }

    let mut phone_saw_discovered = false;
    let mut phone_code: Option<String> = None;
    while !phone_saw_discovered || phone_code.is_none() {
        let ev = wait_for(&mut phone, "phone discovered + sas code", |e| Some(e.clone())).await;
        match ev {
            NodeEvent::Discovered(d) if d.id == pc_id => phone_saw_discovered = true,
            NodeEvent::Pairing(PairingEvent::SasCode { peer, code }) if peer == pc_id => {
                phone_code = Some(code);
            }
            _ => {}
        }
    }

    // 2. Both emitted matching 6-digit SAS codes without calling pairing_start_qr or pairing_start_nearby.
    let code_a = pc_code.unwrap();
    let code_b = phone_code.unwrap();
    assert_eq!(code_a.len(), 6);
    assert_eq!(code_a, code_b);

    // 3. Before user confirmation on both sides, neither device is paired.
    assert!(pc.node.paired_devices().unwrap().is_empty());
    assert!(phone.node.paired_devices().unwrap().is_empty());

    // 4. Declining on one side aborts pairing and keeps paired_devices() empty on both sides.
    phone.node.pairing_confirm(false).unwrap();
    let phone_fail = wait_for(&mut phone, "phone pairing declined", |e| match e {
        NodeEvent::Pairing(PairingEvent::Failed(r)) => Some(r.clone()),
        _ => None,
    })
    .await;
    let pc_fail = wait_for(&mut pc, "pc pairing rejected", |e| match e {
        NodeEvent::Pairing(PairingEvent::Failed(r)) => Some(r.clone()),
        _ => None,
    })
    .await;
    assert_eq!(phone_fail, PairingFailure::Declined);
    assert_eq!(pc_fail, PairingFailure::Rejected);
    assert!(pc.node.paired_devices().unwrap().is_empty());
    assert!(phone.node.paired_devices().unwrap().is_empty());

    // 5. Re-triggering over USB (via refresh) starts a fresh SAS prompt; confirming on both sides pairs
    //    and connects with ConnectionPath::Usb.
    pc.node.refresh().await;
    let code_pc_2 = wait_for(&mut pc, "pc second sas code", |e| match e {
        NodeEvent::Pairing(PairingEvent::SasCode { peer, code }) if *peer == phone_id => Some(code.clone()),
        _ => None,
    })
    .await;
    let code_phone_2 = wait_for(&mut phone, "phone second sas code", |e| match e {
        NodeEvent::Pairing(PairingEvent::SasCode { peer, code }) if *peer == pc_id => Some(code.clone()),
        _ => None,
    })
    .await;
    assert_eq!(code_pc_2, code_phone_2);

    pc.node.pairing_confirm(true).unwrap();
    phone.node.pairing_confirm(true).unwrap();

    wait_for(&mut pc, "pc paired over usb", |e| match e {
        NodeEvent::Pairing(PairingEvent::Paired(d)) if d.id == phone_id => Some(()),
        _ => None,
    })
    .await;
    wait_for(&mut phone, "phone paired over usb", |e| match e {
        NodeEvent::Pairing(PairingEvent::Paired(d)) if d.id == pc_id => Some(()),
        _ => None,
    })
    .await;

    wait_for(&mut pc, "pc online over usb", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Online { path: ConnectionPath::Usb, .. } }
            if *device == phone_id =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
    wait_for(&mut phone, "phone online over usb", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Online { path: ConnectionPath::Usb, .. } }
            if *device == pc_id =>
        {
            Some(())
        }
        _ => None,
    })
    .await;

    // 6. Plugged in, the link stays up: the phone lets the PC dial through the cable instead
    //    of racing it, so neither side sees its connection replaced and dropped.
    let settle = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
    let mut drops = 0;
    while tokio::time::Instant::now() < settle {
        tokio::select! {
            ev = pc.events.recv() => if let Ok(NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } }) = ev
                && device == phone_id { drops += 1 },
            ev = phone.events.recv() => if let Ok(NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } }) = ev
                && device == pc_id { drops += 1 },
            () = tokio::time::sleep_until(settle) => {}
        }
    }
    assert_eq!(drops, 0, "the USB link must not drop while plugged in");
    let pc_paired = pc.node.paired_devices().unwrap();
    assert!(
        matches!(pc_paired[0].link, LinkState::Online { path: ConnectionPath::Usb, .. }),
        "still connected over USB after settling"
    );

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

/// Any app on a phone can reach the phone's end of a USB tunnel, so the phone
/// takes a pairing over it only while its own pairing screen is open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usb_pairing_waits_for_the_phones_pairing_screen() {
    let mut phone = start_usb_device("Pixel USB", DeviceKind::Phone, Some(0), &[]).await;
    let mut pc = start_usb_device("Desktop USB", DeviceKind::Desktop, None, &[]).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let usb_port = phone.node.usb_port().expect("phone USB TCP listener");
    let tunnel_addr: SocketAddr = ("127.0.0.1".parse::<std::net::IpAddr>().unwrap(), usb_port).into();

    // Plugged in with the phone's pairing screen closed: the phone refuses the PC's
    // attempt, and neither side shows a code, nor a failure nobody asked about.
    pc.node.connect_usb_tunnel(tunnel_addr).await.expect("connect USB tunnel");
    let quiet_until = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < quiet_until {
        tokio::select! {
            ev = pc.events.recv() => if let Ok(NodeEvent::Pairing(p)) = ev { panic!("pc: {p:?}") },
            ev = phone.events.recv() => if let Ok(NodeEvent::Pairing(p)) = ev { panic!("phone: {p:?}") },
            () = tokio::time::sleep_until(quiet_until) => {}
        }
    }
    // Asking from the PC doesn't get past it either.
    assert!(
        with_timeout("pair over USB", pc.node.pairing_start_nearby(phone_id)).await.is_err(),
        "the phone isn't in pairing mode"
    );
    assert!(pc.node.paired_devices().unwrap().is_empty());
    assert!(phone.node.paired_devices().unwrap().is_empty());

    // The phone's "Pair with PC" opens: it starts the code check with the PC itself.
    phone.node.set_discovery_fast(true);
    let code_phone = wait_for(&mut phone, "phone sas code", |e| match e {
        NodeEvent::Pairing(PairingEvent::SasCode { peer, code }) if *peer == pc_id => Some(code.clone()),
        _ => None,
    })
    .await;
    let code_pc = wait_for(&mut pc, "pc sas code", |e| match e {
        NodeEvent::Pairing(PairingEvent::SasCode { peer, code }) if *peer == phone_id => Some(code.clone()),
        _ => None,
    })
    .await;
    assert_eq!(code_pc, code_phone);
    phone.node.pairing_confirm(true).unwrap();
    pc.node.pairing_confirm(true).unwrap();
    wait_for(&mut pc, "pc paired over usb", |e| match e {
        NodeEvent::Pairing(PairingEvent::Paired(d)) if d.id == phone_id => Some(()),
        _ => None,
    })
    .await;
    wait_for(&mut phone, "phone paired over usb", |e| match e {
        NodeEvent::Pairing(PairingEvent::Paired(d)) if d.id == pc_id => Some(()),
        _ => None,
    })
    .await;

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usb_transport_carries_features_when_wifi_unavailable() {
    use g7link_core::{ConnectionPath, MirrorSend, PacketKind};

    let mut phone = start_usb_device(
        "Pixel USB",
        DeviceKind::Phone,
        Some(0),
        &["clip.write", g7link_core::MIRROR_CAPTURE],
    )
    .await;
    let mut pc =
        start_usb_device("Desktop USB", DeviceKind::Desktop, None, &["clip.write", g7link_core::MIRROR_VIEW])
            .await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let usb_port = phone.node.usb_port().expect("phone USB TCP listener");
    let tunnel_addr: SocketAddr = ("127.0.0.1".parse::<std::net::IpAddr>().unwrap(), usb_port).into();
    phone.node.set_discovery_fast(true);
    pc.node.connect_usb_tunnel(tunnel_addr).await.expect("connect USB tunnel");

    let code_pc = wait_for(&mut pc, "pc sas code", |e| match e {
        NodeEvent::Pairing(PairingEvent::SasCode { peer, code }) if *peer == phone_id => Some(code.clone()),
        _ => None,
    })
    .await;
    let code_phone = wait_for(&mut phone, "phone sas code", |e| match e {
        NodeEvent::Pairing(PairingEvent::SasCode { peer, code }) if *peer == pc_id => Some(code.clone()),
        _ => None,
    })
    .await;
    assert_eq!(code_pc, code_phone);

    pc.node.pairing_confirm(true).unwrap();
    phone.node.pairing_confirm(true).unwrap();

    wait_for(&mut pc, "pc online over usb", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Online { path: ConnectionPath::Usb, .. } }
            if *device == phone_id =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
    wait_for(&mut phone, "phone online over usb", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Online { path: ConnectionPath::Usb, .. } }
            if *device == pc_id =>
        {
            Some(())
        }
        _ => None,
    })
    .await;

    // 4. paired_devices() reports LinkState::Online { path: ConnectionPath::Usb, .. }.
    let pc_paired = pc.node.paired_devices().unwrap();
    assert_eq!(pc_paired.len(), 1);
    assert!(matches!(pc_paired[0].link, LinkState::Online { path: ConnectionPath::Usb, .. }));

    // 1. Clipboard sync over USB tunnel.
    with_timeout("send clipboard over USB", pc.node.send_clipboard(phone_id, "Clipboard over USB!".into()))
        .await
        .unwrap();
    wait_for(&mut phone, "phone clipboard history changed", |e| match e {
        NodeEvent::ClipboardHistoryChanged => Some(()),
        _ => None,
    })
    .await;
    assert_eq!(*phone.platform.clipboard.lock().unwrap(), ["Clipboard over USB!"]);

    // 2. File transfer over USB tunnel.
    let src = tempfile::tempdir().unwrap();
    let payload = b"USB cable file payload 1234567890".repeat(512);
    let transfer_id = with_timeout(
        "send file over USB",
        pc.node.send_files(phone_id, vec![outgoing(src.path(), "usb_file.bin", &payload)]),
    )
    .await
    .unwrap();
    let received_paths = wait_transfer(&mut phone, &transfer_id, "phone received usb_file.bin", saved).await;
    assert_eq!(std::fs::read(&received_paths[0]).unwrap(), payload);
    wait_transfer(&mut pc, &transfer_id, "pc sent usb_file.bin", saved).await;

    // 3. Screen mirroring over USB tunnel.
    let options = g7link_core::MirrorStart {
        max_size: 1280,
        fps: 60,
        bitrate: 6_000_000,
        audio: false,
        session: 0,
        app: None,
        stay_awake: false,
        screen_off: false,
        watch: None,
        sound_only: false,
    };
    with_timeout("start mirror over USB", pc.node.mirror_start(phone_id, options)).await.unwrap();
    let stream = Arc::new(with_timeout("open mirror over USB", phone.node.mirror_open(pc_id)).await.unwrap());
    let mirror_cfg =
        g7link_core::MirrorConfig { codec: "h264".into(), width: 1080, height: 2400, session: 0 };
    let sender = stream.clone();
    let sent = tokio::task::spawn_blocking(move || {
        [
            sender.send(PacketKind::Config, 0, mirror_cfg.to_cbor()),
            sender.send(PacketKind::Keyframe, 42, vec![7u8; 16_384]),
        ]
    })
    .await
    .unwrap();
    assert!(sent.iter().all(|s| *s == MirrorSend::Queued), "{sent:?}");

    wait_for(&mut pc, "pc mirroring active over USB", |e| {
        matches!(e, NodeEvent::Mirroring { on: true, .. }).then_some(())
    })
    .await;

    let screen = pc.platform.screen.clone();
    let deadline = tokio::time::Instant::now() + WAIT;
    while screen.got.lock().unwrap().len() < 2 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(*screen.got.lock().unwrap(), ["config 1080x2400", "key 42 16384"]);

    pc.node.mirror_stop(phone_id, 0).await;
    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reverse_mirror_requires_pc_consent_and_remote_input_toggle() {
    use g7link_core::{
        MIRROR_CAPTURE, MIRROR_INPUT, MIRROR_SCREEN, MirrorConfig, MirrorInput, MirrorSend, MirrorStart,
        PC_MIRROR_TOGGLE, PacketKind, TouchAction,
    };
    use std::sync::atomic::Ordering;

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[MIRROR_CAPTURE, MIRROR_INPUT]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::MIRROR_VIEW]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let options = MirrorStart {
        max_size: 1280,
        fps: 30,
        bitrate: 4_000_000,
        audio: false,
        session: MIRROR_SCREEN,
        app: None,
        stay_awake: false,
        screen_off: false,
        watch: None,
        sound_only: false,
    };

    // (a) A paired phone calling `mirror_start` against a PC is denied by default
    // (`pc_mirror` is false by default), and emits `NodeEvent::PcMirrorRequested` on the PC.
    let denied_default = phone.node.mirror_start(pc_id, options.clone()).await;
    assert!(
        matches!(denied_default, Err(Error::Denied)),
        "expected Denied by default, got {denied_default:?}"
    );
    let prompt_opts = wait_for(&mut pc, "pc mirror consent prompt event", |e| match e {
        NodeEvent::PcMirrorRequested { device, options } if *device == phone_id => Some(options.clone()),
        _ => None,
    })
    .await;
    assert_eq!(prompt_opts.max_size, 1280);
    assert!(pc.platform.mirror_asks.lock().unwrap().is_empty());

    // Opening a PC mirror stream from the PC while `pc_mirror` is off is also denied.
    assert!(matches!(pc.node.mirror_open(phone_id).await, Err(Error::Denied)));

    // (b) Even after enabling `pc_mirror` on the PC, if the platform consent hook
    // (`Platform::mirror_requested`) rejects, `mirror_start` is denied.
    pc.node.set_device_toggle(phone_id, PC_MIRROR_TOGGLE, true).unwrap();
    pc.platform.mirror_reject.store(true, Ordering::SeqCst);
    let declined = phone.node.mirror_start(pc_id, options.clone()).await;
    assert!(
        matches!(declined, Err(Error::Denied)),
        "expected Denied when consent hook rejects, got {declined:?}"
    );
    assert!(pc.platform.mirror_asks.lock().unwrap().is_empty());

    // Once both `pc_mirror` is enabled on the PC AND the platform consent hook approves,
    // `mirror_start` succeeds and the PC streams frames to the phone.
    pc.platform.mirror_reject.store(false, Ordering::SeqCst);
    with_timeout("mirror_start after toggle + consent", phone.node.mirror_start(pc_id, options.clone()))
        .await
        .unwrap();
    assert_eq!(*pc.platform.mirror_asks.lock().unwrap(), ["start 1280"]);

    let stream = Arc::new(with_timeout("pc mirror_open", pc.node.mirror_open(phone_id)).await.unwrap());
    let cfg = MirrorConfig { codec: "h264".into(), width: 1280, height: 720, session: MIRROR_SCREEN };
    let sender = stream.clone();
    let sent = tokio::task::spawn_blocking(move || {
        [
            sender.send(PacketKind::Config, 0, cfg.to_cbor()),
            sender.send(PacketKind::Keyframe, 100, vec![9u8; 4096]),
        ]
    })
    .await
    .unwrap();
    assert!(sent.iter().all(|s| *s == MirrorSend::Queued), "{sent:?}");
    wait_for(&mut phone, "phone mirroring on", |e| {
        matches!(e, NodeEvent::Mirroring { on: true, .. }).then_some(())
    })
    .await;

    // (c) `remote_input` is the PC's to decide: the phone has no such switch of its
    // own, so with it off on the PC the input is refused there and the PC asks.
    let touch = MirrorInput::Touch { action: TouchAction::Down, x: 0.5, y: 0.5 };
    phone.node.mirror_input(pc_id, MIRROR_SCREEN, touch.clone()).await.unwrap();
    wait_for(&mut pc, "pc remote input prompt event", |e| match e {
        NodeEvent::RemoteInputRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert_eq!(
        *pc.platform.mirror_asks.lock().unwrap(),
        ["start 1280"],
        "PC must refuse to inject mirror_input while remote_input is off on PC"
    );

    // Once the PC also enables `remote_input`, `mirror_input` succeeds.
    pc.node.set_device_toggle(phone_id, "remote_input", true).unwrap();
    with_timeout("mirror_input with remote_input on", phone.node.mirror_input(pc_id, MIRROR_SCREEN, touch))
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + WAIT;
    while pc.platform.mirror_asks.lock().unwrap().len() < 2 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(pc.platform.mirror_asks.lock().unwrap().len(), 2);

    phone.node.mirror_stop(pc_id, MIRROR_SCREEN).await;
    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_offline_fifo_clipboard_replacement_and_drain_on_reconnect() {
    use g7link_core::{QueueKind, QueueStatus};

    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &["clip.write", "link.open"]).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    let phone_dir = phone.dir;
    phone.node.shutdown().await;
    wait_for(&mut pc, "phone offline", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    let src = tempfile::tempdir().unwrap();
    let file_id = pc
        .node
        .send_files(phone_id, vec![outgoing(src.path(), "alpha.txt", b"queued-alpha-payload")])
        .await
        .unwrap();
    pc.node.send_clipboard(phone_id, "clip-v1".into()).await.unwrap();
    pc.node.open_link(phone_id, "https://example.com/queued".into()).await.unwrap();
    pc.node.send_clipboard(phone_id, "clip-v2".into()).await.unwrap();

    let queued = pc.node.send_queue(Some(phone_id)).unwrap();
    assert_eq!(queued.len(), 3, "second clipboard send must replace the waiting first clipboard item");
    assert_eq!(queued[0].id, file_id);
    assert_eq!(queued[0].kind, QueueKind::Files);
    assert_eq!(queued[0].status, QueueStatus::Waiting);
    assert_eq!(queued[1].kind, QueueKind::Link);
    assert_eq!(queued[1].status, QueueStatus::Waiting);
    assert_eq!(queued[2].kind, QueueKind::Clipboard);
    assert_eq!(queued[2].title, "clip-v2");
    assert_eq!(queued[2].status, QueueStatus::Waiting);
    assert!(queued[0].seq < queued[1].seq && queued[1].seq < queued[2].seq);

    let mut phone = start_in(phone_dir, "Pixel", DeviceKind::Phone).await;
    phone
        .node
        .update_power(
            PowerLevel::NotApplicable,
            vec!["media.control".into(), "clip.write".into(), "link.open".into()],
        )
        .await;

    let received = wait_transfer(&mut phone, &file_id, "queued file delivered on reconnect", saved).await;
    assert_eq!(std::fs::read(&received[0]).unwrap(), b"queued-alpha-payload");

    let deadline = tokio::time::Instant::now() + WAIT;
    while (phone.platform.clipboard.lock().unwrap().is_empty()
        || phone.platform.links.lock().unwrap().is_empty()
        || !pc.node.send_queue(Some(phone_id)).unwrap().is_empty())
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    assert_eq!(*phone.platform.clipboard.lock().unwrap(), ["clip-v2"]);
    assert_eq!(*phone.platform.links.lock().unwrap(), ["https://example.com/queued"]);
    assert!(pc.node.send_queue(Some(phone_id)).unwrap().is_empty());

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_survives_node_restart_and_delivers_on_reconnect() {
    use g7link_core::QueueStatus;

    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &["clip.write"]).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    let phone_dir = phone.dir;
    phone.node.shutdown().await;
    wait_for(&mut pc, "phone offline", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    let src = tempfile::tempdir().unwrap();
    let file_id = pc
        .node
        .send_files(phone_id, vec![outgoing(src.path(), "persisted.txt", b"survives-node-restart")])
        .await
        .unwrap();
    pc.node.send_clipboard(phone_id, "persisted-clip".into()).await.unwrap();
    assert_eq!(pc.node.send_queue(Some(phone_id)).unwrap().len(), 2);

    let pc_dir = pc.dir;
    pc.node.shutdown().await;

    let pc = start_in(pc_dir, "Desktop", DeviceKind::Desktop).await;
    let recovered = pc.node.send_queue(Some(phone_id)).unwrap();
    assert_eq!(recovered.len(), 2);
    assert!(recovered.iter().all(|i| i.status == QueueStatus::Waiting));

    let mut phone = start_in(phone_dir, "Pixel", DeviceKind::Phone).await;
    phone
        .node
        .update_power(PowerLevel::NotApplicable, vec!["media.control".into(), "clip.write".into()])
        .await;
    pc.node.add_known_addrs(phone_id, &phone.node.direct_addrs().await);
    phone.node.add_known_addrs(pc.node.device_id(), &pc.node.direct_addrs().await);

    let received = wait_transfer(&mut phone, &file_id, "persisted file delivered", saved).await;
    assert_eq!(std::fs::read(&received[0]).unwrap(), b"survives-node-restart");

    let deadline = tokio::time::Instant::now() + WAIT;
    while (phone.platform.clipboard.lock().unwrap().is_empty()
        || !pc.node.send_queue(Some(phone_id)).unwrap().is_empty())
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(*phone.platform.clipboard.lock().unwrap(), ["persisted-clip"]);
    assert!(pc.node.send_queue(None).unwrap().is_empty());

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_source_file_deleted_or_modified_fails_cleanly_and_retry_works() {
    use g7link_core::QueueStatus;

    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    let phone_dir = phone.dir;
    phone.node.shutdown().await;
    wait_for(&mut pc, "phone offline", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    let src = tempfile::tempdir().unwrap();
    let missing_path = src.path().join("missing.txt");
    let modified_path = src.path().join("modified.txt");
    let valid_path = src.path().join("valid.txt");
    std::fs::write(&missing_path, b"will-be-deleted").unwrap();
    std::fs::write(&modified_path, b"original-bytes").unwrap();
    std::fs::write(&valid_path, b"good-bytes").unwrap();

    let missing_id = pc
        .node
        .send_files(
            phone_id,
            vec![OutgoingFile {
                name: "missing.txt".into(),
                folder: None,
                source: FileSource::Path(missing_path.clone()),
            }],
        )
        .await
        .unwrap();
    let modified_id = pc
        .node
        .send_files(
            phone_id,
            vec![OutgoingFile {
                name: "modified.txt".into(),
                folder: None,
                source: FileSource::Path(modified_path.clone()),
            }],
        )
        .await
        .unwrap();
    let valid_id = pc
        .node
        .send_files(
            phone_id,
            vec![OutgoingFile {
                name: "valid.txt".into(),
                folder: None,
                source: FileSource::Path(valid_path),
            }],
        )
        .await
        .unwrap();

    // Mutate/remove the first two source files while queued.
    std::fs::remove_file(&missing_path).unwrap();
    std::fs::write(&modified_path, b"modified-after-queuing-different-length").unwrap();

    let mut phone = start_in(phone_dir, "Pixel", DeviceKind::Phone).await;
    let received =
        wait_transfer(&mut phone, &valid_id, "valid file still delivered past failed items", saved).await;
    assert_eq!(std::fs::read(&received[0]).unwrap(), b"good-bytes");

    let deadline = tokio::time::Instant::now() + WAIT;
    while pc.node.send_queue(Some(phone_id)).unwrap().len() != 2 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let queue = pc.node.send_queue(Some(phone_id)).unwrap();
    assert_eq!(queue.len(), 2);
    let missing_item = queue.iter().find(|i| i.id == missing_id).unwrap();
    assert!(matches!(
        &missing_item.status,
        QueueStatus::Failed { error } if error.contains("File was deleted before sending: missing.txt")
    ));
    let modified_item = queue.iter().find(|i| i.id == modified_id).unwrap();
    assert!(matches!(
        &modified_item.status,
        QueueStatus::Failed { error } if error.contains("File was modified before sending: modified.txt")
    ));

    // Clearing finished items removes both failed entries.
    assert_eq!(pc.node.clear_finished_queue_items(Some(phone_id)).unwrap(), 2);
    assert!(pc.node.send_queue(Some(phone_id)).unwrap().is_empty());

    // Also verify retry_queue_item when a toggle caused a failure while queued:
    let phone_dir2 = phone.dir;
    phone.node.shutdown().await;
    wait_for(&mut pc, "phone offline 2", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    let retry_id = pc
        .node
        .send_files(phone_id, vec![outgoing(src.path(), "retry.txt", b"retry-success")])
        .await
        .unwrap();
    pc.node.set_device_toggle(phone_id, "files", false).unwrap();

    let mut phone = start_in(phone_dir2, "Pixel", DeviceKind::Phone).await;
    wait_online(&mut pc, phone_id).await;

    let deadline = tokio::time::Instant::now() + WAIT;
    while !pc
        .node
        .send_queue(Some(phone_id))
        .unwrap()
        .iter()
        .any(|i| i.id == retry_id && matches!(i.status, QueueStatus::Failed { .. }))
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    pc.node.set_device_toggle(phone_id, "files", true).unwrap();
    assert!(pc.node.retry_queue_item(&retry_id).unwrap());

    let retried_paths = wait_transfer(&mut phone, &retry_id, "retried file delivered", saved).await;
    assert_eq!(std::fs::read(&retried_paths[0]).unwrap(), b"retry-success");

    let deadline = tokio::time::Instant::now() + WAIT;
    while !pc.node.send_queue(Some(phone_id)).unwrap().is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(pc.node.send_queue(Some(phone_id)).unwrap().is_empty());

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_cancel_waiting_and_mid_transfer_cleans_up_and_unpair_drops_queue() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    phone.node.shutdown().await;
    wait_for(&mut pc, "phone offline", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    let src = tempfile::tempdir().unwrap();
    let fd_src_path = src.path().join("fd_source.bin");
    std::fs::write(&fd_src_path, b"staged-descriptor-bytes").unwrap();
    let fd_handle = std::fs::File::open(&fd_src_path).unwrap();

    let staged_id = pc
        .node
        .send_files(
            phone_id,
            vec![OutgoingFile {
                name: "staged.bin".into(),
                folder: None,
                source: FileSource::File(fd_handle),
            }],
        )
        .await
        .unwrap();

    let staging_dir = pc.dir.path().join("queue_staging").join(&staged_id);
    assert!(staging_dir.exists(), "FileSource::File must be staged in queue_staging/<id>");

    assert!(pc.node.cancel_queue_item(&staged_id).unwrap());
    assert!(pc.node.send_queue(Some(phone_id)).unwrap().is_empty());
    assert!(!staging_dir.exists(), "cancelling a queued item must delete its staged files");

    // Enqueue 2 items (one staged file + one clipboard clip) and unpair: both are dropped and
    // QueueDroppedOnUnpair is emitted with dropped_count == 2.
    let fd_handle2 = std::fs::File::open(&fd_src_path).unwrap();
    let staged_id2 = pc
        .node
        .send_files(
            phone_id,
            vec![OutgoingFile {
                name: "staged2.bin".into(),
                folder: None,
                source: FileSource::File(fd_handle2),
            }],
        )
        .await
        .unwrap();
    pc.node.send_clipboard(phone_id, "dropped-on-unpair".into()).await.unwrap();
    let staging_dir2 = pc.dir.path().join("queue_staging").join(&staged_id2);
    assert!(staging_dir2.exists());
    assert_eq!(pc.node.send_queue(Some(phone_id)).unwrap().len(), 2);

    pc.node.unpair(phone_id).await.unwrap();
    let dropped_count = wait_for(&mut pc, "QueueDroppedOnUnpair event", |e| match e {
        NodeEvent::QueueDroppedOnUnpair { peer, dropped_count, .. } if *peer == phone_id => {
            Some(*dropped_count)
        }
        _ => None,
    })
    .await;
    assert_eq!(dropped_count, 2);
    assert!(pc.node.send_queue(None).unwrap().is_empty());
    assert!(!staging_dir2.exists(), "unpairing must clean up staged files for dropped queue items");

    pc.node.shutdown().await;
}

/// Whether the database files hold `needle`, as someone reading the disk would see them.
fn database_contains(dir: &std::path::Path, needle: &[u8]) -> bool {
    ["g7link.db", "g7link.db-wal"].iter().any(|name| {
        std::fs::read(dir.join(name)).is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle))
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn clearing_removes_queued_clips_and_links_but_not_files() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &["clip.write", "link.open"]).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    phone.node.shutdown().await;
    wait_for(&mut pc, "phone offline", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("keep.txt");
    std::fs::write(&path, b"keep").unwrap();
    pc.node
        .send_files(
            phone_id,
            vec![OutgoingFile {
                name: "keep.txt".into(),
                folder: None,
                source: FileSource::File(std::fs::File::open(&path).unwrap()),
            }],
        )
        .await
        .unwrap();
    pc.node.send_clipboard(phone_id, "a-copied-secret".into()).await.unwrap();
    pc.node.open_link(phone_id, "https://example.com/a-link".into()).await.unwrap();
    assert_eq!(pc.node.send_queue(Some(phone_id)).unwrap().len(), 3);

    // Clearing the clipboard history takes the clip, not the link or the file.
    pc.node.clear_clipboard_history().unwrap();
    let kinds: Vec<_> = pc.node.send_queue(Some(phone_id)).unwrap().iter().map(|i| i.title.clone()).collect();
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    assert!(!kinds.iter().any(|t| t.contains("a-copied-secret")));

    // Clearing everything takes the link too; the file the user chose to send stays.
    pc.node.clear_all_local_data().unwrap();
    let left = pc.node.send_queue(Some(phone_id)).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].title, "keep.txt");
    pc.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_offline_no_busy_loop_timeline_only_on_delivery_and_clear_history_preserves_queue() {
    use g7link_core::{TimelineKind, TimelineQuery};

    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &["clip.write", "link.open"]).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    let phone_dir = phone.dir;
    phone.node.shutdown().await;
    wait_for(&mut pc, "phone offline", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    // Enqueue a staged file, a clipboard item, and a link while phone is offline.
    let src = tempfile::tempdir().unwrap();
    let fd_src_path = src.path().join("staged_deliver.txt");
    std::fs::write(&fd_src_path, b"staged-for-delivery").unwrap();
    let fd_handle = std::fs::File::open(&fd_src_path).unwrap();
    let file_id = pc
        .node
        .send_files(
            phone_id,
            vec![OutgoingFile {
                name: "staged_deliver.txt".into(),
                folder: None,
                source: FileSource::File(fd_handle),
            }],
        )
        .await
        .unwrap();
    let staging_dir = pc.dir.path().join("queue_staging").join(&file_id);
    assert!(staging_dir.exists());

    // Clearing local history and caches leaves a file waiting to send, and its staged copy.
    pc.node.clear_all_local_data().unwrap();
    assert_eq!(pc.node.send_queue(Some(phone_id)).unwrap().len(), 1);
    assert!(staging_dir.exists(), "clear_all_local_data must not delete queue_staging");

    pc.node.send_clipboard(phone_id, "offline-clip-text".into()).await.unwrap();
    pc.node.open_link(phone_id, "https://example.com/offline-link".into()).await.unwrap();

    // 1. Timeline must NOT record any Sent/Opened entries while items are still waiting in the queue.
    let tl_before = pc
        .node
        .timeline_page(&TimelineQuery { device: Some(phone_id), limit: 50, ..Default::default() })
        .unwrap();
    assert_eq!(
        tl_before.total, 0,
        "timeline must not record queued items before they are actually delivered"
    );

    // 2. Setting a past next_retry_ms on a waiting item for an offline peer must NOT cause a 50ms busy loop.
    assert!(pc.node.debug_set_queue_retry_ms(&file_id, 1, 1_000).unwrap());
    tokio::time::sleep(Duration::from_millis(60)).await;
    let ticks_start = pc.node.queue_coordinator_ticks();
    tokio::time::sleep(Duration::from_millis(450)).await;
    let ticks_elapsed = pc.node.queue_coordinator_ticks().saturating_sub(ticks_start);
    assert!(
        ticks_elapsed <= 1,
        "queue coordinator must stay idle while peer is offline, got {ticks_elapsed} ticks in 450ms"
    );

    // 3. What's waiting to send isn't readable in the database, though the queue shows it.
    let queue = pc.node.send_queue(Some(phone_id)).unwrap();
    assert!(queue.iter().any(|i| i.title == "offline-clip-text"));
    assert!(!database_contains(pc.dir.path(), b"offline-clip-text"), "a queued clip is sealed on disk");
    assert!(
        !database_contains(pc.dir.path(), b"example.com/offline-link"),
        "a queued link is sealed on disk"
    );

    // 4. Bring phone online: all 3 items are delivered, staged files are deleted, and timeline records all 3.
    let mut phone = start_in(phone_dir, "Pixel", DeviceKind::Phone).await;
    phone
        .node
        .update_power(
            PowerLevel::NotApplicable,
            vec!["media.control".into(), "clip.write".into(), "link.open".into()],
        )
        .await;

    let received = wait_transfer(&mut phone, &file_id, "staged file delivered", saved).await;
    assert_eq!(std::fs::read(&received[0]).unwrap(), b"staged-for-delivery");

    let deadline = tokio::time::Instant::now() + WAIT;
    while (phone.platform.clipboard.lock().unwrap().is_empty()
        || phone.platform.links.lock().unwrap().is_empty()
        || !pc.node.send_queue(Some(phone_id)).unwrap().is_empty())
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    assert!(!staging_dir.exists(), "staged directory must be deleted once file delivery completes");
    let tl_after = pc
        .node
        .timeline_page(&TimelineQuery { device: Some(phone_id), limit: 50, ..Default::default() })
        .unwrap();
    assert_eq!(tl_after.total, 3, "timeline must record File, Clip, and Link once delivered");
    assert!(tl_after.entries.iter().any(|e| e.kind == TimelineKind::File && !e.incoming));
    assert!(tl_after.entries.iter().any(|e| e.kind == TimelineKind::Clip && !e.incoming));
    assert!(tl_after.entries.iter().any(|e| e.kind == TimelineKind::Link && !e.incoming));

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pc_audio_reaches_the_phone_over_both_paths() {
    use g7link_core::{DECK_ACTIONS, PC_AUDIO};

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[DECK_ACTIONS, PC_AUDIO]).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    // "Phone speaker" can be turned on from either device, so the phone plays whatever its
    // paired PC streams: small frames go as datagrams, larger ones on a stream.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut heard = 0;
    while heard == 0 && tokio::time::Instant::now() < deadline {
        let _ = pc.node.send_pc_audio_pcm(&phone_id, 48_000, 2, vec![1u8; 480]).await;
        let _ = pc.node.send_pc_audio_pcm(&phone_id, 48_000, 2, vec![2u8; 4000]).await;
        while let Ok(Ok(ev)) = tokio::time::timeout(Duration::from_millis(40), phone.events.recv()).await {
            if matches!(ev, NodeEvent::PcAudioFrame { .. }) {
                heard += 1;
            }
        }
    }
    assert!(heard > 0, "the phone never surfaced the PC's audio");

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lagging_event_subscriber_doesnt_keep_sound_piling_up() {
    use g7link_core::{DECK_ACTIONS, PC_AUDIO};

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[DECK_ACTIONS, PC_AUDIO]).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    // `phone.events` isn't read from here on. Sound comes in pieces of at most
    // 960 bytes (`docs/protocol/v1.md` §5), so the event ring holds at most its
    // 2048 events of it, under 2 MiB: a subscriber that lags loses the oldest
    // instead of keeping sound piling up. A second subscriber keeps up.
    let mut listener = phone.node.events();
    const FRAME: usize = 64 * 1024;
    const SENT: usize = 64;
    let mut heard = 0;
    for sent in 1..=SENT {
        pc.node.send_pc_audio_pcm(&phone_id, 48_000, 2, vec![0; FRAME]).await.unwrap();
        while heard < sent * FRAME {
            match tokio::time::timeout(Duration::from_millis(500), listener.recv()).await {
                Ok(Ok(NodeEvent::PcAudioFrame { pcm, .. })) => heard += pcm.len(),
                Ok(Ok(_)) | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                _ => break,
            }
        }
    }
    assert!(heard > SENT * FRAME / 2, "the phone surfaced {heard} bytes of the PC's audio");
    assert!(
        matches!(phone.events.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_))),
        "the subscriber that didn't read lost the oldest sound rather than holding all of it"
    );

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

/// Play on phone for a video file: the phone reads only the file the PC
/// offered, in ranges, while the offer is open.
#[tokio::test(flavor = "multi_thread")]
async fn play_on_phone_offers_a_video_file_the_phone_reads_in_ranges() {
    use g7link_core::{MEDIA_STREAM, MEDIA_WATCH, MEDIA_WATCH_MAX_READ};

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[MEDIA_STREAM]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[MEDIA_WATCH, g7link_core::MIRROR_VIEW]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Nothing offered yet: nothing to read.
    let early = phone.node.watch_file_read(pc_id, 0, 1024).await;
    assert!(matches!(early, Err(Error::Denied)), "{early:?}");

    let dir = tempfile::tempdir().unwrap();
    let movie = dir.path().join("Holiday.mp4");
    let bytes: Vec<u8> = (0..(MEDIA_WATCH_MAX_READ as usize + 300_000)).map(|i| (i % 251) as u8).collect();
    std::fs::write(&movie, &bytes).unwrap();
    with_timeout(
        "offer the file",
        pc.node.offer_watch_file(phone_id, "vlc".into(), Some("Holiday".into()), movie.clone(), 61_500),
    )
    .await
    .unwrap();
    let offered = format!("vlc: Holiday [Holiday.mp4 {} at 61500]", bytes.len());
    assert_eq!(*phone.platform.watch_offers.lock().unwrap(), [offered]);

    // Ranges come back as they are on the PC, capped at one read's worth.
    let (size, head) =
        with_timeout("read the start", phone.node.watch_file_read(pc_id, 0, 1000)).await.unwrap();
    assert_eq!((size, head.as_slice()), (bytes.len() as u64, &bytes[..1000]));
    let (_, big) = with_timeout("read a lot", phone.node.watch_file_read(pc_id, 10, u64::MAX)).await.unwrap();
    assert_eq!(big.len() as u64, MEDIA_WATCH_MAX_READ);
    assert_eq!(big.as_slice(), &bytes[10..10 + MEDIA_WATCH_MAX_READ as usize]);
    let tail_at = bytes.len() as u64 - 100;
    let (_, tail) =
        with_timeout("read the end", phone.node.watch_file_read(pc_id, tail_at, 4096)).await.unwrap();
    assert_eq!(tail.as_slice(), &bytes[tail_at as usize..]);
    let (_, past) =
        with_timeout("read past it", phone.node.watch_file_read(pc_id, bytes.len() as u64 + 5, 10))
            .await
            .unwrap();
    assert!(past.is_empty());

    // A streamed offer replaces it: the file can't be read any more.
    with_timeout("offer a stream", pc.node.offer_watch(phone_id, "Chrome".into(), None)).await.unwrap();
    let after = phone.node.watch_file_read(pc_id, 0, 10).await;
    assert!(matches!(after, Err(Error::Denied)), "{after:?}");

    // Turning media off for the phone takes the offer back, and turning it on again doesn't restore it.
    with_timeout(
        "offer the file again",
        pc.node.offer_watch_file(phone_id, "vlc".into(), None, movie.clone(), 0),
    )
    .await
    .unwrap();
    with_timeout("read it again", phone.node.watch_file_read(pc_id, 0, 10)).await.unwrap();
    pc.node.set_device_toggle(phone_id, "media", false).unwrap();
    let off = phone.node.watch_file_read(pc_id, 0, 10).await;
    assert!(matches!(off, Err(Error::Denied)), "{off:?}");
    pc.node.set_device_toggle(phone_id, "media", true).unwrap();
    let on_again = phone.node.watch_file_read(pc_id, 0, 10).await;
    assert!(matches!(on_again, Err(Error::Denied)), "{on_again:?}");

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn play_on_phone_lets_the_phone_watch_only_what_the_pc_offered() {
    use g7link_core::{MEDIA_STREAM, MEDIA_WATCH, MIRROR_SCREEN, MirrorStart};

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[MEDIA_STREAM]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[MEDIA_WATCH, g7link_core::MIRROR_VIEW]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let watch = |player: &str| MirrorStart {
        max_size: 1280,
        fps: 30,
        bitrate: 4_000_000,
        audio: true,
        session: MIRROR_SCREEN,
        app: None,
        stay_awake: false,
        screen_off: false,
        watch: Some(player.into()),
        sound_only: false,
    };

    // Not offered: the phone can't watch, and the PC isn't asked to share its screen.
    let early = phone.node.mirror_start(pc_id, watch("Chrome")).await;
    assert!(matches!(early, Err(Error::Denied)), "{early:?}");
    assert!(pc.platform.mirror_asks.lock().unwrap().is_empty());

    with_timeout("offer", pc.node.offer_watch(phone_id, "Chrome".into(), Some("A video".into())))
        .await
        .unwrap();
    assert_eq!(*phone.platform.watch_offers.lock().unwrap(), ["Chrome: A video"]);

    // Another player than the one offered: no.
    let other = phone.node.mirror_start(pc_id, watch("Spotify")).await;
    assert!(matches!(other, Err(Error::Denied)), "{other:?}");

    // The one offered: the PC streams it, with its sound.
    with_timeout("watch", phone.node.mirror_start(pc_id, watch("Chrome"))).await.unwrap();
    assert_eq!(*pc.platform.mirror_asks.lock().unwrap(), ["start 1280 with sound watching Chrome"]);
    // The PC may open the stream to it, though screen sharing is off for the phone.
    let stream = with_timeout("open", pc.node.mirror_open(phone_id)).await.unwrap();
    drop(stream);

    // Stopped and started again (the phone rotated): still allowed.
    with_timeout("stop", phone.node.mirror_stop(pc_id, MIRROR_SCREEN)).await;
    with_timeout("watch again", phone.node.mirror_start(pc_id, watch("Chrome"))).await.unwrap();

    // The screen itself stays off limits.
    let screen = MirrorStart { watch: None, audio: false, ..watch("Chrome") };
    let denied = phone.node.mirror_start(pc_id, screen).await;
    assert!(matches!(denied, Err(Error::Denied)), "{denied:?}");

    // With media off for the phone, the PC can't offer.
    pc.node.set_device_toggle(phone_id, "media", false).unwrap();
    let off = pc.node.offer_watch(phone_id, "Chrome".into(), None).await;
    assert!(matches!(off, Err(Error::Denied)), "{off:?}");

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn play_on_phone_needs_a_phone_that_can_watch() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::MEDIA_STREAM]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[]).await;
    pair_qr(&mut pc, &mut phone).await;
    let offer = pc.node.offer_watch(phone.node.device_id(), "Chrome".into(), None).await;
    assert!(matches!(offer, Err(Error::Unsupported)), "{offer:?}");
    assert!(phone.platform.watch_offers.lock().unwrap().is_empty());
    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_streams_its_microphone_when_its_user_agrees() {
    use g7link_core::{MIC_LISTEN, MIC_STREAM, MirrorAudioConfig, MirrorSend, PacketKind};

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[MIC_LISTEN]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[MIC_STREAM]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // The PC asks; the phone asks its user (the platform), nothing streams yet.
    with_timeout("mic_start", pc.node.mic_start(phone_id)).await.unwrap();
    assert_eq!(*phone.platform.mic_asks.lock().unwrap(), ["start"]);

    // The user agreed: the phone streams 48 kHz mono.
    let stream = Arc::new(with_timeout("mic_open", phone.node.mic_open(pc_id)).await.unwrap());
    let config = MirrorAudioConfig { codec: g7link_core::MIRROR_PCM.into(), rate: 48_000, channels: 1 };
    let sender = stream.clone();
    let sent = tokio::task::spawn_blocking(move || {
        [
            sender.send(PacketKind::Config, 0, config.to_cbor()),
            sender.send(PacketKind::Frame, 10_000, vec![0u8; 960]),
            sender.send(PacketKind::Frame, 20_000, vec![1u8; 960]),
        ]
    })
    .await
    .unwrap();
    assert!(sent.iter().all(|s| *s == MirrorSend::Queued), "{sent:?}");
    wait_for(&mut pc, "mic on", |e| matches!(e, NodeEvent::Mic { on: true, .. }).then_some(())).await;
    let deadline = std::time::Instant::now() + WAIT;
    while pc.platform.mic.got.lock().unwrap().len() < 3 {
        assert!(std::time::Instant::now() < deadline, "{:?}", pc.platform.mic.got.lock().unwrap());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(pc.platform.mic.got.lock().unwrap()[..3], ["config 48000 Hz x1", "audio 960", "audio 960"]);

    // The PC stops listening: the phone is told to stop recording.
    pc.node.mic_stop(phone_id).await;
    wait_for(&mut pc, "mic off", |e| matches!(e, NodeEvent::Mic { on: false, .. }).then_some(())).await;
    let deadline = std::time::Instant::now() + WAIT;
    while !phone.platform.mic_asks.lock().unwrap().contains(&"stop".to_owned()) {
        assert!(std::time::Instant::now() < deadline, "the phone wasn't told to stop");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // With the microphone off for the PC on the phone, the PC's ask is refused.
    phone.node.set_device_toggle(pc_id, "mic", false).unwrap();
    let denied = pc.node.mic_start(phone_id).await;
    assert!(matches!(denied, Err(Error::Denied)), "{denied:?}");
    assert_eq!(phone.platform.mic_asks.lock().unwrap().iter().filter(|a| *a == "start").count(), 1);

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_can_start_its_microphone_for_a_pc_itself() {
    use g7link_core::{MIC_LISTEN, MIC_STREAM, MirrorAudioConfig, MirrorSend, PacketKind};

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[MIC_LISTEN]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[MIC_STREAM]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Started on the phone, as the webcam can be: the PC never asked.
    let stream = Arc::new(with_timeout("mic_open", phone.node.mic_open(pc_id)).await.unwrap());
    let config = MirrorAudioConfig { codec: g7link_core::MIRROR_PCM.into(), rate: 48_000, channels: 1 };
    let sender = stream.clone();
    let sent = tokio::task::spawn_blocking(move || sender.send(PacketKind::Config, 0, config.to_cbor()))
        .await
        .unwrap();
    assert_eq!(sent, MirrorSend::Queued);
    let device = wait_for(&mut pc, "mic on", |e| match e {
        NodeEvent::Mic { device, on: true } => Some(*device),
        _ => None,
    })
    .await;
    assert_eq!(device, phone_id);
    assert!(phone.platform.mic_asks.lock().unwrap().is_empty(), "nobody was asked");

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_hears_when_the_phone_declines_its_microphone_request() {
    use g7link_core::{MIC_LISTEN, MIC_STREAM};

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[MIC_LISTEN]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[MIC_STREAM]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    with_timeout("mic_start", pc.node.mic_start(phone_id)).await.unwrap();
    // The user taps Not now: no sound ever came, and the PC stops waiting.
    phone.node.mic_stop(pc_id).await;
    let device = wait_for(&mut pc, "mic off", |e| match e {
        NodeEvent::Mic { device, on: false } => Some(*device),
        _ => None,
    })
    .await;
    assert_eq!(device, phone_id);

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_without_mic_listen_refuses_a_phones_microphone() {
    use g7link_core::{MIC_STREAM, MirrorAudioConfig, PacketKind};

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[MIC_STREAM]).await;
    pair_qr(&mut pc, &mut phone).await;
    let stream = Arc::new(with_timeout("mic_open", phone.node.mic_open(pc.node.device_id())).await.unwrap());
    let config = MirrorAudioConfig { codec: g7link_core::MIRROR_PCM.into(), rate: 48_000, channels: 1 };
    let sender = stream.clone();
    tokio::task::spawn_blocking(move || sender.send(PacketKind::Config, 0, config.to_cbor())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(pc.platform.mic.got.lock().unwrap().is_empty());
    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phones_notifications_show_on_a_paired_tablet_that_shows_them() {
    let mut tablet = device_with("Tab", DeviceKind::Tablet, &[g7link_core::NOTIFY_SHOW]).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut tablet, &mut phone).await;
    let phone_id = phone.node.device_id();
    grant_notification_access(&phone).await;

    phone.node.notifications_reset(vec![note("a", "com.chat", "Sam", true)]).await;
    let items = wait_for(&mut tablet, "snapshot", |e| match e {
        NodeEvent::NotificationsReset { device, items } if *device == phone_id => Some(items.clone()),
        _ => None,
    })
    .await;
    assert_eq!(items.len(), 1);

    phone.node.notification_posted(note("b", "com.chat", "Kim", true)).await;
    let posted = wait_for(&mut tablet, "posted", |e| match e {
        NodeEvent::NotificationPosted { device, notification } if *device == phone_id => {
            Some(notification.key.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(posted, "b");

    // Dismissed on the tablet: gone on the phone too.
    with_timeout("dismiss", tablet.node.dismiss_notification(phone_id, "b".into())).await.unwrap();

    tablet.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_keeps_its_notifications_from_a_phone_that_doesnt_show_them() {
    let mut other = device("Other phone", DeviceKind::Phone).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut other, &mut phone).await;
    grant_notification_access(&phone).await;
    phone.node.notification_posted(note("a", "com.chat", "Sam", true)).await;
    let got = tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            match other.events.recv().await {
                Ok(NodeEvent::NotificationPosted { .. } | NodeEvent::NotificationsReset { .. }) => {
                    return true;
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => return false,
            }
        }
    })
    .await;
    assert!(!matches!(got, Ok(true)), "notifications went to a phone that doesn't show them");
    other.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test]
async fn usb_upgrade_reconnect_and_realtime_pcm_do_not_starve_control_rpcs() {
    use g7link_core::ConnectionPath;

    let mut phone = start_usb_device("Pixel", DeviceKind::Phone, Some(0), &["find.ring"]).await;
    let mut pc = start_usb_device("Desktop", DeviceKind::Desktop, None, &["find.ring"]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    assert!(matches!(
        pc.node.paired_devices().unwrap()[0].link,
        LinkState::Online { path: ConnectionPath::Lan | ConnectionPath::Tailscale, .. }
    ));

    // Attach USB tunnel while a LAN session is already live: both sides upgrade to ConnectionPath::Usb.
    let usb_port = phone.node.usb_port().expect("phone USB TCP listener");
    let tunnel_addr: SocketAddr = ("127.0.0.1".parse::<std::net::IpAddr>().unwrap(), usb_port).into();
    pc.node.connect_usb_tunnel(tunnel_addr).await.expect("connect USB tunnel");

    wait_for(&mut pc, "pc upgrades to usb", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Online { path: ConnectionPath::Usb, .. } }
            if *device == phone_id =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
    wait_for(&mut phone, "phone upgrades to usb", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Online { path: ConnectionPath::Usb, .. } }
            if *device == pc_id =>
        {
            Some(())
        }
        _ => None,
    })
    .await;

    // Enqueue a burst of 512 >960-byte PCM frames (2x OUTBOX_CAPACITY, going through
    // try_send_realtime) before the writer task runs and verify that excess frames past
    // half outbox capacity are dropped while control RPCs (ring) still complete promptly.
    const BURST_FRAMES: usize = 512;
    let payload = vec![0u8; 1_000];
    for _ in 0..BURST_FRAMES {
        pc.node
            .send_pc_audio_pcm(&phone_id, 48_000, 2, payload.clone())
            .await
            .expect("realtime PCM enqueue returns Ok even when dropping frames");
    }
    with_timeout("ring during realtime pcm burst", pc.node.ring(phone_id, true))
        .await
        .expect("control RPC succeeds during realtime burst");
    assert_eq!(*phone.platform.rings.lock().unwrap(), [true]);

    let mut received_pcm = 0usize;
    while let Ok(ev) = phone.events.try_recv() {
        if matches!(ev, NodeEvent::PcAudioFrame { .. }) {
            received_pcm += 1;
        }
    }
    assert!(
        (1..=128).contains(&received_pcm),
        "expected excess realtime frames to be dropped past half outbox capacity (128), got {received_pcm}"
    );

    // Shut down the phone (which closes the USB TCP tunnel) and restart it over LAN.
    phone.node.shutdown().await;
    wait_for(&mut pc, "phone offline before restart", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;

    let TestDevice { node: old_phone, dir, .. } = phone;
    drop(old_phone);
    let mut phone = start_in(dir, "Pixel", DeviceKind::Phone).await;
    wait_online(&mut phone, pc_id).await;
    wait_online(&mut pc, phone_id).await;
    with_timeout("ring after restart", pc.node.ring(phone_id, false))
        .await
        .expect("ring works after restart");

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

/// Throughput of one large file between two devices in this process, over
/// loopback. Not a check: run it to measure (`--ignored --nocapture`, in
/// release), with G7LINK_BENCH_MB for the size (default 1024).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark"]
async fn bench_file_throughput() {
    use std::io::Write;
    let mb: usize = std::env::var("G7LINK_BENCH_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(1024);
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("bench.bin");
    {
        let block = data(8 * 1024 * 1024, 7);
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        for _ in 0..mb / 8 {
            f.write_all(&block).unwrap();
        }
    }
    let files = vec![OutgoingFile { name: "bench.bin".into(), folder: None, source: FileSource::Path(path) }];
    let started = std::time::Instant::now();
    let id = pc.node.send_files(phone_id, files).await.expect("send starts");
    let first_byte =
        wait_transfer(&mut phone, &id, "first bytes", |t| (t.done > 0).then(|| started.elapsed())).await;
    wait_transfer(&mut phone, &id, "received", saved).await;
    let total = started.elapsed();
    let moving = total - first_byte;
    println!(
        "bench: {mb} MB in {:.2} s = {:.0} MB/s overall; first byte after {:.2} s, then {:.0} MB/s",
        total.as_secs_f64(),
        mb as f64 / total.as_secs_f64(),
        first_byte.as_secs_f64(),
        mb as f64 / moving.as_secs_f64()
    );
    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

/// A phone whose app reports `install_id` (the same on this phone after a reinstall).
async fn phone_with_install_id(name: &'static str, install_id: &str) -> TestDevice {
    init_logging();
    let dir = tempfile::tempdir().unwrap();
    let mut phone_info = info(name, DeviceKind::Phone);
    phone_info.install_id = Some(install_id.into());
    let mut config = NodeConfig::new(dir.path(), phone_info, "0.0.1-test");
    config.lan_discovery = false;
    config.away_mode = false;
    config.key_protector = Some(Arc::new(PlainKeyProtector));
    config.capabilities = vec!["media.control".into()];
    let platform = Arc::new(RecordingPlatform::default());
    let node = Node::start(config, platform.clone()).await.expect("node starts");
    let events = node.events();
    TestDevice { node, events, platform, dir, name }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reinstalled_phone_replaces_its_old_pairing() {
    const INSTALL: &str = "0123456789abcdef0123456789abcdef";
    let mut pc = device("pc", DeviceKind::Desktop).await;
    let mut old = phone_with_install_id("phone", INSTALL).await;
    pair_qr(&mut pc, &mut old).await;
    let old_id = old.node.device_id();
    old.node.shutdown().await;
    wait_for(&mut pc, "old phone offline", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if *device == old_id => Some(()),
        _ => None,
    })
    .await;

    // Reinstalled: a new identity, the same install ID. Pairing it replaces the old entry.
    let mut new = phone_with_install_id("phone", INSTALL).await;
    let mut pc_events = pc.node.events();
    pair_qr(&mut pc, &mut new).await;
    let new_id = new.node.device_id();
    let mut removed = false;
    while let Ok(event) = pc_events.try_recv() {
        removed |= matches!(event, NodeEvent::DeviceRemoved(device) if device == old_id);
    }
    assert!(removed, "the old entry is announced as removed");
    let ids: Vec<g7link_core::DeviceId> =
        pc.node.paired_devices().unwrap().into_iter().map(|d| d.id).collect();
    assert_eq!(ids, vec![new_id]);

    pc.node.shutdown().await;
    new.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_time_screen_consent_ends_with_the_pairing() {
    use g7link_core::{MIRROR_CAPTURE, MIRROR_INPUT, MIRROR_SCREEN, MirrorStart};

    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[MIRROR_CAPTURE, MIRROR_INPUT]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::MIRROR_VIEW]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let options = MirrorStart {
        max_size: 1280,
        fps: 30,
        bitrate: 4_000_000,
        audio: false,
        session: MIRROR_SCREEN,
        app: None,
        stay_awake: false,
        screen_off: false,
        watch: None,
        sound_only: false,
    };

    // "Allow once" lets the phone view the PC's screen during this connection
    // (granted here before the phone starts, so nothing else clears it).
    pc.node.set_pc_mirror_session_allowed(phone_id, true);

    // After unpairing and pairing again, the phone has to be allowed again.
    with_timeout("unpair", pc.node.unpair(phone_id)).await.unwrap();
    wait_for(&mut phone, "phone forgets the PC", |e| match e {
        NodeEvent::DeviceRemoved(device) if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    pair_qr(&mut pc, &mut phone).await;
    let again = phone.node.mirror_start(pc_id, options).await;
    assert!(matches!(again, Err(Error::Denied)), "expected Denied after re-pairing, got {again:?}");

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn notification_history_keeps_the_newest_thousand() {
    let phone = device("phone", DeviceKind::Phone).await;
    for i in 0..1005i64 {
        phone
            .node
            .notification_posted(Notification {
                key: format!("app|{i:04}"),
                app: "org.example.chat".into(),
                app_name: "Chat".into(),
                title: Some(format!("Message {i}")),
                text: None,
                sub: None,
                when: 1_760_000_000_000 + i * 1000,
                silent: false,
                ongoing: false,
                actions: Vec::new(),
                icon: None,
                image: None,
                live: None,
                conversation: None,
            })
            .await;
    }
    let history = phone.node.notification_history(None, 5000);
    assert_eq!(history.len(), 1000);
    assert!(history.iter().all(|n| n.key.as_str() >= "app|0005"), "the oldest five are gone");
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn elevated_over_usb_is_refused_without_a_usb_connection() {
    let mut pc = device("pc", DeviceKind::Desktop).await;
    let mut phone = device("phone", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let pc_id = pc.node.device_id();
    let apk = "/data/app/~~a==/app.g7link-b==/base.apk".to_owned();
    let token = "0123456789abcdef0123456789abcdef".to_owned();

    // This PC doesn't watch adb, so it can't start anything over USB.
    let refused = phone.node.elevated_usb_start(pc_id, apk.clone(), token.clone(), Vec::new(), None).await;
    assert!(matches!(refused, Err(Error::Unsupported)), "{refused:?}");
    // A request for anything but the app's own helper never leaves the phone.
    let bad = phone.node.elevated_usb_start(pc_id, apk, token, vec!["reboot".into()], None).await;
    assert!(matches!(bad, Err(Error::Protocol(_))), "{bad:?}");

    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spoofed_usb_hello_is_not_trusted_without_matching_quic_identity() {
    use g7link_core::{ConnectionPath, DeviceId};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn send_usb_hello(stream: &mut tokio::net::TcpStream, id: &DeviceId, name: &str) {
        let json = format!(r#"{{"v":1,"id":"{id}","name":"{name}"}}"#);
        let bytes = json.as_bytes();
        let mut header = [0u8; 6];
        header[..4].copy_from_slice(b"G7QU");
        header[4..6].copy_from_slice(&(bytes.len() as u16).to_be_bytes());
        stream.write_all(&header).await.unwrap();
        stream.write_all(bytes).await.unwrap();
        stream.flush().await.unwrap();
    }

    async fn read_usb_hello(stream: &mut tokio::net::TcpStream) {
        let mut header = [0u8; 6];
        stream.read_exact(&mut header).await.unwrap();
        assert_eq!(&header[..4], b"G7QU");
        let len = u16::from_be_bytes([header[4], header[5]]) as usize;
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await.unwrap();
    }

    let mut phone = start_usb_device("Pixel", DeviceKind::Phone, Some(0), &[]).await;
    let mut pc = start_usb_device("Desktop", DeviceKind::Desktop, None, &[]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    assert!(matches!(
        phone.node.paired_devices().unwrap()[0].link,
        LinkState::Online { path: ConnectionPath::Lan | ConnectionPath::Tailscale, .. }
    ));

    // 1. Phone side: a local TCP client sends a spoofed UsbHello claiming the paired PC's
    //    DeviceId (and another claiming an unpaired DeviceId) without completing QUIC TLS.
    let usb_port = phone.node.usb_port().expect("phone USB TCP listener");
    let mut spoof_pc = tokio::net::TcpStream::connect(("127.0.0.1", usb_port)).await.unwrap();
    send_usb_hello(&mut spoof_pc, &pc_id, "Spoofed PC").await;
    read_usb_hello(&mut spoof_pc).await;

    let fake_id = DeviceId([0x42; 32]);
    let mut spoof_unpaired = tokio::net::TcpStream::connect(("127.0.0.1", usb_port)).await.unwrap();
    send_usb_hello(&mut spoof_unpaired, &fake_id, "Spoofed Unpaired").await;
    read_usb_hello(&mut spoof_unpaired).await;

    // 2. PC side: a local TCP listener replies to connect_usb_tunnel with a spoofed UsbHello
    //    claiming the paired phone's DeviceId, without forwarding QUIC traffic to the phone.
    let fake_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let fake_addr = fake_listener.local_addr().unwrap();
    let server_task = tokio::spawn(async move {
        let (mut s, _) = fake_listener.accept().await.unwrap();
        read_usb_hello(&mut s).await;
        send_usb_hello(&mut s, &phone_id, "Spoofed Phone").await;
        tokio::time::sleep(Duration::from_secs(2)).await;
    });
    let claimed = pc.node.connect_usb_tunnel(fake_addr).await.unwrap();
    assert_eq!(claimed, phone_id);

    tokio::time::sleep(Duration::from_millis(400)).await;

    // Neither side trusts the unauthenticated UsbHello: links stay direct (on a machine with
    // Tailscale up, two local nodes may settle on its address, named `Tailscale`), never USB,
    // and fake_id is not discovered.
    assert!(
        matches!(
            phone.node.paired_devices().unwrap()[0].link,
            LinkState::Online { path: ConnectionPath::Lan | ConnectionPath::Tailscale, .. }
        ),
        "phone session must remain on Lan when UsbHello is not backed by QUIC identity"
    );
    assert!(
        matches!(
            pc.node.paired_devices().unwrap()[0].link,
            LinkState::Online { path: ConnectionPath::Lan | ConnectionPath::Tailscale, .. }
        ),
        "pc session must remain on Lan when UsbHello is not backed by QUIC identity"
    );
    assert!(
        !phone.node.discovered_devices().iter().any(|d| d.id == fake_id),
        "unauthenticated UsbHello must not emit or record DiscoveredDevice"
    );

    server_task.abort();
    pc.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_takes_screenshots_and_recordings_of_a_pc_that_lets_it_see_the_screen() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::pc_capture::CAP]).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let shots = tempfile::tempdir().unwrap();
    let png = shots.path().join("G7 Link screenshot.png");
    std::fs::write(&png, b"\x89PNG not really").unwrap();
    *pc.platform.capture_file.lock().unwrap() = Some(png.clone());

    // Seeing the screen is off until the PC allows it, and then nothing is captured.
    assert_eq!(phone.node.pc_capture_shot(pc_id).await, Err(PcCaptureError::Off));
    assert_eq!(phone.node.pc_capture_record_start(pc_id).await, Err(PcCaptureError::Off));
    assert!(pc.platform.capture_asks.lock().unwrap().is_empty());

    // Allowed: the PC shows its dialog, takes the shot and sends it as a transfer.
    pc.node.set_device_toggle(phone_id, g7link_core::PC_MIRROR_TOGGLE, true).unwrap();
    pc.platform.capture_prompts.store(true, std::sync::atomic::Ordering::SeqCst);
    let file = with_timeout("shot", phone.node.pc_capture_shot(pc_id)).await.expect("a screenshot");
    assert_eq!(file.name, "G7 Link screenshot.png");
    wait_for(&mut phone, "the PC's dialog", |e| match e {
        NodeEvent::PcCapturePrompt { device } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    let received = wait_transfer(&mut phone, &file.transfer, "the screenshot", saved).await;
    assert_eq!(std::fs::read(&received[0]).unwrap(), b"\x89PNG not really");

    // A locked PC says so.
    *pc.platform.capture_file.lock().unwrap() = None;
    assert_eq!(phone.node.pc_capture_shot(pc_id).await, Err(PcCaptureError::Locked));

    // One recording at a time; stopping it brings the video.
    let video = shots.path().join("G7 Link screen recording.mp4");
    std::fs::write(&video, b"not really a video").unwrap();
    *pc.platform.capture_file.lock().unwrap() = Some(video.clone());
    assert_eq!(phone.node.pc_capture_record_stop(pc_id).await, Err(PcCaptureError::NotRecording));
    with_timeout("record", phone.node.pc_capture_record_start(pc_id)).await.expect("recording");
    assert_eq!(phone.node.pc_capture_record_start(pc_id).await, Err(PcCaptureError::Busy));
    let file = with_timeout("stop", phone.node.pc_capture_record_stop(pc_id)).await.expect("the video");
    assert_eq!(file.name, "G7 Link screen recording.mp4");
    let received = wait_transfer(&mut phone, &file.transfer, "the recording", saved).await;
    assert_eq!(std::fs::read(&received[0]).unwrap(), b"not really a video");
    assert_eq!(*pc.platform.capture_asks.lock().unwrap(), ["shot", "shot", "record", "stop"]);

    // A recording the PC ends by itself still reaches the phone, which hears that it stopped.
    with_timeout("ended", pc.node.pc_capture_record_ended(phone_id, Some(video))).await;
    let mut ended = None;
    let mut arrived = std::collections::HashSet::new();
    wait_for(&mut phone, "the recording ended, and its file", |e| {
        match e {
            NodeEvent::PcCaptureRecordEnded { device, file } if *device == pc_id => {
                ended = Some(file.clone());
            }
            NodeEvent::Transfer(t) if matches!(t.state, TransferState::Done { .. }) => {
                arrived.insert(t.id.clone());
            }
            _ => {}
        }
        match &ended {
            Some(Some(file)) if arrived.contains(&file.transfer) => Some(()),
            Some(None) => panic!("the PC saved the recording, so it sends it"),
            _ => None,
        }
    })
    .await;

    // With files off for the phone, the PC can't send the shot.
    *pc.platform.capture_file.lock().unwrap() = Some(png);
    pc.node.set_device_toggle(phone_id, "files", false).unwrap();
    assert_eq!(phone.node.pc_capture_shot(pc_id).await, Err(PcCaptureError::FilesOff));
}

// ---- Agents (docs/protocol/agents.md) ----

const AGENT_THREAD: &str = "0123456789abcdef0123456789abcdef";
const BUSY_THREAD: &str = "fedcba9876543210fedcba9876543210";
const AGENT_FOLDER: &str = "f0123456789ab";
const AGENT_TOKEN: &str = "aaaabbbbccccddddeeeeffff00001111";
/// A thread whose turn has the agent thinking, with every field added for `agent.items2`.
const THINKING_THREAD: &str = "00001111222233334444555566667777";

fn agent_thread(id: &str, state: &str, from_phone: bool) -> g7link_core::AgentThread {
    g7link_core::AgentThread {
        id: id.into(),
        agent: "claude".into(),
        folder: "g7linux".into(),
        title: "List the files here.".into(),
        state: state.into(),
        mode: "ask".into(),
        from_phone,
        updated: 1_795_000_000_000,
        asks: 0,
        summary: None,
        error: None,
        error_code: None,
        step: None,
        since: None,
        plan: None,
        ..Default::default()
    }
}

fn agent_text(seq: u64, text: &str) -> g7link_core::AgentItem {
    g7link_core::AgentItem {
        seq,
        id: format!("msg_01:{seq}"),
        kind: "assistant".into(),
        when: 1_795_000_000_000,
        text: Some(text.into()),
        truncated: false,
        tool: None,
        ..Default::default()
    }
}

fn agent_catalog() -> g7link_core::AgentCatalog {
    g7link_core::AgentCatalog {
        agents: vec![g7link_core::AgentCatalogAgent {
            id: "claude".into(),
            name: "Claude".into(),
            installed: true,
            signed_in: true,
            login_hint: "claude auth login".into(),
            auth: Some("subscription".into()),
            modes: ["auto", "ask", "accept_edits", "plan", "full"].map(String::from).to_vec(),
            ..Default::default()
        }],
        folders: vec![g7link_core::AgentCatalogFolder {
            id: AGENT_FOLDER.into(),
            label: "g7linux".into(),
            checkpoints: true,
            follow_remote: false,
            modes: vec![g7link_core::AgentFolderMode { agent: "claude".into(), mode: "full".into() }],
            ..Default::default()
        }],
        push: false,
        add_folders: false,
    }
}

fn agent_ask_for(thread: &str) -> g7link_core::AgentAsk {
    g7link_core::AgentAsk {
        token: AGENT_TOKEN.into(),
        thread: thread.into(),
        thread_title: "List the files here.".into(),
        agent: "claude".into(),
        kind: "command".into(),
        title: "Run command".into(),
        command: Some("touch a.txt".into()),
        diff: None,
        path: None,
        reason: None,
        truncated: false,
        expires: 1_795_007_200_000,
        item: None,
        deny_stops: false,
    }
}

fn agent_start(prompt: &str, mode: &str) -> g7link_core::AgentStart {
    g7link_core::AgentStart {
        agent: "claude".into(),
        folder: AGENT_FOLDER.into(),
        prompt: prompt.into(),
        mode: mode.into(),
        ..Default::default()
    }
}

/// A PC with the agent host and its catalog, paired with `phone`.
async fn agents_pc_with(phone: &mut TestDevice) -> TestDevice {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::agent::HOST]).await;
    pc.node.set_agent_catalog(Some(agent_catalog())).await.unwrap();
    pair_qr(&mut pc, phone).await;
    pc
}

/// Turns `agents` on for `phone` and waits for its catalog.
async fn allow_agents(pc: &TestDevice, phone: &mut TestDevice) {
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.set_device_toggle(phone_id, "agents", true).unwrap();
    wait_for(phone, "agent catalog", |e| match e {
        NodeEvent::AgentCatalog { device, catalog: Some(_), .. } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
}

fn agent_events(dev: &mut TestDevice) -> Vec<g7link_core::AgentEvent> {
    drain(dev)
        .into_iter()
        .filter_map(|e| match e {
            NodeEvent::AgentEvent { event, .. } => Some(*event),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn agents_are_refused_until_the_pc_allows_the_phone() {
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    let mut pc = agents_pc_with(&mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    assert!(pc.node.device_toggles(phone_id).unwrap().contains(&("agents", false)));
    assert_eq!(phone.node.agent_catalog(pc_id), None, "no catalog while it's off");

    assert!(matches!(phone.node.agent_threads(pc_id).await, Err(Error::Denied)));
    wait_for(&mut pc, "agents prompt", |e| match e {
        NodeEvent::AgentsRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert!(matches!(phone.node.agent_start(pc_id, agent_start("hi", "ask")).await, Err(Error::Denied)));
    assert!(
        !drain(&mut pc).iter().any(|e| matches!(e, NodeEvent::AgentsRequested { .. })),
        "the prompt shows once"
    );
    assert!(pc.platform.agent_requests.lock().unwrap().is_empty());

    allow_agents(&pc, &mut phone).await;
    let catalog = phone.node.agent_catalog(pc_id).expect("catalog kept");
    assert_eq!(catalog.agents.len(), 1);
    assert_eq!(catalog.agents[0].auth.as_deref(), Some("subscription"));
    assert_eq!(catalog.folders[0].label, "g7linux");
    let claude = &catalog.agents[0];
    assert!(claude.modes.is_empty(), "a phone without agent.control gets no modes");
    assert!(catalog.folders[0].modes.is_empty());
    let threads = with_timeout("threads", phone.node.agent_threads(pc_id)).await.unwrap();
    assert_eq!(threads.len(), 1);

    // The host going away is an empty catalog.
    pc.node.set_agent_catalog(None).await.unwrap();
    wait_for(&mut phone, "empty catalog", |e| match e {
        NodeEvent::AgentCatalog { device, catalog: None, .. } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    assert_eq!(phone.node.agent_catalog(pc_id), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_lists_starts_and_follows_a_thread() {
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    let pc = agents_pc_with(&mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    allow_agents(&pc, &mut phone).await;

    let thread = with_timeout(
        "start",
        // An older phone's word for ask: the PC's host gets today's.
        phone.node.agent_start(pc_id, agent_start("List the files here.", "supervised")),
    )
    .await
    .unwrap();
    assert_eq!(thread.id, AGENT_THREAD);
    let request = pc.platform.agent_requests.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        request,
        (
            phone_id,
            g7link_core::AgentRequest::Start {
                agent: "claude".into(),
                folder: AGENT_FOLDER.into(),
                prompt: "List the files here.".into(),
                mode: "ask".into(),
                model: None,
                effort: None,
                images: Vec::new(),
                custom_model: false,
                key: None
            }
        )
    );

    with_timeout("watch", phone.node.agent_watch(pc_id, Some(AGENT_THREAD.into()))).await.unwrap();
    let page =
        with_timeout("read", phone.node.agent_read(pc_id, AGENT_THREAD.into(), None, 50)).await.unwrap();
    assert_eq!(page.items.len(), 1);

    // Text streams in, paced and joined, and adds up to what was said.
    let words: Vec<String> = (0..30).map(|i| format!("word{i} ")).collect();
    let mut offset = 0u32;
    for word in &words {
        let event =
            g7link_core::AgentEvent::delta(AGENT_THREAD.into(), 2, "msg_01:2".into(), offset, word.clone());
        pc.node.agent_event(event, Some(phone_id)).await.unwrap();
        offset += word.len() as u32;
    }
    let full: String = words.concat();
    let mut got = String::new();
    let mut deltas = 0;
    while got.len() < full.len() {
        let (at, text) = wait_for(&mut phone, "delta", |e| match e {
            NodeEvent::AgentEvent { event, .. } if event.kind == "delta" => {
                Some((event.offset.unwrap(), event.text.clone().unwrap()))
            }
            _ => None,
        })
        .await;
        assert_eq!(at as usize, got.len(), "no gap");
        got.push_str(&text);
        deltas += 1;
    }
    assert_eq!(got, full);
    assert!(deltas < words.len(), "deltas are joined, not sent one by one ({deltas})");

    pc.node
        .agent_event(g7link_core::AgentEvent::item(AGENT_THREAD.into(), agent_text(2, &full)), None)
        .await
        .unwrap();
    let mut done = agent_thread(AGENT_THREAD, "done", false);
    done.summary = Some("word0".into());
    pc.node
        .agent_event(g7link_core::AgentEvent::done(done, "done", "word0".into()), Some(phone_id))
        .await
        .unwrap();
    let item = wait_for(&mut phone, "item", |e| match e {
        NodeEvent::AgentEvent { event, .. } if event.kind == "item" => event.item.clone(),
        _ => None,
    })
    .await;
    assert_eq!(item.text.as_deref(), Some(full.as_str()));
    let state = wait_for(&mut phone, "done", |e| match e {
        NodeEvent::AgentEvent { event, .. } if event.kind == "done" => event.state.clone(),
        _ => None,
    })
    .await;
    assert!(state.from_phone, "the phone that started it is told so");

    with_timeout("send", phone.node.agent_send(pc_id, AGENT_THREAD.into(), "Go on".into())).await.unwrap();
    with_timeout("cancel", phone.node.agent_cancel(pc_id, AGENT_THREAD.into())).await.unwrap();
    match phone.node.agent_send(pc_id, BUSY_THREAD.into(), "Go on".into()).await {
        Err(Error::Protocol(reason)) => assert!(reason.starts_with("BUSY"), "{reason}"),
        other => panic!("a busy thread is the generic failure, not {other:?}"),
    }

    // Unwatched, the phone gets no more text.
    with_timeout("unwatch", phone.node.agent_watch(pc_id, None)).await.unwrap();
    pc.node
        .agent_event(
            g7link_core::AgentEvent::delta(AGENT_THREAD.into(), 3, "msg_01:3".into(), 0, "x".into()),
            None,
        )
        .await
        .unwrap();
    pc.node
        .agent_event(g7link_core::AgentEvent::state(agent_thread(AGENT_THREAD, "working", false)), None)
        .await
        .unwrap();
    wait_for(&mut phone, "state", |e| match e {
        NodeEvent::AgentEvent { event, .. } if event.kind == "state" => Some(()),
        _ => None,
    })
    .await;
    assert!(agent_events(&mut phone).iter().all(|e| e.kind != "delta"));
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_watching_phone_gets_text() {
    let mut watcher = device("Pixel", DeviceKind::Phone).await;
    let mut other = device("Galaxy", DeviceKind::Phone).await;
    let mut pc = agents_pc_with(&mut watcher).await;
    pair_qr(&mut pc, &mut other).await;
    let (pc_id, watcher_id) = (pc.node.device_id(), watcher.node.device_id());
    allow_agents(&pc, &mut watcher).await;
    allow_agents(&pc, &mut other).await;

    with_timeout("watch", watcher.node.agent_watch(pc_id, Some(AGENT_THREAD.into()))).await.unwrap();
    pc.node
        .agent_event(
            g7link_core::AgentEvent::delta(AGENT_THREAD.into(), 1, "msg_01:1".into(), 0, "Hi".into()),
            None,
        )
        .await
        .unwrap();
    pc.node
        .agent_event(g7link_core::AgentEvent::item(AGENT_THREAD.into(), agent_text(1, "Hi")), None)
        .await
        .unwrap();
    pc.node
        .agent_event(
            g7link_core::AgentEvent::state(agent_thread(AGENT_THREAD, "working", false)),
            Some(watcher_id),
        )
        .await
        .unwrap();

    let seen = agent_events_until_state(&mut watcher).await;
    let kinds: Vec<&str> = seen.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["delta", "item", "state"], "the watcher gets the text, in order");
    assert!(seen[2].state.as_ref().unwrap().from_phone, "from_phone is per phone");
    let seen = agent_events_until_state(&mut other).await;
    let kinds: Vec<&str> = seen.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["state"], "the other phone gets the state only");
    assert!(!seen[0].state.as_ref().unwrap().from_phone);
}

/// The agent events `dev` gets up to and with the first `state`.
async fn agent_events_until_state(dev: &mut TestDevice) -> Vec<g7link_core::AgentEvent> {
    let mut seen = Vec::new();
    loop {
        let event = wait_for(dev, "agent event", |e| match e {
            NodeEvent::AgentEvent { event, .. } => Some((**event).clone()),
            _ => None,
        })
        .await;
        let last = event.kind == "state";
        seen.push(event);
        if last {
            return seen;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_reaches_every_phone_and_one_answer_closes_it() {
    let mut first = device("Pixel", DeviceKind::Phone).await;
    let mut second = device("Galaxy", DeviceKind::Phone).await;
    let mut pc = agents_pc_with(&mut first).await;
    pair_qr(&mut pc, &mut second).await;
    let pc_id = pc.node.device_id();
    allow_agents(&pc, &mut first).await;
    allow_agents(&pc, &mut second).await;

    pc.node.agent_ask(agent_ask_for(AGENT_THREAD)).await.unwrap();
    for dev in [&mut first, &mut second] {
        let ask = wait_for(dev, "ask", |e| match e {
            NodeEvent::AgentAsk { device, ask } if *device == pc_id => Some(ask.clone()),
            _ => None,
        })
        .await;
        assert_eq!(ask.command.as_deref(), Some("touch a.txt"));
    }
    with_timeout("answer", first.node.agent_answer(pc_id, AGENT_TOKEN.into(), true, None)).await.unwrap();
    assert!(pc.platform.agent_requests.lock().unwrap().iter().any(|(_, r)| matches!(
        r,
        g7link_core::AgentRequest::Answer { token, allow: true, message: None } if token == AGENT_TOKEN
    )));
    pc.node
        .agent_asked(g7link_core::AgentAsked {
            token: AGENT_TOKEN.into(),
            thread: AGENT_THREAD.into(),
            outcome: "allowed".into(),
            by_name: Some("Pixel".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    for dev in [&mut first, &mut second] {
        let outcome = wait_for(dev, "asked", |e| match e {
            NodeEvent::AgentAsked { token, outcome, by_name, .. } if token == AGENT_TOKEN => {
                Some((outcome.clone(), by_name.clone()))
            }
            _ => None,
        })
        .await;
        assert_eq!(outcome, ("allowed".to_owned(), Some("Pixel".to_owned())));
    }
    assert!(matches!(
        second.node.agent_answer(pc_id, AGENT_TOKEN.into(), false, None).await,
        Err(Error::NotFound)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_cant_pick_a_mode_a_scope_or_a_path() {
    use g7link_core::agents::{AgentAnswer, AgentStart};
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    let pc = agents_pc_with(&mut phone).await;
    let pc_id = pc.node.device_id();
    allow_agents(&pc, &mut phone).await;

    // Without agent.control a phone starts in `ask` only.
    assert!(matches!(
        phone.node.agent_start(pc_id, agent_start("hi", "auto_edit")).await,
        Err(Error::Unsupported)
    ));
    let scoped = AgentAnswer {
        token: AGENT_TOKEN.into(),
        decision: "allow".into(),
        scope: Some("thread".into()),
        message: None,
    };
    assert!(matches!(
        phone.node.debug_agent_request(pc_id, "agent.answer", &scoped).await,
        Err(Error::Denied)
    ));
    let pathed = AgentStart {
        agent: "claude".into(),
        folder: "/home/me/project".into(),
        prompt: "hi".into(),
        mode: "ask".into(),
        ..Default::default()
    };
    match phone.node.debug_agent_request(pc_id, "agent.start", &pathed).await {
        Err(Error::Protocol(reason)) => assert!(reason.contains("BAD_MESSAGE"), "{reason}"),
        other => panic!("a path is a bad message, not {other:?}"),
    }
    assert!(matches!(
        phone
            .node
            .agent_start(pc_id, AgentStart { folder: "../x".into(), ..agent_start("hi", "supervised") })
            .await,
        Err(Error::Protocol(_))
    ));
    assert!(pc.platform.agent_requests.lock().unwrap().is_empty(), "none reach the platform");
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_agent_messages_are_refused() {
    use g7link_core::agents::AgentSend;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    let pc = agents_pc_with(&mut phone).await;
    let pc_id = pc.node.device_id();
    allow_agents(&pc, &mut phone).await;

    let long = "x".repeat(8193);
    assert!(matches!(
        phone.node.agent_start(pc_id, agent_start(&long, "supervised")).await,
        Err(Error::Protocol(_))
    ));
    let refused = |result: Result<(), Error>| match result {
        Err(Error::Protocol(reason)) => assert!(reason.contains("BAD_MESSAGE"), "{reason}"),
        other => panic!("not refused as a bad message: {other:?}"),
    };
    refused(phone.node.debug_agent_request(pc_id, "agent.start", &agent_start(&long, "supervised")).await);
    let send = AgentSend { thread: AGENT_THREAD.into(), text: long.clone(), ..Default::default() };
    refused(phone.node.debug_agent_request(pc_id, "agent.send", &send).await);
    assert!(pc.platform.agent_requests.lock().unwrap().is_empty());

    // What the PC would send over the limits is refused too.
    let delta =
        g7link_core::AgentEvent::delta(AGENT_THREAD.into(), 1, "msg_01:1".into(), 0, "y".repeat(4097));
    assert!(pc.node.agent_event(delta, None).await.is_err());
    let mut bad = agent_ask_for("Not/AnId");
    bad.kind = "command".into();
    assert!(pc.node.agent_ask(bad).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_agents_off_stops_everything() {
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    let pc = agents_pc_with(&mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    allow_agents(&pc, &mut phone).await;
    with_timeout("watch", phone.node.agent_watch(pc_id, Some(AGENT_THREAD.into()))).await.unwrap();

    pc.node.set_device_toggle(phone_id, "agents", false).unwrap();
    wait_for(&mut phone, "last, empty catalog", |e| match e {
        NodeEvent::AgentCatalog { device, catalog: None, .. } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    pc.node
        .agent_event(
            g7link_core::AgentEvent::delta(AGENT_THREAD.into(), 1, "msg_01:1".into(), 0, "Hi".into()),
            None,
        )
        .await
        .unwrap();
    pc.node
        .agent_event(g7link_core::AgentEvent::state(agent_thread(AGENT_THREAD, "working", false)), None)
        .await
        .unwrap();
    pc.node.agent_ask(agent_ask_for(AGENT_THREAD)).await.unwrap();
    pc.node
        .set_agent_catalog(Some(g7link_core::AgentCatalog { folders: vec![], ..agent_catalog() }))
        .await
        .unwrap();
    // A reply comes on the same stream after anything sent before it.
    assert!(matches!(phone.node.agent_threads(pc_id).await, Err(Error::Denied)));
    let after = drain(&mut phone);
    assert!(
        !after.iter().any(|e| matches!(
            e,
            NodeEvent::AgentEvent { .. } | NodeEvent::AgentAsk { .. } | NodeEvent::AgentCatalog { .. }
        )),
        "{after:?}"
    );
    assert!(pc.platform.agent_requests.lock().unwrap().is_empty());

    // Turned back on, the watch is gone: no text until the phone watches again.
    allow_agents(&pc, &mut phone).await;
    pc.node
        .agent_event(
            g7link_core::AgentEvent::delta(AGENT_THREAD.into(), 1, "msg_01:1".into(), 0, "Hi".into()),
            None,
        )
        .await
        .unwrap();
    assert!(with_timeout("threads", phone.node.agent_threads(pc_id)).await.is_ok());
    assert!(agent_events(&mut phone).iter().all(|e| e.kind != "delta"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_without_the_host_says_unsupported() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.set_device_toggle(phone_id, "agents", true).unwrap();
    assert!(matches!(phone.node.agent_threads(pc_id).await, Err(Error::Unsupported)));
    assert!(matches!(
        phone
            .node
            .debug_agent_request(pc_id, "agent.threads", &g7link_core::agents::AgentWatch::default())
            .await,
        Err(Error::Unsupported)
    ));
    // A PC sending its catalog anyway is ignored when it doesn't offer the host.
    pc.node.set_agent_catalog(Some(agent_catalog())).await.unwrap();
    assert_eq!(phone.node.agent_catalog(pc_id), None);
    assert!(pc.platform.agent_requests.lock().unwrap().is_empty());
}

/// A turn as a PC that keeps turns sends it: the prompt, the agent thinking
/// (still), an edit with its class and path that was denied, the words the
/// user denied it with, thinking that ended, and the answer.
fn thinking_turn() -> Vec<g7link_core::AgentItem> {
    use g7link_core::{AgentItem, AgentTool, agent};
    let end = Some(1_795_000_000_500);
    vec![
        AgentItem { kind: agent::item::USER.into(), turn: Some(1), ..agent_text(1, "Fix the failing test.") },
        AgentItem {
            kind: agent::item::REASONING.into(),
            turn: Some(1),
            ..agent_text(2, "**Reading the tests** to see what fails.")
        },
        AgentItem {
            kind: agent::item::TOOL.into(),
            text: None,
            tool: Some(AgentTool {
                name: "Edit".into(),
                title: "Edit file".into(),
                detail: Some("-old\n+new".into()),
                status: agent::tool::DENIED.into(),
                output: None,
                class: Some(agent::tool_class::EDIT.into()),
                path: Some("src/lib.rs".into()),
                by_rule: false,
                review: None,
            }),
            turn: Some(1),
            end,
            group: Some("msg_01:0".into()),
            ..agent_text(3, "")
        },
        AgentItem {
            kind: agent::item::USER.into(),
            turn: Some(1),
            deny_of: Some("msg_01:3".into()),
            ..agent_text(4, "Use the test runner instead.")
        },
        AgentItem {
            kind: agent::item::REASONING.into(),
            text: None,
            turn: Some(1),
            end,
            ..agent_text(5, "")
        },
        AgentItem { turn: Some(1), phase: Some(agent::phase::FINAL.into()), end, ..agent_text(6, "Fixed.") },
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_that_reads_turns_gets_every_new_field_and_the_thinking() {
    use g7link_core::{AgentEvent, agent};
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[agent::ITEMS2]).await;
    let pc = agents_pc_with(&mut phone).await;
    let pc_id = pc.node.device_id();
    allow_agents(&pc, &mut phone).await;

    let page =
        with_timeout("read", phone.node.agent_read(pc_id, THINKING_THREAD.into(), None, 50)).await.unwrap();
    assert_eq!(page.items, thinking_turn(), "every field arrives as it was sent");

    with_timeout("watch", phone.node.agent_watch(pc_id, Some(THINKING_THREAD.into()))).await.unwrap();
    for item in thinking_turn() {
        pc.node.agent_event(AgentEvent::item(THINKING_THREAD.into(), item), None).await.unwrap();
    }
    let thinking = &thinking_turn()[1];
    let offset = thinking.text.as_ref().unwrap().len() as u32;
    let more =
        AgentEvent::delta(THINKING_THREAD.into(), 2, thinking.id.clone(), offset, " Then fix it.".into());
    pc.node.agent_event(more, None).await.unwrap();
    pc.node
        .agent_event(AgentEvent::state(agent_thread(THINKING_THREAD, "working", false)), None)
        .await
        .unwrap();
    let seen = agent_events_until_state(&mut phone).await;
    let items: Vec<_> = seen.iter().filter_map(|e| e.item.clone()).collect();
    assert_eq!(items, thinking_turn());
    let deltas: Vec<_> = seen.iter().filter(|e| e.kind == "delta").map(|e| e.item_id.clone()).collect();
    assert_eq!(deltas, [Some(thinking.id.clone())], "the thinking's text streams");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_older_phone_gets_thinking_as_a_tool_step_and_none_of_its_text() {
    use g7link_core::{AgentEvent, agent};
    let mut old = device("Galaxy", DeviceKind::Phone).await;
    let mut new = device_with("Pixel", DeviceKind::Phone, &[agent::ITEMS2]).await;
    let mut pc = agents_pc_with(&mut old).await;
    pair_qr(&mut pc, &mut new).await;
    let pc_id = pc.node.device_id();
    allow_agents(&pc, &mut old).await;
    allow_agents(&pc, &mut new).await;

    // A page: the thinking is a tool step and no seq is missing, so a
    // 0.1.15 phone takes the page and never reads again for a gap.
    let page =
        with_timeout("read", old.node.agent_read(pc_id, THINKING_THREAD.into(), None, 50)).await.unwrap();
    let seqs: Vec<u64> = page.items.iter().map(|i| i.seq).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5, 6]);
    assert!(page.items.iter().all(|i| agent::item::LEGACY.contains(&i.kind.as_str())));
    let thinking = &page.items[1];
    assert_eq!((thinking.kind.as_str(), thinking.text.as_deref()), ("tool", None));
    let tool = thinking.tool.as_ref().unwrap();
    assert_eq!(
        (tool.name.as_str(), tool.title.as_str(), tool.status.as_str()),
        ("reasoning", "Thinking", "running")
    );
    assert_eq!(page.items[4].tool.as_ref().unwrap().status, "ok", "thinking that ended");
    assert_eq!(page.items[5..], thinking_turn()[5..], "the rest as it was");
    let page =
        with_timeout("read", new.node.agent_read(pc_id, THINKING_THREAD.into(), None, 50)).await.unwrap();
    assert_eq!(page.items, thinking_turn(), "per phone");

    // Live: the item as a tool step, and none of its text.
    for dev in [&old, &new] {
        with_timeout("watch", dev.node.agent_watch(pc_id, Some(THINKING_THREAD.into()))).await.unwrap();
    }
    let thinking = thinking_turn()[1].clone();
    let offset = thinking.text.as_ref().unwrap().len() as u32;
    pc.node.agent_event(AgentEvent::item(THINKING_THREAD.into(), thinking.clone()), None).await.unwrap();
    let more =
        AgentEvent::delta(THINKING_THREAD.into(), 2, thinking.id.clone(), offset, " Then fix it.".into());
    pc.node.agent_event(more, None).await.unwrap();
    let answer = AgentEvent::delta(THINKING_THREAD.into(), 6, "msg_01:6".into(), 0, "Fixed".into());
    pc.node.agent_event(answer, None).await.unwrap();
    pc.node
        .agent_event(AgentEvent::state(agent_thread(THINKING_THREAD, "working", false)), None)
        .await
        .unwrap();

    let kinds = |seen: &[AgentEvent]| -> Vec<(String, Option<String>)> {
        seen.iter().map(|e| (e.kind.clone(), e.item_id.clone())).collect()
    };
    let seen = agent_events_until_state(&mut old).await;
    assert_eq!(
        kinds(&seen),
        [("item".into(), None), ("delta".into(), Some("msg_01:6".into())), ("state".into(), None)]
    );
    assert_eq!(seen[0].item.as_ref(), Some(&thinking.clone().for_legacy()));
    let seen = agent_events_until_state(&mut new).await;
    assert_eq!(
        kinds(&seen),
        [
            ("item".into(), None),
            ("delta".into(), Some(thinking.id.clone())),
            ("delta".into(), Some("msg_01:6".into())),
            ("state".into(), None)
        ]
    );
    assert_eq!(seen[0].item.as_ref(), Some(&thinking));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denial_carries_the_users_words_to_a_pc_that_takes_them() {
    use g7link_core::{AgentRequest, agent, agents::AgentAnswer};
    let mut first = device_with("Pixel", DeviceKind::Phone, &[agent::ITEMS2]).await;
    let mut second = device("Galaxy", DeviceKind::Phone).await;
    let mut pc = agents_pc_with(&mut first).await;
    pair_qr(&mut pc, &mut second).await;
    let (pc_id, first_id) = (pc.node.device_id(), first.node.device_id());
    allow_agents(&pc, &mut first).await;
    allow_agents(&pc, &mut second).await;
    assert!(!first.node.agent_can_say_why(pc_id));

    // A PC that can't pass them on: the phone doesn't send them, and the PC refuses them.
    let words = "Use the test runner instead.";
    assert!(matches!(
        first.node.agent_answer(pc_id, AGENT_TOKEN.into(), false, Some(words.into())).await,
        Err(Error::Unsupported)
    ));
    let deny = AgentAnswer {
        token: AGENT_TOKEN.into(),
        decision: agent::decision::DENY.into(),
        scope: None,
        message: Some(words.into()),
    };
    assert!(matches!(
        first.node.debug_agent_request(pc_id, "agent.answer", &deny).await,
        Err(Error::Unsupported)
    ));
    assert!(pc.platform.agent_requests.lock().unwrap().is_empty(), "the request still waits");

    // Its host learns to: the phones hear it without a new catalog.
    let caps = [agent::HOST, agent::SAY_WHY, "media.control"].map(String::from).to_vec();
    pc.node.update_power(PowerLevel::NotApplicable, caps).await;
    for dev in [&mut first, &mut second] {
        wait_for(dev, "a catalog that takes words", |e| match e {
            NodeEvent::AgentCatalog { device, catalog: Some(_), can_say_why: true, .. }
                if *device == pc_id =>
            {
                Some(())
            }
            _ => None,
        })
        .await;
    }
    assert!(first.node.agent_can_say_why(pc_id));

    // Words go with a denial only, within their limit, and say something.
    let refused = |result: Result<(), Error>| match result {
        Err(Error::Protocol(reason)) => assert!(reason.contains("BAD_MESSAGE"), "{reason}"),
        other => panic!("not refused as a bad message: {other:?}"),
    };
    let allow = AgentAnswer { decision: agent::decision::ALLOW.into(), ..deny.clone() };
    refused(first.node.debug_agent_request(pc_id, "agent.answer", &allow).await);
    let long = AgentAnswer { message: Some("x".repeat(agent::MAX_DENY_MESSAGE_BYTES + 1)), ..deny.clone() };
    refused(first.node.debug_agent_request(pc_id, "agent.answer", &long).await);
    let blank = AgentAnswer { message: Some(" \u{202E}\n".into()), ..deny.clone() };
    refused(first.node.debug_agent_request(pc_id, "agent.answer", &blank).await);
    for (allow, message) in [(true, words.to_owned()), (false, "x".repeat(2001)), (false, " ".into())] {
        assert!(matches!(
            first.node.agent_answer(pc_id, AGENT_TOKEN.into(), allow, Some(message)).await,
            Err(Error::Protocol(_))
        ));
    }
    assert!(pc.platform.agent_requests.lock().unwrap().is_empty());

    // A denial with words reaches the PC's host cleaned, from the API and as sent.
    let typed = format!("{words}\u{202E}");
    with_timeout("deny", first.node.agent_answer(pc_id, AGENT_TOKEN.into(), false, Some(typed.clone())))
        .await
        .unwrap();
    let other_token = "0000aaaa1111bbbb2222cccc3333dddd";
    let raw = AgentAnswer { token: other_token.into(), message: Some(typed), ..deny.clone() };
    with_timeout("raw deny", first.node.debug_agent_request(pc_id, "agent.answer", &raw)).await.unwrap();
    let requests = pc.platform.agent_requests.lock().unwrap().clone();
    let answers: Vec<_> = requests
        .into_iter()
        .filter_map(|(from, r)| match r {
            AgentRequest::Answer { token, allow, message } => Some((from, token, allow, message)),
            _ => None,
        })
        .collect();
    assert_eq!(
        answers,
        [
            (first_id, AGENT_TOKEN.to_owned(), false, Some(words.to_owned())),
            (first_id, other_token.to_owned(), false, Some(words.to_owned())),
        ]
    );

    // Every phone hears how it ended, with the words, an older one too.
    pc.node
        .agent_asked(g7link_core::AgentAsked {
            token: AGENT_TOKEN.into(),
            thread: AGENT_THREAD.into(),
            outcome: agent::asked::DENIED.into(),
            by_name: Some("Pixel".into()),
            message: Some(words.into()),
        })
        .await
        .unwrap();
    for dev in [&mut first, &mut second] {
        let heard = wait_for(dev, "asked", |e| match e {
            NodeEvent::AgentAsked { token, outcome, by_name, message, .. } if token == AGENT_TOKEN => {
                Some((outcome.clone(), by_name.clone(), message.clone()))
            }
            _ => None,
        })
        .await;
        assert_eq!(heard, ("denied".to_owned(), Some("Pixel".to_owned()), Some(words.to_owned())));
    }
}

/// The same test also carries a step the PC's rule allowed and the agent's
/// reviewer approved, flagged as such, and an ask whose Deny stops the turn.
#[tokio::test(flavor = "multi_thread")]
async fn a_phone_never_receives_an_absolute_path() {
    use g7link_core::{AgentEvent, agent};
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[agent::ITEMS2]).await;
    let pc = agents_pc_with(&mut phone).await;
    let pc_id = pc.node.device_id();
    allow_agents(&pc, &mut phone).await;
    with_timeout("watch", phone.node.agent_watch(pc_id, Some(AGENT_THREAD.into()))).await.unwrap();

    let outside = "/home/me/Code/app/src/lib.rs";
    let mut edit = thinking_turn()[2].clone();
    let tool = edit.tool.as_mut().unwrap();
    tool.path = Some(outside.into());
    tool.status = agent::tool::OK.into();
    tool.by_rule = true;
    let review = g7link_core::AgentToolReview {
        status: agent::review::APPROVED.into(),
        risk: Some("low".into()),
        rationale: Some("The user asked for this edit.".into()),
    };
    tool.review = Some(review.clone());
    pc.node.agent_event(AgentEvent::item(AGENT_THREAD.into(), edit.clone()), None).await.unwrap();
    let ask = g7link_core::AgentAsk {
        path: Some(outside.into()),
        item: Some(edit.id.clone()),
        deny_stops: true,
        ..agent_ask_for(AGENT_THREAD)
    };
    pc.node.agent_ask(ask).await.unwrap();
    let item = wait_for(&mut phone, "item", |e| match e {
        NodeEvent::AgentEvent { event, .. } if event.kind == "item" => event.item.clone(),
        _ => None,
    })
    .await;
    let tool = item.tool.unwrap();
    assert_eq!((tool.path.as_deref(), tool.by_rule), (Some("lib.rs"), true));
    assert_eq!(tool.review, Some(review), "the reviewer's decision arrives with its step");
    let ask = wait_for(&mut phone, "ask", |e| match e {
        NodeEvent::AgentAsk { ask, .. } => Some(ask.clone()),
        _ => None,
    })
    .await;
    assert_eq!((ask.path.as_deref(), ask.item.as_deref()), (Some("lib.rs"), Some(edit.id.as_str())));
    assert!(ask.deny_stops, "Deny says it stops the turn");
}

// ---- Agents: sessions started on the PC (docs/protocol/agents.md §3) ----

const FOLLOWED_SESSION: &str = "sc0123456789abcdef0123456789abcdef";
const OPEN_SESSION: &str = "sxfedcba9876543210fedcba9876543210";
const OTHER_SESSION: &str = "sc00000000000000000000000000000001";

/// A session's item: its `seq` comes from the file and may be 0.
fn agent_text_at(seq: u64, text: &str) -> g7link_core::AgentItem {
    g7link_core::AgentItem { seq, id: format!("u-{seq}"), ..agent_text(1, text) }
}

fn agent_session(id: &str, title: &str) -> g7link_core::AgentSession {
    g7link_core::AgentSession {
        id: id.into(),
        agent: "claude".into(),
        folder: "app".into(),
        title: title.into(),
        state: "working".into(),
        guess: false,
        source: "terminal".into(),
        updated: 1_795_000_100_000,
        step: Some("Run command".into()),
        summary: None,
        waiting_for: None,
        can_continue: false,
        blocked: Some("open".into()),
        since: Some(1_795_000_000_000),
        limit: None,
    }
}

/// An export bigger than a control message can carry (1 MiB).
fn big_export(title: &str) -> String {
    let mut text = format!("# {title}\n\n- Agent: Claude\n- Folder: app\n\n---\n\n");
    while text.len() < 3 * 1024 * 1024 / 2 {
        text.push_str("> **You on Pixel**\n>\n> Wie geht's? ✓\n\n```diff\n+fn main() {}\n```\n\n");
    }
    text
}

/// A phone that shows a PC's sessions.
async fn follow_phone(name: &'static str) -> TestDevice {
    device_with(name, DeviceKind::Phone, &[g7link_core::agent::FOLLOW]).await
}

/// A PC with the agent host that lists its sessions, paired with `phone`.
async fn sessions_pc_with(phone: &mut TestDevice) -> TestDevice {
    let mut pc = device_with(
        "Desktop",
        DeviceKind::Desktop,
        &[g7link_core::agent::HOST, g7link_core::agent::SESSIONS],
    )
    .await;
    pc.node.set_agent_catalog(Some(agent_catalog())).await.unwrap();
    *pc.platform.agent_sessions.lock().unwrap() = vec![
        agent_session(FOLLOWED_SESSION, "Fix the login test"),
        agent_session(OPEN_SESSION, "Still open"),
    ];
    pair_qr(&mut pc, phone).await;
    pc
}

/// Turns both `agents` and `agent_sessions` on for `phone`.
async fn allow_sessions(pc: &TestDevice, phone: &mut TestDevice) {
    allow_agents(pc, phone).await;
    pc.node.set_device_toggle(phone.node.device_id(), "agent_sessions", true).unwrap();
}

/// What `dev` got of §3, as (session changed or removed, item) events.
fn session_events(dev: &mut TestDevice) -> Vec<NodeEvent> {
    drain(dev)
        .into_iter()
        .filter(|e| matches!(e, NodeEvent::AgentSession { .. } | NodeEvent::AgentSessionItem { .. }))
        .collect()
}

fn session_requests(pc: &TestDevice) -> usize {
    pc.platform
        .agent_requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, r)| {
            matches!(
                r,
                g7link_core::AgentRequest::Sessions
                    | g7link_core::AgentRequest::SessionRead { .. }
                    | g7link_core::AgentRequest::SessionContinue { .. }
            )
        })
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_need_both_switches() {
    let mut phone = follow_phone("Pixel").await;
    let mut pc = sessions_pc_with(&mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    assert!(pc.node.device_toggles(phone_id).unwrap().contains(&("agent_sessions", false)), "off by default");

    // `agents` off wins: its own prompt, not the sessions one.
    assert!(matches!(phone.node.agent_sessions(pc_id).await, Err(Error::Denied)));
    wait_for(&mut pc, "agents prompt", |e| match e {
        NodeEvent::AgentsRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert!(!drain(&mut pc).iter().any(|e| matches!(e, NodeEvent::AgentSessionsRequested { .. })));

    // `agents` on, `agent_sessions` off: the second prompt, once.
    allow_agents(&pc, &mut phone).await;
    assert!(matches!(phone.node.agent_sessions(pc_id).await, Err(Error::Denied)));
    wait_for(&mut pc, "sessions prompt", |e| match e {
        NodeEvent::AgentSessionsRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert!(matches!(
        phone.node.agent_session_read(pc_id, FOLLOWED_SESSION.into(), None, 50).await,
        Err(Error::Denied)
    ));
    assert!(matches!(
        phone.node.agent_session_watch(pc_id, Some(FOLLOWED_SESSION.into())).await,
        Err(Error::Denied)
    ));
    assert!(
        !drain(&mut pc).iter().any(|e| matches!(e, NodeEvent::AgentSessionsRequested { .. })),
        "the prompt shows once"
    );
    // Nothing is pushed to it meanwhile; threads still work.
    pc.node.agent_session(Some(agent_session(FOLLOWED_SESSION, "Changed")), None).await.unwrap();
    assert!(with_timeout("threads", phone.node.agent_threads(pc_id)).await.is_ok());
    assert!(session_events(&mut phone).is_empty());
    assert_eq!(session_requests(&pc), 0, "none reach the platform");

    pc.node.set_device_toggle(phone_id, "agent_sessions", true).unwrap();
    let sessions = with_timeout("sessions", phone.node.agent_sessions(pc_id)).await.unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].title, "Fix the login test");

    // A PC that doesn't list sessions says so.
    let mut plain = device_with("Laptop", DeviceKind::Desktop, &[g7link_core::agent::HOST]).await;
    pair_qr(&mut plain, &mut phone).await;
    plain.node.set_device_toggle(phone_id, "agents", true).unwrap();
    plain.node.set_device_toggle(phone_id, "agent_sessions", true).unwrap();
    let plain_id = plain.node.device_id();
    assert!(matches!(phone.node.agent_sessions(plain_id).await, Err(Error::Unsupported)));
    match phone
        .node
        .debug_agent_request(plain_id, "agent.sessions", &g7link_core::agents::AgentWatch::default())
        .await
    {
        Err(Error::Unsupported) => {}
        other => panic!("not unsupported: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_lists_follows_and_continues_a_session() {
    let mut phone = follow_phone("Pixel").await;
    let pc = sessions_pc_with(&mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    allow_sessions(&pc, &mut phone).await;

    let sessions = with_timeout("sessions", phone.node.agent_sessions(pc_id)).await.unwrap();
    assert_eq!(sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), [FOLLOWED_SESSION, OPEN_SESSION]);
    assert_eq!(sessions[0].since, Some(1_795_000_000_000));

    with_timeout("watch", phone.node.agent_session_watch(pc_id, Some(FOLLOWED_SESSION.into())))
        .await
        .unwrap();
    let page = with_timeout("read", phone.node.agent_session_read(pc_id, FOLLOWED_SESSION.into(), None, 50))
        .await
        .unwrap();
    assert_eq!(page.session.id, FOLLOWED_SESSION);
    assert_eq!(page.items.iter().map(|i| i.seq).collect::<Vec<_>>(), [0, 5 << 6], "seqs from the file");
    assert!(pc.platform.agent_requests.lock().unwrap().iter().any(|(from, r)| *from == phone_id
        && *r == g7link_core::AgentRequest::SessionWatch { session: Some(FOLLOWED_SESSION.into()) }));

    // A finished step arrives, then the session changes and leaves.
    pc.node.agent_session_item(FOLLOWED_SESSION.into(), agent_text_at(9 << 6, "Done.")).await.unwrap();
    let (session, item) = wait_for(&mut phone, "session item", |e| match e {
        NodeEvent::AgentSessionItem { device, session, item } if *device == pc_id => {
            Some((session.clone(), item.clone()))
        }
        _ => None,
    })
    .await;
    assert_eq!((session.as_str(), item.seq, item.text.as_deref()), (FOLLOWED_SESSION, 9 << 6, Some("Done.")));

    // Continue: refused while open, a thread once it isn't.
    match phone.node.agent_session_continue(pc_id, OPEN_SESSION.into()).await {
        Err(Error::Protocol(reason)) => assert!(reason.starts_with("BUSY"), "{reason}"),
        other => panic!("an open session is the generic failure, not {other:?}"),
    }
    let thread = with_timeout("continue", phone.node.agent_session_continue(pc_id, FOLLOWED_SESSION.into()))
        .await
        .unwrap();
    assert_eq!(thread.id, AGENT_THREAD);
    assert!(thread.from_phone);
    assert!(pc.platform.agent_requests.lock().unwrap().iter().any(|(from, r)| *from == phone_id
        && *r == g7link_core::AgentRequest::SessionContinue { session: FOLLOWED_SESSION.into() }));
    pc.node.agent_session(None, Some(FOLLOWED_SESSION.into())).await.unwrap();
    let removed = wait_for(&mut phone, "removed", |e| match e {
        NodeEvent::AgentSession { session: None, removed: Some(id), .. } => Some(id.clone()),
        _ => None,
    })
    .await;
    assert_eq!(removed, FOLLOWED_SESSION);
    // The watch went with it: the platform is told, and no more items come.
    let deadline = tokio::time::Instant::now() + WAIT;
    while !pc
        .platform
        .agent_requests
        .lock()
        .unwrap()
        .iter()
        .any(|(from, r)| *from == phone_id && *r == g7link_core::AgentRequest::SessionWatch { session: None })
    {
        assert!(tokio::time::Instant::now() < deadline, "the platform wasn't told the watch ended");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Bad ids never leave the phone, and the PC refuses them.
    assert!(matches!(
        phone.node.agent_session_continue(pc_id, "../etc".into()).await,
        Err(Error::Protocol(_))
    ));
    let bad = g7link_core::agents::AgentSessionContinue { session: "/home/me".into() };
    match phone.node.debug_agent_request(pc_id, "agent.session.continue", &bad).await {
        Err(Error::Protocol(reason)) => assert!(reason.contains("BAD_MESSAGE"), "{reason}"),
        other => panic!("a path is a bad message, not {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_watching_phone_gets_session_items() {
    let mut watcher = follow_phone("Pixel").await;
    let mut other = follow_phone("Galaxy").await;
    let mut pc = sessions_pc_with(&mut watcher).await;
    pair_qr(&mut pc, &mut other).await;
    let pc_id = pc.node.device_id();
    allow_sessions(&pc, &mut watcher).await;
    allow_sessions(&pc, &mut other).await;

    with_timeout("watch", watcher.node.agent_session_watch(pc_id, Some(FOLLOWED_SESSION.into())))
        .await
        .unwrap();
    with_timeout("watch other", other.node.agent_session_watch(pc_id, Some(OTHER_SESSION.into())))
        .await
        .unwrap();
    pc.node.agent_session_item(FOLLOWED_SESSION.into(), agent_text_at(0, "Hi")).await.unwrap();
    pc.node.agent_session(Some(agent_session(FOLLOWED_SESSION, "Changed")), None).await.unwrap();

    // The item went first, on the same stream: by the change, it's there or never comes.
    assert_eq!(session_events_until_change(&mut watcher).await, ["item", "change"], "the watcher gets it");
    assert_eq!(
        session_events_until_change(&mut other).await,
        ["change"],
        "the other watches another session"
    );
}

/// The §3 events `dev` gets up to and with the first session change.
async fn session_events_until_change(dev: &mut TestDevice) -> Vec<&'static str> {
    let mut seen = Vec::new();
    loop {
        let kind = wait_for(dev, "session event", |e| match e {
            NodeEvent::AgentSessionItem { .. } => Some("item"),
            NodeEvent::AgentSession { session: Some(_), .. } => Some("change"),
            _ => None,
        })
        .await;
        seen.push(kind);
        if kind == "change" {
            return seen;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn session_updates_are_limited_per_session() {
    let mut phone = follow_phone("Pixel").await;
    let pc = sessions_pc_with(&mut phone).await;
    allow_sessions(&pc, &mut phone).await;

    let started = tokio::time::Instant::now();
    for i in 0..10 {
        pc.node
            .agent_session(Some(agent_session(FOLLOWED_SESSION, &format!("Step {i}"))), None)
            .await
            .unwrap();
    }
    pc.node.agent_session(Some(agent_session(OTHER_SESSION, "Elsewhere")), None).await.unwrap();

    let mut followed = Vec::new();
    let mut other = Vec::new();
    while followed.last().map(String::as_str) != Some("Step 9") {
        let (id, title) = wait_for(&mut phone, "session", |e| match e {
            NodeEvent::AgentSession { session: Some(s), .. } => Some((s.id.clone(), s.title.clone())),
            _ => None,
        })
        .await;
        if id == FOLLOWED_SESSION { followed.push(title) } else { other.push(title) }
    }
    let waited = started.elapsed();
    assert_eq!(followed, ["Step 0", "Step 9"], "the first at once, then only the newest");
    assert!(waited >= Duration::from_millis(1_800), "the newest waited its turn ({waited:?})");
    // Another session isn't held back by this one.
    other.extend(session_events(&mut phone).into_iter().filter_map(|e| match e {
        NodeEvent::AgentSession { session: Some(s), .. } => Some(s.title),
        _ => None,
    }));
    assert_eq!(other, ["Elsewhere"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_sessions_off_ends_the_watch() {
    let mut phone = follow_phone("Pixel").await;
    let pc = sessions_pc_with(&mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    allow_sessions(&pc, &mut phone).await;
    with_timeout("watch", phone.node.agent_session_watch(pc_id, Some(FOLLOWED_SESSION.into())))
        .await
        .unwrap();

    pc.node.set_device_toggle(phone_id, "agent_sessions", false).unwrap();
    let deadline = tokio::time::Instant::now() + WAIT;
    while !pc
        .platform
        .agent_requests
        .lock()
        .unwrap()
        .iter()
        .any(|(from, r)| *from == phone_id && *r == g7link_core::AgentRequest::SessionWatch { session: None })
    {
        assert!(tokio::time::Instant::now() < deadline, "the platform wasn't told the watch ended");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    pc.node.agent_session_item(FOLLOWED_SESSION.into(), agent_text_at(0, "Hi")).await.unwrap();
    pc.node.agent_session(Some(agent_session(FOLLOWED_SESSION, "Changed")), None).await.unwrap();
    assert!(matches!(phone.node.agent_sessions(pc_id).await, Err(Error::Denied)));
    assert!(session_events(&mut phone).is_empty(), "nothing after the switch went off");

    // Back on, the watch is gone: no items until the phone watches again.
    pc.node.set_device_toggle(phone_id, "agent_sessions", true).unwrap();
    pc.node.agent_session_item(FOLLOWED_SESSION.into(), agent_text_at(1 << 6, "Again")).await.unwrap();
    assert!(with_timeout("sessions", phone.node.agent_sessions(pc_id)).await.is_ok());
    assert!(!session_events(&mut phone).iter().any(|e| matches!(e, NodeEvent::AgentSessionItem { .. })));

    // `agents` going off ends it too.
    with_timeout("watch again", phone.node.agent_session_watch(pc_id, Some(FOLLOWED_SESSION.into())))
        .await
        .unwrap();
    pc.node.set_device_toggle(phone_id, "agents", false).unwrap();
    wait_for(&mut phone, "last, empty catalog", |e| match e {
        NodeEvent::AgentCatalog { device, catalog: None, .. } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    pc.node.agent_session_item(FOLLOWED_SESSION.into(), agent_text_at(2 << 6, "More")).await.unwrap();
    assert!(matches!(phone.node.agent_threads(pc_id).await, Err(Error::Denied)));
    assert!(session_events(&mut phone).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_never_carries_a_path() {
    let mut phone = follow_phone("Pixel").await;
    let pc = sessions_pc_with(&mut phone).await;
    let pc_id = pc.node.device_id();
    allow_sessions(&pc, &mut phone).await;

    let pathed = g7link_core::AgentSession {
        folder: "/home/me/code/secret-project".into(),
        ..agent_session(FOLLOWED_SESSION, "Fix it")
    };
    *pc.platform.agent_sessions.lock().unwrap() = vec![pathed.clone()];
    let listed = with_timeout("sessions", phone.node.agent_sessions(pc_id)).await.unwrap();
    assert_eq!(listed[0].folder, "secret-project");
    let page = with_timeout("read", phone.node.agent_session_read(pc_id, FOLLOWED_SESSION.into(), None, 10))
        .await
        .unwrap();
    assert_eq!(page.session.folder, "secret-project");
    pc.node.agent_session(Some(pathed), None).await.unwrap();
    let pushed = wait_for(&mut phone, "session", |e| match e {
        NodeEvent::AgentSession { session: Some(s), .. } => Some(s.clone()),
        _ => None,
    })
    .await;
    assert_eq!(pushed.folder, "secret-project");
    assert!(!format!("{pushed:?}").contains("/home"));

    // A guess is never the agent's own "needs you".
    let guessed = g7link_core::AgentSession {
        state: "needs_you".into(),
        guess: true,
        ..agent_session(OTHER_SESSION, "Maybe")
    };
    pc.node.agent_session(Some(guessed), None).await.unwrap();
    let state = wait_for(&mut phone, "guessed", |e| match e {
        NodeEvent::AgentSession { session: Some(s), .. } if s.id == OTHER_SESSION => Some(s.state.clone()),
        _ => None,
    })
    .await;
    assert_eq!(state, "may_need_you");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_at_its_usage_limit_says_so_on_the_phone() {
    let mut phone = follow_phone("Pixel").await;
    let pc = sessions_pc_with(&mut phone).await;
    let pc_id = pc.node.device_id();
    allow_sessions(&pc, &mut phone).await;

    let limit = g7link_core::AgentSessionLimit { resets_ms: Some(1_795_010_000_000) };
    let limited = g7link_core::AgentSession {
        state: "idle".into(),
        limit: Some(limit),
        ..agent_session(FOLLOWED_SESSION, "Fix it")
    };
    *pc.platform.agent_sessions.lock().unwrap() =
        vec![limited.clone(), agent_session(OTHER_SESSION, "Still going")];
    let listed = with_timeout("sessions", phone.node.agent_sessions(pc_id)).await.unwrap();
    assert_eq!(listed[0].limit, Some(limit));
    assert_eq!(listed[1].limit, None);
    let page = with_timeout("read", phone.node.agent_session_read(pc_id, FOLLOWED_SESSION.into(), None, 10))
        .await
        .unwrap();
    assert_eq!(page.session.limit, Some(limit));
    pc.node.agent_session(Some(limited), None).await.unwrap();
    let pushed = wait_for(&mut phone, "limited session", |e| match e {
        NodeEvent::AgentSession { session: Some(s), .. } if s.id == FOLLOWED_SESSION => Some(s.clone()),
        _ => None,
    })
    .await;
    assert_eq!((pushed.state.as_str(), pushed.limit), ("idle", Some(limit)));
}

// ---- Media: large artwork, icons, rate and age (docs/protocol/media.md §2.1-2.3) ----

/// A JPEG's first bytes, then filler, `len` bytes in all.
fn jpeg(len: usize) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0];
    bytes.resize(len, 7);
    bytes
}

fn png(len: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(len, 3);
    bytes
}

const ICON_KEY: &str = "00112233aabbccdd";

/// A PC and a phone that see its media, the PC playing `player`.
async fn media_pair(playing: MediaPlayer) -> (TestDevice, TestDevice) {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let pc_id = pc.node.device_id();
    pc.node.media_changed(vec![playing]).await;
    wait_for(&mut phone, "pc media", |e| match e {
        NodeEvent::MediaChanged { device, players } if *device == pc_id && !players.is_empty() => Some(()),
        _ => None,
    })
    .await;
    (pc, phone)
}

#[tokio::test(flavor = "multi_thread")]
async fn media_art_is_fetched_once_and_bounded() {
    let small = jpeg(2_000);
    let shown = MediaPlayer {
        art_full: Some("big1".into()),
        ..player("spotify", "On the PC", Some(("small1", small)))
    };
    let (pc, mut phone) = media_pair(shown).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let large = jpeg(1_500_000);
    *pc.platform.media_art.lock().unwrap() = Some((large.clone(), g7link_core::ArtFormat::Jpeg));

    let (bytes, format) = with_timeout("art", phone.node.media_art(pc_id, "big1".into())).await.unwrap();
    assert_eq!((bytes.len(), format), (large.len(), g7link_core::ArtFormat::Jpeg));
    assert_eq!(bytes, large);
    assert_eq!(pc.platform.media_art_asks.load(Ordering::SeqCst), 1, "one fetch, one ask");
    // The state never carries it: only the small picture and the key.
    pc.node
        .media_changed(vec![MediaPlayer {
            art_full: Some("big1".into()),
            ..player("spotify", "Again", None)
        }])
        .await;
    let players = wait_for(&mut phone, "state", |e| match e {
        NodeEvent::MediaChanged { players, .. }
            if players.first().is_some_and(|p| p.title.as_deref() == Some("Again")) =>
        {
            Some(players.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(players[0].art_full.as_deref(), Some("big1"));
    assert!(players[0].art.as_ref().is_none_or(|a| a.len() < 256 * 1024));

    // A key no player names isn't asked of the platform.
    assert!(matches!(phone.node.media_art(pc_id, "other".into()).await, Err(Error::NotFound)));
    assert_eq!(pc.platform.media_art_asks.load(Ordering::SeqCst), 1);
    // Over 2 MiB, or not what it says it is, is never sent.
    *pc.platform.media_art.lock().unwrap() = Some((jpeg(3 * 1024 * 1024), g7link_core::ArtFormat::Jpeg));
    assert!(matches!(phone.node.media_art(pc_id, "big1".into()).await, Err(Error::Protocol(_))));
    *pc.platform.media_art.lock().unwrap() = Some((png(1_000), g7link_core::ArtFormat::Jpeg));
    assert!(matches!(phone.node.media_art(pc_id, "big1".into()).await, Err(Error::Protocol(_))));
    *pc.platform.media_art.lock().unwrap() = None;
    assert!(matches!(phone.node.media_art(pc_id, "big1".into()).await, Err(Error::NotFound)));
    // Media off for the phone: refused.
    *pc.platform.media_art.lock().unwrap() = Some((large, g7link_core::ArtFormat::Jpeg));
    pc.node.set_device_toggle(phone_id, "media", false).unwrap();
    assert!(matches!(phone.node.media_art(pc_id, "big1".into()).await, Err(Error::Denied)));
}

#[tokio::test(flavor = "multi_thread")]
async fn media_icon_needs_the_media_toggle() {
    let shown = MediaPlayer { icon: Some(ICON_KEY.into()), ..player("spotify", "On the PC", None) };
    let (pc, phone) = media_pair(shown).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let icon = png(900);
    *pc.platform.media_icon.lock().unwrap() = Some(icon.clone());

    assert_eq!(with_timeout("icon", phone.node.media_icon(pc_id, ICON_KEY.into())).await.unwrap(), icon);
    assert!(matches!(phone.node.media_icon(pc_id, "ffffffffffffffff".into()).await, Err(Error::NotFound)));
    assert!(matches!(phone.node.media_icon(pc_id, "NOT-HEX".into()).await, Err(Error::Protocol(_))));
    // Too large, or not a PNG: not sent.
    *pc.platform.media_icon.lock().unwrap() = Some(png(17 * 1024));
    assert!(matches!(phone.node.media_icon(pc_id, ICON_KEY.into()).await, Err(Error::Protocol(_))));
    *pc.platform.media_icon.lock().unwrap() = Some(icon.clone());

    pc.node.set_device_toggle(phone_id, "media", false).unwrap();
    assert!(matches!(phone.node.media_icon(pc_id, ICON_KEY.into()).await, Err(Error::Denied)));
    pc.node.set_device_toggle(phone_id, "media", true).unwrap();
    phone.node.set_device_toggle(pc_id, "media", false).unwrap();
    assert!(matches!(phone.node.media_icon(pc_id, ICON_KEY.into()).await, Err(Error::Denied)));
}

#[tokio::test(flavor = "multi_thread")]
async fn position_uses_rate_and_age() {
    let playing = MediaPlayer {
        rate_milli: Some(1_500),
        age: Some(200),
        shuffle: Some(true),
        repeat: Some("track".into()),
        ..player("spotify", "Fast", None)
    };
    let (pc, mut phone) = media_pair(playing.clone()).await;
    let pc_id = pc.node.device_id();
    pc.node.media_changed(vec![MediaPlayer { title: Some("Faster".into()), ..playing }]).await;
    let got = wait_for(&mut phone, "state", |e| match e {
        NodeEvent::MediaChanged { device, players } if *device == pc_id => {
            players.iter().find(|p| p.title.as_deref() == Some("Faster")).cloned()
        }
        _ => None,
    })
    .await;
    assert_eq!(got.rate_milli, Some(1_500));
    assert_eq!((got.shuffle, got.repeat.as_deref()), (Some(true), Some("track")));
    let age = got.age.unwrap();
    assert!((200..5_000).contains(&age), "the sender's own age, and the wait before sending ({age})");
    let expected = 12_000 + (age + 1_000) * 3 / 2;
    assert_eq!(got.position_at(1_000), Some(expected));
    assert_eq!(got.position_at(10_000_000), Some(200_000), "never past the end");

    // A rate out of range is dropped before it's sent.
    pc.node
        .media_changed(vec![MediaPlayer { rate_milli: Some(9_000), ..player("spotify", "Silly", None) }])
        .await;
    let silly = wait_for(&mut phone, "state", |e| match e {
        NodeEvent::MediaChanged { players, .. } => {
            players.iter().find(|p| p.title.as_deref() == Some("Silly")).cloned()
        }
        _ => None,
    })
    .await;
    assert_eq!(silly.rate_milli, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn shuffle_and_loop_need_a_value() {
    let (pc, phone) = media_pair(player("spotify", "On the PC", None)).await;
    let pc_id = pc.node.device_id();

    with_timeout(
        "loop",
        phone.node.media_command_value(pc_id, "spotify".into(), MediaAction::Loop, "track".into()),
    )
    .await
    .unwrap();
    with_timeout(
        "shuffle",
        phone.node.media_command_value(pc_id, "spotify".into(), MediaAction::Shuffle, "off".into()),
    )
    .await
    .unwrap();
    assert_eq!(
        *pc.platform.media_values.lock().unwrap(),
        [
            ("spotify".to_owned(), MediaAction::Loop, "track".to_owned()),
            ("spotify".to_owned(), MediaAction::Shuffle, "off".to_owned())
        ]
    );
    // Without a value, the PC refuses it as a bad message.
    match phone.node.media_command(pc_id, "spotify".into(), MediaAction::Shuffle, None).await {
        Err(Error::Protocol(reason)) => assert!(reason.contains("BAD_MESSAGE"), "{reason}"),
        other => panic!("not a bad message: {other:?}"),
    }
    // A value the action doesn't take never leaves the phone.
    assert!(matches!(
        phone.node.media_command_value(pc_id, "spotify".into(), MediaAction::Loop, "forever".into()).await,
        Err(Error::Protocol(_))
    ));
    assert_eq!(pc.platform.media_values.lock().unwrap().len(), 2);
    assert_eq!(pc.platform.media.lock().unwrap().len(), 2, "the refused ones never reached the player");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_exports_a_thread_bigger_than_a_control_message() {
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    let mut pc = agents_pc_with(&mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    // Off: refused, and the PC's user is asked once.
    assert!(matches!(
        phone.node.agent_export(pc_id, Some(AGENT_THREAD.into()), None).await,
        Err(Error::Denied)
    ));
    wait_for(&mut pc, "agents prompt", |e| match e {
        NodeEvent::AgentsRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    assert!(pc.platform.agent_requests.lock().unwrap().is_empty());

    allow_agents(&pc, &mut phone).await;
    let export = with_timeout("export", phone.node.agent_export(pc_id, Some(AGENT_THREAD.into()), None))
        .await
        .unwrap();
    assert_eq!(export.text, big_export("List the files here."));
    assert!(export.text.len() > 1024 * 1024, "more than a control message carries");
    assert!(!export.truncated);
    assert_eq!(export.file_name, "claude-list-the-files-here-2026-11-19.md");
    assert!(pc.platform.agent_requests.lock().unwrap().iter().any(|(from, r)| *from == phone_id
        && *r == g7link_core::AgentRequest::Export { thread: Some(AGENT_THREAD.into()), session: None }));

    // The PC keeps what it sends within the limits.
    let big = with_timeout("big export", phone.node.agent_export(pc_id, Some(BUSY_THREAD.into()), None))
        .await
        .unwrap();
    assert!(big.truncated && big.text.len() == g7link_core::agent::MAX_EXPORT_BYTES);
    assert!(big.text.starts_with("# Big\n"), "no controls");
    assert_eq!(big.file_name, "-home-me-big.md");

    assert!(matches!(
        phone.node.agent_export(pc_id, Some("00000000000000000000000000000000".into()), None).await,
        Err(Error::NotFound)
    ));
    // One of the two, never neither: the phone doesn't send it, and the PC refuses it.
    assert!(matches!(phone.node.agent_export(pc_id, None, None).await, Err(Error::Protocol(_))));
    match phone.node.debug_agent_export(pc_id, g7link_core::agents::AgentExportRequest::default()).await {
        Err(Error::Protocol(reason)) => assert!(reason.contains("BAD_MESSAGE"), "{reason}"),
        other => panic!("neither is a bad message, not {other:?}"),
    }
    let pathed = g7link_core::agents::AgentExportRequest { thread: Some("../etc".into()), session: None };
    match phone.node.debug_agent_export(pc_id, pathed).await {
        Err(Error::Protocol(reason)) => assert!(reason.contains("BAD_MESSAGE"), "{reason}"),
        other => panic!("a path is a bad message, not {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn exporting_a_session_needs_both_switches() {
    let mut phone = follow_phone("Pixel").await;
    let mut pc = sessions_pc_with(&mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    allow_agents(&pc, &mut phone).await;

    assert!(matches!(
        phone.node.agent_export(pc_id, None, Some(FOLLOWED_SESSION.into())).await,
        Err(Error::Denied)
    ));
    wait_for(&mut pc, "sessions prompt", |e| match e {
        NodeEvent::AgentSessionsRequested { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    // A thread needs only `agents`.
    assert!(
        with_timeout("thread export", phone.node.agent_export(pc_id, Some(AGENT_THREAD.into()), None))
            .await
            .is_ok()
    );

    pc.node.set_device_toggle(phone_id, "agent_sessions", true).unwrap();
    let export =
        with_timeout("session export", phone.node.agent_export(pc_id, None, Some(FOLLOWED_SESSION.into())))
            .await
            .unwrap();
    assert!(export.text.starts_with("# Fix the login test\n"));
    assert_eq!(export.text.len(), big_export("Fix the login test").len());
    assert!(matches!(
        phone.node.agent_export(pc_id, None, Some(OTHER_SESSION.into())).await,
        Err(Error::NotFound)
    ));
    // Never both: the phone doesn't send it, and the PC refuses it.
    assert!(matches!(
        phone.node.agent_export(pc_id, Some(AGENT_THREAD.into()), Some(FOLLOWED_SESSION.into())).await,
        Err(Error::Protocol(_))
    ));
    let both = g7link_core::agents::AgentExportRequest {
        thread: Some(AGENT_THREAD.into()),
        session: Some(FOLLOWED_SESSION.into()),
    };
    match phone.node.debug_agent_export(pc_id, both).await {
        Err(Error::Protocol(reason)) => assert!(reason.contains("BAD_MESSAGE"), "{reason}"),
        other => panic!("both is a bad message, not {other:?}"),
    }

    // A PC that doesn't list sessions doesn't export them.
    let mut plain = device_with("Laptop", DeviceKind::Desktop, &[g7link_core::agent::HOST]).await;
    pair_qr(&mut plain, &mut phone).await;
    let plain_id = plain.node.device_id();
    assert!(matches!(
        phone.node.agent_export(plain_id, None, Some(FOLLOWED_SESSION.into())).await,
        Err(Error::Unsupported)
    ));
}

// ---- Agents: control (docs/protocol/agents.md §5) ----

/// A queued message the PC already sent.
const SENT_QUEUED: &str = "ffffffffffffffff";
const QUEUED_ID: &str = "0123456789abcdef";
/// An image whose thumbnail the PC has, and one whose thumbnail is too large.
const AGENT_IMAGE: &str = "aaaabbbbccccddddeeeeffff00001111";
const BIG_IMAGE: &str = "bbbbccccddddeeeeffff000011112222";
const QUESTION_TOKEN: &str = "ffff0000ffff0000ffff0000ffff0000";

/// A catalog whose agent reports models, what it can do and its limits.
fn control_catalog() -> g7link_core::AgentCatalog {
    let model = |id: &str, short: bool| g7link_core::AgentModel {
        id: id.into(),
        name: id.to_uppercase(),
        note: Some(format!("A model. {}", "More words. ".repeat(20))),
        is_default: id == "default",
        efforts: vec!["low".into(), "high".into()],
        default_effort: Some("high".into()),
        images: true,
        short,
        exact: (id != "default").then(|| "Opus 4.5".to_owned()),
    };
    let mut catalog = agent_catalog();
    catalog.agents[0].models =
        vec![model("default", true), model("opus", true), model("claude-opus-4-5", false)];
    catalog.agents[0].can =
        ["plan", "questions", "steer", "images", "commands", "teleport"].map(String::from).to_vec();
    catalog.agents[0].limits = vec![g7link_core::AgentLimit {
        window: "five_hour".into(),
        used_pct: Some(41),
        resets: Some(1_795_010_000_000),
        state: "ok".into(),
    }];
    catalog
}

/// A phone that offers §5 (as the G7 Link app does).
async fn control_phone(name: &'static str) -> TestDevice {
    device_with(name, DeviceKind::Phone, &[g7link_core::agent::ITEMS2, g7link_core::agent::CONTROL]).await
}

/// A PC whose agent host offers §5, with [`control_catalog`], paired with `phone`.
async fn control_pc_with(phone: &mut TestDevice) -> TestDevice {
    let caps = [g7link_core::agent::HOST, g7link_core::agent::CONTROL];
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &caps).await;
    pc.node.set_agent_catalog(Some(control_catalog())).await.unwrap();
    pair_qr(&mut pc, phone).await;
    pc
}

/// A thread in plan mode with everything §5 added.
fn control_thread() -> g7link_core::AgentThread {
    let mut thread = agent_thread(AGENT_THREAD, "working", false);
    thread.mode = "plan".into();
    thread.model = Some("opus".into());
    thread.effort = Some("high".into());
    thread.answered_model = Some("claude-sonnet-4-5".into());
    thread.queued = vec![g7link_core::AgentQueued {
        id: QUEUED_ID.into(),
        text: "Then run the tests.".into(),
        by_name: Some("Pixel".into()),
        created: 1_795_000_000_000,
        ..Default::default()
    }];
    thread.context = Some(g7link_core::AgentContext { used: 120_000, max: 1_000_000 });
    thread.mcp = Some(g7link_core::AgentMcp { on: true, servers: 3, connected: 2, need_login: 1 });
    thread.asking = 1;
    thread
}

/// A plan, a question and the plan file's edit, as the PC's host writes them.
fn control_items() -> Vec<g7link_core::AgentItem> {
    use g7link_core::{AgentItem, AgentTool, agent};
    vec![
        AgentItem {
            via: Some(agent::via::STEER.into()),
            images: vec![g7link_core::AgentImage {
                id: AGENT_IMAGE.into(),
                name: "/home/me/Pictures/error.png".into(),
                bytes: 2_000,
                w: 10,
                h: 10,
            }],
            kind: agent::item::USER.into(),
            ..agent_text(1, "And the logout.")
        },
        AgentItem {
            kind: agent::item::TOOL.into(),
            text: None,
            tool: Some(AgentTool {
                name: "Write".into(),
                title: "Write /home/me/.claude/plans/fix-login.md".into(),
                detail: Some("# Secret plan".into()),
                status: "ok".into(),
                class: Some(agent::tool_class::PLAN.into()),
                path: Some("fix-login.md".into()),
                ..Default::default()
            }),
            ..agent_text(2, "")
        },
        AgentItem {
            kind: agent::item::PLAN.into(),
            plan_state: Some(agent::plan_state::PROPOSED.into()),
            ..agent_text(3, "1. Read the code\n2. Fix it")
        },
        AgentItem {
            kind: agent::item::QUESTION.into(),
            question: Some(g7link_core::AgentItemQuestion {
                count: 1,
                secret: false,
                outcome: Some("answered".into()),
                by_name: Some("Pixel".into()),
            }),
            ..agent_text(4, "Auth method: OAuth")
        },
    ]
}

fn control_question(secret: bool) -> g7link_core::AgentQuestion {
    g7link_core::AgentQuestion {
        token: QUESTION_TOKEN.into(),
        thread: AGENT_THREAD.into(),
        thread_title: "List the files here.".into(),
        agent: "codex".into(),
        item: "q:4".into(),
        secret,
        asks: vec![g7link_core::AgentQuestionAsk {
            id: "0".into(),
            header: "Auth method".into(),
            text: "Which sign-in should the app use?".into(),
            other: true,
            secret,
            options: vec![
                g7link_core::AgentQuestionOption { label: "OAuth".into(), note: None },
                g7link_core::AgentQuestionOption { label: "Password".into(), note: None },
            ],
            ..Default::default()
        }],
        created: 1_795_000_000_000,
        expires: 1_795_007_200_000,
    }
}

fn last_request(pc: &TestDevice) -> g7link_core::AgentRequest {
    pc.platform.agent_requests.lock().unwrap().last().cloned().unwrap().1
}

fn requests(pc: &TestDevice) -> usize {
    pc.platform.agent_requests.lock().unwrap().len()
}

/// `result` is the PC's refusal with `code`.
fn refused_with<T: std::fmt::Debug>(result: Result<T, Error>, code: &str) {
    match result {
        Err(Error::Protocol(reason)) => assert!(reason.contains(code), "{reason}"),
        Err(Error::Denied) if code == "DENIED" => {}
        Err(Error::Unsupported) if code == "UNSUPPORTED" => {}
        Err(Error::NotFound) if code == "NOT_FOUND" => {}
        other => panic!("not refused with {code}: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_with_control_picks_models_and_plans() {
    use g7link_core::{AgentConfigure, AgentRequest, agent};
    let mut phone = control_phone("Pixel").await;
    let pc = control_pc_with(&mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.set_device_toggle(phone_id, "agents", true).unwrap();
    let (catalog, can_control) = wait_for(&mut phone, "catalog", |e| match e {
        NodeEvent::AgentCatalog { device, catalog: Some(c), can_control, .. } if *device == pc_id => {
            Some((c.clone(), *can_control))
        }
        _ => None,
    })
    .await;
    assert!(can_control && phone.node.agent_can_control(pc_id));
    let claude = &catalog.agents[0];
    assert_eq!(claude.modes, ["auto", "ask", "accept_edits", "plan", "full"], "every mode the PC offers");
    assert_eq!(
        catalog.folders[0].modes,
        [g7link_core::AgentFolderMode { agent: "claude".into(), mode: "full".into() }],
        "the mode the folder remembers"
    );
    assert_eq!(claude.models.len(), 3);
    assert!(claude.models.iter().all(|m| m.note.as_ref().unwrap().len() <= agent::MAX_MODEL_NOTE_BYTES));
    assert_eq!(claude.can, ["plan", "questions", "steer", "images", "commands"], "a later word goes");
    assert_eq!(claude.limits[0].used_pct, Some(41));

    let start = g7link_core::AgentStart {
        model: Some("opus".into()),
        effort: Some("high".into()),
        ..agent_start("Plan the login fix", "plan")
    };
    with_timeout("start", phone.node.agent_start(pc_id, start)).await.unwrap();
    match last_request(&pc) {
        AgentRequest::Start { mode, model, effort, custom_model, .. } => {
            assert_eq!(
                (mode.as_str(), model.as_deref(), effort.as_deref()),
                ("plan", Some("opus"), Some("high"))
            );
            assert!(!custom_model);
        }
        other => panic!("{other:?}"),
    }
    // Full and the old word for accept_edits go through; the host is the judge.
    for (sent, forwarded) in [("full", "full"), ("auto_edit", "accept_edits")] {
        with_timeout("start", phone.node.agent_start(pc_id, agent_start("hi", sent))).await.unwrap();
        match last_request(&pc) {
            AgentRequest::Start { mode, .. } => assert_eq!(mode, forwarded),
            other => panic!("{other:?}"),
        }
    }
    let before = requests(&pc);
    let flag = g7link_core::AgentStart { model: Some("--bare".into()), ..agent_start("hi", "ask") };
    assert!(matches!(phone.node.agent_start(pc_id, flag.clone()).await, Err(Error::Protocol(_))));
    refused_with(phone.node.debug_agent_request(pc_id, "agent.start", &flag).await, "BAD_MESSAGE");

    let plan =
        AgentConfigure { thread: AGENT_THREAD.into(), mode: Some("plan".into()), ..Default::default() };
    let thread = with_timeout("configure", phone.node.agent_configure(pc_id, plan)).await.unwrap();
    assert_eq!(thread.mode, "plan");
    let typed = AgentConfigure {
        thread: AGENT_THREAD.into(),
        model: Some("my-own-model".into()),
        custom_model: true,
        ..Default::default()
    };
    let thread = with_timeout("configure", phone.node.agent_configure(pc_id, typed)).await.unwrap();
    assert_eq!(thread.model.as_deref(), Some("my-own-model"));
    let full =
        AgentConfigure { thread: AGENT_THREAD.into(), mode: Some("full".into()), ..Default::default() };
    let thread = with_timeout("configure", phone.node.agent_configure(pc_id, full)).await.unwrap();
    assert_eq!(thread.mode, "full", "a phone may set Full");
    assert_eq!(requests(&pc), before + 3, "bad messages never reach the host");

    // A thread carries what §5 added, to a phone that takes it.
    pc.node.agent_event(g7link_core::AgentEvent::state(control_thread()), None).await.unwrap();
    let state = wait_for(&mut phone, "state", |e| match e {
        NodeEvent::AgentEvent { event, .. } if event.kind == "state" => event.state.clone(),
        _ => None,
    })
    .await;
    assert_eq!(state.mode, "plan");
    assert_eq!(state.queued.len(), 1);
    assert_eq!(state.queued[0].by_name.as_deref(), Some("Pixel"));
    assert_eq!(state.mcp, Some(g7link_core::AgentMcp { on: true, servers: 3, connected: 2, need_login: 1 }));
    assert_eq!((state.asking, state.answered_model.as_deref()), (1, Some("claude-sonnet-4-5")));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_older_phone_gets_what_it_got_before() {
    use g7link_core::agent;
    // A 0.1.16 phone: it shows turns and thinking, not §5.
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[agent::ITEMS2]).await;
    let mut pc = control_pc_with(&mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.set_device_toggle(phone_id, "agents", true).unwrap();
    let (catalog, can_control) = wait_for(&mut phone, "catalog", |e| match e {
        NodeEvent::AgentCatalog { device, catalog: Some(c), can_control, .. } if *device == pc_id => {
            Some((c.clone(), *can_control))
        }
        _ => None,
    })
    .await;
    assert!(!can_control);
    let claude = &catalog.agents[0];
    assert!(claude.models.is_empty() && claude.can.is_empty() && claude.limits.is_empty());
    assert!(claude.modes.is_empty() && catalog.folders[0].modes.is_empty());

    with_timeout("watch", phone.node.agent_watch(pc_id, Some(AGENT_THREAD.into()))).await.unwrap();
    pc.node.agent_event(g7link_core::AgentEvent::state(control_thread()), None).await.unwrap();
    for item in control_items() {
        pc.node.agent_event(g7link_core::AgentEvent::item(AGENT_THREAD.into(), item), None).await.unwrap();
    }
    pc.node.agent_question(control_question(false)).await.unwrap();
    pc.node.agent_draft(AGENT_THREAD.into(), 2, None).await.unwrap();
    let state = wait_for(&mut phone, "state", |e| match e {
        NodeEvent::AgentEvent { event, .. } if event.kind == "state" => event.state.clone(),
        _ => None,
    })
    .await;
    // Sent as `supervised` (the old form, `for_legacy`); this build reads it as `ask`.
    assert_eq!(state.mode, "ask", "plan mode reads as supervised, then ask");
    assert!(state.queued.is_empty() && state.model.is_none() && state.mcp.is_none() && state.asking == 0);
    let mut items = Vec::new();
    while items.len() < 4 {
        items.push(
            wait_for(&mut phone, "item", |e| match e {
                NodeEvent::AgentEvent { event, .. } if event.kind == "item" => event.item.clone(),
                _ => None,
            })
            .await,
        );
    }
    assert!(items.iter().all(|i| agent::item::ITEMS2.contains(&i.kind.as_str())), "{items:?}");
    assert_eq!(items.iter().map(|i| i.seq).collect::<Vec<_>>(), [1, 2, 3, 4], "no gap");
    assert!(items[0].via.is_none() && items[0].images.is_empty());
    let plan_file = items[1].tool.as_ref().unwrap();
    assert_eq!(
        (plan_file.title.as_str(), plan_file.detail.as_ref(), plan_file.path.as_ref()),
        ("Plan", None, None)
    );
    assert_eq!(
        (items[2].kind.as_str(), items[2].text.as_deref()),
        ("assistant", Some("1. Read the code\n2. Fix it"))
    );
    assert_eq!(items[3].tool.as_ref().unwrap().name, "question");
    assert_eq!(items[3].tool.as_ref().unwrap().status, "ok", "it was answered");
    assert!(items[3].text.is_none() && items[3].question.is_none());

    // Its page is the same, without questions.
    pc.platform.agent_requests.lock().unwrap().clear();
    let page =
        with_timeout("read", phone.node.agent_read(pc_id, AGENT_THREAD.into(), None, 50)).await.unwrap();
    assert!(page.questions.is_empty());
    assert!(
        !drain(&mut phone)
            .iter()
            .any(|e| matches!(e, NodeEvent::AgentQuestion { .. } | NodeEvent::AgentDraft { .. }))
    );

    // What it may not ask for is refused as before; nothing reaches the host.
    let before = requests(&pc);
    refused_with(
        phone.node.debug_agent_request(pc_id, "agent.start", &agent_start("hi", "plan")).await,
        "UNSUPPORTED",
    );
    let configure = g7link_core::AgentConfigure {
        thread: AGENT_THREAD.into(),
        mode: Some("plan".into()),
        ..Default::default()
    };
    refused_with(phone.node.debug_agent_request(pc_id, "agent.configure", &configure).await, "UNSUPPORTED");
    refused_with(phone.node.agent_configure(pc_id, configure).await, "UNSUPPORTED");
    let steer = g7link_core::AgentSend {
        thread: AGENT_THREAD.into(),
        text: "Go on".into(),
        dispatch: Some("steer".into()),
        images: Vec::new(),
        key: None,
    };
    refused_with(phone.node.debug_agent_request(pc_id, "agent.send", &steer).await, "UNSUPPORTED");
    assert_eq!(requests(&pc), before);
    // What it always did still works.
    with_timeout("send", phone.node.agent_send(pc_id, AGENT_THREAD.into(), "Go on".into())).await.unwrap();
    assert!(
        with_timeout("start", phone.node.agent_start(pc_id, agent_start("hi", "supervised"))).await.is_ok()
    );
    drain(&mut pc);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_with_control_keeps_to_what_an_older_pc_does() {
    use g7link_core::agent;
    let mut phone = control_phone("Pixel").await;
    let pc = agents_pc_with(&mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node.set_device_toggle(phone_id, "agents", true).unwrap();
    let can_control = wait_for(&mut phone, "catalog", |e| match e {
        NodeEvent::AgentCatalog { device, catalog: Some(_), can_control, .. } if *device == pc_id => {
            Some(*can_control)
        }
        _ => None,
    })
    .await;
    assert!(!can_control && !phone.node.agent_can_control(pc_id));

    let configure = g7link_core::AgentConfigure {
        thread: AGENT_THREAD.into(),
        mode: Some("plan".into()),
        ..Default::default()
    };
    assert!(matches!(phone.node.agent_configure(pc_id, configure.clone()).await, Err(Error::Unsupported)));
    assert!(matches!(
        phone.node.agent_start(pc_id, agent_start("hi", "plan")).await,
        Err(Error::Unsupported)
    ));
    let steer = g7link_core::AgentSend {
        thread: AGENT_THREAD.into(),
        text: "Go on".into(),
        dispatch: Some("queue".into()),
        images: Vec::new(),
        key: None,
    };
    assert!(matches!(phone.node.agent_send_with(pc_id, steer).await, Err(Error::Unsupported)));
    assert!(matches!(
        phone.node.agent_image_send(pc_id, "a.jpg".into(), jpeg(100)).await,
        Err(Error::Unsupported)
    ));
    assert!(matches!(phone.node.agent_draft_get(pc_id, AGENT_THREAD.into()).await, Err(Error::Unsupported)));
    assert_eq!(requests(&pc), 0, "nothing was sent");
    // The PC says so too when asked anyway.
    refused_with(phone.node.debug_agent_request(pc_id, "agent.configure", &configure).await, "UNSUPPORTED");
    // What phase 1 had works as it did.
    with_timeout("send", phone.node.agent_send(pc_id, AGENT_THREAD.into(), "Go on".into())).await.unwrap();
    let thread =
        with_timeout("start", phone.node.agent_start(pc_id, agent_start("hi", "supervised"))).await.unwrap();
    assert_eq!(thread.mode, agent::mode::ASK);

    // The PC's host learns §5: the phone gets the catalog again, whole.
    pc.node.set_agent_catalog(Some(control_catalog())).await.unwrap();
    let models = wait_for(&mut phone, "the catalog without models", |e| match e {
        NodeEvent::AgentCatalog { device, catalog: Some(c), .. } if *device == pc_id => {
            Some(c.agents[0].models.len())
        }
        _ => None,
    })
    .await;
    assert_eq!(models, 0, "not before the PC offers it");
    let caps = [agent::HOST, agent::CONTROL, "media.control"].map(String::from).to_vec();
    pc.node.update_power(PowerLevel::NotApplicable, caps).await;
    let (catalog, can_control) = wait_for(&mut phone, "the whole catalog", |e| match e {
        NodeEvent::AgentCatalog { device, catalog: Some(c), can_control: true, .. }
            if *device == pc_id && !c.agents[0].models.is_empty() =>
        {
            Some((c.clone(), true))
        }
        _ => None,
    })
    .await;
    assert!(can_control && phone.node.agent_can_control(pc_id));
    assert_eq!(catalog.agents[0].modes, ["auto", "ask", "accept_edits", "plan", "full"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_steers_queues_and_edits_the_queue() {
    use g7link_core::{AgentQueueChange, AgentRequest, AgentSend, QueueAction};
    let mut phone = control_phone("Pixel").await;
    let pc = control_pc_with(&mut phone).await;
    let pc_id = pc.node.device_id();
    allow_agents(&pc, &mut phone).await;

    let image = with_timeout("image", phone.node.agent_image_send(pc_id, "PXL_1.jpg".into(), jpeg(5_000)))
        .await
        .unwrap();
    let steer = AgentSend {
        thread: AGENT_THREAD.into(),
        text: String::new(),
        dispatch: Some("steer".into()),
        images: vec![image.id.clone()],
        key: None,
    };
    with_timeout("steer", phone.node.agent_send_with(pc_id, steer)).await.unwrap();
    assert_eq!(
        last_request(&pc),
        AgentRequest::Send {
            thread: AGENT_THREAD.into(),
            text: String::new(),
            dispatch: Some("steer".into()),
            images: vec![image.id.clone()],
            key: None
        }
    );
    let change = |op: &str, text: Option<&str>| AgentQueueChange {
        thread: AGENT_THREAD.into(),
        id: QUEUED_ID.into(),
        op: op.into(),
        text: text.map(Into::into),
    };
    with_timeout("edit", phone.node.agent_queue(pc_id, change("edit", Some("Run all the tests.\u{1b}"))))
        .await
        .unwrap();
    match last_request(&pc) {
        AgentRequest::Queue { id, action: QueueAction::Edit(text), .. } => {
            assert_eq!((id.as_str(), text.as_str()), (QUEUED_ID, "Run all the tests."));
        }
        other => panic!("{other:?}"),
    }
    with_timeout("remove", phone.node.agent_queue(pc_id, change("remove", None))).await.unwrap();
    assert!(matches!(last_request(&pc), AgentRequest::Queue { action: QueueAction::Remove, .. }));
    with_timeout("send now", phone.node.agent_queue(pc_id, change("send_now", None))).await.unwrap();
    assert!(matches!(last_request(&pc), AgentRequest::Queue { action: QueueAction::SendNow, .. }));
    let gone = AgentQueueChange { id: SENT_QUEUED.into(), ..change("remove", None) };
    assert!(matches!(phone.node.agent_queue(pc_id, gone).await, Err(Error::NotFound)));

    let before = requests(&pc);
    let five = AgentSend {
        thread: AGENT_THREAD.into(),
        text: "look".into(),
        dispatch: None,
        images: (0..5).map(|i| format!("{i:032x}")).collect(),
        key: None,
    };
    assert!(matches!(phone.node.agent_send_with(pc_id, five.clone()).await, Err(Error::Protocol(_))));
    refused_with(phone.node.debug_agent_request(pc_id, "agent.send", &five).await, "BAD_MESSAGE");
    let long = change("edit", Some(&"q".repeat(g7link_core::agent::MAX_QUEUED_TEXT_BYTES + 1)));
    refused_with(phone.node.debug_agent_request(pc_id, "agent.queue", &long).await, "BAD_MESSAGE");
    refused_with(
        phone.node.debug_agent_request(pc_id, "agent.queue", &change("reorder", None)).await,
        "BAD_MESSAGE",
    );
    assert_eq!(requests(&pc), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_question_reaches_phones_with_control_and_one_answer_closes_it() {
    use g7link_core::{AgentQuestionAnswer, AgentQuestionReply, AgentRequest};
    let mut first = control_phone("Pixel").await;
    let mut second = control_phone("Galaxy").await;
    let mut older = device_with("Moto", DeviceKind::Phone, &[g7link_core::agent::ITEMS2]).await;
    let mut pc = control_pc_with(&mut first).await;
    pair_qr(&mut pc, &mut second).await;
    pair_qr(&mut pc, &mut older).await;
    let pc_id = pc.node.device_id();
    for phone in [&mut first, &mut second, &mut older] {
        allow_agents(&pc, phone).await;
    }

    pc.node.agent_question(control_question(false)).await.unwrap();
    for dev in [&mut first, &mut second] {
        let question = wait_for(dev, "question", |e| match e {
            NodeEvent::AgentQuestion { device, question } if *device == pc_id => Some(question.clone()),
            _ => None,
        })
        .await;
        assert_eq!(question.asks.len(), 1);
        assert_eq!(question.asks[0].options[0].label, "OAuth");
    }
    let answer = AgentQuestionReply {
        token: QUESTION_TOKEN.into(),
        answers: vec![AgentQuestionAnswer { id: "0".into(), labels: vec!["OAuth".into()], other: None }],
        dismiss: false,
    };
    with_timeout("answer", first.node.agent_reply(pc_id, answer.clone())).await.unwrap();
    assert!(matches!(
        last_request(&pc),
        AgentRequest::QuestionAnswer { token, answers } if token == QUESTION_TOKEN && answers == answer.answers
    ));
    pc.node
        .agent_questioned(g7link_core::AgentQuestioned {
            token: QUESTION_TOKEN.into(),
            thread: AGENT_THREAD.into(),
            outcome: "answered".into(),
            by_name: Some("Pixel".into()),
        })
        .await
        .unwrap();
    for dev in [&mut first, &mut second] {
        let (outcome, by) = wait_for(dev, "questioned", |e| match e {
            NodeEvent::AgentQuestioned { token, outcome, by_name, .. } if token == QUESTION_TOKEN => {
                Some((outcome.clone(), by_name.clone()))
            }
            _ => None,
        })
        .await;
        assert_eq!((outcome.as_str(), by.as_deref()), ("answered", Some("Pixel")));
    }
    let dismiss = AgentQuestionReply { token: QUESTION_TOKEN.into(), answers: Vec::new(), dismiss: true };
    assert!(
        matches!(second.node.agent_reply(pc_id, dismiss).await, Err(Error::NotFound)),
        "first answer wins"
    );
    // An answer that isn't one is refused here.
    let empty = AgentQuestionReply { token: QUESTION_TOKEN.into(), answers: Vec::new(), dismiss: false };
    let before = requests(&pc);
    refused_with(second.node.debug_agent_request(pc_id, "agent.reply", &empty).await, "BAD_MESSAGE");
    assert_eq!(requests(&pc), before);
    // The older phone heard nothing of it.
    assert!(with_timeout("threads", older.node.agent_threads(pc_id)).await.is_ok());
    assert!(
        !drain(&mut older)
            .iter()
            .any(|e| matches!(e, NodeEvent::AgentQuestion { .. } | NodeEvent::AgentQuestioned { .. }))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_question_is_answered_on_the_pc_only() {
    use g7link_core::{AgentQuestionAnswer, AgentQuestionReply};
    let mut phone = control_phone("Pixel").await;
    let pc = control_pc_with(&mut phone).await;
    let pc_id = pc.node.device_id();
    allow_agents(&pc, &mut phone).await;

    pc.node.agent_question(control_question(true)).await.unwrap();
    let question = wait_for(&mut phone, "question", |e| match e {
        NodeEvent::AgentQuestion { question, .. } => Some(question.clone()),
        _ => None,
    })
    .await;
    assert!(question.secret && question.asks.is_empty(), "no asks reach a phone");
    let before = requests(&pc);
    let answer = AgentQuestionReply {
        token: QUESTION_TOKEN.into(),
        answers: vec![AgentQuestionAnswer {
            id: "0".into(),
            labels: Vec::new(),
            other: Some("hunter2".into()),
        }],
        dismiss: false,
    };
    refused_with(phone.node.agent_reply(pc_id, answer).await, "DENIED");
    let dismiss = AgentQuestionReply { token: QUESTION_TOKEN.into(), answers: Vec::new(), dismiss: true };
    refused_with(phone.node.agent_reply(pc_id, dismiss).await, "DENIED");
    assert_eq!(requests(&pc), before, "the host never hears a phone's answer to it");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_implements_a_plan_in_the_mode_it_picks() {
    use g7link_core::AgentRequest;
    let mut phone = control_phone("Pixel").await;
    let pc = control_pc_with(&mut phone).await;
    let pc_id = pc.node.device_id();
    allow_agents(&pc, &mut phone).await;

    with_timeout("watch", phone.node.agent_watch(pc_id, Some(AGENT_THREAD.into()))).await.unwrap();
    for item in control_items() {
        pc.node.agent_event(g7link_core::AgentEvent::item(AGENT_THREAD.into(), item), None).await.unwrap();
    }
    let mut items = Vec::new();
    while items.len() < 4 {
        items.push(
            wait_for(&mut phone, "item", |e| match e {
                NodeEvent::AgentEvent { event, .. } if event.kind == "item" => event.item.clone(),
                _ => None,
            })
            .await,
        );
    }
    assert_eq!(items[0].via.as_deref(), Some("steer"));
    assert_eq!(items[0].images[0].name, "error.png", "an image's name, never its path");
    let plan_file = items[1].tool.as_ref().unwrap();
    assert_eq!(plan_file.class.as_deref(), Some("plan"), "a phone with control hides it");
    assert_eq!((plan_file.detail.as_ref(), plan_file.path.as_ref()), (None, None));
    assert!(!plan_file.title.contains('/'));
    assert_eq!((items[2].kind.as_str(), items[2].plan_state.as_deref()), ("plan", Some("proposed")));
    assert_eq!(items[3].kind, "question");
    let question = items[3].question.as_ref().unwrap();
    assert_eq!((question.count, question.outcome.as_deref()), (1, Some("answered")));

    with_timeout(
        "implement",
        phone.node.agent_implement(pc_id, AGENT_THREAD.into(), "msg_01:3".into(), None),
    )
    .await
    .unwrap();
    assert_eq!(
        last_request(&pc),
        AgentRequest::Implement { thread: AGENT_THREAD.into(), item: "msg_01:3".into(), mode: None },
        "none said: the folder's remembered mode, on the PC"
    );
    let full = phone.node.agent_implement(pc_id, AGENT_THREAD.into(), "msg_01:3".into(), Some("full".into()));
    with_timeout("implement", full).await.unwrap();
    assert_eq!(
        last_request(&pc),
        AgentRequest::Implement {
            thread: AGENT_THREAD.into(),
            item: "msg_01:3".into(),
            mode: Some("full".into())
        }
    );
    let before = requests(&pc);
    let plan = phone.node.agent_implement(pc_id, AGENT_THREAD.into(), "msg_01:3".into(), Some("plan".into()));
    assert!(matches!(plan.await, Err(Error::Protocol(_))), "a plan isn't implemented in plan mode");
    assert_eq!(requests(&pc), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn files_and_commands_stay_inside_the_folder() {
    use g7link_core::{AgentFilesRequest, AgentRequest};
    let mut phone = control_phone("Pixel").await;
    let pc = control_pc_with(&mut phone).await;
    let pc_id = pc.node.device_id();
    allow_agents(&pc, &mut phone).await;

    let request = AgentFilesRequest {
        thread: Some(AGENT_THREAD.into()),
        folder: None,
        query: "../../etc".into(),
        limit: Some(8),
    };
    let files = with_timeout("files", phone.node.agent_files(pc_id, request)).await.unwrap();
    let paths: Vec<&str> = files.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["src/main.rs", "src/"], "an absolute path or one that climbs out never arrives");
    assert!(matches!(
        last_request(&pc),
        AgentRequest::Files { thread: Some(_), folder: None, limit: 8, query } if query == "../../etc"
    ));
    let both = AgentFilesRequest {
        thread: Some(AGENT_THREAD.into()),
        folder: Some(AGENT_FOLDER.into()),
        query: String::new(),
        limit: None,
    };
    refused_with(phone.node.debug_agent_request(pc_id, "agent.files", &both).await, "BAD_MESSAGE");

    let commands =
        with_timeout("commands", phone.node.agent_commands(pc_id, "claude".into(), AGENT_FOLDER.into()))
            .await
            .unwrap();
    let names: Vec<(&str, &str)> = commands.iter().map(|c| (c.name.as_str(), c.kind.as_str())).collect();
    assert_eq!(names, [("review", "command"), ("pdf", "skill")]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_draft_follows_the_user_to_another_phone() {
    use g7link_core::AgentDraftSet;
    let mut first = control_phone("Pixel").await;
    let mut second = control_phone("Galaxy").await;
    let mut pc = control_pc_with(&mut first).await;
    pair_qr(&mut pc, &mut second).await;
    let (pc_id, first_id) = (pc.node.device_id(), first.node.device_id());
    allow_agents(&pc, &mut first).await;
    allow_agents(&pc, &mut second).await;

    let key = format!("new:{AGENT_FOLDER}");
    let set = AgentDraftSet { key: key.clone(), text: "Half a thought".into(), images: Vec::new(), rev: 0 };
    let saved = with_timeout("draft", first.node.agent_draft_set(pc_id, set.clone())).await.unwrap();
    assert_eq!((saved.rev, saved.conflict), (1, None));
    // The PC's app tells the other phones.
    pc.node.agent_draft(key.clone(), 1, Some(first_id)).await.unwrap();
    let rev = wait_for(&mut second, "draft", |e| match e {
        NodeEvent::AgentDraft { key: k, rev, .. } if *k == key => Some(*rev),
        _ => None,
    })
    .await;
    assert_eq!(rev, 1);
    let draft = with_timeout("get", second.node.agent_draft_get(pc_id, key.clone())).await.unwrap().unwrap();
    assert_eq!(
        (draft.text.as_str(), draft.rev, draft.by_name.as_deref()),
        ("Half a thought", 1, Some("Pixel"))
    );
    // A change from a stale copy doesn't win: the phone gets the draft as it is.
    let stale = AgentDraftSet { text: "Something else".into(), ..set };
    let saved = with_timeout("stale", second.node.agent_draft_set(pc_id, stale)).await.unwrap();
    assert_eq!(saved.conflict.map(|d| d.text), Some("Half a thought".into()));
    assert!(
        !drain(&mut first).iter().any(|e| matches!(e, NodeEvent::AgentDraft { .. })),
        "not to the phone that wrote it"
    );
    let bad_key = AgentDraftSet { key: "new:../x".into(), text: String::new(), images: Vec::new(), rev: 0 };
    refused_with(first.node.debug_agent_request(pc_id, "agent.draft.set", &bad_key).await, "BAD_MESSAGE");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_image_goes_up_once_and_never_larger_than_announced() {
    use g7link_core::{AgentRequest, agents::AgentImageMeta};
    let mut phone = control_phone("Pixel").await;
    let pc = control_pc_with(&mut phone).await;
    let pc_id = pc.node.device_id();
    allow_agents(&pc, &mut phone).await;

    let bytes = jpeg(300_000);
    let image = with_timeout(
        "image",
        phone.node.agent_image_send(pc_id, "DCIM/Camera/PXL_1.jpg".into(), bytes.clone()),
    )
    .await
    .unwrap();
    assert_eq!((image.bytes, image.name.as_str()), (300_000, "PXL_1.jpg"));
    match last_request(&pc) {
        AgentRequest::ImageAdd { name, bytes: got } => {
            assert_eq!(name, "PXL_1.jpg", "a name, never a path");
            assert!(got == bytes, "the bytes as sent");
        }
        other => panic!("{other:?}"),
    }

    // More bytes than it said: refused, and the host never sees them.
    let before = requests(&pc);
    let meta = AgentImageMeta { name: "a.jpg".into(), bytes: 1_000 };
    refused_with(phone.node.debug_agent_image_send(pc_id, meta, jpeg(200_000)).await, "BAD_MESSAGE");
    let over = AgentImageMeta { name: "a.jpg".into(), bytes: g7link_core::agent::MAX_IMAGE_BYTES as u64 + 1 };
    refused_with(phone.node.debug_agent_image_send(pc_id, over, jpeg(10)).await, "BAD_MESSAGE");
    assert!(matches!(
        phone
            .node
            .agent_image_send(pc_id, "big.jpg".into(), jpeg(g7link_core::agent::MAX_IMAGE_BYTES + 1))
            .await,
        Err(Error::Protocol(_))
    ));
    assert_eq!(requests(&pc), before);

    // One at a time.
    let gate = Arc::new(Gate::default());
    *pc.platform.agent_image_gate.lock().unwrap() = Some(gate.clone());
    let node = phone.node.clone();
    let first =
        tokio::spawn(async move { node.agent_image_send(pc_id, "one.jpg".into(), jpeg(1_000)).await });
    with_timeout("held", async {
        while gate.held() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    refused_with(phone.node.agent_image_send(pc_id, "two.jpg".into(), jpeg(1_000)).await, "BUSY");
    gate.open();
    assert!(with_timeout("first", first).await.unwrap().is_ok());
    assert!(
        with_timeout("again", phone.node.agent_image_send(pc_id, "two.jpg".into(), jpeg(1_000)))
            .await
            .is_ok()
    );

    // Off for this phone: nothing goes up.
    pc.node.set_device_toggle(phone.node.device_id(), "agents", false).unwrap();
    refused_with(phone.node.agent_image_send(pc_id, "three.jpg".into(), jpeg(1_000)).await, "DENIED");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_thumbnail_is_a_small_jpeg() {
    let mut phone = control_phone("Pixel").await;
    let pc = control_pc_with(&mut phone).await;
    let pc_id = pc.node.device_id();
    allow_agents(&pc, &mut phone).await;

    let thumb = with_timeout("thumb", phone.node.agent_image_thumb(pc_id, AGENT_IMAGE.into(), Some(256)))
        .await
        .unwrap();
    assert_eq!(thumb, jpeg(9_000));
    assert!(matches!(
        pc.platform.agent_requests.lock().unwrap().last().map(|r| r.1.clone()),
        Some(g7link_core::AgentRequest::ImageThumb { max: 256, .. })
    ));
    refused_with(phone.node.agent_image_thumb(pc_id, BIG_IMAGE.into(), None).await, "INTERNAL");
    refused_with(
        phone.node.agent_image_thumb(pc_id, "cccc0000cccc0000cccc0000cccc0000".into(), None).await,
        "NOT_FOUND",
    );
    assert!(matches!(phone.node.agent_image_thumb(pc_id, "x".into(), None).await, Err(Error::Protocol(_))));
}

// ---- Protocol v1: lanes, RPC streams, limits, handshake (plans/protocol-v1-contract.md §4) ----

/// How long an answer may take on loopback while another service is busy.
const SNAPPY: Duration = Duration::from_millis(300);

/// A `ping` on control from `dev` to `peer`: how long its `pong` took.
async fn ping(dev: &TestDevice, peer: g7link_core::DeviceId) -> Duration {
    use g7link_protocol::{
        Envelope,
        messages::{Ping, types},
    };
    let started = std::time::Instant::now();
    let env = Envelope::new(types::PING, &Ping { ts: 0 }).unwrap();
    let reply = with_timeout("ping", dev.node.debug_request(peer, env, WAIT)).await.unwrap();
    assert_eq!(reply.t, types::PONG);
    started.elapsed()
}

/// A PC that takes `phone`'s touchpad and keyboard, paired with it.
async fn input_pair() -> (TestDevice, TestDevice) {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    pc.node
        .update_power(
            PowerLevel::NotApplicable,
            vec!["media.control".into(), g7link_core::INPUT_INJECT.into()],
        )
        .await;
    wait_for(&mut phone, "input available", |e| match e {
        NodeEvent::Capabilities(m)
            if m.device == pc_id && m.state("input.remote") == Some(FeatureState::Available) =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
    pc.node.set_device_toggle(phone_id, "remote_input", true).unwrap();
    with_timeout("check", phone.node.remote_check(pc_id)).await.unwrap();
    (pc, phone)
}

fn enter() -> g7link_core::RemoteInput {
    g7link_core::RemoteInput::Key { key: "enter".into(), mods: Vec::new() }
}

fn ringing(id: &str) -> g7link_core::CallState {
    g7link_core::CallState {
        id: id.into(),
        state: "ringing".into(),
        incoming: true,
        number: Some("+15550100".into()),
        name: Some("Sam".into()),
        photo: None,
        missed: false,
        since: None,
        controls: None,
    }
}

/// A PC showing `phone`'s calls, with a call ringing on the phone.
async fn ringing_call(pc: &mut TestDevice, phone: &TestDevice) {
    let phone_id = phone.node.device_id();
    phone.node.call_changed(ringing("c1")).await.unwrap();
    wait_for(pc, "ringing", |e| match e {
        NodeEvent::Call { device, call } if *device == phone_id && call.id == "c1" => Some(()),
        _ => None,
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_media_command_doesnt_delay_a_key_press() {
    let (mut pc, phone) = input_pair().await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    phone.node.media_changed(vec![player("com.music", "Song", None)]).await;
    wait_for(&mut pc, "media", media_of(phone_id)).await;

    phone.platform.set_delay("media_command", Duration::from_secs(2));
    let slow = {
        let pc = pc.node.clone();
        tokio::spawn(
            async move { pc.media_command(phone_id, "com.music".into(), MediaAction::Pause, None).await },
        )
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let started = std::time::Instant::now();
    with_timeout("key", phone.node.remote_input(pc_id, enter())).await.unwrap();
    let key = started.elapsed();
    assert!(key < SNAPPY, "the key took {key:?}");
    let pong = ping(&phone, pc_id).await;
    assert!(pong < SNAPPY, "the ping took {pong:?}");
    assert!(!slow.is_finished(), "the media command is still running");
    with_timeout("media command", slow).await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_agent_read_doesnt_delay_a_call_action() {
    use g7link_core::CallCommand;
    let mut phone = device_with(
        "Pixel",
        DeviceKind::Phone,
        &[g7link_core::agent::FOLLOW, g7link_core::CALLS_STATE, g7link_core::CALLS_CONTROL],
    )
    .await;
    let mut pc = device_with(
        "Desktop",
        DeviceKind::Desktop,
        &[g7link_core::agent::HOST, g7link_core::agent::SESSIONS, g7link_core::CALLS_SHOW],
    )
    .await;
    pc.node.set_agent_catalog(Some(agent_catalog())).await.unwrap();
    *pc.platform.agent_sessions.lock().unwrap() = vec![agent_session(FOLLOWED_SESSION, "Fix the login test")];
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    allow_sessions(&pc, &mut phone).await;
    ringing_call(&mut pc, &phone).await;

    pc.platform.set_delay("agent_read", Duration::from_secs(5));
    let reading = {
        let phone = phone.node.clone();
        tokio::spawn(async move { phone.agent_session_read(pc_id, FOLLOWED_SESSION.into(), None, 50).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = std::time::Instant::now();
    with_timeout("answer", pc.node.call_command(phone_id, "c1".into(), CallCommand::Answer)).await.unwrap();
    let answer = started.elapsed();
    assert!(answer < SNAPPY, "the call action took {answer:?}");
    with_timeout("read", reading).await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn media_commands_still_run_in_the_order_sent() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    phone.node.media_changed(vec![player("com.music", "Song", None)]).await;
    wait_for(&mut pc, "media", media_of(phone_id)).await;
    phone.platform.set_delay("media_command", Duration::from_millis(20));

    let command = |action| pc.node.media_command(phone_id, "com.music".into(), action, None);
    let (play, pause, again) =
        tokio::join!(command(MediaAction::Play), command(MediaAction::Pause), command(MediaAction::Play));
    play.unwrap();
    pause.unwrap();
    again.unwrap();
    let ran: Vec<MediaAction> = phone.platform.media.lock().unwrap().iter().map(|(_, a, _)| *a).collect();
    assert_eq!(ran, [MediaAction::Play, MediaAction::Pause, MediaAction::Play]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_reply_doesnt_delay_control() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::SMS_SHOW]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::SMS_READ]).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    let parts: Vec<_> = (0..4)
        .map(|_| {
            let pc = pc.node.clone();
            tokio::spawn(async move {
                for _ in 0..5 {
                    let (_, data) = pc.sms_part(phone_id, "big".into()).await.unwrap();
                    assert_eq!(data.len(), 900 * 1024);
                }
            })
        })
        .collect();
    let mut slowest = Duration::ZERO;
    for _ in 0..50 {
        slowest = slowest.max(ping(&pc, phone_id).await);
    }
    assert!(slowest < SNAPPY, "a ping took {slowest:?}");
    for part in parts {
        with_timeout("parts", part).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_burst_of_notifications_doesnt_delay_a_reply() {
    use g7link_core::CallCommand;
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::CALLS_SHOW]).await;
    let mut phone =
        device_with("Pixel", DeviceKind::Phone, &[g7link_core::CALLS_STATE, g7link_core::CALLS_CONTROL])
            .await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    // Notification access and calls together (`grant_notification_access` sets only the first).
    phone
        .node
        .update_power(
            PowerLevel::Basic,
            vec![
                "notify.mirror".into(),
                "notify.reply".into(),
                g7link_core::CALLS_STATE.into(),
                g7link_core::CALLS_CONTROL.into(),
            ],
        )
        .await;
    ringing_call(&mut pc, &phone).await;

    let burst = {
        let phone = phone.node.clone();
        tokio::spawn(async move {
            for i in 0..200 {
                phone.notification_posted(note(&format!("n{i}"), "com.chat", "Sam", false)).await;
            }
        })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    let started = std::time::Instant::now();
    with_timeout("answer", pc.node.call_command(phone_id, "c1".into(), CallCommand::Answer)).await.unwrap();
    let answer = started.elapsed();
    assert!(answer < SNAPPY, "the call action took {answer:?}");
    with_timeout("burst", burst).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_list_larger_than_a_frame_arrives_whole() {
    use g7link_core::{Contact, ContactNumber};
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::CONTACTS_SHOW]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::CONTACTS_READ]).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    // 3 MiB of 16 KiB photos: three times what one frame holds.
    let list: Vec<Contact> = (0..192)
        .map(|i| Contact {
            id: format!("c{i}"),
            name: format!("Contact {i}"),
            numbers: vec![ContactNumber { number: format!("+1555{i:07}"), label: None }],
            starred: false,
            photo: Some(vec![(i % 251) as u8; 16 * 1024]),
        })
        .collect();
    *phone.platform.contacts_list.lock().unwrap() = Some(list.clone());

    let got = with_timeout("contacts", pc.node.contacts(phone_id, None, 0, 200)).await.unwrap();
    assert_eq!(got, list, "every contact, in order, photos and all");
    let parts = pc.node.wire_counters(phone_id).unwrap().parts_received;
    assert!(parts >= 11, "came in {parts} parts before its ok");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_list_is_one_ok() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::CONTACTS_SHOW]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::CONTACTS_READ]).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    *phone.platform.contacts_list.lock().unwrap() = Some(Vec::new());
    let got = with_timeout("contacts", pc.node.contacts(phone_id, None, 0, 200)).await.unwrap();
    assert!(got.is_empty());
    assert_eq!(pc.node.wire_counters(phone_id).unwrap().parts_received, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_request_resets_its_stream_and_frees_the_slot() {
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    let mut pc = agents_pc_with(&mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    allow_agents(&pc, &mut phone).await;

    pc.platform.set_delay("agent_read", Duration::from_secs(5));
    let read = phone.node.agent_read(pc_id, AGENT_THREAD.into(), None, 20);
    assert!(tokio::time::timeout(Duration::from_millis(100), read).await.is_err(), "still waiting");
    let dropped = std::time::Instant::now();
    until("the PC stops answering it", || pc.node.wire_counters(phone_id).is_some_and(|c| c.rpcs_in == 0))
        .await;
    assert!(dropped.elapsed() < Duration::from_millis(500), "freed after {:?}", dropped.elapsed());

    pc.platform.set_delay("agent_read", Duration::ZERO);
    let page =
        with_timeout("read", phone.node.agent_read(pc_id, AGENT_THREAD.into(), None, 20)).await.unwrap();
    assert_eq!(page.items.len(), 1);
    drain(&mut pc);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_timed_out_lane_request_drops_its_late_reply() {
    use g7link_protocol::{Envelope, messages::types};
    let (pc, phone) = input_pair().await;
    let pc_id = pc.node.device_id();

    pc.platform.set_delay("remote_input", Duration::from_millis(700));
    let env = Envelope::new(types::INPUT_EVENT, &enter()).unwrap();
    let late = phone.node.debug_request(pc_id, env, Duration::from_millis(200)).await;
    assert!(matches!(late, Err(Error::Timeout)), "{late:?}");
    until("the late answer is dropped", || {
        phone.node.wire_counters(pc_id).is_some_and(|c| c.late_replies == 1)
    })
    .await;
    assert_eq!(pc.platform.remote_inputs.lock().unwrap().len(), 1, "run once");

    pc.platform.set_delay("remote_input", Duration::ZERO);
    with_timeout("key", phone.node.remote_input(pc_id, enter())).await.unwrap();
    assert_eq!(pc.platform.remote_inputs.lock().unwrap().len(), 2, "the lane still works");
    assert_eq!(phone.node.wire_counters(pc_id).unwrap().late_replies, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_65th_incoming_rpc_is_told_busy() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::CONTACTS_SHOW]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::CONTACTS_READ]).await;
    // Past its own limit of 16, as a misbehaving peer would.
    pc.node.debug_set_outgoing_rpc_limit(100);
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    phone.platform.set_delay("contacts", Duration::from_secs(1));

    let asks: Vec<_> = (0..65)
        .map(|_| {
            let pc = pc.node.clone();
            tokio::spawn(async move { pc.contacts(phone_id, None, 0, 10).await })
        })
        .collect();
    let (mut answered, mut busy) = (0, 0);
    for ask in asks {
        match with_timeout("lookup", ask).await.unwrap() {
            Ok(_) => answered += 1,
            Err(Error::Busy) => busy += 1,
            Err(e) => panic!("unexpected {e:?}"),
        }
    }
    assert_eq!((answered, busy), (64, 1));
    assert_eq!(phone.node.wire_counters(pc.node.device_id()).unwrap().busy_sent, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn callers_past_sixteen_wait_instead_of_failing() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::PHOTOS_SHOW]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::PHOTOS_READ]).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    phone.platform.set_delay("photo_thumbs", Duration::from_millis(100));

    let asks: Vec<_> = (0..40)
        .map(|_| {
            let pc = pc.node.clone();
            tokio::spawn(async move { pc.photo_thumbs(phone_id, vec!["media:4".into()]).await })
        })
        .collect();
    for ask in asks {
        let thumbs = with_timeout("thumbs", ask).await.unwrap().unwrap();
        assert_eq!(thumbs.len(), 1);
    }
    let counters = pc.node.wire_counters(phone_id).unwrap();
    assert_eq!(counters.busy_received, 0);
    assert_eq!(phone.node.wire_counters(pc.node.device_id()).unwrap().busy_sent, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_33rd_bulk_stream_is_told_busy() {
    use g7link_protocol::messages::{ErrorBody, files, types};
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();

    // File streams that say nothing hold their slot while the phone waits for an offer.
    let held: Vec<_> = (0..32)
        .map(|_| {
            let pc = pc.node.clone();
            tokio::spawn(async move {
                pc.debug_open_stream(phone_id, files::SERVICE, files::OP_SEND, Duration::from_secs(5)).await
            })
        })
        .collect();
    for (i, stream) in held.into_iter().enumerate() {
        let answer = with_timeout("open", stream).await.unwrap().unwrap();
        assert!(answer.is_none(), "stream {i} was answered: {answer:?}");
    }
    let refused = pc
        .node
        .debug_open_stream(phone_id, files::SERVICE, files::OP_SEND, Duration::from_secs(5))
        .await
        .unwrap()
        .expect("the 33rd is answered at once");
    assert_eq!(refused.t, types::ERROR);
    let body: ErrorBody = refused.body().unwrap();
    assert_eq!(body.code, g7link_protocol::ErrorCode::Busy);
    assert_eq!(phone.node.wire_counters(pc.node.device_id()).unwrap().busy_sent, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn shutting_down_fails_waiting_requests_at_once() {
    use g7link_core::CallCommand;
    let mut pc =
        device_with("Desktop", DeviceKind::Desktop, &[g7link_core::CALLS_SHOW, g7link_core::CONTACTS_SHOW])
            .await;
    let mut phone = device_with(
        "Pixel",
        DeviceKind::Phone,
        &[g7link_core::CALLS_STATE, g7link_core::CALLS_CONTROL, g7link_core::CONTACTS_READ],
    )
    .await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    ringing_call(&mut pc, &phone).await;
    phone.platform.set_delay("call_command", Duration::from_secs(5));
    phone.platform.set_delay("contacts", Duration::from_secs(5));

    let on_lane = {
        let pc = pc.node.clone();
        tokio::spawn(async move { pc.call_command(phone_id, "c1".into(), CallCommand::Answer).await })
    };
    let on_rpc = {
        let pc = pc.node.clone();
        tokio::spawn(async move { pc.contacts(phone_id, None, 0, 10).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = std::time::Instant::now();
    pc.node.shutdown().await;
    let (lane, rpc) =
        (with_timeout("lane", on_lane).await.unwrap(), with_timeout("rpc", on_rpc).await.unwrap());
    assert!(started.elapsed() < Duration::from_secs(1), "took {:?}", started.elapsed());
    // What matters is that they fail at once. Which word wins depends on whether the
    // closing connection or the shutdown reaches a waiter first, so both are fine.
    fn ended<T>(r: &Result<T, Error>) -> bool {
        matches!(r, Err(Error::Offline | Error::Network(_)))
    }
    assert!(ended(&lane), "{lane:?}");
    assert!(ended(&rpc), "{rpc:?}");
    until("no session task is left", || pc.node.debug_session_tasks() == 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_peer_going_away_mid_handler_is_harmless() {
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[g7link_core::CONTACTS_SHOW]).await;
    let mut phone = device_with("Pixel", DeviceKind::Phone, &[g7link_core::CONTACTS_READ]).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    phone.platform.set_delay("contacts", Duration::from_secs(1));
    let asking = {
        let pc = pc.node.clone();
        tokio::spawn(async move { pc.contacts(phone_id, None, 0, 10).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    pc.node.shutdown().await;
    assert!(with_timeout("lookup", asking).await.unwrap().is_err());
    until("the phone's session tasks end", || phone.node.debug_session_tasks() == 0).await;
    assert!(phone.node.wire_counters(pc_id).is_none(), "the session is gone");

    // A new PC pairs, and the phone answers it.
    phone.platform.set_delay("contacts", Duration::ZERO);
    let mut other = device_with("Laptop", DeviceKind::Desktop, &[g7link_core::CONTACTS_SHOW]).await;
    pair_qr(&mut other, &mut phone).await;
    let got = with_timeout("lookup", other.node.contacts(phone_id, None, 0, 10)).await.unwrap();
    assert!(!got.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lane_reopens_after_its_stream_fails() {
    let (pc, phone) = input_pair().await;
    let pc_id = pc.node.device_id();
    with_timeout("key", phone.node.remote_input(pc_id, enter())).await.unwrap();
    phone.node.debug_fail_lanes(pc_id);
    with_timeout("key after", phone.node.remote_input(pc_id, enter())).await.unwrap();
    assert_eq!(pc.platform.remote_inputs.lock().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn motion_is_summed_not_dropped_when_injection_is_slow() {
    let (pc, phone) = input_pair().await;
    let pc_id = pc.node.device_id();
    pc.platform.set_delay("remote_input", Duration::from_millis(20));
    for _ in 0..500 {
        phone.node.remote_move(pc_id, 1.0, 0.0).await.unwrap();
    }
    let moved = || -> f32 {
        pc.platform
            .remote_inputs
            .lock()
            .unwrap()
            .iter()
            .map(|e| match e {
                g7link_core::RemoteInput::Move { dx, .. } => *dx,
                _ => 0.0,
            })
            .sum()
    };
    until("every move arrived", || moved() >= 499.5).await;
    assert!((moved() - 500.0).abs() < 0.5, "moved {}", moved());
    let counters = pc.node.wire_counters(phone.node.device_id()).unwrap();
    assert!(counters.motion_summed > 0, "{counters:?}");
    assert_eq!(counters.motion_refused, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn sound_arrives_with_sequence_and_time() {
    use g7link_core::{DECK_ACTIONS, PC_AUDIO};
    let mut pc = device_with("Desktop", DeviceKind::Desktop, &[DECK_ACTIONS, PC_AUDIO]).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    // 5 ms of 48 kHz stereo a piece, sent together.
    for _ in 0..4 {
        pc.node.send_pc_audio_pcm(&phone_id, 48_000, 2, vec![0; 960]).await.unwrap();
    }
    let mut heard = Vec::new();
    while heard.len() < 4 {
        let piece = wait_for(&mut phone, "sound", |e| match e {
            NodeEvent::PcAudioFrame { seq, ts_ms, pcm, channels, sample_rate, .. } => {
                Some((*seq, *ts_ms, pcm.len(), *channels, *sample_rate))
            }
            _ => None,
        })
        .await;
        heard.push(piece);
    }
    let seqs: Vec<u16> = heard.iter().map(|h| h.0).collect();
    assert_eq!(seqs, [0, 1, 2, 3]);
    let times: Vec<u32> = heard.iter().map(|h| h.1).collect();
    assert_eq!(times, [0, 5, 10, 15]);
    assert!(heard.iter().all(|h| h.2 == 960 && h.3 == 2 && h.4 == 48_000));
    // A long buffer goes in pieces a datagram carries.
    pc.node.send_pc_audio_pcm(&phone_id, 48_000, 2, vec![0; 4 * 960]).await.unwrap();
    for expected in 4..8 {
        let seq = wait_for(&mut phone, "a piece", |e| match e {
            NodeEvent::PcAudioFrame { seq, .. } => Some(*seq),
            _ => None,
        })
        .await;
        assert_eq!(seq, expected);
    }
}

/// The secret key a test device keeps in its data folder (`PlainKeyProtector`).
fn test_device_key(dev: &TestDevice) -> iroh::SecretKey {
    let file = std::fs::read(dev.dir.path().join("identity.key")).unwrap();
    assert_eq!(&file[..5], b"NLK1\0", "a plain key file");
    let bytes: [u8; 32] = file[5..].try_into().unwrap();
    iroh::SecretKey::from_bytes(&bytes)
}

/// An iroh endpoint on loopback that speaks `alpns`, as another build would.
async fn raw_endpoint(secret: iroh::SecretKey, alpns: Vec<Vec<u8>>) -> iroh::Endpoint {
    iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(secret)
        .alpns(alpns)
        .relay_mode(iroh::RelayMode::Disabled)
        .clear_ip_transports()
        .bind_addr(SocketAddr::from(([0, 0, 0, 0], 0)))
        .unwrap()
        .bind()
        .await
        .unwrap()
}

/// `node`'s loopback address.
async fn loopback_addr(node: &Node) -> iroh::EndpointAddr {
    let id = node.device_id();
    let port =
        node.direct_addrs().await.iter().find(|a| a.is_ipv4()).map(SocketAddr::port).expect("an address");
    let key = iroh::PublicKey::from_bytes(&id.0).unwrap();
    iroh::EndpointAddr::new(key).with_ip_addr(SocketAddr::from(([127, 0, 0, 1], port)))
}

/// The application close code a connection ended with.
async fn close_code(conn: &iroh::endpoint::Connection) -> Option<u64> {
    match tokio::time::timeout(WAIT, conn.closed()).await.expect("closed") {
        iroh::endpoint::ConnectionError::ApplicationClosed(close) => Some(close.error_code.into_inner()),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_v0_peer_is_told_to_update() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    let key = test_device_key(&phone);
    phone.node.shutdown().await;
    let pc_addr = loopback_addr(&pc.node).await;

    // The paired phone, still on g7link/0.
    let old = raw_endpoint(key, Vec::new()).await;
    let conn = with_timeout("dial", old.connect(pc_addr.clone(), b"g7link/0")).await.unwrap();
    assert_eq!(close_code(&conn).await, Some(3), "VERSION_TOO_OLD");
    wait_for(&mut pc, "needs update", |e| match e {
        NodeEvent::PeerNeedsUpdate { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    // Again within the minute: closed the same way, said once.
    let again = with_timeout("dial again", old.connect(pc_addr.clone(), b"g7link/0")).await.unwrap();
    assert_eq!(close_code(&again).await, Some(3));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!drain(&mut pc).iter().any(|e| matches!(e, NodeEvent::PeerNeedsUpdate { .. })), "said once");
    old.close().await;

    // A device that isn't paired is told so, and nothing is said.
    let stranger = raw_endpoint(iroh::SecretKey::generate(), Vec::new()).await;
    let conn = with_timeout("dial", stranger.connect(pc_addr, b"g7link/0")).await.unwrap();
    assert_eq!(close_code(&conn).await, Some(2), "UNPAIRED");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!drain(&mut pc).iter().any(|e| matches!(e, NodeEvent::PeerNeedsUpdate { .. })));
    stranger.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dialing_a_peer_without_v1_says_which_side_is_old() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    let key = test_device_key(&phone);
    phone.node.shutdown().await;

    // The phone comes back on 0.1.17: it takes only g7link/0.
    let old = raw_endpoint(key, vec![b"g7link/0".to_vec()]).await;
    let accepting = old.clone();
    tokio::spawn(async move {
        while let Some(incoming) = accepting.accept().await {
            let _ = incoming.await;
        }
    });
    // A peer's loopback address isn't one it's reached at: this machine's
    // own LAN address, as the PC has it, with the old phone's port.
    let port = old.bound_sockets().iter().map(SocketAddr::port).next().unwrap();
    let lan = pc.node.direct_addrs().await.into_iter().find(|a| a.is_ipv4()).expect("a LAN address").ip();
    drain(&mut pc);
    pc.node.add_known_addrs(phone_id, &[SocketAddr::new(lan, port)]);
    let dialed = with_timeout("dial", pc.node.debug_dial(phone_id)).await;
    assert!(matches!(dialed, Err(Error::VersionTooOld { side: g7link_core::Side::Remote })), "{dialed:?}");
    // The reconnect loop meets the same and tells the apps, once.
    wait_for(&mut pc, "needs update", |e| match e {
        NodeEvent::PeerNeedsUpdate { device } if *device == phone_id => Some(()),
        _ => None,
    })
    .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!drain(&mut pc).iter().any(|e| matches!(e, NodeEvent::PeerNeedsUpdate { .. })), "said once");
    old.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn hello_with_another_proto_is_refused() {
    use g7link_protocol::{
        Envelope, ErrorCode,
        messages::{ErrorBody, Hello, types},
    };
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let key = test_device_key(&phone);
    phone.node.shutdown().await;
    let pc_addr = loopback_addr(&pc.node).await;
    let raw = raw_endpoint(key, Vec::new()).await;
    for proto in [0, 2] {
        let conn =
            with_timeout("dial", raw.connect(pc_addr.clone(), g7link_protocol::ALPN_SESSION)).await.unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let hello = Hello {
            proto,
            app: "9.9.9".into(),
            device: info("Pixel", DeviceKind::Phone),
            caps: Vec::new(),
            power: PowerLevel::Assist,
            localsend: None,
        };
        g7link_protocol::write_frame(&mut send, &Envelope::new(types::HELLO, &hello).unwrap().to_cbor())
            .await
            .unwrap();
        let frame = with_timeout("answer", g7link_protocol::read_frame(&mut recv)).await.unwrap().unwrap();
        let answer = Envelope::from_cbor(&frame).unwrap();
        assert_eq!(answer.t, types::ERROR, "proto {proto}");
        assert_eq!(answer.body::<ErrorBody>().unwrap().code, ErrorCode::VersionTooOld);
        assert_eq!(close_code(&conn).await, Some(3), "proto {proto}");
        drop((send, recv));
    }
    raw.close().await;
}

// ---- Disconnects (plans/disconnects.md §5) ----

#[tokio::test(flavor = "multi_thread")]
async fn a_network_change_doesnt_close_a_session_whose_peer_is_busy() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    phone.platform.set_delay("set_clipboard", Duration::from_secs(3));
    let copying = {
        let pc = pc.node.clone();
        tokio::spawn(async move { pc.send_clipboard(phone_id, "hello".into()).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    drain(&mut pc);
    pc.node.network_changed().await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    let offline = drain(&mut pc).into_iter().any(|e| {
        matches!(e, NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } if device == phone_id)
    });
    assert!(!offline, "the session closed");
    with_timeout("copy", copying).await.unwrap().unwrap();
    assert!(ping(&pc, phone_id).await < Duration::from_secs(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_burst_of_network_changes_sends_one_ping() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    tokio::time::sleep(Duration::from_millis(500)).await;
    let serials = (pc.node.debug_session_serial(phone_id), phone.node.debug_session_serial(pc_id));
    let pings = |dev: &TestDevice, peer| dev.node.wire_counters(peer).unwrap().heartbeat_pings;
    let before = (pings(&pc, phone_id), pings(&phone, pc_id));
    for _ in 0..20 {
        pc.node.network_changed().await;
        phone.node.network_changed().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!((pc.node.debug_session_serial(phone_id), phone.node.debug_session_serial(pc_id)), serials);
    let after = (pings(&pc, phone_id), pings(&phone, pc_id));
    assert!(after.0 - before.0 <= 1 && after.1 - before.1 <= 1, "{before:?} → {after:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_survives_a_moment_with_no_direct_path() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    let serial = pc.node.debug_session_serial(phone_id);
    tokio::time::sleep(Duration::from_secs(5)).await;
    pc.node.debug_refuse_paths(true);
    pc.node.network_changed().await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    pc.node.debug_refuse_paths(false);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(pc.node.debug_session_serial(phone_id), serial, "the session stayed up");
}

#[tokio::test(flavor = "multi_thread")]
async fn three_quick_drops_still_reconnect_at_once() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let phone_id = phone.node.device_id();
    for round in 0..3 {
        drain(&mut pc);
        pc.node.debug_close_session(phone_id);
        let started = std::time::Instant::now();
        wait_online(&mut pc, phone_id).await;
        assert!(started.elapsed() < Duration::from_secs(3), "round {round}: {:?}", started.elapsed());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dial_that_lands_on_a_live_session_doesnt_replace_it() {
    let mut pc = device("Desktop", DeviceKind::Desktop).await;
    let mut phone = device("Pixel", DeviceKind::Phone).await;
    pair_qr(&mut pc, &mut phone).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    // Settled, and heard from.
    tokio::time::sleep(Duration::from_secs(4)).await;
    ping(&pc, phone_id).await;
    let serial = pc.node.debug_session_serial(phone_id);
    with_timeout("dial", pc.node.debug_dial(phone_id)).await.unwrap();
    assert_eq!(pc.node.debug_session_serial(phone_id), serial);
    assert!(phone.node.debug_session_serial(pc_id).is_some());
    assert!(ping(&pc, phone_id).await < Duration::from_secs(1));
}
