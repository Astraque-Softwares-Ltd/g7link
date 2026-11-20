// SPDX-License-Identifier: MPL-2.0
//! Track F (§Q10): the `@` file list on temporary trees, and image intake
//! and storage on hand-made images.
#![cfg(feature = "host")]

use std::io::Cursor;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use g7_agents::api::{ErrorCode, FileHit, MAX_IMAGE_UPLOAD};
use g7_agents::host::attachments::{self, ImageError, ImageStore, Kind, Place, Upload};
use g7_agents::host::files::{self, FileIndex, IndexLimits};
use image::codecs::gif::GifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{DynamicImage, Frame, Rgb, RgbImage, Rgba, RgbaImage};
use serde_json::json;

// ---- Helpers: trees ----

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn set_mtime(root: &Path, rel: &str, secs: u64) {
    let file = std::fs::File::options().write(true).open(root.join(rel)).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
}

/// A small repository: ignored, hidden and listed entries.
fn project() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap().join("work");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    write(&root, ".gitignore", "target/\n*.log\n");
    write(&root, ".ignore", "notes/\n");
    write(&root, ".env", "TOKEN=not-for-the-list\n");
    for rel in [
        ".github/workflows/ci.yml",
        "Cargo.toml",
        "README.md",
        "build.log",
        "notes/todo.md",
        "target/debug/files.rs",
        "docs/files-guide.md",
        "src/main.rs",
        "src/lib.rs",
        "src/host/mod.rs",
        "src/host/files.rs",
        "src/host/attachments.rs",
        "tests/files.rs",
    ] {
        write(&root, rel, "x");
        set_mtime(&root, rel, 1_700_000_000);
    }
    set_mtime(&root, "tests/files.rs", 1_700_000_200);
    set_mtime(&root, "src/host/files.rs", 1_700_000_100);
    (dir, root)
}

fn index(root: &Path) -> files::Index {
    files::build_index(root, &IndexLimits::default(), &AtomicBool::new(false)).unwrap()
}

fn paths(hits: &[FileHit]) -> Vec<&str> {
    hits.iter().map(|h| h.path.as_str()).collect()
}

fn ranked(index: &files::Index, query: &str) -> Vec<String> {
    files::rank(index, query, Some(50)).files.into_iter().map(|h| h.path).collect()
}

// ---- The file list ----

#[test]
fn gitignore_ignore_and_hidden_entries_stay_out() {
    let (_dir, root) = project();
    let index = index(&root);
    for listed in [
        ".github",
        ".github/workflows/ci.yml",
        "Cargo.toml",
        "docs",
        "docs/files-guide.md",
        "src/host/files.rs",
        "tests/files.rs",
    ] {
        assert!(index.contains(listed), "{listed} should be listed");
    }
    for left_out in [
        ".git",
        ".gitignore",
        ".ignore",
        ".env",
        "build.log",
        "notes",
        "notes/todo.md",
        "target",
        "target/debug/files.rs",
    ] {
        assert!(!index.contains(left_out), "{left_out} should not be listed");
    }
    assert_eq!(index.len(), 16);
    assert!(!index.partial());
}

#[test]
fn links_are_never_followed_or_listed() {
    let (dir, root) = project();
    let outside = std::fs::canonicalize(dir.path()).unwrap().join("outside");
    write(&outside, "secret.txt", "not yours");
    symlink(&outside, root.join("link")).unwrap();
    symlink(outside.join("secret.txt"), root.join("file-link")).unwrap();
    symlink(root.join("src"), root.join("inner-link")).unwrap();
    let index = index(&root);
    for name in ["link", "link/secret.txt", "file-link", "inner-link", "inner-link/main.rs"] {
        assert!(!index.contains(name), "{name} should not be listed");
    }
    assert!(ranked(&index, "secret").is_empty());
}

#[test]
fn a_hit_that_moved_outside_since_the_walk_is_dropped() {
    let (dir, root) = project();
    let index = index(&root);
    let outside = std::fs::canonicalize(dir.path()).unwrap().join("elsewhere");
    write(&outside, "files-guide.md", "x");
    std::fs::remove_dir_all(root.join("docs")).unwrap();
    symlink(&outside, root.join("docs")).unwrap();
    std::fs::remove_file(root.join("tests/files.rs")).unwrap();
    assert_eq!(ranked(&index, "files"), ["src/host/files.rs"]);
}

#[test]
fn past_the_entry_limit_the_list_is_partial() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("many");
    std::fs::create_dir_all(&root).unwrap();
    for i in 0..25_001 {
        std::fs::write(root.join(format!("f{i:05}.txt")), "").unwrap();
    }
    let index = index(&root);
    assert!(index.partial());
    assert!(index.len() <= files::MAX_ENTRIES);
    let found = files::rank(&index, "f0000", Some(5));
    assert!(found.more);

    // The exact boundary, with a small limit.
    let small = IndexLimits { max_entries: 100, ..IndexLimits::default() };
    let exact = tempfile::tempdir().unwrap();
    for i in 0..100 {
        std::fs::write(exact.path().join(format!("f{i:03}")), "").unwrap();
    }
    let built = files::build_index(exact.path(), &small, &AtomicBool::new(false)).unwrap();
    assert_eq!((built.len(), built.partial()), (100, false));
    std::fs::write(exact.path().join("f100"), "").unwrap();
    let built = files::build_index(exact.path(), &small, &AtomicBool::new(false)).unwrap();
    assert_eq!((built.len(), built.partial()), (100, true));
    let stopped = files::build_index(exact.path(), &small, &AtomicBool::new(true)).unwrap();
    assert!(stopped.partial());
}

#[test]
fn ranking_order_is_fixed() {
    let (_dir, root) = project();
    let index = index(&root);
    assert_eq!(ranked(&index, "files"), ["tests/files.rs", "src/host/files.rs", "docs/files-guide.md"]);
    assert_eq!(ranked(&index, "main"), ["src/main.rs"]);
    assert_eq!(ranked(&index, "MAIN"), ["src/main.rs"]);
    assert_eq!(
        ranked(&index, "host"),
        ["src/host", "src/host/mod.rs", "src/host/files.rs", "src/host/attachments.rs"]
    );
    assert_eq!(ranked(&index, "s/h/f"), ["src/host/files.rs"]);
    assert_eq!(
        ranked(&index, "rs"),
        [
            "src/lib.rs",
            "src/main.rs",
            "tests/files.rs",
            "src/host/mod.rs",
            "src/host/files.rs",
            "src/host/attachments.rs",
            ".github/workflows",
            ".github/workflows/ci.yml",
        ]
    );
    // Without a slash the letters must fall in one component.
    assert!(ranked(&index, "srcmain").is_empty());
    let first_three = files::rank(&index, "rs", Some(3));
    assert_eq!(paths(&first_three.files), ["src/lib.rs", "src/main.rs", "tests/files.rs"]);
    assert!(first_three.more);
    assert!(!files::rank(&index, "main", None).more);
}

#[test]
fn an_empty_query_shows_the_top_level_then_recent_files() {
    let (_dir, root) = project();
    let index = index(&root);
    let all = files::rank(&index, "", Some(8));
    assert_eq!(
        paths(&all.files),
        [
            ".github",
            "docs",
            "src",
            "tests",
            "tests/files.rs",
            "src/host/files.rs",
            ".github/workflows/ci.yml",
            "docs/files-guide.md"
        ]
    );
    // With room for all of it: the whole top level, then every deeper file
    // (deeper directories are found by typing).
    let everything = files::rank(&index, "", Some(50));
    assert_eq!(paths(&everything.files)[..6], [".github", "docs", "src", "tests", "Cargo.toml", "README.md"]);
    assert_eq!(everything.files.len(), 14);
    assert!(!everything.more);
    assert!(all.files[0].dir && !all.files[4].dir);
    let few = files::rank(&index, "  ", Some(4));
    assert_eq!(paths(&few.files), [".github", "docs", "tests/files.rs", "src/host/files.rs"]);
    assert!(few.more);
}

#[test]
fn dots_and_slashes_in_a_query_are_harmless() {
    let (_dir, root) = project();
    let index = index(&root);
    for query in ["../", "../../etc/passwd", "..", "/etc", "./src", "%2e%2e", "\0", "src/../../"] {
        for hit in files::rank(&index, query, Some(50)).files {
            assert!(files::is_safe_relative(&hit.path), "{query:?} gave {:?}", hit.path);
            let real = std::fs::canonicalize(root.join(&hit.path)).unwrap();
            assert!(real.starts_with(&root), "{query:?} gave {:?}", hit.path);
        }
    }
    assert_eq!(ranked(&index, "./src"), ranked(&index, "src"));
}

#[tokio::test]
async fn the_host_index_builds_on_use_and_again_after_a_turn() {
    let (_dir, root) = project();
    let files = FileIndex::default();
    let found = files.search(&root, "main", None).await.unwrap();
    assert_eq!(paths(&found.files), ["src/main.rs"]);
    write(&root, "src/brand_new.rs", "x");
    assert!(files.search(&root, "brand_new", None).await.unwrap().files.is_empty(), "the list is a snapshot");
    files.turn_ended(&root);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let found = files.search(&root, "brand_new", None).await.unwrap();
        if paths(&found.files) == ["src/brand_new.rs"] {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "the list wasn't rebuilt after the turn");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let missing = files.search(&root.join("gone"), "x", None).await.unwrap_err();
    assert_eq!(missing.code, ErrorCode::NotFound);
    let relative = files.search(Path::new("work"), "x", None).await.unwrap_err();
    assert_eq!(relative.code, ErrorCode::BadRequest);
    files.shutdown().await;
    assert_eq!(files.search(&root, "main", None).await.unwrap_err().code, ErrorCode::Unavailable);
    assert_eq!(files.folders(), 0);
}

#[tokio::test]
async fn at_most_so_many_folders_are_kept_and_idle_ones_go() {
    let dirs: Vec<tempfile::TempDir> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    for d in &dirs {
        write(d.path(), "a.txt", "x");
    }
    let files = FileIndex::new(IndexLimits { max_folders: 2, ..IndexLimits::default() });
    for d in &dirs {
        files.search(d.path(), "a", None).await.unwrap();
    }
    assert_eq!(files.folders(), 2);
    let idle = FileIndex::new(IndexLimits { idle_drop: Duration::ZERO, ..IndexLimits::default() });
    idle.search(dirs[0].path(), "a", None).await.unwrap();
    idle.sweep();
    assert_eq!(idle.folders(), 0);
    files.shutdown().await;
    idle.shutdown().await;
}

// ---- Helpers: images ----

/// A deterministic picture with some texture.
fn picture(w: u32, h: u32, seed: u32) -> RgbImage {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    RgbImage::from_fn(w, h, |x, y| {
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let noise = u8::try_from((state >> 16) & 7).unwrap();
        let r = u8::try_from((x * 255) / w.max(1)).unwrap();
        let g = u8::try_from((y * 255) / h.max(1)).unwrap();
        Rgb([r.saturating_add(noise), g.saturating_add(noise), 128u8.saturating_add(noise)])
    })
}

/// Full-range noise: a PNG that hardly compresses.
fn noise(w: u32, h: u32, seed: u32) -> RgbImage {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    let mut next = move || {
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        u8::try_from((state >> 16) & 0xff).unwrap()
    };
    RgbImage::from_fn(w, h, |_, _| Rgb([next(), next(), next()]))
}

fn png_of(image: &DynamicImage) -> Vec<u8> {
    let mut out = Vec::new();
    image.write_with_encoder(PngEncoder::new(&mut out)).unwrap();
    out
}

fn jpeg_of(image: &DynamicImage) -> Vec<u8> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 90).encode_image(&image.to_rgb8()).unwrap();
    out
}

fn webp_of(image: &DynamicImage) -> Vec<u8> {
    let mut out = Vec::new();
    image.write_with_encoder(WebPEncoder::new_lossless(&mut out)).unwrap();
    out
}

fn gif_of(frames: &[Rgba<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut out);
        let frames = frames.iter().map(|&c| Frame::new(RgbaImage::from_pixel(16, 16, c)));
        encoder.encode_frames(frames).unwrap();
    }
    out
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = kind.to_vec();
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    out
}

/// A PNG that declares `w`×`h` and carries almost no data.
fn png_declaring(w: u32, h: u32, color_type: u8) -> Vec<u8> {
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, color_type, 0, 0, 0]);
    // zlib: one stored block of five zero bytes.
    let idat = [0x78, 0x01, 0x01, 0x05, 0x00, 0xfa, 0xff, 0, 0, 0, 0, 0, 0x00, 0x05, 0x00, 0x01];
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend(chunk(b"IHDR", &ihdr));
    out.extend(chunk(b"IDAT", &idat));
    out.extend(chunk(b"IEND", &[]));
    out
}

/// `png` with a `tEXt` chunk after its header.
fn png_with_text(png: &[u8], text: &str) -> Vec<u8> {
    let ihdr_end = 8 + 8 + 13 + 4;
    let mut data = b"Comment\0".to_vec();
    data.extend_from_slice(text.as_bytes());
    let mut out = png[..ihdr_end].to_vec();
    out.extend(chunk(b"tEXt", &data));
    out.extend_from_slice(&png[ihdr_end..]);
    out
}

/// A little-endian TIFF block for EXIF: a description, an orientation and a
/// GPS directory, both texts carrying `marker`.
fn exif_tiff(marker: &str, orientation: u16) -> Vec<u8> {
    let description = format!("{marker}\0");
    let processing = format!("ASCII\0\0\0{marker}");
    let gps_at: u32 = 8 + 2 + 3 * 12 + 4;
    let text_at: u32 = gps_at + 2 + 2 * 12 + 4;
    let processing_at = text_at + u32::try_from(description.len()).unwrap();
    let mut t = b"II*\0".to_vec();
    t.extend_from_slice(&8u32.to_le_bytes());
    let entry = |t: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: [u8; 4]| {
        t.extend_from_slice(&tag.to_le_bytes());
        t.extend_from_slice(&kind.to_le_bytes());
        t.extend_from_slice(&count.to_le_bytes());
        t.extend_from_slice(&value);
    };
    t.extend_from_slice(&3u16.to_le_bytes());
    entry(&mut t, 0x010e, 2, u32::try_from(description.len()).unwrap(), text_at.to_le_bytes());
    let o = orientation.to_le_bytes();
    entry(&mut t, 0x0112, 3, 1, [o[0], o[1], 0, 0]);
    entry(&mut t, 0x8825, 4, 1, gps_at.to_le_bytes());
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(&2u16.to_le_bytes());
    entry(&mut t, 0x0001, 2, 2, *b"N\0\0\0");
    entry(&mut t, 0x001b, 7, u32::try_from(processing.len()).unwrap(), processing_at.to_le_bytes());
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(description.as_bytes());
    t.extend_from_slice(processing.as_bytes());
    t
}

/// `jpeg` with an APP1 EXIF segment after its start marker.
fn jpeg_with_exif(jpeg: &[u8], tiff: &[u8]) -> Vec<u8> {
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(tiff);
    let mut out = vec![0xff, 0xd8, 0xff, 0xe1];
    out.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&jpeg[2..]);
    out
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn decode(bytes: &[u8]) -> DynamicImage {
    image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().unwrap().decode().unwrap()
}

fn store(dir: &Path) -> ImageStore {
    ImageStore::open(dir.join("images"), vec![dir.join("agent-home")]).unwrap()
}

const THREAD: &str = "0123456789abcdef0123456789abcdef";
const OTHER_THREAD: &str = "fedcba9876543210fedcba9876543210";

// ---- Intake ----

#[test]
fn each_type_is_decoded_and_encoded_again() {
    let rgb = DynamicImage::ImageRgb8(picture(64, 48, 1));
    let rgba = DynamicImage::ImageRgba8(RgbaImage::from_pixel(32, 32, Rgba([10, 200, 30, 128])));
    let cases: [(&str, Vec<u8>, Kind); 5] = [
        ("png", png_of(&rgb), Kind::Png),
        ("jpeg", jpeg_of(&rgb), Kind::Jpeg),
        ("gif", gif_of(&[Rgba([255, 0, 0, 255])]), Kind::Png),
        ("webp", webp_of(&rgb), Kind::Jpeg),
        ("webp with alpha", webp_of(&rgba), Kind::Png),
    ];
    for (what, bytes, kind) in cases {
        let out = attachments::reencode(&bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(out.kind, kind, "{what}");
        let back = decode(&out.data);
        assert_eq!((back.width(), back.height()), (out.w, out.h), "{what}");
    }
    let rgba_out = attachments::reencode(&webp_of(&rgba)).unwrap();
    assert_eq!(decode(&rgba_out.data).to_rgba8().get_pixel(0, 0)[3], 128, "alpha survives as PNG");
}

#[test]
fn large_images_are_scaled_and_heavy_pngs_become_jpeg() {
    let wide = DynamicImage::ImageRgb8(picture(3000, 600, 2));
    let out = attachments::reencode(&png_of(&wide)).unwrap();
    assert_eq!((out.w, out.h), (2048, 410));
    let busy = DynamicImage::ImageRgb8(noise(1000, 1000, 3));
    let png = png_of(&busy);
    assert!(png.len() > attachments::PNG_KEEP, "the test image must be a heavy PNG");
    let out = attachments::reencode(&png).unwrap();
    assert_eq!(out.kind, Kind::Jpeg);
    assert!(out.data.len() <= attachments::MAX_STORED);
}

#[test]
fn not_images_and_truncated_files_are_refused() {
    assert_eq!(
        attachments::reencode(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
        Err(ImageError::NotAnImage)
    );
    assert_eq!(attachments::reencode(b""), Err(ImageError::NotAnImage));
    let png = png_of(&DynamicImage::ImageRgb8(picture(64, 64, 4)));
    assert_eq!(attachments::reencode(&png[..png.len() / 2]), Err(ImageError::NotAnImage));
    let mut bad_crc = png.clone();
    bad_crc[20] ^= 0xff;
    assert!(attachments::reencode(&bad_crc).is_err());
}

#[test]
fn decompression_bombs_are_refused_before_decoding() {
    assert_eq!(attachments::reencode(&png_declaring(60_000, 60_000, 2)), Err(ImageError::TooLarge));
    // Under the side limit, but 484 MB of pixels: over the decoder's budget.
    assert_eq!(attachments::reencode(&png_declaring(11_000, 11_000, 6)), Err(ImageError::TooLarge));
    assert_eq!(attachments::reencode(&png_declaring(12_001, 10, 2)), Err(ImageError::TooLarge));
}

#[test]
fn metadata_never_survives_and_orientation_is_applied() {
    const MARKER: &str = "G7-SECRET-GPS-48.8584N";
    let source = DynamicImage::ImageRgb8(picture(40, 20, 5));
    let jpeg = jpeg_with_exif(&jpeg_of(&source), &exif_tiff(MARKER, 6));
    assert!(contains(&jpeg, MARKER.as_bytes()) && contains(&jpeg, b"Exif"));
    let out = attachments::reencode(&jpeg).unwrap();
    assert!(!contains(&out.data, MARKER.as_bytes()), "the GPS text left the host");
    assert!(!contains(&out.data, b"Exif"));
    assert_eq!((out.w, out.h), (20, 40), "turned the way the EXIF said");

    let png = png_with_text(&png_of(&source), MARKER);
    assert!(contains(&png, MARKER.as_bytes()));
    let out = attachments::reencode(&png).unwrap();
    assert!(!contains(&out.data, MARKER.as_bytes()));
    for chunk_name in [&b"tEXt"[..], b"iTXt", b"zTXt", b"eXIf", b"iCCP"] {
        assert!(!contains(&out.data, chunk_name));
    }
}

#[test]
fn an_animated_gif_keeps_its_first_frame() {
    let gif = gif_of(&[Rgba([255, 0, 0, 255]), Rgba([0, 0, 255, 255]), Rgba([0, 255, 0, 255])]);
    let out = attachments::reencode(&gif).unwrap();
    assert_eq!(out.kind, Kind::Png);
    assert!(!contains(&out.data, b"acTL"), "not an animated PNG");
    let back = decode(&out.data).to_rgba8();
    assert_eq!(back.get_pixel(8, 8).0, [255, 0, 0, 255]);
}

// ---- The store ----

#[tokio::test]
async fn a_wrong_extension_changes_nothing_and_files_are_private() {
    let dir = tempfile::tempdir().unwrap();
    let images = store(dir.path());
    let path = dir.path().join("photo.jpg");
    std::fs::write(&path, png_of(&DynamicImage::ImageRgb8(picture(30, 30, 6)))).unwrap();
    let info = images.add_path(path.to_str().unwrap(), "pc").await.unwrap();
    assert_eq!((info.kind, info.name.as_str(), info.w, info.h), (Kind::Png, "photo.jpg", 30, 30));
    assert_eq!(info.place, Place::Pending { owner: "pc".into() });
    assert!(g7_agents::api::is_valid_image_id(&info.id));
    let stored = images.path(&info.id).unwrap();
    assert_eq!(stored.extension().unwrap(), "png");
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&stored), 0o600);
    assert_eq!(mode(&stored.with_extension("json")), 0o600);
    assert_eq!(mode(stored.parent().unwrap()), 0o700);
    assert_eq!(mode(&dir.path().join("images")), 0o700);
    assert_eq!(images.total(), u64::from(info.bytes));
}

#[tokio::test]
async fn add_path_opens_only_regular_files_it_may() {
    let dir = tempfile::tempdir().unwrap();
    let images = store(dir.path());
    let png = png_of(&DynamicImage::ImageRgb8(picture(8, 8, 7)));
    let err = |r: Result<attachments::ImageInfo, ImageError>| r.unwrap_err();

    assert_eq!(err(images.add_path("photo.png", "pc").await), ImageError::NotAllowed);
    let hidden = dir.path().join(".hidden.png");
    std::fs::write(&hidden, &png).unwrap();
    assert_eq!(err(images.add_path(hidden.to_str().unwrap(), "pc").await), ImageError::NotAllowed);
    let agent_file = dir.path().join("agent-home/picture.png");
    write(dir.path(), "agent-home/picture.png", "");
    std::fs::write(&agent_file, &png).unwrap();
    assert_eq!(err(images.add_path(agent_file.to_str().unwrap(), "pc").await), ImageError::NotAllowed);
    let pointer = dir.path().join("innocent.png");
    symlink(&agent_file, &pointer).unwrap();
    assert_eq!(err(images.add_path(pointer.to_str().unwrap(), "pc").await), ImageError::NotAllowed);
    let folder = dir.path().join("Pictures");
    std::fs::create_dir(&folder).unwrap();
    assert_eq!(err(images.add_path(folder.to_str().unwrap(), "pc").await), ImageError::NotAnImage);
    assert_eq!(
        err(images.add_path(dir.path().join("missing.png").to_str().unwrap(), "pc").await),
        ImageError::Unreadable
    );

    let fifo = dir.path().join("pipe.png");
    rustix::fs::mknodat(rustix::fs::CWD, &fifo, rustix::fs::FileType::Fifo, rustix::fs::Mode::from(0o600), 0)
        .unwrap();
    let opened =
        tokio::time::timeout(Duration::from_secs(5), images.add_path(fifo.to_str().unwrap(), "pc")).await;
    assert_eq!(opened.expect("a FIFO must not block the intake").unwrap_err(), ImageError::NotAnImage);

    let huge = dir.path().join("huge.png");
    let file = std::fs::File::create(&huge).unwrap();
    file.set_len(g7_agents::api::MAX_IMAGE_FILE + 1).unwrap();
    assert_eq!(err(images.add_path(huge.to_str().unwrap(), "pc").await), ImageError::TooLarge);

    assert_eq!(
        err(images.add_path(&format!("/{}", "a".repeat(5000)), "pc").await).code(),
        ErrorCode::BadRequest
    );
    assert_eq!(images.total(), 0);
}

#[tokio::test]
async fn uploads_claims_and_owners() {
    let dir = tempfile::tempdir().unwrap();
    let images = store(dir.path());
    let jpeg = jpeg_of(&DynamicImage::ImageRgb8(picture(50, 40, 8)));
    let mut upload = Upload::begin("../../phone photo.jpg", u32::try_from(jpeg.len()).unwrap()).unwrap();
    for (seq, part) in jpeg.chunks(1000).enumerate() {
        upload.chunk(u32::try_from(seq).unwrap(), &data_encoding::BASE64.encode(part)).unwrap();
    }
    let (name, bytes) = upload.finish().unwrap();
    assert_eq!(name, "phone photo.jpg");
    let info = images.add_bytes(&name, bytes, "phone:dev1").await.unwrap();
    assert_eq!(info.kind, Kind::Jpeg);
    let ids = vec![info.id.clone()];

    assert_eq!(images.claim(&ids, Some("pc"), THREAD).await.unwrap_err(), ImageError::NotFound);
    assert_eq!(images.drop_pending(&info.id, Some("pc")).await.unwrap_err(), ImageError::NotFound);
    assert!(images.files(THREAD, &ids).is_err(), "not in the thread yet");
    let claimed = images.claim(&ids, Some("phone:dev1"), THREAD).await.unwrap();
    assert_eq!(claimed[0].place, Place::Thread(THREAD.into()));
    assert_eq!(claimed[0].image_ref().id, info.id);
    // Sent again from a queue: already in this thread is fine; another thread can't have it.
    assert!(images.claim(&ids, Some("phone:dev1"), THREAD).await.is_ok());
    assert_eq!(images.claim(&ids, None, OTHER_THREAD).await.unwrap_err(), ImageError::NotFound);
    let stored = images.files(THREAD, &ids).unwrap();
    assert_eq!(stored[0].media_type, "image/jpeg");
    assert!(stored[0].path.starts_with(dir.path().join("images").join(THREAD)));
    assert!(stored[0].path.is_file());
    let blocks = images.claude_blocks(THREAD, &ids).await.unwrap();
    assert_eq!(
        (&blocks[0]["type"], &blocks[0]["source"]["media_type"]),
        (&json!("image"), &json!("image/jpeg"))
    );
    let sent =
        data_encoding::BASE64.decode(blocks[0]["source"]["data"].as_str().unwrap().as_bytes()).unwrap();
    assert_eq!(sent, std::fs::read(&stored[0].path).unwrap());
    let inputs = images.codex_inputs(THREAD, &ids).unwrap();
    assert_eq!(inputs, [json!({"type": "localImage", "path": stored[0].path.to_str().unwrap()})]);
    assert!(images.files(OTHER_THREAD, &ids).is_err());
    assert!(images.claude_blocks(OTHER_THREAD, &ids).await.is_err());
    assert_eq!(
        images.drop_pending(&info.id, None).await.unwrap_err(),
        ImageError::NotFound,
        "no longer pending"
    );

    // All or nothing: one bad id moves none.
    let other =
        images.add_bytes("b.png", png_of(&DynamicImage::ImageRgb8(picture(9, 9, 9))), "pc").await.unwrap();
    let mixed = vec![other.id.clone(), "0".repeat(32)];
    assert_eq!(images.claim(&mixed, None, THREAD).await.unwrap_err(), ImageError::NotFound);
    assert!(matches!(images.info(&other.id).unwrap().place, Place::Pending { .. }));
    images.drop_pending(&other.id, Some("pc")).await.unwrap();
    assert!(images.info(&other.id).is_none());

    let five: Vec<String> = (0..5).map(|i| format!("{i:032x}")).collect();
    assert!(matches!(images.claim(&five, None, THREAD).await.unwrap_err(), ImageError::BadRequest(_)));

    images.remove_thread(THREAD).await;
    assert!(images.info(&info.id).is_none());
    assert!(!dir.path().join("images").join(THREAD).exists());
    assert_eq!(images.total(), 0);

    // An upload larger than announced, or too large to announce.
    let mut over = Upload::begin("x.png", 3).unwrap();
    assert_eq!(over.chunk(0, &data_encoding::BASE64.encode(b"abcd")), Err(ImageError::TooLarge));
    assert!(Upload::begin("x.png", MAX_IMAGE_UPLOAD + 1).is_err());
}

#[tokio::test]
async fn thumbnails_are_small_jpegs() {
    let dir = tempfile::tempdir().unwrap();
    let images = store(dir.path());
    let png = png_of(&DynamicImage::ImageRgb8(picture(1500, 900, 10)));
    let info = images.add_bytes("big.png", png, "pc").await.unwrap();
    let thumb = images.read(&info.id, 256).await.unwrap();
    assert_eq!(thumb.mime, "image/jpeg");
    let bytes = data_encoding::BASE64.decode(thumb.data.as_bytes()).unwrap();
    assert!(bytes.starts_with(&[0xff, 0xd8]));
    assert!(bytes.len() <= g7_agents::api::MAX_THUMB_BYTES);
    let back = decode(&bytes);
    assert_eq!((back.width(), back.height()), (256, 154));
    assert!(matches!(images.read(&info.id, 0).await, Err(ImageError::BadRequest(_))));
    assert!(matches!(images.read(&info.id, 2000).await, Err(ImageError::BadRequest(_))));
}

#[tokio::test]
async fn the_store_cap_drops_the_oldest_pending_then_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let png = png_of(&DynamicImage::ImageRgb8(picture(64, 64, 11)));
    let size = {
        let probe = ImageStore::open(dir.path().join("probe"), Vec::new()).unwrap();
        u64::from(probe.add_bytes("a.png", png.clone(), "pc").await.unwrap().bytes)
    };
    let images =
        ImageStore::open_with_cap(dir.path().join("images"), Vec::new(), size * 2 + size / 2).unwrap();
    let mut ids = Vec::new();
    for name in ["a.png", "b.png", "c.png"] {
        ids.push(images.add_bytes(name, png.clone(), "pc").await.unwrap().id);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(images.info(&ids[0]).is_none(), "the oldest pending image made room");
    assert!(!dir.path().join("images/pending").join(format!("{}.png", ids[0])).exists());
    assert!(images.info(&ids[1]).is_some() && images.info(&ids[2]).is_some());
    assert_eq!(images.total(), size * 2);
    images.claim(&ids[1..], Some("pc"), THREAD).await.unwrap();
    let full = images.add_bytes("d.png", png.clone(), "pc").await.unwrap_err();
    assert_eq!(full, ImageError::Full);
    assert_eq!(full.code(), ErrorCode::Limit);
    assert_eq!(images.total(), size * 2);
}

#[tokio::test]
async fn image_ids_and_thread_ids_never_become_paths() {
    let dir = tempfile::tempdir().unwrap();
    let images = store(dir.path());
    let canary = dir.path().join("canary.png");
    std::fs::write(&canary, png_of(&DynamicImage::ImageRgb8(picture(8, 8, 12)))).unwrap();
    let mut bad_ids: Vec<String> =
        ["../canary", "/etc/passwd", "", "..", "pending", "a/b"].into_iter().map(String::from).collect();
    bad_ids.extend(["0".repeat(31), "G".repeat(32), format!("../{}", "0".repeat(29))]);
    for id in &bad_ids {
        assert!(images.info(id).is_none(), "{id:?}");
        assert!(images.path(id).is_none(), "{id:?}");
        assert_eq!(images.drop_pending(id, None).await.unwrap_err(), ImageError::NotFound, "{id:?}");
        assert!(images.read(id, 64).await.is_err(), "{id:?}");
        assert!(images.claim(std::slice::from_ref(id), None, THREAD).await.is_err(), "{id:?}");
    }
    let info = images.add_path(canary.to_str().unwrap(), "pc").await.unwrap();
    for thread in ["..", "../x", "/tmp", "", "pending", "ABCDEF0123456789ABCDEF0123456789"] {
        assert!(images.claim(std::slice::from_ref(&info.id), None, thread).await.is_err(), "{thread:?}");
        images.remove_thread(thread).await;
    }
    assert!(canary.exists());
    assert!(dir.path().join("images/pending").exists());
    assert!(images.info(&info.id).is_some());
}

#[tokio::test]
async fn the_store_reopens_and_sweeps_old_pending_images() {
    let dir = tempfile::tempdir().unwrap();
    let png = png_of(&DynamicImage::ImageRgb8(picture(20, 20, 13)));
    let (kept, old) = {
        let images = store(dir.path());
        let kept = images.add_bytes("kept.png", png.clone(), "pc").await.unwrap();
        let old = images.add_bytes("old.png", png.clone(), "pc").await.unwrap();
        images.claim(std::slice::from_ref(&kept.id), None, THREAD).await.unwrap();
        (kept, old)
    };
    std::fs::write(dir.path().join("images/pending/.stale.tmp"), "x").unwrap();
    let images = store(dir.path());
    assert_eq!(images.info(&kept.id).unwrap().name, "kept.png");
    assert_eq!(images.info(&old.id).unwrap().place, Place::Pending { owner: "pc".into() });
    assert!(!dir.path().join("images/pending/.stale.tmp").exists());
    drop(images);

    let sidecar = dir.path().join("images/pending").join(format!("{}.json", old.id));
    let mut meta: serde_json::Value = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
    meta["created_ms"] = json!(1);
    std::fs::write(&sidecar, serde_json::to_vec(&meta).unwrap()).unwrap();
    let images = store(dir.path());
    assert!(images.info(&old.id).is_none(), "a pending image over 24 hours old is swept");
    assert!(!sidecar.exists());
    assert!(images.info(&kept.id).is_some(), "a sent image stays");
}
