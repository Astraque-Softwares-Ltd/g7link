// SPDX-License-Identifier: MPL-2.0
//! Images attached to messages (§Q4.5).
//!
//! Every image is typed by its first bytes, decoded with the limits set in
//! the decoder (12,000 px a side, 64 MiB decoded), scaled to at most 2,048 px
//! and encoded again, so nothing of the source's metadata (EXIF, GPS, colour
//! profiles, text chunks) is kept and the original bytes are never stored.
//!
//! The store: `<data dir>/images/<thread id or "pending">/<id>.<png|jpg>`,
//! each with a `<id>.json` beside it holding its name, size and owner. Files
//! 0600, directories 0700. Pending images are swept after 24 hours; the total
//! is capped at 512 MiB (the oldest pending images go first, then intake is
//! refused).

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use data_encoding::BASE64;
use image::codecs::gif::GifDecoder;
use image::codecs::jpeg::{JpegDecoder, JpegEncoder};
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngDecoder, PngEncoder};
use image::codecs::webp::WebPDecoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{ColorType, DynamicImage, GenericImageView, ImageDecoder, Limits, Rgb, RgbImage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::api::{
    ErrorCode, IMAGE_UPLOAD_IDLE_MS, ImageReadResult, ImageRef, MAX_IMAGE_CHUNK, MAX_IMAGE_FILE,
    MAX_IMAGE_NAME, MAX_IMAGE_SIDE, MAX_IMAGE_UPLOAD, MAX_IMAGES, MAX_THUMB_BYTES, MAX_THUMB_SIDE, RpcError,
    clean_cut, is_valid_image_id, is_valid_thread_id,
};
use crate::host::store::{atomic_write, now_ms};

/// An upload idle this long is dropped.
pub const UPLOAD_IDLE: Duration = Duration::from_millis(IMAGE_UPLOAD_IDLE_MS);
/// What the decoder may allocate, the decoded pixels included.
pub const MAX_DECODED: u64 = 64 * 1024 * 1024;
/// The longest side of a stored image.
pub const OUT_SIDE: u32 = 2048;
/// A PNG stays PNG below this, else it becomes a JPEG.
pub const PNG_KEEP: usize = 2 * 1024 * 1024;
pub const JPEG_QUALITY: u8 = 85;
/// A stored image larger than this is refused.
pub const MAX_STORED: usize = 5 * 1024 * 1024;
/// Everything stored, at most.
pub const STORE_CAP: u64 = 512 * 1024 * 1024;
/// Pending images kept at most; the oldest go first.
pub const MAX_PENDING: usize = 200;
/// A pending image no message took is removed after this.
pub const PENDING_LIFE: Duration = Duration::from_secs(24 * 60 * 60);
/// A path `image.add_path` takes, in bytes.
pub const MAX_ADD_PATH: usize = 4096;

const PENDING: &str = "pending";
const MAX_OWNER: usize = 128;
const MAX_META_FILE: u64 = 4096;
const MIN_THUMB_SIDE: u32 = 16;

// ---- Errors ----

/// Why an image was refused. Each becomes an [`RpcError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    /// Not PNG, JPEG, GIF or WebP by its content, not a regular file, or it
    /// doesn't decode.
    NotAnImage,
    /// Over a limit: the file, the pixels, the decoded size or the result.
    TooLarge,
    /// The file can't be opened or read.
    Unreadable,
    /// A path the host never opens (relative, hidden, or in an agent's own
    /// configuration folder).
    NotAllowed,
    /// No such image, or not one this request may use.
    NotFound,
    /// A malformed id, count or upload step.
    BadRequest(&'static str),
    /// The store is full of images that are in use.
    Full,
    /// Writing the store failed.
    Internal,
}

impl ImageError {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotAnImage => "That isn't an image G7 can send.",
            Self::TooLarge => "That image is too large.",
            Self::Unreadable => "That file can't be read.",
            Self::NotAllowed => "G7 doesn't open that file.",
            Self::NotFound => "That image isn't there any more.",
            Self::BadRequest(message) => message,
            Self::Full => "There's no room for more images. Remove some threads first.",
            Self::Internal => "Couldn't store the image.",
        }
    }

    pub fn code(self) -> ErrorCode {
        match self {
            Self::NotAnImage | Self::TooLarge | Self::Unreadable | Self::BadRequest(_) => {
                ErrorCode::BadRequest
            }
            Self::NotAllowed => ErrorCode::Denied,
            Self::NotFound => ErrorCode::NotFound,
            Self::Full => ErrorCode::Limit,
            Self::Internal => ErrorCode::Internal,
        }
    }
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ImageError {}

impl From<ImageError> for RpcError {
    fn from(e: ImageError) -> Self {
        RpcError::new(e.code(), e.message())
    }
}

fn from_image(e: image::ImageError) -> ImageError {
    match e {
        image::ImageError::Limits(_) => ImageError::TooLarge,
        _ => ImageError::NotAnImage,
    }
}

// ---- Types and encoding ----

/// A source type, told by the first bytes only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Png,
    Jpeg,
    Gif,
    WebP,
}

pub fn sniff(bytes: &[u8]) -> Option<Source> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Source::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(Source::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(Source::Gif)
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(Source::WebP)
    } else {
        None
    }
}

/// What a stored image is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Png,
    Jpeg,
}

impl Kind {
    pub fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }

    fn from_ext(ext: &str) -> Option<Kind> {
        match ext {
            "png" => Some(Self::Png),
            "jpg" => Some(Self::Jpeg),
            _ => None,
        }
    }
}

/// An image as it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub kind: Kind,
    pub data: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

/// Decodes with the limits set in the decoder before anything is allocated
/// for the pixels, and turns the image the way its EXIF says.
fn decode_limited(bytes: &[u8], source: Source) -> Result<DynamicImage, ImageError> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(MAX_DECODED);
    let input = Cursor::new(bytes);
    let mut decoder: Box<dyn ImageDecoder + '_> = match source {
        Source::Png => Box::new(PngDecoder::with_limits(input, limits.clone()).map_err(from_image)?),
        Source::Jpeg => Box::new(JpegDecoder::new(input).map_err(from_image)?),
        Source::Gif => Box::new(GifDecoder::new(input).map_err(from_image)?),
        Source::WebP => Box::new(WebPDecoder::new(input).map_err(from_image)?),
    };
    // The pixels count against the budget first; the decoder gets the rest.
    limits.reserve(decoder.total_bytes()).map_err(from_image)?;
    decoder.set_limits(limits).map_err(from_image)?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(from_image)?;
    image.apply_orientation(orientation);
    Ok(image)
}

/// `w`×`h` scaled so the longest side is at most `side`.
fn fitted(w: u32, h: u32, side: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= side || long == 0 {
        return (w, h);
    }
    let scale = |n: u32| {
        let scaled = (u64::from(n) * u64::from(side) + u64::from(long) / 2) / u64::from(long);
        u32::try_from(scaled).unwrap_or(side).clamp(1, side)
    };
    (scale(w), scale(h))
}

/// Scales down: a box filter for large steps (no big intermediate buffer), a
/// sharper one for small steps.
fn scale_to(image: &DynamicImage, side: u32) -> Option<DynamicImage> {
    let (w, h) = image.dimensions();
    let (nw, nh) = fitted(w, h, side);
    if (nw, nh) == (w, h) {
        return None;
    }
    Some(if w.max(h) >= side.saturating_mul(2) {
        image.thumbnail_exact(nw, nh)
    } else {
        image.resize_exact(nw, nh, FilterType::CatmullRom)
    })
}

fn to_8bit(image: DynamicImage) -> DynamicImage {
    match image.color() {
        ColorType::L8 | ColorType::La8 | ColorType::Rgb8 | ColorType::Rgba8 => image,
        ColorType::L16 => DynamicImage::ImageLuma8(image.into_luma8()),
        ColorType::La16 => DynamicImage::ImageLumaA8(image.into_luma_alpha8()),
        color if color.has_alpha() => DynamicImage::ImageRgba8(image.into_rgba8()),
        _ => DynamicImage::ImageRgb8(image.into_rgb8()),
    }
}

/// Transparent pixels over white, for JPEG.
fn flatten(image: &DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    let blend = |c: u8, a: u8| {
        let (c, a) = (u32::from(c), u32::from(a));
        u8::try_from((c * a + 255 * (255 - a) + 127) / 255).unwrap_or(u8::MAX)
    };
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y).0;
        Rgb([blend(p[0], p[3]), blend(p[1], p[3]), blend(p[2], p[3])])
    })
}

fn encode_png(image: &DynamicImage) -> Result<Vec<u8>, ImageError> {
    let mut out = Vec::new();
    image
        .write_with_encoder(PngEncoder::new_with_quality(
            &mut out,
            CompressionType::Default,
            PngFilter::Adaptive,
        ))
        .map_err(|_| ImageError::Internal)?;
    Ok(out)
}

fn encode_jpeg(image: &DynamicImage, quality: u8) -> Result<Vec<u8>, ImageError> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(&flatten(image))
        .map_err(|_| ImageError::Internal)?;
    Ok(out)
}

/// The intake (blocking): type by the first bytes, bounded decode, at most
/// [`OUT_SIDE`] a side, then PNG (PNG and GIF sources, and WebP with alpha,
/// while under [`PNG_KEEP`]) or JPEG at [`JPEG_QUALITY`]. A GIF keeps its
/// first frame only. Nothing of the source's metadata is written.
pub fn reencode(bytes: &[u8]) -> Result<Encoded, ImageError> {
    let source = sniff(bytes).ok_or(ImageError::NotAnImage)?;
    let decoded = decode_limited(bytes, source)?;
    let image = to_8bit(scale_to(&decoded, OUT_SIDE).unwrap_or(decoded));
    let (w, h) = image.dimensions();
    let png_first = match source {
        Source::Png | Source::Gif => true,
        Source::WebP => image.color().has_alpha(),
        Source::Jpeg => false,
    };
    let (kind, data) = match png_first.then(|| encode_png(&image)).transpose()? {
        Some(png) if png.len() < PNG_KEEP => (Kind::Png, png),
        _ => (Kind::Jpeg, encode_jpeg(&image, JPEG_QUALITY)?),
    };
    if data.len() > MAX_STORED {
        return Err(ImageError::TooLarge);
    }
    Ok(Encoded { kind, data, w, h })
}

/// A JPEG of a stored image at most `max` px a side and [`MAX_THUMB_BYTES`].
pub fn thumbnail(bytes: &[u8], max: u16) -> Result<Vec<u8>, ImageError> {
    let source = sniff(bytes).ok_or(ImageError::NotAnImage)?;
    let image = decode_limited(bytes, source)?;
    let mut side = u32::from(max.clamp(1, MAX_THUMB_SIDE)).max(MIN_THUMB_SIDE);
    loop {
        let scaled = scale_to(&image, side);
        let small = scaled.as_ref().unwrap_or(&image);
        for quality in [80, 65, 50] {
            let jpeg = encode_jpeg(small, quality)?;
            if jpeg.len() <= MAX_THUMB_BYTES {
                return Ok(jpeg);
            }
        }
        if side <= MIN_THUMB_SIDE {
            return Err(ImageError::TooLarge);
        }
        side = (side * 3 / 4).max(MIN_THUMB_SIDE);
    }
}

// ---- Names and ids ----

fn random_hex(bytes: usize) -> String {
    let mut out = String::with_capacity(bytes * 2);
    for _ in 0..bytes {
        out.push_str(&format!("{:02x}", rand::random::<u8>()));
    }
    out
}

/// A name to show: the last component of what the client sent, cleaned, on
/// one line, at most [`MAX_IMAGE_NAME`] bytes.
fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let one_line: String = base.chars().map(|c| if c == '\n' || c == '\t' { ' ' } else { c }).collect();
    let (cleaned, _) = clean_cut(one_line.trim(), MAX_IMAGE_NAME);
    let cleaned = cleaned.trim();
    if cleaned.is_empty() { "image".to_owned() } else { cleaned.to_owned() }
}

fn valid_owner(owner: &str) -> bool {
    !owner.is_empty() && owner.len() <= MAX_OWNER && !owner.chars().any(char::is_control)
}

// ---- Uploads ----

/// One `image.begin` … `image.end` in progress. A connection holds at most
/// one; the bytes may never exceed what was announced, and one idle for
/// [`UPLOAD_IDLE`] is over.
#[derive(Debug)]
pub struct Upload {
    id: String,
    name: String,
    declared: usize,
    data: Vec<u8>,
    next_seq: u32,
    last: Instant,
}

impl Upload {
    pub fn begin(name: &str, bytes: u32) -> Result<Upload, ImageError> {
        if bytes == 0 {
            return Err(ImageError::BadRequest("The image is empty."));
        }
        if bytes > MAX_IMAGE_UPLOAD {
            return Err(ImageError::TooLarge);
        }
        let declared = usize::try_from(bytes).map_err(|_| ImageError::TooLarge)?;
        Ok(Upload {
            id: random_hex(8),
            name: clean_name(name),
            declared,
            data: Vec::with_capacity(declared),
            next_seq: 0,
            last: Instant::now(),
        })
    }

    /// The `upload` the client names in its chunks.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Nothing arrived for [`UPLOAD_IDLE`]: the host drops it.
    pub fn expired(&self) -> bool {
        self.last.elapsed() >= UPLOAD_IDLE
    }

    /// Adds chunk `seq` (they count from 0, in order).
    pub fn chunk(&mut self, seq: u32, data: &str) -> Result<(), ImageError> {
        if self.expired() {
            return Err(ImageError::BadRequest("The upload took too long."));
        }
        if seq != self.next_seq {
            return Err(ImageError::BadRequest("A chunk arrived out of order."));
        }
        if data.len() > MAX_IMAGE_CHUNK {
            return Err(ImageError::BadRequest("A chunk is too large."));
        }
        let room = self.declared - self.data.len();
        let most =
            BASE64.decode_len(data.len()).map_err(|_| ImageError::BadRequest("A chunk isn't base64."))?;
        if most > room.saturating_add(2) {
            return Err(ImageError::TooLarge);
        }
        let bytes =
            BASE64.decode(data.as_bytes()).map_err(|_| ImageError::BadRequest("A chunk isn't base64."))?;
        if bytes.len() > room {
            return Err(ImageError::TooLarge);
        }
        self.data.extend_from_slice(&bytes);
        self.next_seq = self.next_seq.saturating_add(1);
        self.last = Instant::now();
        Ok(())
    }

    /// The name and the bytes, when every announced byte arrived in time.
    pub fn finish(self) -> Result<(String, Vec<u8>), ImageError> {
        if self.expired() {
            return Err(ImageError::BadRequest("The upload took too long."));
        }
        if self.data.len() != self.declared {
            return Err(ImageError::BadRequest("The upload is incomplete."));
        }
        Ok((self.name, self.data))
    }
}

// ---- The store ----

/// Where an image is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// Added, not yet sent; usable by `owner` only.
    Pending { owner: String },
    /// Sent (or queued) in this thread.
    Thread(String),
}

/// A stored image, as the host reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub id: String,
    pub name: String,
    pub bytes: u32,
    pub w: u16,
    pub h: u16,
    pub kind: Kind,
    pub created_ms: u64,
    pub place: Place,
}

impl ImageInfo {
    /// As messages, queues and drafts list it.
    pub fn image_ref(&self) -> ImageRef {
        ImageRef { id: self.id.clone(), name: self.name.clone(), bytes: self.bytes, w: self.w, h: self.h }
    }
}

/// A stored image file of a message, for the agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub path: PathBuf,
    /// `"image/png"` or `"image/jpeg"`.
    pub media_type: &'static str,
}

/// Claude's content block for an image, before the text block of the `user` line.
pub fn claude_block(media_type: &str, bytes: &[u8]) -> Value {
    json!({"type": "image", "source": {"type": "base64", "media_type": media_type, "data": BASE64.encode(bytes)}})
}

/// Codex's input for an image, before the text input.
pub fn codex_input(path: &str) -> Value {
    json!({"type": "localImage", "path": path})
}

/// A stored file's bytes (blocking), at most [`MAX_STORED`].
fn read_bounded(path: &Path) -> Result<Vec<u8>, ImageError> {
    let file = File::open(path).map_err(|_| ImageError::NotFound)?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(MAX_STORED).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ImageError::NotFound)?;
    if bytes.len() > MAX_STORED {
        return Err(ImageError::TooLarge);
    }
    Ok(bytes)
}

/// The `<id>.json` beside each image.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Meta {
    name: String,
    bytes: u32,
    w: u16,
    h: u16,
    kind: Kind,
    created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
}

#[derive(Debug, Clone)]
struct Held {
    meta: Meta,
    /// [`PENDING`] or a thread id.
    dir: String,
}

#[derive(Debug, Default)]
struct State {
    images: HashMap<String, Held>,
    total: u64,
}

#[derive(Debug)]
struct Inner {
    root: PathBuf,
    cap: u64,
    /// Folders `image.add_path` never opens anything in.
    refused: Vec<PathBuf>,
    state: Mutex<State>,
    /// One decode at a time: each may hold 64 MiB of pixels and more to scale them.
    intake: Arc<Semaphore>,
    thumbs: Arc<Semaphore>,
}

/// The host's image store. Its methods run their file work on the blocking pool.
#[derive(Debug, Clone)]
pub struct ImageStore {
    inner: Arc<Inner>,
}

fn private_dir(dir: &Path) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

fn read_meta(path: &Path) -> Option<Meta> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_META_FILE).read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// What is on disk: images with their sidecars. Stale temporary files, and
/// pending images without a readable sidecar, are removed.
fn scan(root: &Path) -> State {
    let mut state = State::default();
    let Ok(dirs) = fs::read_dir(root) else { return state };
    for dir in dirs.flatten() {
        let Ok(dir_name) = dir.file_name().into_string() else { continue };
        if (dir_name != PENDING && !is_valid_thread_id(&dir_name))
            || !dir.file_type().is_ok_and(|t| t.is_dir())
        {
            continue;
        }
        let dir_path = dir.path();
        let Ok(files) = fs::read_dir(&dir_path) else { continue };
        let mut images: Vec<(String, Kind, u64, u64)> = Vec::new();
        let mut sidecars: Vec<String> = Vec::new();
        for file in files.flatten() {
            let Ok(name) = file.file_name().into_string() else { continue };
            let Ok(meta) = file.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            if name.starts_with('.') {
                let _ = fs::remove_file(file.path());
                continue;
            }
            let Some((id, ext)) = name.split_once('.') else { continue };
            if !is_valid_image_id(id) {
                continue;
            }
            if ext == "json" {
                sidecars.push(id.to_owned());
            } else if let Some(kind) = Kind::from_ext(ext) {
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
                images.push((id.to_owned(), kind, meta.len(), modified));
            }
        }
        for (id, kind, size, modified) in images {
            let meta = read_meta(&dir_path.join(format!("{id}.json"))).filter(|m| m.kind == kind);
            let meta = match (meta, dir_name == PENDING) {
                (Some(meta), _) => meta,
                (None, true) => {
                    let _ = fs::remove_file(dir_path.join(format!("{id}.{}", kind.ext())));
                    continue;
                }
                (None, false) => Meta {
                    name: format!("image.{}", kind.ext()),
                    bytes: u32::try_from(size).unwrap_or(u32::MAX),
                    w: 0,
                    h: 0,
                    kind,
                    created_ms: modified,
                    owner: None,
                },
            };
            state.total += size;
            sidecars.retain(|s| s != &id);
            state.images.insert(
                id,
                Held {
                    meta: Meta { bytes: u32::try_from(size).unwrap_or(u32::MAX), ..meta },
                    dir: dir_name.clone(),
                },
            );
        }
        for orphan in sidecars {
            let _ = fs::remove_file(dir_path.join(format!("{orphan}.json")));
        }
    }
    state
}

fn info_of(id: &str, held: &Held) -> ImageInfo {
    ImageInfo {
        id: id.to_owned(),
        name: held.meta.name.clone(),
        bytes: held.meta.bytes,
        w: held.meta.w,
        h: held.meta.h,
        kind: held.meta.kind,
        created_ms: held.meta.created_ms,
        place: if held.dir == PENDING {
            Place::Pending { owner: held.meta.owner.clone().unwrap_or_default() }
        } else {
            Place::Thread(held.dir.clone())
        },
    }
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn file(&self, dir: &str, id: &str, kind: Kind) -> PathBuf {
        self.root.join(dir).join(format!("{id}.{}", kind.ext()))
    }

    fn sidecar(&self, dir: &str, id: &str) -> PathBuf {
        self.root.join(dir).join(format!("{id}.json"))
    }

    /// Removes an image's files (blocking). The caller already dropped it
    /// from the state.
    fn remove_files(&self, id: &str, held: &Held) {
        let _ = fs::remove_file(self.file(&held.dir, id, held.meta.kind));
        let _ = fs::remove_file(self.sidecar(&held.dir, id));
    }

    /// Takes out pending images older than [`PENDING_LIFE`]; returns them for removal.
    fn take_expired(state: &mut State, now: u64) -> Vec<(String, Held)> {
        let life = u64::try_from(PENDING_LIFE.as_millis()).unwrap_or(u64::MAX);
        let expired: Vec<String> = state
            .images
            .iter()
            .filter(|(_, h)| h.dir == PENDING && now.saturating_sub(h.meta.created_ms) >= life)
            .map(|(id, _)| id.clone())
            .collect();
        Self::take(state, expired)
    }

    fn take(state: &mut State, ids: Vec<String>) -> Vec<(String, Held)> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(held) = state.images.remove(&id) {
                state.total = state.total.saturating_sub(u64::from(held.meta.bytes));
                out.push((id, held));
            }
        }
        out
    }

    /// Makes room for `size` more bytes and one more pending image by taking
    /// out the oldest pending images; `Full` when that isn't enough.
    fn make_room(&self, state: &mut State, size: u64) -> Result<Vec<(String, Held)>, ImageError> {
        let mut pending: Vec<(u64, String, u64)> = state
            .images
            .iter()
            .filter(|(_, h)| h.dir == PENDING)
            .map(|(id, h)| (h.meta.created_ms, id.clone(), u64::from(h.meta.bytes)))
            .collect();
        pending.sort();
        let mut count = pending.len();
        let mut total = state.total;
        let mut out = Vec::new();
        for (_, id, bytes) in pending {
            if total.saturating_add(size) <= self.cap && count < MAX_PENDING {
                break;
            }
            total = total.saturating_sub(bytes);
            count -= 1;
            out.push(id);
        }
        if total.saturating_add(size) > self.cap {
            return Err(ImageError::Full);
        }
        Ok(Self::take(state, out))
    }

    /// Stores an encoded image as pending for `owner` (blocking).
    fn put_pending(&self, name: String, owner: String, encoded: Encoded) -> Result<ImageInfo, ImageError> {
        let size = u64::try_from(encoded.data.len()).map_err(|_| ImageError::TooLarge)?;
        let meta = Meta {
            name,
            bytes: u32::try_from(size).map_err(|_| ImageError::TooLarge)?,
            w: u16::try_from(encoded.w).map_err(|_| ImageError::TooLarge)?,
            h: u16::try_from(encoded.h).map_err(|_| ImageError::TooLarge)?,
            kind: encoded.kind,
            created_ms: now_ms(),
            owner: Some(owner),
        };
        let (reserved, gone) = {
            let mut state = self.state();
            let mut gone = Self::take_expired(&mut state, meta.created_ms);
            let reserved = self.make_room(&mut state, size).map(|more| {
                gone.extend(more);
                // Counted now so a concurrent intake can't pass the cap.
                state.total += size;
                let mut id = random_hex(16);
                while state.images.contains_key(&id) {
                    id = random_hex(16);
                }
                id
            });
            (reserved, gone)
        };
        for (old, held) in &gone {
            self.remove_files(old, held);
        }
        let id = reserved?;
        let written = self.write_pending(&id, &meta, &encoded.data);
        let mut state = self.state();
        match written {
            Ok(()) => {
                let held = Held { meta, dir: PENDING.to_owned() };
                let info = info_of(&id, &held);
                state.images.insert(id, held);
                Ok(info)
            }
            Err(e) => {
                state.total = state.total.saturating_sub(size);
                tracing::warn!("couldn't store an image: {e}");
                Err(ImageError::Internal)
            }
        }
    }

    fn write_pending(&self, id: &str, meta: &Meta, data: &[u8]) -> io::Result<()> {
        let dir = self.root.join(PENDING);
        private_dir(&dir)?;
        let path = self.file(PENDING, id, meta.kind);
        let result = (|| {
            let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
            file.write_all(data)?;
            file.sync_all()?;
            let json = serde_json::to_vec(meta).map_err(io::Error::other)?;
            atomic_write(&self.sidecar(PENDING, id), &json)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&path);
        }
        result
    }

    /// Opens a file a PC client picked (blocking): never a path in an agent's
    /// configuration folder or a hidden file, never anything but a regular
    /// file, and never more than [`MAX_IMAGE_FILE`] bytes read.
    fn read_picked(&self, path: &Path) -> Result<Vec<u8>, ImageError> {
        if !path.is_absolute() {
            return Err(ImageError::NotAllowed);
        }
        let real = fs::canonicalize(path).map_err(|_| ImageError::Unreadable)?;
        let hidden = real.file_name().is_some_and(|n| n.as_encoded_bytes().starts_with(b"."));
        // Resolved now: a folder created after the host started counts too.
        let refused = |root: &PathBuf| {
            real.starts_with(root) || fs::canonicalize(root).is_ok_and(|r| real.starts_with(r))
        };
        if hidden || self.refused.iter().any(refused) {
            return Err(ImageError::NotAllowed);
        }
        use rustix::fs::{Mode, OFlags};
        // Non-blocking, so a FIFO can't hold the open; no link at the end.
        let fd = rustix::fs::open(
            real.as_path(),
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|_| ImageError::Unreadable)?;
        let file = File::from(fd);
        let meta = file.metadata().map_err(|_| ImageError::Unreadable)?;
        if !meta.is_file() {
            return Err(ImageError::NotAnImage);
        }
        if meta.len() > MAX_IMAGE_FILE {
            return Err(ImageError::TooLarge);
        }
        let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
        file.take(MAX_IMAGE_FILE + 1).read_to_end(&mut bytes).map_err(|_| ImageError::Unreadable)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_IMAGE_FILE {
            return Err(ImageError::TooLarge);
        }
        Ok(bytes)
    }

    /// The stored bytes of an image (blocking), bounded.
    fn read_stored(&self, id: &str, held: &Held) -> Result<Vec<u8>, ImageError> {
        read_bounded(&self.file(&held.dir, id, held.meta.kind))
    }

    fn claim(
        &self,
        ids: &[String],
        owner: Option<&str>,
        thread_id: &str,
    ) -> Result<Vec<ImageInfo>, ImageError> {
        let mut state = self.state();
        for id in ids {
            let held = state.images.get(id).ok_or(ImageError::NotFound)?;
            let usable = if held.dir == PENDING {
                owner.is_none_or(|o| held.meta.owner.as_deref() == Some(o))
            } else {
                held.dir == thread_id
            };
            if !usable {
                return Err(ImageError::NotFound);
            }
        }
        let to_move: Vec<&String> =
            ids.iter().filter(|id| state.images.get(*id).is_some_and(|h| h.dir == PENDING)).collect();
        if !to_move.is_empty() {
            private_dir(&self.root.join(thread_id)).map_err(|_| ImageError::Internal)?;
        }
        let mut moved: Vec<(&String, Kind)> = Vec::new();
        let mut failed = false;
        for id in &to_move {
            let Some(kind) = state.images.get(*id).map(|h| h.meta.kind) else { continue };
            let image = fs::rename(self.file(PENDING, id, kind), self.file(thread_id, id, kind));
            if image.is_err() {
                failed = true;
                break;
            }
            if fs::rename(self.sidecar(PENDING, id), self.sidecar(thread_id, id)).is_err() {
                let _ = fs::rename(self.file(thread_id, id, kind), self.file(PENDING, id, kind));
                failed = true;
                break;
            }
            moved.push((id, kind));
        }
        if failed {
            for (id, kind) in moved {
                let _ = fs::rename(self.file(thread_id, id, kind), self.file(PENDING, id, kind));
                let _ = fs::rename(self.sidecar(thread_id, id), self.sidecar(PENDING, id));
            }
            return Err(ImageError::Internal);
        }
        for id in &to_move {
            if let Some(held) = state.images.get_mut(*id) {
                held.dir = thread_id.to_owned();
            }
        }
        Ok(ids.iter().filter_map(|id| state.images.get(id).map(|h| info_of(id, h))).collect())
    }
}

fn check_ids(ids: &[String]) -> Result<(), ImageError> {
    if ids.len() > MAX_IMAGES {
        return Err(ImageError::BadRequest("A message can carry up to 4 images."));
    }
    for (i, id) in ids.iter().enumerate() {
        if !is_valid_image_id(id) {
            return Err(ImageError::BadRequest("That isn't an image id."));
        }
        if ids[..i].contains(id) {
            return Err(ImageError::BadRequest("The same image is there twice."));
        }
    }
    Ok(())
}

impl ImageStore {
    /// Opens the store at `root` (blocking: it reads the directory once).
    /// `refused`: folders `image.add_path` never opens anything in, such as
    /// the agents' own configuration folders and the keyrings.
    pub fn open(root: PathBuf, refused: Vec<PathBuf>) -> io::Result<ImageStore> {
        Self::open_with_cap(root, refused, STORE_CAP)
    }

    /// [`ImageStore::open`] with another total cap (tests).
    pub fn open_with_cap(root: PathBuf, refused: Vec<PathBuf>, cap: u64) -> io::Result<ImageStore> {
        private_dir(&root)?;
        let mut state = scan(&root);
        let expired = Inner::take_expired(&mut state, now_ms());
        let inner = Inner {
            root,
            cap,
            refused,
            state: Mutex::new(state),
            intake: Arc::new(Semaphore::new(1)),
            thumbs: Arc::new(Semaphore::new(2)),
        };
        for (id, held) in &expired {
            inner.remove_files(id, held);
        }
        Ok(ImageStore { inner: Arc::new(inner) })
    }

    async fn blocking<T: Send + 'static>(
        &self,
        permits: Option<&Arc<Semaphore>>,
        job: impl FnOnce(&Inner) -> Result<T, ImageError> + Send + 'static,
    ) -> Result<T, ImageError> {
        let permit = match permits {
            Some(semaphore) => {
                Some(semaphore.clone().acquire_owned().await.map_err(|_| ImageError::Internal)?)
            }
            None => None,
        };
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let result = job(&inner);
            drop(permit);
            result
        })
        .await
        .map_err(|_| ImageError::Internal)?
    }

    /// `image.add_path`: a file on this PC, for PC clients only.
    pub async fn add_path(&self, path: &str, owner: &str) -> Result<ImageInfo, ImageError> {
        if path.len() > MAX_ADD_PATH || path.contains('\0') {
            return Err(ImageError::BadRequest("That path is too long."));
        }
        if !valid_owner(owner) {
            return Err(ImageError::BadRequest("Bad owner."));
        }
        let path = PathBuf::from(path);
        let owner = owner.to_owned();
        let intake = self.inner.intake.clone();
        self.blocking(Some(&intake), move |inner| {
            let bytes = inner.read_picked(&path)?;
            let name =
                clean_name(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            let encoded = reencode(&bytes)?;
            drop(bytes);
            inner.put_pending(name, owner, encoded)
        })
        .await
    }

    /// An image whose bytes arrived another way (an [`Upload`]).
    pub async fn add_bytes(&self, name: &str, bytes: Vec<u8>, owner: &str) -> Result<ImageInfo, ImageError> {
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_IMAGE_FILE {
            return Err(ImageError::TooLarge);
        }
        if !valid_owner(owner) {
            return Err(ImageError::BadRequest("Bad owner."));
        }
        let name = clean_name(name);
        let owner = owner.to_owned();
        let intake = self.inner.intake.clone();
        self.blocking(Some(&intake), move |inner| {
            let encoded = reencode(&bytes)?;
            drop(bytes);
            inner.put_pending(name, owner, encoded)
        })
        .await
    }

    pub fn info(&self, id: &str) -> Option<ImageInfo> {
        if !is_valid_image_id(id) {
            return None;
        }
        self.inner.state().images.get(id).map(|h| info_of(id, h))
    }

    /// The stored file (for Codex's `localImage`).
    pub fn path(&self, id: &str) -> Option<PathBuf> {
        if !is_valid_image_id(id) {
            return None;
        }
        self.inner.state().images.get(id).map(|h| self.inner.file(&h.dir, id, h.meta.kind))
    }

    /// Moves pending images into `thread_id` for a message (or a queued one).
    /// Each must be pending for `owner` (any owner when `None`), or already in
    /// that thread. Nothing moves unless all can.
    pub async fn claim(
        &self,
        ids: &[String],
        owner: Option<&str>,
        thread_id: &str,
    ) -> Result<Vec<ImageInfo>, ImageError> {
        check_ids(ids)?;
        if !is_valid_thread_id(thread_id) {
            return Err(ImageError::BadRequest("That isn't a thread id."));
        }
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids = ids.to_vec();
        let owner = owner.map(str::to_owned);
        let thread_id = thread_id.to_owned();
        self.blocking(None, move |inner| inner.claim(&ids, owner.as_deref(), &thread_id)).await
    }

    /// `image.drop`: removes a pending image of `owner` (any owner when `None`).
    pub async fn drop_pending(&self, id: &str, owner: Option<&str>) -> Result<(), ImageError> {
        if !is_valid_image_id(id) {
            return Err(ImageError::NotFound);
        }
        let id = id.to_owned();
        let owner = owner.map(str::to_owned);
        self.blocking(None, move |inner| {
            let held = {
                let mut state = inner.state();
                let usable = state.images.get(&id).is_some_and(|h| {
                    h.dir == PENDING && owner.as_deref().is_none_or(|o| h.meta.owner.as_deref() == Some(o))
                });
                if !usable {
                    return Err(ImageError::NotFound);
                }
                Inner::take(&mut state, vec![id.clone()])
            };
            for (id, held) in &held {
                inner.remove_files(id, held);
            }
            Ok(())
        })
        .await
    }

    /// `image.read`: a JPEG thumbnail. The host checks first that the client
    /// may see the image's [`Place`].
    pub async fn read(&self, id: &str, max: u16) -> Result<ImageReadResult, ImageError> {
        if !is_valid_image_id(id) {
            return Err(ImageError::NotFound);
        }
        if max == 0 || max > MAX_THUMB_SIDE {
            return Err(ImageError::BadRequest("The size must be 1 to 1024."));
        }
        let id = id.to_owned();
        let thumbs = self.inner.thumbs.clone();
        self.blocking(Some(&thumbs), move |inner| {
            let held = inner.state().images.get(&id).cloned().ok_or(ImageError::NotFound)?;
            let bytes = inner.read_stored(&id, &held)?;
            let jpeg = thumbnail(&bytes, max)?;
            Ok(ImageReadResult { mime: "image/jpeg".to_owned(), data: BASE64.encode(&jpeg) })
        })
        .await
    }

    /// A message's stored images, in order; each must be in `thread_id`
    /// (claimed). Each file is at most [`MAX_STORED`] bytes.
    pub fn files(&self, thread_id: &str, ids: &[String]) -> Result<Vec<StoredFile>, ImageError> {
        check_ids(ids)?;
        let state = self.inner.state();
        ids.iter()
            .map(|id| {
                let held = state.images.get(id).filter(|h| h.dir == thread_id).ok_or(ImageError::NotFound)?;
                Ok(StoredFile {
                    path: self.inner.file(&held.dir, id, held.meta.kind),
                    media_type: held.meta.kind.mime(),
                })
            })
            .collect()
    }

    /// Claude's `image` blocks for a message's images (read on the blocking
    /// pool), to go before its text block.
    pub async fn claude_blocks(&self, thread_id: &str, ids: &[String]) -> Result<Vec<Value>, ImageError> {
        let files = self.files(thread_id, ids)?;
        self.blocking(None, move |_| {
            files.iter().map(|file| Ok(claude_block(file.media_type, &read_bounded(&file.path)?))).collect()
        })
        .await
    }

    /// Codex's `localImage` inputs for a message's images, to go before its
    /// text input.
    pub fn codex_inputs(&self, thread_id: &str, ids: &[String]) -> Result<Vec<Value>, ImageError> {
        self.files(thread_id, ids)?
            .iter()
            .map(|file| file.path.to_str().map(codex_input).ok_or(ImageError::Internal))
            .collect()
    }

    /// Removes a thread's images with it.
    pub async fn remove_thread(&self, thread_id: &str) {
        if !is_valid_thread_id(thread_id) {
            return;
        }
        let thread_id = thread_id.to_owned();
        let _ = self
            .blocking(None, move |inner| {
                {
                    let mut state = inner.state();
                    let ids: Vec<String> = state
                        .images
                        .iter()
                        .filter(|(_, h)| h.dir == thread_id)
                        .map(|(id, _)| id.clone())
                        .collect();
                    Inner::take(&mut state, ids);
                }
                match fs::remove_dir_all(inner.root.join(&thread_id)) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(e) => {
                        tracing::warn!("couldn't remove a thread's images: {e}");
                        Err(ImageError::Internal)
                    }
                }
            })
            .await;
    }

    /// Removes pending images older than [`PENDING_LIFE`].
    pub async fn sweep(&self) {
        let _ = self
            .blocking(None, |inner| {
                let expired = Inner::take_expired(&mut inner.state(), now_ms());
                for (id, held) in &expired {
                    inner.remove_files(id, held);
                }
                Ok(())
            })
            .await;
    }

    /// Bytes stored now.
    pub fn total(&self) -> u64 {
        self.inner.state().total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_are_told_by_their_first_bytes() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest"), Some(Source::Png));
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some(Source::Jpeg));
        assert_eq!(sniff(b"GIF89a.."), Some(Source::Gif));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(Source::WebP));
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), None);
        assert_eq!(sniff(b"<svg"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn fitted_keeps_the_aspect_and_never_grows() {
        assert_eq!(fitted(4096, 2048, 2048), (2048, 1024));
        assert_eq!(fitted(100, 50, 2048), (100, 50));
        assert_eq!(fitted(12_000, 1, 2048), (2048, 1));
        assert_eq!(fitted(3, 9000, 2048), (1, 2048));
    }

    #[test]
    fn names_are_one_clean_line() {
        assert_eq!(clean_name("/home/me/Pictures/cat.png"), "cat.png");
        assert_eq!(clean_name("C:\\x\\dog.jpg"), "dog.jpg");
        assert_eq!(clean_name("a\nb\u{202e}c"), "a bc");
        assert_eq!(clean_name("   "), "image");
        assert!(clean_name(&"é".repeat(200)).len() <= MAX_IMAGE_NAME);
    }

    #[test]
    fn an_upload_never_takes_more_than_it_announced() {
        let mut up = Upload::begin("a.png", 4).unwrap();
        assert_eq!(up.chunk(1, "AAAA"), Err(ImageError::BadRequest("A chunk arrived out of order.")));
        up.chunk(0, &BASE64.encode(b"abc")).unwrap();
        assert_eq!(up.chunk(1, &BASE64.encode(b"de")), Err(ImageError::TooLarge));
        up.chunk(1, &BASE64.encode(b"d")).unwrap();
        assert_eq!(up.finish().unwrap(), ("a.png".to_owned(), b"abcd".to_vec()));
        assert_eq!(Upload::begin("x", MAX_IMAGE_UPLOAD + 1).unwrap_err(), ImageError::TooLarge);
        assert!(Upload::begin("x", 0).is_err());
        let short = Upload::begin("x", 10).unwrap();
        assert!(short.finish().is_err());
        let mut bad = Upload::begin("x", 10).unwrap();
        assert!(bad.chunk(0, "not base64!").is_err());
    }
}
