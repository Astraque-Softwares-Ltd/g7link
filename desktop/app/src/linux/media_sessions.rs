// SPDX-License-Identifier: GPL-3.0-or-later
//! What plays on this PC, for paired phones (`docs/protocol/media.md`): active
//! MPRIS2 media sessions on Linux via native in-process `zbus` D-Bus queries
//! and signal subscriptions (`PropertiesChanged`, `Seeked`, `NameOwnerChanged`),
//! artwork resolution/caching, and remote transport commands.
//!
//! Artwork comes from `file://` URLs and, bounded, from `https://` ones. Each
//! player gets a small picture sent inline and, when the cover is bigger, a
//! large one (`art_full`) a phone fetches on its own stream; the player's app
//! icon is fetched the same way.
//!
//! G7 Link's own MPRIS session (`org.mpris.MediaPlayer2.g7link`) is
//! excluded so a phone never gets its own music echoed back.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, mpsc};
use std::time::{Duration, Instant};

use g7link_core::{ArtFormat, MediaAction, MediaError, MediaPlayer};
use zbus::zvariant::{ObjectPath, OwnedValue};

use super::image::Bitmap;

/// Maximum inline artwork payload (256 KiB); larger covers are scaled and JPEG-encoded.
const ART_BYTES: usize = 256 * 1024;
/// The small picture's longest side, stepping down until it fits.
const ART_SIDES: [u32; 4] = [360, 280, 220, 160];
/// The large picture (`art_full`): at most 2 MiB (`media_limits::ART_FULL_BYTES`)
/// and 1200 px on its longer side, stepping down until it fits.
const ART_FULL_BYTES: usize = 2 * 1024 * 1024;
const ART_FULL_SIDES: [u32; 3] = [1200, 1000, 800];
/// Large pictures kept for phones to fetch.
const KEPT_FULL_ART: usize = 4;
/// The largest art file read from disk.
const ART_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// The largest picture loaded from an `https://` art URL, and how long that may take.
const HTTPS_ART_BYTES: usize = 4 * 1024 * 1024;
const HTTPS_ART_TIMEOUT: Duration = Duration::from_secs(5);
const HTTPS_ART_REDIRECTS: u32 = 3;
const MAX_ART_URL_BYTES: usize = 4096;
/// The largest picture from the network decoded (64 MiB of pixels).
const MAX_FETCHED_PIXELS: u64 = 4096 * 4096;
/// Artwork that couldn't be loaded is tried again after this.
const ART_RETRY: Duration = Duration::from_secs(10 * 60);
/// Artwork URLs remembered at most.
const MAX_ART_URLS: usize = 64;
/// App icons kept for phones to fetch, and desktop entries remembered.
const KEPT_ICONS: usize = 16;
const MAX_ICON_ENTRIES: usize = 32;
/// The longest desktop entry id looked up.
const MAX_ENTRY_BYTES: usize = 255;
const ACTIVE_REFRESH_INTERVAL: Duration = Duration::from_secs(3);
const IDLE_REFRESH_INTERVAL: Duration = Duration::from_secs(10);
const SIGNAL_COALESCE_WINDOW: Duration = Duration::from_millis(60);
const CALL_PAUSE_TTL: Duration = Duration::from_secs(60 * 30);

const PLAYERCTL_FORMAT: &str = "{{playerName}}\t{{status}}\t{{mpris:trackid}}\t{{xesam:title}}\t{{xesam:artist}}\t{{xesam:album}}\t{{mpris:length}}\t{{position}}\t{{mpris:artUrl}}";

type OnChange = Box<dyn Fn(Vec<LocalPlayer>) + Send + Sync>;

static STARTED: OnceLock<()> = OnceLock::new();
static PAUSED_FOR_CALL: Mutex<Option<PausedForCall>> = Mutex::new(None);
static CALL_MEDIA: Mutex<()> = Mutex::new(());
/// Large pictures by key, for `media.art`.
static FULL_ART: Mutex<FullArt> = Mutex::new(FullArt::new());
/// App icons by key, for `media.icon`.
static ICONS: Mutex<IconStore> = Mutex::new(IconStore::new());

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug)]
struct PausedForCall {
    players: Vec<String>,
    paused_at: Instant,
}

impl PausedForCall {
    fn take_valid(&mut self, now: Instant) -> Vec<String> {
        let taken = std::mem::take(&mut self.players);
        if now.saturating_duration_since(self.paused_at) <= CALL_PAUSE_TTL { taken } else { Vec::new() }
    }

    fn observe_playing(&mut self, id: &str, now: Instant) {
        if now.saturating_duration_since(self.paused_at) >= Duration::from_millis(600) {
            self.players.retain(|p| p != id);
        }
    }
}

/// Whether the MPRIS session for `id` corresponds to a video player or browser.
pub fn says_video(id: &str) -> bool {
    let lower = id.trim().to_ascii_lowercase();
    for needle in [
        "vlc",
        "mpv",
        "totem",
        "celluloid",
        "haruna",
        "dragon",
        "smplayer",
        "kodi",
        "chrome",
        "chromium",
        "firefox",
        "brave",
        "vivaldi",
        "opera",
        "msedge",
        "zen",
    ] {
        if lower.contains(needle) {
            return true;
        }
    }
    false
}

/// MPRIS `LoopStatus` values and the protocol's `loop` ones (`media_loop`).
const LOOP_STATUSES: [(&str, &str); 3] = [("None", "none"), ("Track", "track"), ("Playlist", "playlist")];

/// The protocol's `loop` for an MPRIS `LoopStatus`.
fn loop_from_mpris(status: &str) -> Option<&'static str> {
    LOOP_STATUSES.iter().find(|(mpris, _)| *mpris == status).map(|&(_, wire)| wire)
}

/// The MPRIS `LoopStatus` for a `loop` command's value.
fn loop_to_mpris(value: &str) -> Option<&'static str> {
    LOOP_STATUSES.iter().find(|(_, wire)| *wire == value).map(|&(mpris, _)| mpris)
}

/// Raw MPRIS player record before artwork bytes are loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawMprisPlayer {
    pub id: String,
    pub app: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub playing: bool,
    pub duration: Option<u64>,
    pub position: Option<u64>,
    pub actions: Vec<String>,
    pub art_url: Option<String>,
    /// Playback rate in thousandths, when it isn't 1 (MPRIS `Rate`).
    pub rate_milli: Option<u32>,
    pub shuffle: Option<bool>,
    /// The protocol's `loop`, from MPRIS `LoopStatus`.
    pub repeat: Option<&'static str>,
    /// The player's desktop entry id (MPRIS `DesktopEntry`).
    pub desktop_entry: Option<String>,
    /// When `position` was read.
    pub sampled_at: Option<Instant>,
}

/// One of this PC's players, as it goes to the phones. Its `art_full` is
/// served by [`full_art`] and its `icon` by [`app_icon`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPlayer {
    pub player: MediaPlayer,
    /// When `player.position` was read; the state's `age` counts from it.
    pub sampled_at: Option<Instant>,
}

/// A rate as the protocol carries it: thousandths from 250 to 4000, and
/// `None` for 1, for none at all (0) and for anything a receiver would drop.
fn protocol_rate(rate: f64) -> Option<u32> {
    if !rate.is_finite() {
        return None;
    }
    let milli = (rate * 1000.0).round();
    if !(250.0..=4000.0).contains(&milli) || milli == 1000.0 {
        return None;
    }
    Some(milli as u32)
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

fn art_key(bytes: &[u8]) -> String {
    format!("{:016x}", fnv(bytes))
}

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// A JPEG's or PNG's format and size from its header, without decoding it.
/// Whether a picture's bytes run to its format's own end: a JPEG's end-of-image marker, or a PNG's
/// IEND chunk (with its CRC).
fn complete(format: ArtFormat, bytes: &[u8]) -> bool {
    match format {
        ArtFormat::Jpeg => bytes.ends_with(&[0xFF, 0xD9]),
        ArtFormat::Png => bytes.len() >= 12 && &bytes[bytes.len() - 8..bytes.len() - 4] == b"IEND",
    }
}

fn picture_header(bytes: &[u8]) -> Option<(ArtFormat, u32, u32)> {
    if bytes.starts_with(PNG_SIGNATURE) {
        if bytes.get(12..16)? != b"IHDR" {
            return None;
        }
        let width = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
        let height = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
        return (width > 0 && height > 0).then_some((ArtFormat::Png, width, height));
    }
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut at = 2;
    while at + 1 < bytes.len() {
        if bytes[at] != 0xFF {
            return None;
        }
        let marker = bytes[at + 1];
        at += if marker == 0xFF { 1 } else { 2 };
        match marker {
            0xFF | 0x01 | 0xD0..=0xD8 => continue,
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let length = usize::from(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?));
        if length < 2 {
            return None;
        }
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let frame = bytes.get(at + 2..at + length)?;
            let height = u32::from(u16::from_be_bytes(frame.get(1..3)?.try_into().ok()?));
            let width = u32::from(u16::from_be_bytes(frame.get(3..5)?.try_into().ok()?));
            return (width > 0 && height > 0).then_some((ArtFormat::Jpeg, width, height));
        }
        at += length;
    }
    None
}

/// A picture as it's sent: its key is the FNV-1a hash of its bytes.
#[derive(Clone, PartialEq, Eq)]
struct Art {
    key: String,
    bytes: Vec<u8>,
    kind: ArtFormat,
    /// Its longer side, when known.
    longest: Option<u32>,
}

impl Art {
    fn new(bytes: Vec<u8>, kind: ArtFormat, longest: Option<u32>) -> Art {
        Art { key: art_key(&bytes), bytes, kind, longest }
    }
}

/// A player's artwork: the small picture sent inline and, when the cover
/// is bigger than that, the large one.
#[derive(Default)]
struct Pictures {
    small: Option<Art>,
    full: Option<Art>,
}

impl Pictures {
    /// Artwork from a file a player named: passed on as it is where it fits.
    fn from_file(bytes: &[u8]) -> Pictures {
        let header = picture_header(bytes);
        let mut decoded = Decoded { bytes, bitmap: None };
        let is_png = bytes.starts_with(b"\x89PNG");
        let small = if (is_png || bytes.starts_with(b"\xFF\xD8\xFF")) && bytes.len() <= ART_BYTES {
            let kind = if is_png { ArtFormat::Png } else { ArtFormat::Jpeg };
            Some(Art::new(bytes.to_vec(), kind, header.map(|(_, w, h)| w.max(h))))
        } else {
            decoded.get().and_then(|bitmap| reencode(bitmap, &ART_SIDES, 85, ART_BYTES))
        };
        let full = match header {
            Some((kind, w, h)) if w.max(h) <= ART_FULL_SIDES[0] && bytes.len() <= ART_FULL_BYTES => {
                Some(Art::new(bytes.to_vec(), kind, Some(w.max(h))))
            }
            _ => decoded.get().and_then(|bitmap| reencode(bitmap, &ART_FULL_SIDES, 88, ART_FULL_BYTES)),
        };
        Pictures::new(small, full)
    }

    /// Artwork from the network: only a JPEG or PNG of a sane size, decoded
    /// and encoded again, never passed on as it came.
    fn from_network(bytes: &[u8]) -> Pictures {
        let fits = |w: u32, h: u32| u64::from(w) * u64::from(h) <= MAX_FETCHED_PIXELS;
        let Some((format, width, height)) = picture_header(bytes).filter(|&(_, w, h)| fits(w, h)) else {
            return Pictures::default();
        };
        // A download cut short decodes with a grey band where the rest should be.
        if !complete(format, bytes) {
            return Pictures::default();
        }
        let Ok(bitmap) = super::image::decode(bytes) else { return Pictures::default() };
        let whole = bitmap.width > 0
            && bitmap.height > 0
            && fits(bitmap.width, bitmap.height)
            && bitmap.width.max(bitmap.height) == width.max(height);
        if !whole {
            return Pictures::default();
        }
        let small = reencode(&bitmap, &ART_SIDES, 85, ART_BYTES);
        let full = reencode(&bitmap, &ART_FULL_SIDES, 88, ART_FULL_BYTES);
        Pictures::new(small, full)
    }

    /// Keeps the large picture only when it's bigger than the small one.
    fn new(small: Option<Art>, full: Option<Art>) -> Pictures {
        let bigger = |full: &Art| match &small {
            None => true,
            Some(small) => {
                full.key != small.key && matches!((full.longest, small.longest), (Some(f), Some(s)) if f > s)
            }
        };
        let full = full.filter(bigger);
        Pictures { small, full }
    }

    fn is_empty(&self) -> bool {
        self.small.is_none() && self.full.is_none()
    }
}

/// A picture decoded at most once, when it's first needed.
struct Decoded<'a> {
    bytes: &'a [u8],
    bitmap: Option<Option<Bitmap>>,
}

impl Decoded<'_> {
    fn get(&mut self) -> Option<&Bitmap> {
        let bytes = self.bytes;
        self.bitmap.get_or_insert_with(|| super::image::decode(bytes).ok()).as_ref()
    }
}

/// `bitmap` as a JPEG of at most `limit` bytes, at the first of `sides` that fits.
fn reencode(bitmap: &Bitmap, sides: &[u32], quality: u8, limit: usize) -> Option<Art> {
    sides.iter().find_map(|&side| {
        let scaled = super::image::scale_to(bitmap, side);
        let jpeg = super::image::encode_jpeg(&scaled, quality).ok().filter(|jpeg| jpeg.len() <= limit)?;
        Some(Art::new(jpeg, ArtFormat::Jpeg, Some(scaled.width.max(scaled.height))))
    })
}

/// An art file's bytes: a regular file of at most [`ART_FILE_BYTES`].
/// Opened without blocking, so a FIFO in its place can't hang the thread.
fn read_art_file(path: &Path) -> Option<Vec<u8>> {
    let file: File =
        OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY).open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > ART_FILE_BYTES {
        return None;
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).ok()?);
    file.take(ART_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    (!bytes.is_empty() && bytes.len() as u64 <= ART_FILE_BYTES).then_some(bytes)
}

/// `url` when it's an `https://` URL fit to fetch: nothing that isn't
/// printable, and no user name or password.
fn https_art_url(url: &str) -> Option<&str> {
    let url = url.trim();
    if url.len() > MAX_ART_URL_BYTES || url.chars().any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return None;
    }
    let rest = url.get(8..).filter(|_| url.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("https://")))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    (!authority.is_empty() && !authority.contains('@')).then_some(url)
}

/// `curl` for one picture: no config file (`-q` first), https only even
/// after a redirect, a few redirects, 5 s in all, a size cap, no URL
/// globbing, and no cookies or credentials (none are given).
fn curl_art_args(url: &str) -> Vec<String> {
    let secs = HTTPS_ART_TIMEOUT.as_secs().to_string();
    [
        "-q",
        "--silent",
        "--fail",
        "--globoff",
        "--location",
        "--max-redirs",
        &HTTPS_ART_REDIRECTS.to_string(),
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        &secs,
        "--max-time",
        &secs,
        "--max-filesize",
        &HTTPS_ART_BYTES.to_string(),
        "-H",
        "Accept: image/jpeg, image/png",
        "-A",
        &format!("G7Link/{} (Linux)", env!("CARGO_PKG_VERSION")),
        "--",
        url,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// Fetches an `https://` art URL: at most [`HTTPS_ART_BYTES`], in at most
/// [`HTTPS_ART_TIMEOUT`] (curl stops itself; this stops curl a second later
/// if it hasn't). Errors never name the URL.
fn fetch_https_art(url: &str) -> Result<Vec<u8>, &'static str> {
    let url = https_art_url(url).ok_or("not an https URL")?;
    let mut child = Command::new("curl")
        .args(curl_art_args(url))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "curl isn't available")?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("no output");
    };
    let (sent, received) = mpsc::channel();
    let reader = std::thread::Builder::new().name("art-fetch".into()).spawn(move || {
        let mut body = Vec::new();
        let read = stdout.take(HTTPS_ART_BYTES as u64 + 1).read_to_end(&mut body);
        let _ = sent.send(read.map(|_| body));
    });
    let body = match reader {
        Ok(_) => match received.recv_timeout(HTTPS_ART_TIMEOUT + Duration::from_secs(1)) {
            Ok(Ok(body)) if body.len() <= HTTPS_ART_BYTES => Ok(body),
            Ok(Ok(_)) => Err("too large"),
            Ok(Err(_)) => Err("read failed"),
            Err(_) => Err("timed out"),
        },
        Err(_) => Err("can't read"),
    };
    if body.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|_| "curl failed")?;
    let body = body?;
    if !status.success() {
        return Err(if status.code() == Some(63) { "too large" } else { "fetch failed" });
    }
    Ok(body)
}

type Fetch = fn(&str) -> Result<Vec<u8>, &'static str>;

/// What a URL's artwork came to, kept so polls don't load it again. The
/// large picture's bytes are in [`FullArt`].
#[derive(Clone)]
enum Cached {
    Loaded { small: Option<(String, Vec<u8>)>, full: Option<String> },
    Failed(Instant),
}

/// A player's artwork for one state: the small picture and the large one's key.
#[derive(Default)]
struct Resolved {
    small: Option<(String, Vec<u8>)>,
    full: Option<String>,
}

/// Caches resolved artwork by URL so repeated polls do not load it again.
struct ArtCache {
    by_url: HashMap<String, Cached>,
    /// URLs in the last state, and in the one being made.
    shown: HashSet<String>,
    showing: HashSet<String>,
    full: &'static Mutex<FullArt>,
    fetch: Fetch,
}

impl ArtCache {
    fn new(full: &'static Mutex<FullArt>, fetch: Fetch) -> ArtCache {
        ArtCache { by_url: HashMap::new(), shown: HashSet::new(), showing: HashSet::new(), full, fetch }
    }

    fn resolve(&mut self, art_url: Option<&str>, now: Instant) -> Resolved {
        let Some(url) = art_url.map(str::trim).filter(|url| !url.is_empty()) else {
            return Resolved::default();
        };
        self.showing.insert(url.to_owned());
        match self.by_url.get(url) {
            Some(Cached::Failed(at)) if now.saturating_duration_since(*at) < ART_RETRY => {
                return Resolved::default();
            }
            Some(Cached::Loaded { small, full }) => {
                let held = full.as_ref().is_some_and(|key| lock(self.full).touch(key));
                // A large picture that went while its player was showing stays
                // gone (more players than pictures kept); one that comes back
                // is loaded again.
                if held || full.is_none() || self.shown.contains(url) {
                    return Resolved { small: small.clone(), full: full.clone().filter(|_| held) };
                }
            }
            _ => {}
        }
        if self.by_url.len() >= MAX_ART_URLS {
            self.by_url.retain(|url, _| self.shown.contains(url));
            if self.by_url.len() >= MAX_ART_URLS {
                self.by_url.clear();
            }
        }
        let cached = match self.load(url) {
            Some(pictures) if !pictures.is_empty() => {
                let full = pictures.full.map(|art| {
                    lock(self.full).insert(art.key.clone(), art.bytes, art.kind);
                    art.key
                });
                Cached::Loaded { small: pictures.small.map(|art| (art.key, art.bytes)), full }
            }
            _ => Cached::Failed(now),
        };
        self.by_url.insert(url.to_owned(), cached.clone());
        match cached {
            Cached::Loaded { small, full } => Resolved { small, full },
            Cached::Failed(_) => Resolved::default(),
        }
    }

    fn load(&self, url: &str) -> Option<Pictures> {
        if let Some(path) = resolve_art_file_url(url) {
            return Some(Pictures::from_file(&read_art_file(&path)?));
        }
        let bytes = (self.fetch)(https_art_url(url)?)
            .inspect_err(|error| tracing::debug!(error, "can't load a player's artwork"))
            .ok()?;
        Some(Pictures::from_network(&bytes))
    }

    /// Ends a state: its URLs are the ones shown.
    fn end_state(&mut self) {
        self.shown = std::mem::take(&mut self.showing);
    }
}

/// The large pictures phones may fetch, the most recently shown last.
struct FullArt {
    pictures: VecDeque<(String, Arc<Vec<u8>>, ArtFormat)>,
}

impl FullArt {
    const fn new() -> FullArt {
        FullArt { pictures: VecDeque::new() }
    }

    fn insert(&mut self, key: String, bytes: Vec<u8>, kind: ArtFormat) {
        self.pictures.retain(|(held, _, _)| *held != key);
        self.pictures.push_back((key, Arc::new(bytes), kind));
        while self.pictures.len() > KEPT_FULL_ART {
            self.pictures.pop_front();
        }
    }

    /// Whether `key` is held; it's now the most recently shown.
    fn touch(&mut self, key: &str) -> bool {
        let Some(at) = self.pictures.iter().position(|(held, _, _)| held == key) else { return false };
        if let Some(picture) = self.pictures.remove(at) {
            self.pictures.push_back(picture);
        }
        true
    }

    fn get(&self, key: &str) -> Option<(Arc<Vec<u8>>, ArtFormat)> {
        self.pictures
            .iter()
            .find(|(held, _, _)| held == key)
            .map(|(_, bytes, kind)| (Arc::clone(bytes), *kind))
    }
}

/// The large picture a player's `art_full` names, for a phone's `media.art`
/// (through `media::local_art`).
pub fn full_art(key: &str) -> Option<(Vec<u8>, ArtFormat)> {
    let (bytes, kind) = lock(&FULL_ART).get(key)?;
    Some((bytes.to_vec(), kind))
}

/// App icons by key, the most recently shown last.
struct IconStore {
    pngs: VecDeque<(String, Arc<Vec<u8>>)>,
}

impl IconStore {
    const fn new() -> IconStore {
        IconStore { pngs: VecDeque::new() }
    }

    fn insert(&mut self, key: String, png: Vec<u8>) {
        self.pngs.retain(|(held, _)| *held != key);
        self.pngs.push_back((key, Arc::new(png)));
        while self.pngs.len() > KEPT_ICONS {
            self.pngs.pop_front();
        }
    }

    fn touch(&mut self, key: &str) -> bool {
        let Some(at) = self.pngs.iter().position(|(held, _)| held == key) else { return false };
        if let Some(icon) = self.pngs.remove(at) {
            self.pngs.push_back(icon);
        }
        true
    }

    fn get(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        self.pngs.iter().find(|(held, _)| held == key).map(|(_, png)| Arc::clone(png))
    }
}

/// The icon a player's `icon` names, for a phone's `media.icon` (through
/// `media::local_icon`).
pub fn app_icon(key: &str) -> Option<Vec<u8>> {
    lock(&ICONS).get(key).map(|png| png.to_vec())
}

type IconLookup = fn(&str) -> Option<(String, Vec<u8>)>;

/// Whether an icon is what a phone takes: a PNG of at most 16 KiB with a
/// key of 16 lowercase hex digits.
fn valid_icon(key: &str, png: &[u8]) -> bool {
    key.len() == g7link_core::remote_limits::ICON_KEY_LEN
        && key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && png.len() <= g7link_core::remote_limits::MAX_ICON_BYTES
        && png.starts_with(PNG_SIGNATURE)
}

/// Players' app icons by desktop entry. Looked up (which may draw the icon
/// on the GUI thread) without holding the store's lock.
struct IconCache {
    by_entry: HashMap<String, String>,
    store: &'static Mutex<IconStore>,
    lookup: IconLookup,
}

impl IconCache {
    fn new(store: &'static Mutex<IconStore>, lookup: IconLookup) -> IconCache {
        IconCache { by_entry: HashMap::new(), store, lookup }
    }

    fn key_for(&mut self, entry: &str) -> Option<String> {
        if entry.len() > MAX_ENTRY_BYTES {
            return None;
        }
        if let Some(key) = self.by_entry.get(entry)
            && lock(self.store).touch(key)
        {
            return Some(key.clone());
        }
        let (key, png) = (self.lookup)(entry).filter(|(key, png)| valid_icon(key, png))?;
        lock(self.store).insert(key.clone(), png);
        if self.by_entry.len() >= MAX_ICON_ENTRIES {
            self.by_entry.clear();
        }
        self.by_entry.insert(entry.to_owned(), key.clone());
        Some(key)
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) =
                ((bytes[i + 1] as char).to_digit(16), (bytes[i + 2] as char).to_digit(16))
        {
            out.push(((hi << 4) | lo) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resolves a `file://` MPRIS `artUrl` (with percent-encoding) to a local filesystem path.
pub(crate) fn resolve_art_file_url(art_url: &str) -> Option<PathBuf> {
    let trimmed = art_url.trim().trim_matches('\'').trim_matches('"').trim();
    let rest = trimmed.strip_prefix("file://")?;
    let decoded = percent_decode(rest);
    if decoded.trim().is_empty() { None } else { Some(PathBuf::from(decoded)) }
}

/// Watches this PC's active MPRIS2 media sessions on a background thread;
/// `on_change` is invoked whenever the player list or playback state meaningfully changes.
pub fn start<F>(on_change: F)
where
    F: Fn(Vec<LocalPlayer>) + Send + Sync + 'static,
{
    if STARTED.set(()).is_err() {
        return;
    }
    let on_change: OnChange = Box::new(on_change);
    let spawned = std::thread::Builder::new().name("media-sessions".into()).spawn(move || run(&on_change));
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "can't spawn Linux media-sessions watcher");
    }
}

fn run(on_change: &OnChange) {
    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    spawn_dbus_signal_watchers(wake_tx);

    let mut art_cache = ArtCache::new(&FULL_ART, fetch_https_art);
    let mut icons = IconCache::new(&ICONS, super::app_entry::icon_for_desktop_entry);
    let mut last: Option<(Vec<LocalPlayer>, Instant)> = None;
    loop {
        let players = snapshot(&mut art_cache, &mut icons);
        let any_playing = players.iter().any(|p| p.player.playing);
        let should_emit = match &last {
            None => true,
            Some((prev, prev_at)) => players_meaningfully_changed(prev, &players, prev_at.elapsed()),
        };
        if should_emit {
            on_change(players.clone());
            last = Some((players, Instant::now()));
        }
        let wait_timeout = if any_playing { ACTIVE_REFRESH_INTERVAL } else { IDLE_REFRESH_INTERVAL };
        if wake_rx.recv_timeout(wait_timeout).is_ok() {
            while wake_rx.recv_timeout(SIGNAL_COALESCE_WINDOW).is_ok() {}
        }
    }
}

fn spawn_dbus_signal_watchers(wake_tx: mpsc::Sender<()>) {
    let tx_props = wake_tx.clone();
    let _ = std::thread::Builder::new().name("mpris-props-watch".into()).spawn(move || {
        loop {
            let Ok(conn) = zbus::blocking::Connection::session() else {
                std::thread::sleep(Duration::from_secs(5));
                continue;
            };
            let Ok(rule) = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .path("/org/mpris/MediaPlayer2")
                .map(|b| b.build())
            else {
                return;
            };
            let Ok(iter) = zbus::blocking::MessageIterator::for_match_rule(rule, &conn, None) else {
                std::thread::sleep(Duration::from_secs(5));
                continue;
            };
            for msg in iter {
                let Ok(msg) = msg else { break };
                let hdr = msg.header();
                let member = hdr.member().map(|m| m.as_str()).unwrap_or("");
                if matches!(member, "PropertiesChanged" | "Seeked") && tx_props.send(()).is_err() {
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });

    let _ = std::thread::Builder::new().name("mpris-owner-watch".into()).spawn(move || {
        loop {
            let Ok(conn) = zbus::blocking::Connection::session() else {
                std::thread::sleep(Duration::from_secs(5));
                continue;
            };
            let Ok(rule) = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface("org.freedesktop.DBus")
                .and_then(|b| b.member("NameOwnerChanged"))
                .map(|b| b.build())
            else {
                return;
            };
            let Ok(iter) = zbus::blocking::MessageIterator::for_match_rule(rule, &conn, None) else {
                std::thread::sleep(Duration::from_secs(5));
                continue;
            };
            for msg in iter {
                let Ok(msg) = msg else { break };
                if let Ok((name, _old_owner, _new_owner)) =
                    msg.body().deserialize::<(String, String, String)>()
                    && let Some(suffix) = name.strip_prefix("org.mpris.MediaPlayer2.")
                    && !is_excluded_player(suffix)
                    && wake_tx.send(()).is_err()
                {
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

/// Compares two snapshots of players while ignoring normal playback progress
/// (at the player's rate) so periodic polling does not trigger spurious
/// UI/network updates. `elapsed` is the time between them, for players
/// whose positions weren't timed.
pub(crate) fn players_meaningfully_changed(
    old: &[LocalPlayer],
    new: &[LocalPlayer],
    elapsed: Duration,
) -> bool {
    if old.len() != new.len() {
        return true;
    }
    for (old, new) in old.iter().zip(new.iter()) {
        let (a, b) = (&old.player, &new.player);
        if a.id != b.id
            || a.app != b.app
            || a.title != b.title
            || a.artist != b.artist
            || a.album != b.album
            || a.playing != b.playing
            || a.duration != b.duration
            || a.actions != b.actions
            || a.art_key != b.art_key
            || a.art.is_some() != b.art.is_some()
            || a.rate_milli != b.rate_milli
            || a.shuffle != b.shuffle
            || a.repeat != b.repeat
            || a.art_full != b.art_full
            || a.icon != b.icon
        {
            return true;
        }
        match (a.position, b.position) {
            (None, None) => {}
            (Some(_), None) | (None, Some(_)) => return true,
            (Some(old_pos), Some(new_pos)) => {
                let between = match (old.sampled_at, new.sampled_at) {
                    (Some(then), Some(now)) => now.saturating_duration_since(then),
                    _ => elapsed,
                };
                let expected = if a.playing {
                    let rate = u128::from(a.rate_milli.unwrap_or(1000));
                    let moved = u64::try_from(between.as_millis() * rate / 1000).unwrap_or(u64::MAX);
                    old_pos.saturating_add(moved).min(a.duration.unwrap_or(u64::MAX))
                } else {
                    old_pos
                };
                if new_pos.abs_diff(expected) > 1500 {
                    return true;
                }
            }
        }
    }
    false
}

fn snapshot(art_cache: &mut ArtCache, icons: &mut IconCache) -> Vec<LocalPlayer> {
    let raw_players = query_raw_players();
    let now = Instant::now();
    let mut players = Vec::with_capacity(raw_players.len());
    for raw in raw_players {
        if raw.playing
            && let Ok(mut guard) = PAUSED_FOR_CALL.lock()
            && let Some(state) = guard.as_mut()
        {
            state.observe_playing(&raw.id, now);
        }
        let art = art_cache.resolve(raw.art_url.as_deref(), Instant::now());
        let icon = raw.desktop_entry.as_deref().and_then(|entry| icons.key_for(entry));
        let (art_key, art_bytes) = art.small.unzip();
        players.push(LocalPlayer {
            player: MediaPlayer {
                app: raw.app,
                id: raw.id,
                title: Some(raw.title),
                artist: raw.artist,
                album: raw.album,
                playing: raw.playing,
                duration: raw.duration,
                position: raw.position,
                actions: raw.actions,
                art_key,
                art: art_bytes,
                rate_milli: raw.rate_milli,
                age: None,
                art_full: art.full,
                icon,
                shuffle: raw.shuffle,
                repeat: raw.repeat.map(str::to_owned),
            },
            sampled_at: raw.sampled_at,
        });
    }
    art_cache.end_state();
    players.sort_by_key(|p| !p.player.playing);
    players
}

fn query_raw_players() -> Vec<RawMprisPlayer> {
    if let Some(players) = query_via_zbus() {
        return players;
    }
    if let Some(players) = query_via_playerctl() {
        return players;
    }
    query_via_gdbus().unwrap_or_default()
}

fn deserialize_string_list(msg: &zbus::message::Message) -> Option<Vec<String>> {
    let body = msg.body();
    body.deserialize::<Vec<String>>().ok().or_else(|| body.deserialize::<(Vec<String>,)>().ok().map(|(v,)| v))
}

fn deserialize_properties_map(msg: &zbus::message::Message) -> Option<HashMap<String, OwnedValue>> {
    let body = msg.body();
    body.deserialize::<HashMap<String, OwnedValue>>()
        .ok()
        .or_else(|| body.deserialize::<(HashMap<String, OwnedValue>,)>().ok().map(|(m,)| m))
}

fn deserialize_property_value(msg: &zbus::message::Message) -> Option<OwnedValue> {
    let body = msg.body();
    body.deserialize::<OwnedValue>().ok().or_else(|| body.deserialize::<(OwnedValue,)>().ok().map(|(v,)| v))
}

fn val_as_str(v: &OwnedValue) -> Option<String> {
    if let Ok(s) = <&str>::try_from(v) {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    if let Ok(op) = <ObjectPath<'_>>::try_from(&**v) {
        let s = op.as_str().trim();
        if !s.is_empty() {
            return Some(s.to_owned());
        }
    }
    None
}

fn val_as_artist(v: &OwnedValue) -> Option<String> {
    if let Some(s) = val_as_str(v) {
        return Some(s);
    }
    if let Ok(cloned) = v.try_clone()
        && let Ok(list) = Vec::<String>::try_from(cloned)
    {
        let joined: Vec<&str> =
            list.iter().map(String::as_str).map(str::trim).filter(|s| !s.is_empty()).collect();
        if !joined.is_empty() {
            return Some(joined.join(", "));
        }
    }
    None
}

fn val_as_i64(v: &OwnedValue) -> Option<i64> {
    i64::try_from(v)
        .ok()
        .or_else(|| u64::try_from(v).ok().and_then(|u| i64::try_from(u).ok()))
        .or_else(|| i32::try_from(v).ok().map(i64::from))
        .or_else(|| u32::try_from(v).ok().map(i64::from))
        .or_else(|| f64::try_from(v).ok().filter(|f| f.is_finite()).map(|f| f as i64))
}

fn val_as_bool(v: &OwnedValue) -> Option<bool> {
    bool::try_from(v).ok()
}

fn val_as_f64(v: &OwnedValue) -> Option<f64> {
    f64::try_from(v).ok().filter(|f| f.is_finite())
}

fn val_as_dict(v: &OwnedValue) -> Option<HashMap<String, OwnedValue>> {
    let cloned = v.try_clone().ok()?;
    HashMap::<String, OwnedValue>::try_from(cloned).ok()
}

pub(crate) fn parse_zbus_player_properties(
    bus_name: &str,
    root_props: Option<&HashMap<String, OwnedValue>>,
    player_props: &HashMap<String, OwnedValue>,
) -> Option<RawMprisPlayer> {
    let short_id = bus_name.strip_prefix("org.mpris.MediaPlayer2.").unwrap_or(bus_name);
    if is_excluded_player(short_id) {
        return None;
    }

    let metadata = player_props.get("Metadata").and_then(val_as_dict)?;
    let raw_title = metadata.get("xesam:title").and_then(val_as_str)?;
    let status = player_props.get("PlaybackStatus").and_then(val_as_str).unwrap_or_default();
    let artist = metadata.get("xesam:artist").and_then(val_as_artist);
    let album = metadata.get("xesam:album").and_then(val_as_str);
    let art_url = metadata.get("mpris:artUrl").and_then(val_as_str);
    let duration_us = metadata.get("mpris:length").and_then(val_as_i64).filter(|&v| v > 0);
    let position_us = player_props.get("Position").and_then(val_as_i64).filter(|&v| v >= 0);

    let duration = duration_us.map(|us| (us as u64) / 1000).filter(|&ms| ms > 0);
    let position = position_us.map(|us| {
        let ms = (us as u64) / 1000;
        duration.map_or(ms, |d| ms.min(d))
    });

    let desktop_entry = root_props.and_then(|r| r.get("DesktopEntry")).and_then(val_as_str);
    let identity = root_props.and_then(|r| r.get("Identity")).and_then(val_as_str).or(desktop_entry.clone());
    let mut app = identity.unwrap_or_else(|| readable_player_app(short_id));
    let mut title = raw_title;
    if (says_video(short_id) || says_video(&app))
        && let Some((clean, site)) = title.rsplit_once(" on ")
    {
        let site = site.trim();
        let clean = clean.trim();
        if !site.is_empty() && !clean.is_empty() && site.len() <= 28 {
            app = site.to_owned();
            title = clean.to_owned();
        }
    }

    let mut actions = Vec::new();
    if player_props.get("CanPlay").and_then(val_as_bool).unwrap_or(true) {
        actions.push("play".to_owned());
    }
    if player_props.get("CanPause").and_then(val_as_bool).unwrap_or(true) {
        actions.push("pause".to_owned());
    }
    if player_props.get("CanGoNext").and_then(val_as_bool).unwrap_or(true) {
        actions.push("next".to_owned());
    }
    if player_props.get("CanGoPrevious").and_then(val_as_bool).unwrap_or(true) {
        actions.push("previous".to_owned());
    }
    if player_props.get("CanSeek").and_then(val_as_bool).unwrap_or(duration.is_some()) {
        actions.push("seek".to_owned());
    }
    let rate_milli = player_props.get("Rate").and_then(val_as_f64).and_then(protocol_rate);
    let shuffle = player_props.get("Shuffle").and_then(val_as_bool);
    let repeat = player_props.get("LoopStatus").and_then(val_as_str).as_deref().and_then(loop_from_mpris);
    // Read-only without `CanControl`.
    let can_control = player_props.get("CanControl").and_then(val_as_bool) != Some(false);
    if can_control && shuffle.is_some() {
        actions.push("shuffle".to_owned());
    }
    if can_control && repeat.is_some() {
        actions.push("loop".to_owned());
    }

    Some(RawMprisPlayer {
        id: short_id.to_owned(),
        app,
        title,
        artist,
        album,
        playing: status.eq_ignore_ascii_case("Playing"),
        duration,
        position,
        actions,
        art_url,
        rate_milli,
        shuffle,
        repeat,
        desktop_entry,
        sampled_at: None,
    })
}

fn list_zbus_mpris_buses(conn: &zbus::blocking::Connection) -> Option<Vec<String>> {
    let reply = conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "ListNames",
            &(),
        )
        .ok()?;
    let all_names = deserialize_string_list(&reply)?;
    let mut buses = Vec::new();
    for name in all_names {
        let trimmed = name.trim();
        if let Some(suffix) = trimmed.strip_prefix("org.mpris.MediaPlayer2.")
            && !is_excluded_player(suffix)
            && !buses.iter().any(|existing| existing == trimmed)
        {
            buses.push(trimmed.to_owned());
        }
    }
    Some(buses)
}

fn query_via_zbus() -> Option<Vec<RawMprisPlayer>> {
    let conn = zbus::blocking::Connection::session().ok()?;
    let buses = list_zbus_mpris_buses(&conn)?;
    let mut players = Vec::new();
    for bus in buses {
        let root_props = conn
            .call_method(
                Some(bus.as_str()),
                "/org/mpris/MediaPlayer2",
                Some("org.freedesktop.DBus.Properties"),
                "GetAll",
                &("org.mpris.MediaPlayer2",),
            )
            .ok()
            .and_then(|msg| deserialize_properties_map(&msg));
        let Ok(player_msg) = conn.call_method(
            Some(bus.as_str()),
            "/org/mpris/MediaPlayer2",
            Some("org.freedesktop.DBus.Properties"),
            "GetAll",
            &("org.mpris.MediaPlayer2.Player",),
        ) else {
            continue;
        };
        let read_at = Instant::now();
        let Some(player_props) = deserialize_properties_map(&player_msg) else {
            continue;
        };
        if let Some(mut player) = parse_zbus_player_properties(&bus, root_props.as_ref(), &player_props) {
            player.sampled_at = Some(read_at);
            players.push(player);
        }
    }
    Some(players)
}

fn query_via_playerctl() -> Option<Vec<RawMprisPlayer>> {
    let output = Command::new("playerctl")
        .args(["--all-players", "metadata", "--format", PLAYERCTL_FORMAT])
        .output()
        .ok()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No players found") || output.stdout.is_empty() {
            return Some(Vec::new());
        }
        return None;
    }
    let read_at = Instant::now();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let sampled = |player: RawMprisPlayer| RawMprisPlayer { sampled_at: Some(read_at), ..player };
    Some(stdout.lines().filter_map(parse_playerctl_line).map(sampled).collect())
}

fn is_excluded_player(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    lower.is_empty()
        || lower == "g7link"
        || lower == "g7 link"
        || lower == "playerctld"
        || lower.starts_with("g7link.")
        || lower.ends_with(".g7link")
        || lower.ends_with(".playerctld")
}

fn readable_player_app(id: &str) -> String {
    let stripped = id.strip_prefix("org.mpris.MediaPlayer2.").unwrap_or(id);
    let base = stripped.split('.').next().unwrap_or(stripped).trim();
    match base.to_ascii_lowercase().as_str() {
        "" => "Media".to_owned(),
        "spotify" | "spotifymusic" => "Spotify".to_owned(),
        "vlc" => "VLC".to_owned(),
        "firefox" => "Firefox".to_owned(),
        "chromium" => "Chromium".to_owned(),
        "chrome" | "google-chrome" => "Chrome".to_owned(),
        "brave" => "Brave".to_owned(),
        "msedge" | "edge" => "Edge".to_owned(),
        "mpv" => "mpv".to_owned(),
        "rhythmbox" => "Rhythmbox".to_owned(),
        "amberol" => "Amberol".to_owned(),
        "elisa" => "Elisa".to_owned(),
        "audacious" => "Audacious".to_owned(),
        "clementine" => "Clementine".to_owned(),
        "strawberry" => "Strawberry".to_owned(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => "Media".to_owned(),
            }
        }
    }
}

/// Parses one line of `playerctl --all-players metadata --format ...` output.
pub(crate) fn parse_playerctl_line(line: &str) -> Option<RawMprisPlayer> {
    if line.trim().is_empty() {
        return None;
    }
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() < 4 {
        return None;
    }
    let player_name = fields.first().copied().unwrap_or("").trim();
    if is_excluded_player(player_name) {
        return None;
    }
    let status = fields.get(1).copied().unwrap_or("").trim();
    let _track_id = fields.get(2).copied().unwrap_or("").trim();
    let raw_title = fields.get(3).copied().unwrap_or("").trim();
    if raw_title.is_empty() {
        return None;
    }
    let artist = fields.get(4).map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_owned);
    let album = fields.get(5).map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_owned);
    let duration_us = fields.get(6).and_then(|s| s.trim().parse::<i64>().ok()).filter(|&v| v > 0);
    let position_us = fields.get(7).and_then(|s| s.trim().parse::<i64>().ok()).filter(|&v| v >= 0);
    let art_url = fields.get(8).map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_owned);

    let duration = duration_us.map(|us| (us as u64) / 1000).filter(|&ms| ms > 0);
    let position = position_us.map(|us| {
        let ms = (us as u64) / 1000;
        duration.map_or(ms, |d| ms.min(d))
    });

    let mut app = readable_player_app(player_name);
    let mut title = raw_title.to_owned();
    if matches!(app.as_str(), "Firefox" | "Chrome" | "Chromium" | "Brave" | "Edge")
        && let Some((clean, site)) = title.rsplit_once(" on ")
    {
        let site = site.trim();
        let clean = clean.trim();
        if !site.is_empty() && !clean.is_empty() && site.len() <= 28 {
            app = site.to_owned();
            title = clean.to_owned();
        }
    }

    let playing = status.eq_ignore_ascii_case("Playing");
    let mut actions = vec!["play".to_owned(), "pause".to_owned(), "next".to_owned(), "previous".to_owned()];
    if duration.is_some() {
        actions.push("seek".to_owned());
    }

    Some(RawMprisPlayer {
        id: player_name.to_owned(),
        app,
        title,
        artist,
        album,
        playing,
        duration,
        position,
        actions,
        art_url,
        rate_milli: None,
        shuffle: None,
        repeat: None,
        desktop_entry: None,
        sampled_at: None,
    })
}

/// Extracts `org.mpris.MediaPlayer2.*` bus names from `gdbus ListNames` output,
/// excluding `org.mpris.MediaPlayer2.g7link` and `playerctld`.
pub(crate) fn parse_mpris_bus_names(gdbus_output: &str) -> Vec<String> {
    let mut names = Vec::new();
    for token in gdbus_output.split(|c: char| {
        c == '\'' || c == '"' || c == ',' || c == '[' || c == ']' || c == '(' || c == ')' || c.is_whitespace()
    }) {
        let trimmed = token.trim();
        if let Some(suffix) = trimmed.strip_prefix("org.mpris.MediaPlayer2.")
            && !is_excluded_player(suffix)
            && !names.iter().any(|existing| existing == trimmed)
        {
            names.push(trimmed.to_owned());
        }
    }
    names
}

fn extract_gvariant_string(text: &str, key: &str) -> Option<String> {
    let pattern = format!("'{key}':");
    let start = text.find(&pattern)? + pattern.len();
    let rest = text[start..].trim_start();
    // Look for `<'value'>` or `<['value']>`
    let open_angle = rest.find('<')?;
    let inside = rest[open_angle + 1..].trim_start();
    let inside = inside.strip_prefix('[').map_or(inside, str::trim_start);
    let quote = inside.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let after_quote = &inside[quote.len_utf8()..];
    let end_quote = after_quote.find(quote)?;
    let val = after_quote[..end_quote].trim();
    if val.is_empty() { None } else { Some(val.to_owned()) }
}

fn extract_gvariant_i64(text: &str, key: &str) -> Option<i64> {
    let pattern = format!("'{key}':");
    let start = text.find(&pattern)? + pattern.len();
    let rest = text[start..].trim_start();
    let open = rest.find('<')?;
    let close = rest[open + 1..].find('>')?;
    let inner = rest[open + 1..open + 1 + close].trim();
    let num_str = inner.split_whitespace().next_back()?;
    num_str.parse::<i64>().ok()
}

fn extract_gvariant_bool(text: &str, key: &str) -> Option<bool> {
    let pattern = format!("'{key}':");
    let start = text.find(&pattern)? + pattern.len();
    let rest = text[start..].trim_start();
    let open = rest.find('<')?;
    let close = rest[open + 1..].find('>')?;
    match rest[open + 1..open + 1 + close].trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

pub(crate) fn parse_gdbus_player_properties(bus_name: &str, gdbus_output: &str) -> Option<RawMprisPlayer> {
    let short_id = bus_name.strip_prefix("org.mpris.MediaPlayer2.").unwrap_or(bus_name);
    if is_excluded_player(short_id) {
        return None;
    }
    let title = extract_gvariant_string(gdbus_output, "xesam:title")?;
    let status = extract_gvariant_string(gdbus_output, "PlaybackStatus").unwrap_or_default();
    let artist = extract_gvariant_string(gdbus_output, "xesam:artist");
    let album = extract_gvariant_string(gdbus_output, "xesam:album");
    let art_url = extract_gvariant_string(gdbus_output, "mpris:artUrl");
    let duration_us = extract_gvariant_i64(gdbus_output, "mpris:length").filter(|&v| v > 0);
    let position_us = extract_gvariant_i64(gdbus_output, "Position").filter(|&v| v >= 0);

    let duration = duration_us.map(|us| (us as u64) / 1000).filter(|&ms| ms > 0);
    let position = position_us.map(|us| {
        let ms = (us as u64) / 1000;
        duration.map_or(ms, |d| ms.min(d))
    });

    let mut actions = Vec::new();
    if extract_gvariant_bool(gdbus_output, "CanPlay").unwrap_or(true) {
        actions.push("play".to_owned());
    }
    if extract_gvariant_bool(gdbus_output, "CanPause").unwrap_or(true) {
        actions.push("pause".to_owned());
    }
    if extract_gvariant_bool(gdbus_output, "CanGoNext").unwrap_or(true) {
        actions.push("next".to_owned());
    }
    if extract_gvariant_bool(gdbus_output, "CanGoPrevious").unwrap_or(true) {
        actions.push("previous".to_owned());
    }
    if extract_gvariant_bool(gdbus_output, "CanSeek").unwrap_or(duration.is_some()) {
        actions.push("seek".to_owned());
    }

    Some(RawMprisPlayer {
        id: short_id.to_owned(),
        app: readable_player_app(short_id),
        title,
        artist,
        album,
        playing: status.eq_ignore_ascii_case("Playing"),
        duration,
        position,
        actions,
        art_url,
        rate_milli: None,
        shuffle: None,
        repeat: None,
        desktop_entry: None,
        sampled_at: None,
    })
}

fn list_gdbus_mpris_buses() -> Option<Vec<String>> {
    let out = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.DBus",
            "--object-path",
            "/org/freedesktop/DBus",
            "--method",
            "org.freedesktop.DBus.ListNames",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_mpris_bus_names(&String::from_utf8_lossy(&out.stdout)))
}

fn query_via_gdbus() -> Option<Vec<RawMprisPlayer>> {
    let buses = list_gdbus_mpris_buses()?;
    let mut players = Vec::new();
    for bus in buses {
        if let Ok(out) = Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                &bus,
                "--object-path",
                "/org/mpris/MediaPlayer2",
                "--method",
                "org.freedesktop.DBus.Properties.GetAll",
                "org.mpris.MediaPlayer2.Player",
            ])
            .output()
            && out.status.success()
        {
            let read_at = Instant::now();
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(player) = parse_gdbus_player_properties(&bus, &text) {
                players.push(RawMprisPlayer { sampled_at: Some(read_at), ..player });
            }
        }
    }
    Some(players)
}

fn mpris_bus_name(player: &str) -> String {
    let trimmed = player.trim();
    if trimmed.starts_with("org.mpris.MediaPlayer2.") {
        trimmed.to_owned()
    } else {
        format!("org.mpris.MediaPlayer2.{trimmed}")
    }
}

fn gdbus_player_call(bus_name: &str, method: &str, extra_args: &[&str]) -> bool {
    let method_full = format!("org.mpris.MediaPlayer2.Player.{method}");
    let mut cmd = Command::new("gdbus");
    cmd.args([
        "call",
        "--session",
        "--dest",
        bus_name,
        "--object-path",
        "/org/mpris/MediaPlayer2",
        "--method",
        &method_full,
    ]);
    cmd.args(extra_args);
    cmd.output().is_ok_and(|o| o.status.success())
}

fn gdbus_position_us(bus_name: &str) -> Option<i64> {
    let output = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            bus_name,
            "--object-path",
            "/org/mpris/MediaPlayer2",
            "--method",
            "org.freedesktop.DBus.Properties.Get",
            "org.mpris.MediaPlayer2.Player",
            "Position",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    parse_gdbus_position(&String::from_utf8_lossy(&output.stdout))
}

/// Reads a position from `gdbus`'s `(<int64 1500000>,)`.
fn parse_gdbus_position(output: &str) -> Option<i64> {
    let (_, rest) = output.split_once("int64 ")?;
    let end = rest.find(|c: char| !c.is_ascii_digit() && c != '-').unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Computes the relative MPRIS2 `Seek(Offset: x)` delta in microseconds from a
/// target position (`target_us`) and current position (`current_us`).
pub(crate) fn compute_seek_offset_us(target_us: i64, current_us: i64) -> i64 {
    target_us.saturating_sub(current_us)
}

fn resolve_zbus_target_bus(conn: &zbus::blocking::Connection, player: &str) -> Option<String> {
    let trimmed = player.trim();
    let buses = list_zbus_mpris_buses(conn).unwrap_or_default();
    if trimmed.is_empty() {
        return buses.into_iter().next();
    }
    let exact = mpris_bus_name(trimmed);
    if buses.iter().any(|b| b == &exact) {
        return Some(exact);
    }
    let prefix = format!("{exact}.");
    if let Some(found) = buses.into_iter().find(|b| b.starts_with(&prefix)) {
        return Some(found);
    }
    Some(exact)
}

/// A property a `shuffle` or `loop` command sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setting {
    Shuffle(bool),
    /// An MPRIS `LoopStatus`.
    Loop(&'static str),
}

impl Setting {
    /// What `action` sets with `value`; `None` for actions that aren't settings.
    fn of(action: MediaAction, value: Option<&str>) -> Result<Option<Setting>, MediaError> {
        let unknown = || MediaError::Failed(format!("{} needs a value it takes", action.as_str()));
        match action {
            MediaAction::Shuffle => match value {
                Some("on") => Ok(Some(Setting::Shuffle(true))),
                Some("off") => Ok(Some(Setting::Shuffle(false))),
                _ => Err(unknown()),
            },
            MediaAction::Loop => {
                value.and_then(loop_to_mpris).map(|s| Some(Setting::Loop(s))).ok_or_else(unknown)
            }
            MediaAction::Play
            | MediaAction::Pause
            | MediaAction::Next
            | MediaAction::Previous
            | MediaAction::Seek => Ok(None),
        }
    }

    fn property(self) -> (&'static str, zbus::zvariant::Value<'static>) {
        match self {
            Setting::Shuffle(on) => ("Shuffle", zbus::zvariant::Value::from(on)),
            Setting::Loop(status) => ("LoopStatus", zbus::zvariant::Value::from(status)),
        }
    }
}

fn zbus_set(conn: &zbus::blocking::Connection, bus_name: &str, setting: Setting) -> bool {
    let (name, value) = setting.property();
    conn.call_method(
        Some(bus_name),
        "/org/mpris/MediaPlayer2",
        Some("org.freedesktop.DBus.Properties"),
        "Set",
        &("org.mpris.MediaPlayer2.Player", name, value),
    )
    .is_ok()
}

fn zbus_player_command(
    conn: &zbus::blocking::Connection,
    bus_name: &str,
    action: MediaAction,
    position: Option<u64>,
) -> bool {
    match action {
        // Settings, never sent here.
        MediaAction::Shuffle | MediaAction::Loop => false,
        MediaAction::Play => conn
            .call_method(
                Some(bus_name),
                "/org/mpris/MediaPlayer2",
                Some("org.mpris.MediaPlayer2.Player"),
                "Play",
                &(),
            )
            .is_ok(),
        MediaAction::Pause => conn
            .call_method(
                Some(bus_name),
                "/org/mpris/MediaPlayer2",
                Some("org.mpris.MediaPlayer2.Player"),
                "Pause",
                &(),
            )
            .is_ok(),
        MediaAction::Next => conn
            .call_method(
                Some(bus_name),
                "/org/mpris/MediaPlayer2",
                Some("org.mpris.MediaPlayer2.Player"),
                "Next",
                &(),
            )
            .is_ok(),
        MediaAction::Previous => conn
            .call_method(
                Some(bus_name),
                "/org/mpris/MediaPlayer2",
                Some("org.mpris.MediaPlayer2.Player"),
                "Previous",
                &(),
            )
            .is_ok(),
        MediaAction::Seek => {
            let pos_us = i64::try_from(position.unwrap_or(0)).unwrap_or(0).saturating_mul(1000);
            if let Ok(meta_msg) = conn.call_method(
                Some(bus_name),
                "/org/mpris/MediaPlayer2",
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.mpris.MediaPlayer2.Player", "Metadata"),
            ) && let Some(meta_val) = deserialize_property_value(&meta_msg)
                && let Some(meta_dict) = val_as_dict(&meta_val)
                && let Some(track_id_str) = meta_dict.get("mpris:trackid").and_then(val_as_str)
                && let Ok(track_id) = ObjectPath::try_from(track_id_str.as_str())
                && conn
                    .call_method(
                        Some(bus_name),
                        "/org/mpris/MediaPlayer2",
                        Some("org.mpris.MediaPlayer2.Player"),
                        "SetPosition",
                        &(track_id, pos_us),
                    )
                    .is_ok()
            {
                return true;
            }

            let cur_us = conn
                .call_method(
                    Some(bus_name),
                    "/org/mpris/MediaPlayer2",
                    Some("org.freedesktop.DBus.Properties"),
                    "Get",
                    &("org.mpris.MediaPlayer2.Player", "Position"),
                )
                .ok()
                .and_then(|msg| deserialize_property_value(&msg))
                .and_then(|v| val_as_i64(&v))
                .unwrap_or(0);
            let offset_us = compute_seek_offset_us(pos_us, cur_us);
            conn.call_method(
                Some(bus_name),
                "/org/mpris/MediaPlayer2",
                Some("org.mpris.MediaPlayer2.Player"),
                "Seek",
                &(offset_us,),
            )
            .is_ok()
        }
    }
}

pub fn command(player: &str, action: MediaAction, position: Option<u64>) -> Result<(), MediaError> {
    command_value(player, action, position, None)
}

/// Runs `action` on one of this PC's players: `position` is a seek's,
/// `value` a `shuffle`'s (`on`, `off`) or a `loop`'s (`none`, `track`,
/// `playlist`). Shuffle and loop are set over D-Bus only.
pub fn command_value(
    player: &str,
    action: MediaAction,
    position: Option<u64>,
    value: Option<&str>,
) -> Result<(), MediaError> {
    let trimmed = player.trim();
    if trimmed.starts_with('-') || trimmed.chars().any(char::is_control) {
        return Err(MediaError::Failed("invalid player identifier".into()));
    }
    if let Some(setting) = Setting::of(action, value)? {
        return super::non_tokio(|| {
            let conn = zbus::blocking::Connection::session().map_err(|_| MediaError::Unsupported)?;
            let bus = resolve_zbus_target_bus(&conn, trimmed).ok_or(MediaError::NotFound)?;
            if zbus_set(&conn, &bus, setting) { Ok(()) } else { Err(MediaError::Unsupported) }
        });
    }

    if super::non_tokio(|| {
        let Ok(conn) = zbus::blocking::Connection::session() else {
            return false;
        };
        let Some(target_bus) = resolve_zbus_target_bus(&conn, trimmed) else {
            return false;
        };
        zbus_player_command(&conn, &target_bus, action, position)
    }) {
        return Ok(());
    }

    let mut cmd = Command::new("playerctl");
    if !trimmed.is_empty() {
        cmd.args(["--player", trimmed]);
    }
    match action {
        MediaAction::Play => {
            cmd.arg("play");
        }
        MediaAction::Pause => {
            cmd.arg("pause");
        }
        MediaAction::Next => {
            cmd.arg("next");
        }
        MediaAction::Previous => {
            cmd.arg("previous");
        }
        MediaAction::Seek => {
            let secs = (position.unwrap_or(0) as f64) / 1000.0;
            cmd.args(["position", &format!("{secs:.2}")]);
        }
        MediaAction::Shuffle | MediaAction::Loop => return Err(MediaError::Unsupported),
    }
    if cmd.status().is_ok_and(|s| s.success()) {
        return Ok(());
    }

    // Fallback to direct `gdbus` MPRIS2 call when D-Bus session or `playerctl` fails.
    let target_bus = if trimmed.is_empty() {
        list_gdbus_mpris_buses().and_then(|mut b| b.drain(..).next()).ok_or(MediaError::NotFound)?
    } else {
        mpris_bus_name(trimmed)
    };

    let ok = match action {
        MediaAction::Play => gdbus_player_call(&target_bus, "Play", &[]),
        MediaAction::Pause => gdbus_player_call(&target_bus, "Pause", &[]),
        MediaAction::Next => gdbus_player_call(&target_bus, "Next", &[]),
        MediaAction::Previous => gdbus_player_call(&target_bus, "Previous", &[]),
        MediaAction::Seek => {
            let target_us = i64::try_from(position.unwrap_or(0)).unwrap_or(0).saturating_mul(1000);
            // `Seek` is relative: without the current position it would jump to
            // the wrong place, so it doesn't jump at all.
            gdbus_position_us(&target_bus).is_some_and(|current_us| {
                let offset_str = compute_seek_offset_us(target_us, current_us).to_string();
                gdbus_player_call(&target_bus, "Seek", &[&offset_str])
            })
        }
        MediaAction::Shuffle | MediaAction::Loop => return Err(MediaError::Unsupported),
    };
    if ok {
        Ok(())
    } else {
        Err(MediaError::Failed(format!("MPRIS command {action:?} failed for {target_bus}")))
    }
}

pub fn pause_for_call() {
    let _ = std::thread::Builder::new().name("call-media-pause".into()).spawn(pause_for_call_blocking);
}

fn pause_for_call_blocking() {
    let _serial = CALL_MEDIA.lock().unwrap_or_else(|e| e.into_inner());
    let mut paused = Vec::new();

    if let Ok(conn) = zbus::blocking::Connection::session()
        && let Some(buses) = list_zbus_mpris_buses(&conn)
    {
        for bus in buses {
            let is_playing = conn
                .call_method(
                    Some(bus.as_str()),
                    "/org/mpris/MediaPlayer2",
                    Some("org.freedesktop.DBus.Properties"),
                    "Get",
                    &("org.mpris.MediaPlayer2.Player", "PlaybackStatus"),
                )
                .ok()
                .and_then(|msg| deserialize_property_value(&msg))
                .and_then(|v| val_as_str(&v))
                .is_some_and(|s| s.eq_ignore_ascii_case("Playing"));
            if is_playing
                && conn
                    .call_method(
                        Some(bus.as_str()),
                        "/org/mpris/MediaPlayer2",
                        Some("org.mpris.MediaPlayer2.Player"),
                        "Pause",
                        &(),
                    )
                    .is_ok()
            {
                let short_id = bus.strip_prefix("org.mpris.MediaPlayer2.").unwrap_or(&bus).to_owned();
                paused.push(short_id);
            }
        }
    } else if let Ok(out) = Command::new("playerctl")
        .args(["--all-players", "status", "--format", "{{playerName}}\t{{status}}"])
        .output()
        && out.status.success()
    {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            if let Some((name, status)) = line.split_once('\t') {
                let name = name.trim();
                if !is_excluded_player(name) && status.trim().eq_ignore_ascii_case("Playing") {
                    let _ = Command::new("playerctl").args(["--player", name, "pause"]).status();
                    paused.push(name.to_owned());
                }
            }
        }
    } else if let Some(players) = query_via_gdbus() {
        for p in players {
            if p.playing {
                let bus = mpris_bus_name(&p.id);
                if gdbus_player_call(&bus, "Pause", &[]) {
                    paused.push(p.id);
                }
            }
        }
    }

    if !paused.is_empty()
        && let Ok(mut guard) = PAUSED_FOR_CALL.lock()
    {
        *guard = Some(PausedForCall { players: paused, paused_at: Instant::now() });
    }
}

pub fn resume_after_call() {
    let _ = std::thread::Builder::new().name("call-media-resume".into()).spawn(resume_after_call_blocking);
}

fn resume_after_call_blocking() {
    let _serial = CALL_MEDIA.lock().unwrap_or_else(|e| e.into_inner());
    let players = PAUSED_FOR_CALL
        .lock()
        .ok()
        .and_then(|mut g| g.as_mut().map(|p| p.take_valid(Instant::now())))
        .unwrap_or_default();
    if players.is_empty() {
        return;
    }
    let conn = zbus::blocking::Connection::session().ok();
    for player in players {
        let bus = mpris_bus_name(&player);
        let resumed_via_zbus = conn.as_ref().is_some_and(|c| {
            c.call_method(
                Some(bus.as_str()),
                "/org/mpris/MediaPlayer2",
                Some("org.mpris.MediaPlayer2.Player"),
                "Play",
                &(),
            )
            .is_ok()
        });
        if !resumed_via_zbus {
            let ok = Command::new("playerctl")
                .args(["--player", &player, "play"])
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                let _ = gdbus_player_call(&bus, "Play", &[]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn sample_player(id: &str, playing: bool, position: Option<u64>) -> LocalPlayer {
        LocalPlayer {
            player: MediaPlayer {
                id: id.to_owned(),
                app: "Spotify".to_owned(),
                title: Some("Track 1".to_owned()),
                artist: Some("Artist".to_owned()),
                album: Some("Album".to_owned()),
                playing,
                duration: Some(200_000),
                position,
                actions: vec!["play".to_owned(), "pause".to_owned(), "seek".to_owned()],
                art_key: None,
                art: None,
                rate_milli: None,
                age: None,
                art_full: None,
                icon: None,
                shuffle: None,
                repeat: None,
            },
            sampled_at: None,
        }
    }

    /// A picture whose JPEG doesn't compress much: noise from `seed`.
    fn noisy(width: u32, height: u32, seed: u64) -> Bitmap {
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let [b, g, r, ..] = state.to_le_bytes();
            bgra.extend_from_slice(&[b, g, r, 255]);
        }
        Bitmap { width, height, bgra }
    }

    /// A smooth picture, which compresses well.
    fn smooth(width: u32, height: u32) -> Bitmap {
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                bgra.extend_from_slice(&[(x / 4 % 256) as u8, (y / 4 % 256) as u8, 128, 255]);
            }
        }
        Bitmap { width, height, bgra }
    }

    fn jpeg(bitmap: &Bitmap, quality: u8) -> Vec<u8> {
        super::super::image::encode_jpeg(bitmap, quality).unwrap()
    }

    fn longest(bytes: &[u8]) -> u32 {
        picture_header(bytes).map(|(_, w, h)| w.max(h)).expect("a JPEG or PNG")
    }

    fn store() -> &'static Mutex<FullArt> {
        Box::leak(Box::new(Mutex::new(FullArt::new())))
    }

    #[test]
    fn parses_playerctl_line_and_filters_self_player() {
        let line = "spotify\tPlaying\t/com/spotify/track/123\tMidnight City\tM83\tHurry Up, We're Dreaming\t243000000\t61500000\tfile:///tmp/cover%20art.jpg";
        let parsed = parse_playerctl_line(line).expect("valid playerctl line");
        assert_eq!(parsed.id, "spotify");
        assert_eq!(parsed.app, "Spotify");
        assert_eq!(parsed.title, "Midnight City");
        assert_eq!(parsed.artist.as_deref(), Some("M83"));
        assert_eq!(parsed.album.as_deref(), Some("Hurry Up, We're Dreaming"));
        assert!(parsed.playing);
        assert_eq!(parsed.duration, Some(243_000));
        assert_eq!(parsed.position, Some(61_500));
        assert_eq!(parsed.art_url.as_deref(), Some("file:///tmp/cover%20art.jpg"));

        // Self-player and playerctld must be ignored
        let self_line = "g7link\tPlaying\t/1\tPhone Song\tArtist\tAlbum\t1000000\t0\t";
        assert!(parse_playerctl_line(self_line).is_none());
        let daemon_line = "playerctld\tPlaying\t/1\tPhone Song\tArtist\tAlbum\t1000000\t0\t";
        assert!(parse_playerctl_line(daemon_line).is_none());
    }

    #[test]
    fn parses_mpris_bus_names_and_excludes_g7link() {
        let gdbus_out = "([':1.0', 'org.freedesktop.DBus', 'org.mpris.MediaPlayer2.spotify', 'org.mpris.MediaPlayer2.g7link', 'org.mpris.MediaPlayer2.playerctld', 'org.mpris.MediaPlayer2.vlc.instance42'],)";
        let buses = parse_mpris_bus_names(gdbus_out);
        assert_eq!(
            buses,
            vec![
                "org.mpris.MediaPlayer2.spotify".to_owned(),
                "org.mpris.MediaPlayer2.vlc.instance42".to_owned(),
            ]
        );
    }

    #[test]
    fn parses_gdbus_getall_player_properties() {
        let gdbus_props = "({'PlaybackStatus': <'Playing'>, 'Position': <int64 30000000>, 'Metadata': <{'mpris:length': <int64 180000000>, 'mpris:artUrl': <'file:///home/user/Album%20Cover.png'>, 'xesam:title': <'Starlight'>, 'xesam:artist': <['Muse']>, 'xesam:album': <'Black Holes and Revelations'>}>, 'CanPlay': <true>, 'CanPause': <true>, 'CanSeek': <true>},)";
        assert_eq!(parse_gdbus_position("(<int64 1500000>,)\n"), Some(1_500_000));
        assert_eq!(parse_gdbus_position("(<<int64 7>>,)"), Some(7));
        assert_eq!(parse_gdbus_position("(<'nope'>,)"), None);

        let player = parse_gdbus_player_properties("org.mpris.MediaPlayer2.vlc", gdbus_props)
            .expect("parse gdbus properties");
        assert_eq!(player.id, "vlc");
        assert_eq!(player.app, "VLC");
        assert_eq!(player.title, "Starlight");
        assert_eq!(player.artist.as_deref(), Some("Muse"));
        assert_eq!(player.album.as_deref(), Some("Black Holes and Revelations"));
        assert!(player.playing);
        assert_eq!(player.duration, Some(180_000));
        assert_eq!(player.position, Some(30_000));
    }

    #[test]
    fn players_meaningfully_changed_ignores_linear_progress_and_detects_seeks() {
        let old = vec![sample_player("spotify", true, Some(10_000))];
        assert!(!players_meaningfully_changed(&old, &old, Duration::ZERO));
        // 2 seconds later, position is 12_100 ms (within 1500 ms of expected 12_000 ms)
        let normal_tick = vec![sample_player("spotify", true, Some(12_100))];
        assert!(!players_meaningfully_changed(&old, &normal_tick, Duration::from_secs(2)));

        // User seeked forward to 45_000 ms
        let seeked = vec![sample_player("spotify", true, Some(45_000))];
        assert!(players_meaningfully_changed(&old, &seeked, Duration::from_secs(2)));

        // Playback paused
        let paused = vec![sample_player("spotify", false, Some(12_000))];
        assert!(players_meaningfully_changed(&old, &paused, Duration::from_secs(2)));
    }

    #[test]
    fn progress_counts_at_the_players_rate_and_a_changed_rate_is_a_change() {
        let start = Instant::now();
        let timed = |position: u64, rate: Option<u32>, after: Duration| {
            let mut p = sample_player("vlc", true, Some(position));
            p.player.rate_milli = rate;
            p.sampled_at = Some(start + after);
            p
        };
        let old = [timed(10_000, Some(2000), Duration::ZERO)];
        // Read 4 s apart (artwork took a while), 2 s since the last state: the
        // reads' own times count, at twice the speed.
        let steady = [timed(18_000, Some(2000), Duration::from_secs(4))];
        assert!(!players_meaningfully_changed(&old, &steady, Duration::from_secs(2)));
        let at_normal_speed = [timed(14_000, Some(2000), Duration::from_secs(4))];
        assert!(players_meaningfully_changed(&old, &at_normal_speed, Duration::from_secs(2)));
        let slowed = [timed(18_000, None, Duration::from_secs(4))];
        assert!(players_meaningfully_changed(&old, &slowed, Duration::from_secs(2)));

        let base = [sample_player("vlc", false, Some(0))];
        let changes: [fn(&mut MediaPlayer); 5] = [
            |p| p.shuffle = Some(true),
            |p| p.repeat = Some("track".into()),
            |p| p.art_full = Some("0011223344556677".into()),
            |p| p.icon = Some("8899aabbccddeeff".into()),
            |p| p.rate_milli = Some(1500),
        ];
        for change in changes {
            let mut changed = base.clone();
            change(&mut changed[0].player);
            assert!(players_meaningfully_changed(&base, &changed, Duration::ZERO));
        }
    }

    #[test]
    fn rates_are_carried_in_thousandths_when_a_receiver_takes_them() {
        assert_eq!(protocol_rate(1.5), Some(1500));
        assert_eq!(protocol_rate(0.25), Some(250));
        assert_eq!(protocol_rate(4.0), Some(4000));
        assert_eq!(protocol_rate(1.0), None, "absent means 1");
        for dropped in [0.0, -1.0, 0.1, 8.0, f64::NAN, f64::INFINITY] {
            assert_eq!(protocol_rate(dropped), None, "{dropped}");
        }
    }

    #[test]
    fn reads_a_pictures_size_from_its_header() {
        let small = jpeg(&smooth(300, 200), 80);
        assert_eq!(picture_header(&small), Some((ArtFormat::Jpeg, 300, 200)));
        let png = super::super::image::encode_png(&smooth(40, 70)).unwrap();
        assert_eq!(picture_header(&png), Some((ArtFormat::Png, 40, 70)));
        // A JPEG with an APP segment and fill bytes before its frame.
        let mut padded = vec![0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x04, 0xAA, 0xBB, 0xFF, 0xFF];
        padded.extend_from_slice(&[0xC2, 0x00, 0x0B, 0x08, 0x02, 0x58, 0x03, 0x20, 0x03, 0x01, 0x22, 0x00]);
        assert_eq!(picture_header(&padded), Some((ArtFormat::Jpeg, 800, 600)));
        let garbage: [&[u8]; 6] =
            [b"GIF89a", b"", b"\xFF\xD8", &[0xFF, 0xD8, 0xFF, 0xDA, 0, 2], &png[..20], &small[..30]];
        for garbage in garbage {
            assert_eq!(picture_header(garbage), None, "{garbage:?}");
        }
    }

    #[test]
    fn a_small_cover_file_passes_through_and_a_bigger_one_gets_a_large_picture() {
        let small = jpeg(&smooth(300, 300), 90);
        let pictures = Pictures::from_file(&small);
        assert_eq!(pictures.small.as_ref().map(|a| &a.bytes), Some(&small), "as it is");
        assert_eq!(pictures.small.as_ref().map(|a| a.key.clone()), Some(art_key(&small)));
        assert!(pictures.full.is_none(), "no bigger than the small one");

        let medium = jpeg(&noisy(700, 700, 1), 90);
        assert!((ART_BYTES..=ART_FULL_BYTES).contains(&medium.len()), "{}", medium.len());
        let pictures = Pictures::from_file(&medium);
        let small = pictures.small.expect("a small picture");
        assert!(small.bytes.len() <= ART_BYTES && longest(&small.bytes) <= 360);
        let full = pictures.full.expect("a large picture");
        assert_eq!(full.bytes, medium, "the original, which fits");
        assert_eq!((full.key, full.kind), (art_key(&medium), ArtFormat::Jpeg));
    }

    #[test]
    fn a_big_cover_file_is_encoded_again_under_the_limits() {
        // An uncompressed PNG: over 2 MiB, though only 1000 px wide.
        let heavy = super::super::image::encode_png(&smooth(1000, 700)).unwrap();
        assert!(heavy.len() > ART_FULL_BYTES);
        let full = Pictures::from_file(&heavy).full.expect("a large picture");
        assert_eq!(full.kind, ArtFormat::Jpeg);
        assert!(full.bytes.len() <= ART_FULL_BYTES);
        assert_eq!(longest(&full.bytes), 1000);

        // Over 1200 px on its longer side.
        let wide = super::super::image::encode_png(&smooth(1600, 120)).unwrap();
        let full = Pictures::from_file(&wide).full.expect("a large picture");
        assert_eq!(picture_header(&full.bytes), Some((ArtFormat::Jpeg, 1200, 90)));
    }

    #[test]
    fn art_files_are_read_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let cover = dir.path().join("cover.jpg");
        std::fs::write(&cover, jpeg(&smooth(64, 64), 80)).unwrap();
        assert!(read_art_file(&cover).is_some());

        let huge = dir.path().join("huge.jpg");
        File::create(&huge).unwrap().set_len(20 * 1024 * 1024).unwrap();
        assert_eq!(read_art_file(&huge), None, "over 16 MiB");
        std::fs::write(dir.path().join("empty.jpg"), b"").unwrap();
        assert_eq!(read_art_file(&dir.path().join("empty.jpg")), None);
        assert_eq!(read_art_file(dir.path()), None, "not a file");
        assert_eq!(read_art_file(&dir.path().join("missing.jpg")), None);
    }

    #[test]
    fn artwork_from_the_network_is_never_passed_on_as_it_came() {
        let small = jpeg(&smooth(300, 300), 90);
        let pictures = Pictures::from_network(&small);
        let art = pictures.small.expect("a small picture");
        assert_ne!(art.bytes, small, "encoded again");
        assert_eq!(longest(&art.bytes), 300);
        assert!(pictures.full.is_none());

        let cover = jpeg(&noisy(640, 480, 2), 90);
        let pictures = Pictures::from_network(&cover);
        assert!(pictures.small.as_ref().is_some_and(|a| longest(&a.bytes) == 360));
        let full = pictures.full.expect("a large picture");
        assert_ne!(full.bytes, cover);
        assert_eq!(picture_header(&full.bytes), Some((ArtFormat::Jpeg, 640, 480)));

        // Not JPEG or PNG, corrupt, or too many pixels for its size: nothing.
        let mut corrupt = small.clone();
        corrupt.truncate(small.len() / 3);
        let mut huge = PNG_SIGNATURE.to_vec();
        huge.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0x4E, 0x20, 0, 0, 0x4E, 0x20]);
        huge.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        let refused: [&[u8]; 4] = [b"GIF89a\x01\x00\x01\x00", b"<html></html>", &corrupt, &huge];
        for refused in refused {
            assert!(Pictures::from_network(refused).is_empty());
        }
    }

    #[test]
    fn only_plain_https_urls_are_fetched() {
        for url in [
            "https://i.scdn.co/image/ab67616d0000b273",
            " HTTPS://Example.com/cover.jpg?size=640 ",
            "https://example.com/c.jpg?from=a@b",
        ] {
            assert!(https_art_url(url).is_some(), "{url}");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_ART_URL_BYTES));
        for url in [
            "http://example.com/cover.jpg",
            "https://user:secret@example.com/cover.jpg",
            "https://user@example.com/cover.jpg",
            "https:///cover.jpg",
            "https://example.com/a cover.jpg",
            "https://exa\nmple.com/cover.jpg",
            "https://example.com\\@evil/cover.jpg",
            "file:///tmp/cover.jpg",
            "ftp://example.com/cover.jpg",
            "https:",
            long.as_str(),
        ] {
            assert_eq!(https_art_url(url), None, "{url}");
        }
        assert_eq!(fetch_https_art("http://example.com/cover.jpg"), Err("not an https URL"));
    }

    #[test]
    fn curl_is_bounded_and_carries_no_cookies_or_credentials() {
        let args = curl_art_args("https://example.com/cover.jpg");
        assert_eq!(args[0], "-q", "first, so no curlrc is read");
        let pair = |flag: &str, value: &str| args.windows(2).any(|w| w[0] == flag && w[1] == value);
        assert!(pair("--proto", "=https") && pair("--proto-redir", "=https"));
        assert!(pair("--max-redirs", "3") && pair("--max-time", "5"));
        assert!(pair("--max-filesize", &HTTPS_ART_BYTES.to_string()));
        assert!(args.iter().any(|a| a == "--globoff") && args.iter().any(|a| a == "--fail"));
        assert_eq!(&args[args.len() - 2..], ["--", "https://example.com/cover.jpg"]);
        for never in
            ["-b", "--cookie", "-c", "--cookie-jar", "-n", "--netrc", "-u", "--user", "-k", "--insecure"]
        {
            assert!(!args.iter().any(|a| a == never), "{never}");
        }
    }

    #[test]
    fn failed_artwork_is_tried_again_after_ten_minutes() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn failing(_: &str) -> Result<Vec<u8>, &'static str> {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Err("fetch failed")
        }
        let mut cache = ArtCache::new(store(), failing);
        let start = Instant::now();
        let url = Some("https://example.com/cover.jpg");
        for after in [0, 60, 9 * 60] {
            let art = cache.resolve(url, start + Duration::from_secs(after));
            assert!(art.small.is_none() && art.full.is_none());
            cache.end_state();
        }
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        cache.resolve(url, start + ART_RETRY + Duration::from_secs(1));
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);

        let missing = cache.resolve(Some("file:///nonexistent/g7link/cover.jpg"), start);
        assert!(missing.small.is_none());
        for never in ["http://example.com/c.jpg", "data:image/png;base64,AAAA", "https://u:p@example.com/c"] {
            cache.resolve(Some(never), start);
        }
        assert_eq!(CALLS.load(Ordering::SeqCst), 2, "only https URLs are fetched");
    }

    #[test]
    fn large_pictures_are_served_by_key_and_at_most_four_kept() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn covers(url: &str) -> Result<Vec<u8>, &'static str> {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(jpeg(&noisy(480, 480, fnv(url.as_bytes())), 90))
        }
        let full = store();
        let mut cache = ArtCache::new(full, covers);
        let now = Instant::now();
        let urls: Vec<String> = (0..5).map(|n| format!("https://example.com/{n}.jpg")).collect();
        let mut keys = Vec::new();
        for url in &urls {
            let art = cache.resolve(Some(url), now);
            cache.end_state();
            assert!(art.small.is_some());
            let key = art.full.expect("a large picture");
            let (bytes, kind) = lock(full).get(&key).unwrap();
            assert_eq!((kind, longest(&bytes), art_key(&bytes)), (ArtFormat::Jpeg, 480, key.clone()));
            keys.push(key);
        }
        assert_eq!(CALLS.load(Ordering::SeqCst), 5);
        assert_eq!(lock(full).pictures.len(), KEPT_FULL_ART);
        assert!(lock(full).get(&keys[0]).is_none(), "the oldest went");

        // Shown again: still cached, nothing fetched.
        assert_eq!(cache.resolve(Some(&urls[4]), now).full.as_ref(), Some(&keys[4]));
        cache.end_state();
        assert_eq!(CALLS.load(Ordering::SeqCst), 5);
        // Back after a while: its large picture is made again.
        assert_eq!(cache.resolve(Some(&urls[0]), now).full.as_ref(), Some(&keys[0]));
        assert_eq!(CALLS.load(Ordering::SeqCst), 6);
        assert!(lock(full).get(&keys[0]).is_some());
    }

    #[test]
    fn app_icons_are_kept_by_key_and_bad_ones_refused() {
        fn lookup(entry: &str) -> Option<(String, Vec<u8>)> {
            let png = [PNG_SIGNATURE, entry.as_bytes()].concat();
            match entry {
                "spotify" => Some(("0123456789abcdef".into(), png)),
                "upper" => Some(("0123456789ABCDEF".into(), png)),
                "short" => Some(("0123".into(), png)),
                "huge" => Some(("fedcba9876543210".into(), [PNG_SIGNATURE, &[0u8; 16 * 1024][..]].concat())),
                "gif" => Some(("00000000000000ff".into(), b"GIF89a".to_vec())),
                _ => None,
            }
        }
        let icons_store: &'static Mutex<IconStore> = Box::leak(Box::new(Mutex::new(IconStore::new())));
        let mut icons = IconCache::new(icons_store, lookup);
        assert_eq!(icons.key_for("spotify").as_deref(), Some("0123456789abcdef"));
        assert_eq!(icons.key_for("spotify").as_deref(), Some("0123456789abcdef"));
        let png = lock(icons_store).get("0123456789abcdef").map(|png| png.to_vec());
        assert_eq!(png, Some([PNG_SIGNATURE, &b"spotify"[..]].concat()));
        for refused in ["upper", "short", "huge", "gif", "missing"] {
            assert_eq!(icons.key_for(refused), None, "{refused}");
        }
        for n in 0..KEPT_ICONS + 1 {
            lock(icons_store).insert(format!("{n:016x}"), PNG_SIGNATURE.to_vec());
        }
        assert_eq!(lock(icons_store).pngs.len(), KEPT_ICONS);
    }

    #[test]
    fn shuffle_and_loop_take_only_their_values() {
        let of = Setting::of;
        assert_eq!(of(MediaAction::Shuffle, Some("on")), Ok(Some(Setting::Shuffle(true))));
        assert_eq!(of(MediaAction::Shuffle, Some("off")), Ok(Some(Setting::Shuffle(false))));
        assert_eq!(of(MediaAction::Loop, Some("none")), Ok(Some(Setting::Loop("None"))));
        assert_eq!(of(MediaAction::Loop, Some("track")), Ok(Some(Setting::Loop("Track"))));
        assert_eq!(of(MediaAction::Loop, Some("playlist")), Ok(Some(Setting::Loop("Playlist"))));
        for (action, value) in [
            (MediaAction::Shuffle, None),
            (MediaAction::Shuffle, Some("yes")),
            (MediaAction::Loop, None),
            (MediaAction::Loop, Some("Track")),
            (MediaAction::Loop, Some("on")),
        ] {
            assert!(of(action, value).is_err(), "{action:?} {value:?}");
        }
        assert_eq!(of(MediaAction::Play, Some("on")), Ok(None), "ignored for the rest");
        for (mpris, wire) in LOOP_STATUSES {
            assert_eq!(loop_from_mpris(mpris), Some(wire));
            assert_eq!(loop_to_mpris(wire), Some(mpris));
        }
        assert_eq!(loop_from_mpris("track"), None);
    }

    #[test]
    fn resolves_art_file_url_with_percent_encoding() {
        assert_eq!(
            resolve_art_file_url("file:///path/with%20spaces/cover%231.png"),
            Some(PathBuf::from("/path/with spaces/cover#1.png"))
        );
        assert!(resolve_art_file_url("https://example.com/cover.jpg").is_none());
        assert!(resolve_art_file_url("").is_none());
    }

    #[test]
    fn paused_for_call_expires_after_ttl() {
        let start = Instant::now();
        let mut state = PausedForCall { players: vec!["spotify".to_string()], paused_at: start };
        let expired = state.take_valid(start + CALL_PAUSE_TTL + Duration::from_secs(1));
        assert!(expired.is_empty());
    }

    #[test]
    fn parses_zbus_getall_player_properties_and_seek_offset() {
        use zbus::zvariant::Value;

        let mut meta: HashMap<&str, Value<'_>> = HashMap::new();
        meta.insert("xesam:title", Value::from("Instant Crush"));
        meta.insert("xesam:artist", Value::from(vec!["Daft Punk", "Julian Casablancas"]));
        meta.insert("xesam:album", Value::from("Random Access Memories"));
        meta.insert("mpris:length", Value::from(337_000_000_i64));
        meta.insert("mpris:artUrl", Value::from("file:///tmp/ram.jpg"));

        let mut player_props: HashMap<String, OwnedValue> = HashMap::new();
        player_props
            .insert("PlaybackStatus".to_owned(), OwnedValue::try_from(Value::from("Playing")).unwrap());
        player_props
            .insert("Position".to_owned(), OwnedValue::try_from(Value::from(45_000_000_i64)).unwrap());
        player_props.insert("Metadata".to_owned(), OwnedValue::try_from(Value::from(meta)).unwrap());
        player_props.insert("CanPlay".to_owned(), OwnedValue::try_from(Value::from(true)).unwrap());
        player_props.insert("CanPause".to_owned(), OwnedValue::try_from(Value::from(true)).unwrap());
        player_props.insert("CanSeek".to_owned(), OwnedValue::try_from(Value::from(true)).unwrap());

        let parsed =
            parse_zbus_player_properties("org.mpris.MediaPlayer2.spotify", None, &player_props).unwrap();
        assert_eq!(parsed.id, "spotify");
        assert_eq!(parsed.app, "Spotify");
        assert_eq!(parsed.title, "Instant Crush");
        assert_eq!(parsed.artist.as_deref(), Some("Daft Punk, Julian Casablancas"));
        assert_eq!(parsed.album.as_deref(), Some("Random Access Memories"));
        assert!(parsed.playing);
        assert_eq!(parsed.duration, Some(337_000));
        assert_eq!(parsed.position, Some(45_000));
        assert_eq!(parsed.art_url.as_deref(), Some("file:///tmp/ram.jpg"));
        assert_eq!((parsed.rate_milli, parsed.shuffle, parsed.repeat), (None, None, None));
        assert_eq!(parsed.actions, ["play", "pause", "next", "previous", "seek"]);
        assert_eq!(parsed.desktop_entry, None);

        let value = |v: Value<'_>| OwnedValue::try_from(v).unwrap();
        let mut root: HashMap<String, OwnedValue> = HashMap::new();
        root.insert("Identity".to_owned(), value(Value::from("Spotify Premium")));
        root.insert("DesktopEntry".to_owned(), value(Value::from("com.spotify.Client")));
        player_props.insert("Rate".to_owned(), value(Value::from(1.5_f64)));
        player_props.insert("Shuffle".to_owned(), value(Value::from(true)));
        player_props.insert("LoopStatus".to_owned(), value(Value::from("Playlist")));
        let parsed =
            parse_zbus_player_properties("org.mpris.MediaPlayer2.spotify", Some(&root), &player_props)
                .unwrap();
        assert_eq!(parsed.app, "Spotify Premium");
        assert_eq!(parsed.desktop_entry.as_deref(), Some("com.spotify.Client"));
        assert_eq!(
            (parsed.rate_milli, parsed.shuffle, parsed.repeat),
            (Some(1500), Some(true), Some("playlist"))
        );
        assert_eq!(parsed.actions, ["play", "pause", "next", "previous", "seek", "shuffle", "loop"]);

        // Read-only without CanControl; a rate of 0 or 1 is none; an
        // unknown LoopStatus isn't offered.
        player_props.insert("CanControl".to_owned(), value(Value::from(false)));
        player_props.insert("Rate".to_owned(), value(Value::from(0.0_f64)));
        player_props.insert("LoopStatus".to_owned(), value(Value::from("Forever")));
        let parsed =
            parse_zbus_player_properties("org.mpris.MediaPlayer2.spotify", Some(&root), &player_props)
                .unwrap();
        assert_eq!((parsed.rate_milli, parsed.shuffle, parsed.repeat), (None, Some(true), None));
        assert!(!parsed.actions.iter().any(|a| a == "shuffle" || a == "loop"));
        player_props.insert("Rate".to_owned(), value(Value::from(1.0_f64)));
        let parsed =
            parse_zbus_player_properties("org.mpris.MediaPlayer2.spotify", None, &player_props).unwrap();
        assert_eq!(parsed.rate_milli, None);

        assert_eq!(compute_seek_offset_us(60_000_000, 45_000_000), 15_000_000);
        assert_eq!(compute_seek_offset_us(10_000_000, 45_000_000), -35_000_000);
    }

    #[test]
    fn paused_for_call_returns_players_within_ttl() {
        let start = Instant::now();
        let mut state =
            PausedForCall { players: vec!["spotify".to_string(), "vlc".to_string()], paused_at: start };
        let valid = state.take_valid(start + Duration::from_secs(10));
        assert_eq!(valid, vec!["spotify".to_string(), "vlc".to_string()]);
        assert!(state.take_valid(start + Duration::from_secs(11)).is_empty());
    }
}
