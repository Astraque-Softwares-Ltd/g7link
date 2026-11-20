// SPDX-License-Identifier: MPL-2.0
//! Media control between devices (docs/protocol/media.md).
//!
//! A device that shares its players (`media.control`) keeps every allowed
//! device that shows them (`media.remote`) up to date: the full list when a
//! session starts or the user changes what that device may see, then after
//! each change. Artwork goes once per key and session. The other side turns
//! that into [`NodeEvent::MediaChanged`] and sends the user's commands back.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use g7link_protocol::{
    DeviceId, Envelope, ErrorCode, MAX_FRAME_LEN,
    messages::{
        AudioBtMedia, MediaArtMeta, MediaCommand, MediaFileMeta, MediaFileRead, MediaIcon, MediaKey,
        MediaPlayer, MediaState, MediaWatch, StreamHeader, WatchFile, is_media_art_key, is_media_icon_key,
        media_actions, media_art, media_limits, media_limits::PLAYERS, media_watch, types,
    },
    read_frame, write_frame,
};
use iroh::endpoint::{RecvStream, SendStream};

use crate::{
    Error, Result, dispatch::Responder, events::NodeEvent, features::role_of, node::Shared, session::Session,
};

/// How long a Play on phone offer stays open for the phone to take.
const WATCH_OFFER: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// The device toggle that allows media for a device.
pub(crate) const TOGGLE: &str = "media";
/// The device toggle that lets a phone play its media on a PC over
/// Bluetooth. Both devices keep it the same (`audio.bt_media`).
pub const BT_MEDIA_TOGGLE: &str = "bt_media";
/// Offered by devices that show and control other devices' players.
const REMOTE: &str = "media.remote";
/// Offered by devices that share their own players.
const SHARES: &str = "media.control";

/// A command for a player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaAction {
    Play,
    Pause,
    Next,
    Previous,
    /// To the position given with it.
    Seek,
    /// On or off, by the value given with it (`on`, `off`).
    Shuffle,
    /// By the value given with it (`none`, `track`, `playlist`).
    Loop,
}

impl MediaAction {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaAction::Play => media_actions::PLAY,
            MediaAction::Pause => media_actions::PAUSE,
            MediaAction::Next => media_actions::NEXT,
            MediaAction::Previous => media_actions::PREVIOUS,
            MediaAction::Seek => media_actions::SEEK,
            MediaAction::Shuffle => media_actions::SHUFFLE,
            MediaAction::Loop => media_actions::LOOP,
        }
    }

    pub fn parse(action: &str) -> Option<MediaAction> {
        Some(match action {
            media_actions::PLAY => MediaAction::Play,
            media_actions::PAUSE => MediaAction::Pause,
            media_actions::NEXT => MediaAction::Next,
            media_actions::PREVIOUS => MediaAction::Previous,
            media_actions::SEEK => MediaAction::Seek,
            media_actions::SHUFFLE => MediaAction::Shuffle,
            media_actions::LOOP => MediaAction::Loop,
            _ => return None,
        })
    }
}

/// The format of a large picture (`media.art`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtFormat {
    Jpeg,
    Png,
}

impl ArtFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtFormat::Jpeg => media_art::JPEG,
            ArtFormat::Png => media_art::PNG,
        }
    }

    pub fn parse(format: &str) -> Option<ArtFormat> {
        match format {
            media_art::JPEG => Some(ArtFormat::Jpeg),
            media_art::PNG => Some(ArtFormat::Png),
            _ => None,
        }
    }

    /// The format `bytes` start with, if either.
    pub fn of(bytes: &[u8]) -> Option<ArtFormat> {
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(ArtFormat::Jpeg)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(ArtFormat::Png)
        } else {
            None
        }
    }
}

/// Why a media command didn't run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    /// The player is gone.
    NotFound,
    /// The player (or this device) can't do that.
    Unsupported,
    /// Anything else; the string is for logs.
    Failed(String),
}

/// This device's players, as the app last reported them, with their
/// artwork kept apart for sending once per session.
#[derive(Default)]
pub(crate) struct Players {
    inner: Mutex<PlayersState>,
}

#[derive(Default)]
struct PlayersState {
    /// Set once the app reports players: only then does this device send.
    active: bool,
    players: Vec<MediaPlayer>,
    art: HashMap<String, Vec<u8>>,
    /// When the app reported them: a state sent later adds the time since to
    /// each player's `age`.
    reported: Option<std::time::Instant>,
}

impl Players {
    fn lock(&self) -> std::sync::MutexGuard<'_, PlayersState> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set(&self, players: Vec<MediaPlayer>) {
        let mut state = self.lock();
        state.active = true;
        state.reported = Some(std::time::Instant::now());
        state.players.clear();
        let mut art = HashMap::new();
        for mut player in players.into_iter().filter_map(MediaPlayer::sanitized).take(PLAYERS) {
            if let (Some(key), Some(bytes)) = (&player.art_key, player.art.take()) {
                art.insert(key.clone(), bytes);
            } else if let Some(key) = &player.art_key
                && let Some(bytes) = state.art.remove(key)
            {
                // Artwork is sent with every report; keep what's still in use.
                art.insert(key.clone(), bytes);
            }
            state.players.push(player);
        }
        state.art = art;
    }

    /// The players for one session, with new artwork that fits in the frame.
    fn for_session(&self, sent: &mut HashSet<String>) -> MediaState {
        // Leave room for metadata for every player (four 256-character text
        // fields, ID, art key, actions and CBOR overhead) and the envelope.
        let mut remaining = MAX_FRAME_LEN - 64 * 1024;
        let state = self.lock();
        let mut players = state.players.clone();
        let waited =
            state.reported.map_or(0, |at| u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX));
        for player in players.iter_mut().filter(|p| p.position.is_some()) {
            if waited > 0 || player.age.is_some() {
                player.age = Some(player.age.unwrap_or(0).saturating_add(waited));
            }
        }
        // Prefer artwork for playing media if several players get new artwork at once.
        for playing in [true, false] {
            for player in players.iter_mut().filter(|p| p.playing == playing) {
                if let Some(key) = &player.art_key
                    && let Some(art) = state.art.get(key)
                    && art.len() <= remaining
                    && sent.insert(key.clone())
                {
                    player.art = Some(art.clone());
                    remaining -= art.len();
                }
            }
        }
        MediaState { players }
    }

    /// Whether a current player names `key` as its large picture.
    fn has_art_full(&self, key: &str) -> bool {
        self.lock().players.iter().any(|p| p.art_full.as_deref() == Some(key))
    }

    /// Whether a current player names `key` as its app's icon.
    fn has_icon(&self, key: &str) -> bool {
        self.lock().players.iter().any(|p| p.icon.as_deref() == Some(key))
    }

    /// Returns the title of the currently playing media item on this device, if any.
    pub(crate) fn active_title(&self) -> Option<String> {
        let state = self.lock();
        state
            .players
            .iter()
            .find(|p| p.playing && p.title.as_ref().is_some_and(|t| !t.trim().is_empty()))
            .or_else(|| state.players.iter().find(|p| p.title.as_ref().is_some_and(|t| !t.trim().is_empty())))
            .and_then(|p| p.title.clone())
    }
}

impl Shared {
    fn shows_media(&self, peer: &DeviceId) -> bool {
        self.store.get_peer(peer).ok().flatten().is_some_and(|p| p.caps.contains(REMOTE))
    }

    /// Sends this device's players to one device: all of them when the
    /// user allows it, none (which clears that device) when not.
    pub(crate) async fn send_media_state(&self, session: &Arc<Session>) {
        if !self.players.lock().active || !self.shows_media(&session.peer) {
            return;
        }
        let mut sent_art = session.sent_art.lock().await;
        let state = if self.toggle_on(&session.peer, TOGGLE) {
            self.players.for_session(&mut sent_art)
        } else {
            MediaState { players: Vec::new() }
        };
        if let Ok(env) = Envelope::new(types::MEDIA_STATE, &state) {
            let _ = session.send(env).await;
        }
    }

    pub(crate) async fn media_changed(self: &Arc<Self>, players: Vec<MediaPlayer>) {
        self.players.set(players);
        let mut sends = tokio::task::JoinSet::new();
        for session in self.live_sessions() {
            if self.toggle_on(&session.peer, TOGGLE) {
                let shared = self.clone();
                sends.spawn(async move { shared.send_media_state(&session).await });
            }
        }
        while sends.join_next().await.is_some() {}
    }

    /// Brings both sides in line after the user allowed or stopped media
    /// for a device.
    pub(crate) fn media_toggled(self: &Arc<Self>, peer: DeviceId, enabled: bool) {
        if !enabled {
            // A Play on phone offer goes with it: the phone can't take it or read its file.
            self.watch_offers.lock().unwrap_or_else(|e| e.into_inner()).remove(&peer);
        }
        let Some(session) = self.session(&peer) else {
            if !enabled {
                self.emit(NodeEvent::MediaChanged { device: peer, players: Vec::new() });
            }
            return;
        };
        let shared = self.clone();
        self.runtime.spawn(async move {
            // What this device shares: everything, or nothing.
            shared.send_media_state(&session).await;
            if enabled {
                // What the other one shares: ask for it.
                let _ = session.send(Envelope::empty(types::MEDIA_SYNC)).await;
            } else {
                shared.emit(NodeEvent::MediaChanged { device: peer, players: Vec::new() });
            }
        });
    }

    /// The user changed `bt_media` for `peer` here: kept with when, and sent to
    /// `peer` if it's connected so it keeps the same (`audio.bt_media`). Queued
    /// right away, so quick changes arrive in the order they were made.
    pub(crate) fn set_bt_media(&self, peer: DeviceId, on_pc: bool) -> Result<()> {
        let stamp = self.store.set_toggle_changed(&peer, BT_MEDIA_TOGGLE, on_pc, crate::now_unix_ms())?;
        let Some(session) = self.session(&peer) else { return Ok(()) };
        let body = AudioBtMedia { on_pc, changed_ms: u64::try_from(stamp).ok() };
        let Ok(env) = Envelope::new(types::MEDIA_BT, &body) else { return Ok(()) };
        if let Err(error) = session.send_now(env) {
            tracing::debug!(peer = %peer.short(), %error, "couldn't send bt_media");
        }
        Ok(())
    }

    /// Sends a newly connected phone or PC this device's `bt_media` for it and
    /// when it was changed, so a change made on either while they were apart
    /// reaches the other (the later one wins there). Nothing when it was never
    /// changed, so a default never replaces a choice made on the other device.
    pub(crate) async fn send_bt_media_state(&self, session: &Arc<Session>) {
        let peer = session.peer;
        if !self.phone_and_pc(&peer) {
            return;
        }
        let Ok(Some((on_pc, stamp))) = self.store.stamped_toggle(&peer, BT_MEDIA_TOGGLE) else { return };
        let Ok(stamp) = u64::try_from(stamp) else { return };
        let body = AudioBtMedia { on_pc, changed_ms: Some(stamp) };
        if let Ok(env) = Envelope::new(types::MEDIA_BT, &body) {
            let _ = session.send(env).await;
        }
    }

    /// Whether one of this device and `peer` is a phone and the other a PC.
    fn phone_and_pc(&self, peer: &DeviceId) -> bool {
        let local = role_of(&self.local.read().unwrap_or_else(|e| e.into_inner()).device);
        let remote = self.store.get_peer(peer).ok().flatten().and_then(|p| role_of(&p.info));
        matches!((local, remote), (Some(local), Some(remote)) if local != remote)
    }

    /// `peer` changed `bt_media` for this device: kept here too unless it was
    /// changed here at the same time or later, and never sent back, so the two
    /// don't bounce it between them.
    pub(crate) fn bt_media_from_peer(&self, peer: DeviceId, body: AudioBtMedia) -> Result<()> {
        if !self.store.is_paired(&peer)? {
            return Ok(());
        }
        let AudioBtMedia { on_pc, changed_ms } = body;
        let before = self.toggle_on(&peer, BT_MEDIA_TOGGLE);
        let set = match changed_ms {
            Some(changed_ms) => {
                let Ok(changed_ms) = i64::try_from(changed_ms) else {
                    tracing::debug!(peer = %peer.short(), "ignored an audio.bt_media from too far ahead");
                    return Ok(());
                };
                self.store.set_toggle_if_newer(&peer, BT_MEDIA_TOGGLE, on_pc, changed_ms)?
            }
            // A version that doesn't say when only sends changes as they're
            // made, so this one is the latest.
            None => {
                self.store.set_toggle_changed(&peer, BT_MEDIA_TOGGLE, on_pc, crate::now_unix_ms())?;
                true
            }
        };
        if set && before != on_pc {
            self.emit(NodeEvent::DeviceToggleChanged {
                device: peer,
                toggle: BT_MEDIA_TOGGLE.to_owned(),
                on: on_pc,
            });
        }
        Ok(())
    }
}

/// Handles a `media.*` message on a session. Returns false for other types.
pub(crate) async fn handle(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<bool> {
    let peer = session.peer;
    match env.t.as_str() {
        types::MEDIA_STATE => {
            if shared.toggle_on(&peer, TOGGLE) {
                let state: MediaState = env.body()?;
                let players =
                    state.players.into_iter().filter_map(MediaPlayer::sanitized).take(PLAYERS).collect();
                shared.emit(NodeEvent::MediaChanged { device: peer, players });
            }
        }
        types::MEDIA_SYNC => shared.send_media_state(session).await,
        types::MEDIA_WATCH => {
            let reply = watch_offered(shared, &peer, env.body()?).await;
            rsp.send(reply).await?;
        }
        types::MEDIA_COMMAND => {
            let command: MediaCommand = env.body()?;
            let reply = run_command(shared, &peer, command).await;
            rsp.send(reply).await?;
        }
        types::MEDIA_ICON => {
            let reply = serve_icon(shared, &peer, env).await;
            rsp.send(reply).await?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// A Play on phone offer this PC made a phone.
#[derive(Debug, Clone)]
pub(crate) struct WatchOffer {
    player: String,
    /// When it was made, or last taken or left.
    at: std::time::Instant,
    /// The file the phone may read, for a file it plays itself.
    file: Option<std::path::PathBuf>,
}

/// Play on phone, on the PC: offers `peer` what `player` plays, streamed,
/// or with `file` (and where it had got to, `position_ms`), that video
/// file for the phone to play itself. The offer is what lets that phone
/// start mirroring with `watch` (see [`watch_allowed`]), or read the file
/// (see [`serve_file`]).
pub(crate) async fn offer_watch(
    shared: &Arc<Shared>,
    peer: DeviceId,
    player: String,
    title: Option<String>,
    file: Option<(std::path::PathBuf, u64)>,
) -> Result<()> {
    let described = match &file {
        Some((path, position)) => {
            let size = tokio::fs::metadata(path).await.map_err(|e| Error::Internal(e.to_string()))?.len();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Some(WatchFile { name, size, position: *position })
        }
        None => None,
    };
    let watch = MediaWatch { player, title, file: described };
    if !watch.is_valid() {
        return Err(Error::Internal("not a player to offer".into()));
    }
    if !shared.we_offer(media_watch::STREAM) {
        return Err(Error::Unsupported);
    }
    if !shared.store.get_peer(&peer)?.is_some_and(|p| p.caps.contains(media_watch::WATCH)) {
        return Err(Error::Unsupported);
    }
    if !shared.toggle_on(&peer, TOGGLE) {
        return Err(Error::Denied);
    }
    let session = shared.session(&peer).ok_or(Error::Offline)?;
    let offer = WatchOffer {
        player: watch.player.clone(),
        at: std::time::Instant::now(),
        file: file.map(|(path, _)| path),
    };
    shared.watch_offers.lock().unwrap_or_else(|e| e.into_inner()).insert(peer, offer);
    let env = Envelope::new(types::MEDIA_WATCH, &watch)?;
    let answer =
        session.request(env, crate::session::REQUEST_TIMEOUT).await.and_then(|r| Ok(r.expect(types::OK)?));
    if answer.is_err() {
        shared.watch_offers.lock().unwrap_or_else(|e| e.into_inner()).remove(&peer);
    }
    answer.map(|_| ())
}

/// Whether `peer` may watch `player`: this PC offered it, and the phone
/// takes it (or comes back to it) in time. Taking it renews the offer.
pub(crate) fn watch_allowed(shared: &Shared, peer: &DeviceId, player: &str) -> bool {
    if !shared.toggle_on(peer, TOGGLE) {
        return false;
    }
    let mut offers = shared.watch_offers.lock().unwrap_or_else(|e| e.into_inner());
    match offers.get_mut(peer) {
        Some(offer) if offer.player == player && offer.at.elapsed() < WATCH_OFFER => {
            offer.at = std::time::Instant::now();
            true
        }
        _ => false,
    }
}

/// Whether `peer` has a Play on phone offer from this PC open (so this PC
/// may open the stream to it).
pub(crate) fn watch_open(shared: &Shared, peer: &DeviceId) -> bool {
    shared.toggle_on(peer, TOGGLE)
        && shared
            .watch_offers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(peer)
            .is_some_and(|offer| offer.at.elapsed() < WATCH_OFFER)
}

/// Watching stopped: the offer stays open for a while from now.
pub(crate) fn renew_watch(shared: &Shared, peer: &DeviceId) {
    if let Some(offer) = shared.watch_offers.lock().unwrap_or_else(|e| e.into_inner()).get_mut(peer) {
        offer.at = std::time::Instant::now();
    }
}

/// The file `peer` may read, while its offer is open and media is on for
/// it (reading renews it).
fn offered_file(shared: &Shared, peer: &DeviceId) -> Option<std::path::PathBuf> {
    if !shared.toggle_on(peer, TOGGLE) {
        return None;
    }
    let mut offers = shared.watch_offers.lock().unwrap_or_else(|e| e.into_inner());
    let offer = offers.get_mut(peer).filter(|offer| offer.at.elapsed() < WATCH_OFFER)?;
    offer.at = std::time::Instant::now();
    offer.file.clone()
}

/// How long a `media/file` read may take to arrive or answer.
const FILE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The PC's side of a `media/file` stream (its header already read): one
/// read of the offered file, if `peer` has one open.
pub(crate) async fn serve_file(
    shared: Arc<Shared>,
    peer: DeviceId,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let Ok(Ok(Some(bytes))) = tokio::time::timeout(FILE_TIMEOUT, read_frame(&mut recv)).await else {
        return;
    };
    let Ok(env) = Envelope::from_cbor(&bytes) else { return };
    let reply_error = |code, msg: &'static str| Envelope::error(code, msg).reply_to(env.id);
    let read = match env.expect_body::<MediaFileRead>(types::MEDIA_FILE) {
        Ok(read) => read,
        Err(_) => {
            let _ =
                write_frame(&mut send, &reply_error(ErrorCode::BadMessage, "invalid media.file").to_cbor())
                    .await;
            let _ = send.finish();
            return;
        }
    };
    let Some(path) = offered_file(&shared, &peer) else {
        let _ = write_frame(&mut send, &reply_error(ErrorCode::Denied, "no file offered").to_cbor()).await;
        let _ = send.finish();
        return;
    };
    let opened = async {
        let mut file = tokio::fs::File::open(&path).await?;
        let size = file.metadata().await?.len();
        let length = read.length.min(media_watch::MAX_READ).min(size.saturating_sub(read.offset));
        file.seek(std::io::SeekFrom::Start(read.offset.min(size))).await?;
        let mut data = vec![0u8; length as usize];
        file.read_exact(&mut data).await?;
        std::io::Result::Ok((size, data))
    }
    .await;
    let (size, data) = match opened {
        Ok(read) => read,
        Err(e) => {
            tracing::warn!(error = %e, "can't read the offered file");
            let _ =
                write_frame(&mut send, &reply_error(ErrorCode::Internal, "can't read it").to_cbor()).await;
            let _ = send.finish();
            return;
        }
    };
    let meta = MediaFileMeta { size, length: data.len() as u64 };
    let Ok(reply) = Envelope::new(types::MEDIA_FILE_META, &meta).map(|e| e.reply_to(env.id)) else { return };
    if write_frame(&mut send, &reply.to_cbor()).await.is_ok() && send.write_all(&data).await.is_ok() {
        let _ = send.finish();
    }
}

/// The phone's side: reads up to `length` bytes of the file `peer` (a PC)
/// offered, from `offset`. Returns the file's size and the bytes.
pub(crate) async fn read_file(session: &Session, offset: u64, length: u64) -> Result<(u64, Vec<u8>)> {
    let (mut send, mut recv) = session.open_bi().await?;
    let header = StreamHeader {
        svc: media_watch::SERVICE.into(),
        op: media_watch::OP_FILE.into(),
        v: media_watch::VERSION,
    };
    write_frame(&mut send, &Envelope::new(types::STREAM, &header)?.to_cbor()).await?;
    let read = MediaFileRead { offset, length: length.min(media_watch::MAX_READ) };
    write_frame(&mut send, &Envelope::new(types::MEDIA_FILE, &read)?.with_id(1).to_cbor()).await?;
    let _ = send.finish();
    let reply = match tokio::time::timeout(FILE_TIMEOUT, read_frame(&mut recv)).await {
        Ok(Ok(Some(bytes))) => Envelope::from_cbor(&bytes)?,
        Ok(Ok(None)) => return Err(Error::Offline),
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err(Error::Timeout),
    };
    let meta: MediaFileMeta = reply.expect_body(types::MEDIA_FILE_META)?;
    if meta.length > media_watch::MAX_READ {
        return Err(Error::Protocol("media.file sent too much".into()));
    }
    let mut data = vec![0u8; meta.length as usize];
    tokio::time::timeout(FILE_TIMEOUT, recv.read_exact(&mut data))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|e| Error::Network(e.to_string()))?;
    Ok((meta.size, data))
}

/// The phone's side of `media.watch`: show the offer.
async fn watch_offered(shared: &Arc<Shared>, peer: &DeviceId, watch: MediaWatch) -> Envelope {
    if !shared.we_offer(media_watch::WATCH) {
        return Envelope::error(ErrorCode::Unsupported, "this device doesn't show a PC's playback");
    }
    if !shared.toggle_on(peer, TOGGLE) {
        return Envelope::error(ErrorCode::Denied, "media is off for this device");
    }
    if !watch.is_valid() {
        return Envelope::error(ErrorCode::BadMessage, "invalid watch offer");
    }
    let platform = shared.platform.clone();
    let peer = *peer;
    let MediaWatch { player, title, file } = watch;
    match tokio::task::spawn_blocking(move || {
        platform.watch_offered(&peer, &player, title.as_deref(), file.as_ref())
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()))
    {
        Ok(()) => Envelope::empty(types::OK),
        Err(reason) => {
            tracing::warn!(reason, "can't show a watch offer");
            Envelope::error(ErrorCode::Internal, "can't show it")
        }
    }
}

async fn run_command(shared: &Arc<Shared>, peer: &DeviceId, command: MediaCommand) -> Envelope {
    if !shared.we_offer(SHARES) {
        return Envelope::error(ErrorCode::Unsupported, "media control is not available");
    }
    if !shared.toggle_on(peer, TOGGLE) {
        return Envelope::error(ErrorCode::Denied, "media is off for this device");
    }
    let Some(action) = MediaAction::parse(&command.action) else {
        return Envelope::error(ErrorCode::Unsupported, "unknown media action");
    };
    if action == MediaAction::Seek && command.position.is_none() {
        return Envelope::error(ErrorCode::BadMessage, "seek needs a position");
    }
    if !command.value_fits() {
        return Envelope::error(ErrorCode::BadMessage, "shuffle and loop need a value they take");
    }
    let platform = shared.platform.clone();
    let MediaCommand { player, position, value, .. } = command;
    let ran = tokio::task::spawn_blocking(move || {
        platform.media_command(&player, action, position, value.as_deref())
    })
    .await
    .unwrap_or_else(|e| Err(MediaError::Failed(e.to_string())));
    match ran {
        Ok(()) => Envelope::empty(types::OK),
        Err(MediaError::NotFound) => Envelope::error(ErrorCode::NotFound, "no such player"),
        Err(MediaError::Unsupported) => Envelope::error(ErrorCode::Unsupported, "the player can't do that"),
        Err(MediaError::Failed(reason)) => {
            tracing::warn!(reason, "media command failed");
            Envelope::error(ErrorCode::Internal, "failed")
        }
    }
}

/// The PC's side of `media.icon`: the app icon one of its players names.
async fn serve_icon(shared: &Arc<Shared>, peer: &DeviceId, env: &Envelope) -> Envelope {
    if !shared.we_offer(SHARES) {
        return Envelope::error(ErrorCode::Unsupported, "media control is not available");
    }
    if !shared.toggle_on(peer, TOGGLE) {
        return Envelope::error(ErrorCode::Denied, "media is off for this device");
    }
    let Some(key) = env.body::<MediaKey>().ok().map(|k| k.key).filter(|k| is_media_icon_key(k)) else {
        return Envelope::error(ErrorCode::BadMessage, "invalid media.icon");
    };
    if !shared.players.has_icon(&key) {
        return Envelope::error(ErrorCode::NotFound, "no player has that icon");
    }
    let platform = shared.platform.clone();
    let png = tokio::task::spawn_blocking(move || platform.media_icon(&key)).await.ok().flatten();
    match png {
        Some(png) if is_icon(&png) => Envelope::new(types::OK, &MediaIcon { png })
            .unwrap_or_else(|_| Envelope::error(ErrorCode::Internal, "failed")),
        Some(png) => {
            tracing::warn!(bytes = png.len(), "a player's icon isn't a PNG that fits");
            Envelope::error(ErrorCode::Internal, "can't send that icon")
        }
        None => Envelope::error(ErrorCode::NotFound, "no such icon"),
    }
}

/// A PNG of at most [`media_limits::ICON_BYTES`].
fn is_icon(png: &[u8]) -> bool {
    png.len() <= media_limits::ICON_BYTES && ArtFormat::of(png) == Some(ArtFormat::Png)
}

/// The phone's side of `media.icon`: a PC player's app icon, PNG.
pub(crate) async fn icon(shared: &Arc<Shared>, session: &Session, key: String) -> Result<Vec<u8>> {
    if !is_media_icon_key(&key) {
        return Err(Error::Protocol("invalid icon key".into()));
    }
    if !shared.store.get_peer(&session.peer)?.is_some_and(|p| p.caps.contains(SHARES)) {
        return Err(Error::Unsupported);
    }
    let env = Envelope::new(types::MEDIA_ICON, &MediaKey { key })?;
    let reply = session.request(env, crate::session::REQUEST_TIMEOUT).await?;
    let MediaIcon { png } = reply.expect_body(types::OK)?;
    if !is_icon(&png) {
        return Err(Error::Protocol("media.icon isn't a PNG that fits".into()));
    }
    Ok(png)
}

/// The large picture `key` names, if `peer` may have it: the error reply otherwise.
async fn art_for(
    shared: &Arc<Shared>,
    peer: &DeviceId,
    env: &Envelope,
) -> std::result::Result<(Vec<u8>, ArtFormat), Envelope> {
    if !shared.we_offer(SHARES) {
        return Err(Envelope::error(ErrorCode::Unsupported, "media control is not available"));
    }
    if !shared.toggle_on(peer, TOGGLE) {
        return Err(Envelope::error(ErrorCode::Denied, "media is off for this device"));
    }
    let key =
        env.expect_body::<MediaKey>(types::MEDIA_ART).ok().map(|k| k.key).filter(|k| is_media_art_key(k));
    let Some(key) = key else {
        return Err(Envelope::error(ErrorCode::BadMessage, "invalid media.art"));
    };
    if !shared.players.has_art_full(&key) {
        return Err(Envelope::error(ErrorCode::NotFound, "no player has that picture"));
    }
    let platform = shared.platform.clone();
    let art = tokio::task::spawn_blocking(move || platform.media_art(&key)).await.ok().flatten();
    let Some((bytes, format)) = art else {
        return Err(Envelope::error(ErrorCode::NotFound, "no such picture"));
    };
    if bytes.is_empty() || bytes.len() > media_limits::ART_FULL_BYTES || ArtFormat::of(&bytes) != Some(format)
    {
        tracing::warn!(bytes = bytes.len(), "a player's large picture isn't one that can be sent");
        return Err(Envelope::error(ErrorCode::Internal, "can't send that picture"));
    }
    Ok((bytes, format))
}

/// The PC's side of a `media/art` stream (its header already read): one
/// large picture of a current player.
pub(crate) async fn serve_art(
    shared: Arc<Shared>,
    peer: DeviceId,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let Ok(Ok(Some(bytes))) = tokio::time::timeout(FILE_TIMEOUT, read_frame(&mut recv)).await else {
        return;
    };
    let Ok(env) = Envelope::from_cbor(&bytes) else { return };
    let (art, format) = match art_for(&shared, &peer, &env).await {
        Ok(art) => art,
        Err(reply) => {
            let _ = write_frame(&mut send, &reply.reply_to(env.id).to_cbor()).await;
            let _ = send.finish();
            return;
        }
    };
    let meta = MediaArtMeta { length: art.len() as u64, format: format.as_str().into() };
    let Ok(reply) = Envelope::new(types::MEDIA_ART_META, &meta).map(|e| e.reply_to(env.id)) else { return };
    let sent = tokio::time::timeout(FILE_TIMEOUT, async {
        write_frame(&mut send, &reply.to_cbor()).await.is_ok() && send.write_all(&art).await.is_ok()
    })
    .await;
    if sent == Ok(true) {
        let _ = send.finish();
    }
}

/// The phone's side: one large picture `key` of a player on `session`'s PC,
/// refused before it's read when it's over [`media_limits::ART_FULL_BYTES`].
pub(crate) async fn read_art(
    shared: &Arc<Shared>,
    session: &Session,
    key: String,
) -> Result<(Vec<u8>, ArtFormat)> {
    if !is_media_art_key(&key) {
        return Err(Error::Protocol("invalid art key".into()));
    }
    if !shared.store.get_peer(&session.peer)?.is_some_and(|p| p.caps.contains(SHARES)) {
        return Err(Error::Unsupported);
    }
    let (mut send, mut recv) = session.open_bi().await?;
    let header =
        StreamHeader { svc: media_art::SERVICE.into(), op: media_art::OP.into(), v: media_art::VERSION };
    write_frame(&mut send, &Envelope::new(types::STREAM, &header)?.to_cbor()).await?;
    write_frame(&mut send, &Envelope::new(types::MEDIA_ART, &MediaKey { key })?.with_id(1).to_cbor()).await?;
    let _ = send.finish();
    let reply = match tokio::time::timeout(FILE_TIMEOUT, read_frame(&mut recv)).await {
        Ok(Ok(Some(bytes))) => Envelope::from_cbor(&bytes)?,
        Ok(Ok(None)) => return Err(Error::Offline),
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err(Error::Timeout),
    };
    let meta: MediaArtMeta = reply.expect_body(types::MEDIA_ART_META)?;
    let format = ArtFormat::parse(&meta.format).filter(|_| meta.is_valid());
    let (Some(format), Ok(length)) = (format, usize::try_from(meta.length)) else {
        return Err(Error::Protocol("media.art is too large or not a picture".into()));
    };
    let mut data = vec![0u8; length];
    tokio::time::timeout(FILE_TIMEOUT, recv.read_exact(&mut data))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|e| Error::Network(e.to_string()))?;
    if ArtFormat::of(&data) != Some(format) {
        return Err(Error::Protocol("media.art isn't what it said".into()));
    }
    Ok((data, format))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_round_trip() {
        for action in [
            MediaAction::Play,
            MediaAction::Pause,
            MediaAction::Next,
            MediaAction::Previous,
            MediaAction::Seek,
            MediaAction::Shuffle,
            MediaAction::Loop,
        ] {
            assert_eq!(MediaAction::parse(action.as_str()), Some(action));
        }
        assert_eq!(MediaAction::parse("rewind"), None);
    }

    #[test]
    fn art_formats_are_told_by_their_bytes() {
        assert_eq!(ArtFormat::of(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(ArtFormat::Jpeg));
        assert_eq!(ArtFormat::of(b"\x89PNG\r\n\x1a\n\0\0"), Some(ArtFormat::Png));
        assert_eq!(ArtFormat::of(b"GIF89a"), None);
        assert_eq!(ArtFormat::of(&[]), None);
        for format in [ArtFormat::Jpeg, ArtFormat::Png] {
            assert_eq!(ArtFormat::parse(format.as_str()), Some(format));
        }
        assert_eq!(ArtFormat::parse("gif"), None);
    }

    #[test]
    fn a_state_sent_later_says_how_old_its_position_is() {
        let players = Players::default();
        let player = |age: Option<u64>, position: Option<u64>| MediaPlayer {
            id: "spotify".into(),
            app: "Spotify".into(),
            title: Some("Song".into()),
            artist: None,
            album: None,
            playing: true,
            duration: Some(100_000),
            position,
            actions: Vec::new(),
            art_key: None,
            art: None,
            rate_milli: None,
            age,
            art_full: None,
            icon: None,
            shuffle: None,
            repeat: None,
        };
        players.set(vec![player(Some(200), Some(5_000)), player(None, None)]);
        players.lock().reported = Some(std::time::Instant::now() - std::time::Duration::from_millis(1_000));
        let state = players.for_session(&mut HashSet::new());
        let age = state.players[0].age.unwrap();
        assert!((1_200..5_000).contains(&age), "{age}");
        assert_eq!(state.players[1].age, None, "nothing to age without a position");
    }
}
