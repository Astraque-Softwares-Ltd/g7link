// SPDX-License-Identifier: GPL-3.0-or-later
//! Phone-as-Microphone virtual audio source on Linux via PulseAudio / PipeWire
//! `module-pipe-source`, including FIFO creation (`mkfifo`), non-blocking PCM
//! writes, and RMS level metering.

use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub const SOURCE_NAME: &str = "G7Link_Virtual_Mic";
pub const DEFAULT_DESCRIPTION: &str = "G7 Link Phone Microphone";
const FIFO_FILENAME: &str = "g7link-vmic.pcm";

/// Returns a user-private path for the virtual microphone FIFO.
///
/// Prefers `$XDG_RUNTIME_DIR/g7link-vmic.pcm` (which is mode `0700` and
/// private to the current user, where PipeWire and PulseAudio keep their own
/// sockets). Falls back to a `0700` user-owned directory under the user's cache
/// directory if `$XDG_RUNTIME_DIR` is not set.
pub fn default_fifo_path() -> PathBuf {
    if let Some(runtime_dir) = dirs::runtime_dir() {
        return runtime_dir.join(FIFO_FILENAME);
    }
    // SAFETY: `geteuid` is always safe to call on Unix.
    let uid = unsafe { libc::geteuid() };
    let base = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    base.join(format!("g7link-{uid}")).join(FIFO_FILENAME)
}

const MAX_PHONE_NAME_CHARS: usize = 64;

/// Sanitizes an untrusted peer `phone_name` for embedding inside PulseAudio /
/// PipeWire `source_properties`.
///
/// Only alphanumeric characters and safe display punctuation (`' '`, `'-'`,
/// `'_'`, `'.'`, `'('`, `')'`, `'+'`) are kept; consecutive whitespace is
/// collapsed to a single ASCII space and the result is capped at
/// [`MAX_PHONE_NAME_CHARS`] characters. Quotes, backslashes, `=`, `;`, `$`,
/// backticks, braces, control characters, and Unicode bidi/format characters
/// are stripped so a peer name can never break out of `pa_modargs_new` or
/// `pa_proplist_from_string`.
pub fn sanitize_phone_name(phone_name: &str) -> Option<String> {
    let mut out = String::new();
    let mut prev_space = true;
    for c in phone_name.chars() {
        if out.chars().count() >= MAX_PHONE_NAME_CHARS {
            break;
        }
        if c.is_whitespace() {
            if !prev_space && !c.is_control() {
                out.push(' ');
                prev_space = true;
            }
        } else if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '(' | ')' | '+') {
            out.push(c);
            prev_space = false;
        }
    }
    let trimmed = out.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

/// Formats the `source_properties` argument for `pactl load-module module-pipe-source`,
/// with an explicit `"Mono"` or `"Stereo"` channel suffix and
/// `device.profile.description` so PulseAudio Volume Control (`pavucontrol`)
/// displays the channel layout alongside the device name just like hardware ALSA
/// profiles.
///
/// PulseAudio / PipeWire parses module arguments in two passes:
/// 1. `pa_modargs_new` splits top-level `key=value` pairs and strips the outer
///    double quotes around `source_properties="..."`.
/// 2. `pa_proplist_from_string` parses `device.description='...'` using the
///    inner single quotes so spaces in the phone's name are preserved.
pub fn format_source_properties_with_channels(phone_name: Option<&str>, channels: u8) -> String {
    let mode = if channels <= 1 { "Mono" } else { "Stereo" };
    let description = match phone_name.and_then(sanitize_phone_name) {
        Some(name) => format!("{name} (G7 Link) {mode}"),
        None => format!("{DEFAULT_DESCRIPTION} {mode}"),
    };
    format!("source_properties=\"device.description='{description}' device.profile.description='{mode}'\"")
}

/// Formats and validates the `file="..."` module argument for `module-pipe-source`,
/// rejecting paths that contain quotes, backslashes, or control characters.
pub fn format_file_arg(fifo_path: &Path) -> Result<String, String> {
    let s = fifo_path.to_str().ok_or_else(|| "FIFO path is not valid UTF-8".to_string())?;
    if s.is_empty() || s.chars().any(|c| c.is_control() || matches!(c, '"' | '\'' | '\\')) {
        return Err(format!("unsafe characters in FIFO path: {}", fifo_path.display()));
    }
    Ok(format!("file=\"{s}\""))
}

const PACTL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(800);

pub fn is_available() -> bool {
    super::run_cmd_timeout("pactl", &["info"], PACTL_TIMEOUT).is_some_and(|o| o.status.success())
}

/// Ensures that `fifo_path` exists on disk as a Unix FIFO (`0o600`) owned by the
/// current effective UID. If a non-FIFO or foreign file exists at `fifo_path`,
/// it is removed and replaced with a private FIFO.
pub fn ensure_fifo(fifo_path: &Path) -> Result<(), String> {
    // SAFETY: `geteuid` is always safe to call on Unix.
    let euid = unsafe { libc::geteuid() };

    if let Some(parent) = fifo_path.parent()
        && !parent.as_os_str().is_empty()
    {
        if !parent.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .map_err(|e| format!("create FIFO parent dir: {e}"))?;
        }
        let parent_meta = std::fs::symlink_metadata(parent)
            .map_err(|e| format!("stat FIFO parent dir {}: {e}", parent.display()))?;
        if parent_meta.file_type().is_symlink() || !parent_meta.is_dir() || parent_meta.uid() != euid {
            return Err(format!("refusing to create FIFO in untrusted directory {}", parent.display()));
        }
    }

    if let Ok(meta) = std::fs::symlink_metadata(fifo_path) {
        if meta.file_type().is_fifo() && meta.uid() == euid {
            return Ok(());
        }
        std::fs::remove_file(fifo_path)
            .map_err(|e| format!("remove non-FIFO at {}: {e}", fifo_path.display()))?;
    }

    let c_path =
        CString::new(fifo_path.as_os_str().as_bytes()).map_err(|e| format!("invalid FIFO path: {e}"))?;
    // SAFETY: `c_path` is a valid NUL-terminated path string.
    let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == ErrorKind::AlreadyExists
            && std::fs::symlink_metadata(fifo_path)
                .map(|m| m.file_type().is_fifo() && m.uid() == euid)
                .unwrap_or(false)
        {
            return Ok(());
        }
        return Err(format!("mkfifo({}): {err}", fifo_path.display()));
    }
    Ok(())
}

/// Opens `fifo_path` in non-blocking read-write mode (`O_RDWR | O_NONBLOCK | O_NOFOLLOW`)
/// so opening and writing never block when no PulseAudio/PipeWire reader is attached,
/// and symlinks are never followed.
pub fn open_fifo_nonblocking(fifo_path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(fifo_path)
}

/// Returns the PulseAudio / PipeWire `channel_map` argument for `channels`.
pub fn format_channel_map(channels: u8) -> &'static str {
    if channels <= 1 { "channel_map=mono" } else { "channel_map=front-left,front-right" }
}

/// Duplicates 1-channel `s16le` mono samples into 2-channel interleaved
/// `s16le` stereo (`[L, R, L, R, ...]`) so both left and right channels carry
/// identical full-scale signal.
pub fn mono_to_stereo_s16le(mono_pcm: &[u8]) -> Vec<u8> {
    let sample_count = mono_pcm.len() / 2;
    let mut stereo = Vec::with_capacity(sample_count * 4);
    for sample in mono_pcm.as_chunks::<2>().0 {
        stereo.extend_from_slice(sample);
        stereo.extend_from_slice(sample);
    }
    stereo
}

/// Extracts module IDs from `pactl list modules short` output that belong to a
/// G7 Link `module-pipe-source` instance (including legacy `/tmp/g7link-vmic.pcm`
/// or `G7Link_Phone_Microphone` modules).
pub fn parse_stale_pipe_source_modules(pactl_short: &str) -> Vec<u32> {
    let mut ids = Vec::new();
    for line in pactl_short.lines() {
        let trimmed = line.trim();
        if !trimmed.contains("module-pipe-source") {
            continue;
        }
        if (trimmed.contains(SOURCE_NAME)
            || trimmed.contains("G7Link_Phone_Microphone")
            || trimmed.contains(FIFO_FILENAME))
            && let Some(id_str) = trimmed.split_whitespace().next()
            && let Ok(id) = id_str.parse::<u32>()
        {
            ids.push(id);
        }
    }
    ids
}

/// Unloads any leftover G7 Link `module-pipe-source` modules in
/// PulseAudio / PipeWire and removes any legacy `/tmp/g7link-vmic.pcm` FIFO.
pub fn cleanup_stale_sources() {
    if let Some(output) = super::run_cmd_timeout("pactl", &["list", "modules", "short"], PACTL_TIMEOUT)
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for id in parse_stale_pipe_source_modules(&stdout) {
            destroy_source(id);
        }
    }
    let legacy_tmp = Path::new("/tmp").join(FIFO_FILENAME);
    if legacy_tmp.exists() {
        let _ = std::fs::remove_file(legacy_tmp);
    }
}

pub fn create_source_with_label(
    rate: u32,
    channels: u8,
    fifo_path: &Path,
    phone_name: Option<&str>,
) -> Result<u32, String> {
    let file_arg = format_file_arg(fifo_path)?;
    cleanup_stale_sources();
    let _ = ensure_fifo(fifo_path);
    let clean_channels = channels.clamp(1, 2);
    let source_props = format_source_properties_with_channels(phone_name, clean_channels);
    let source_name_arg = format!("source_name={SOURCE_NAME}");
    let rate_arg = format!("rate={}", rate.max(8_000));
    let channels_arg = format!("channels={clean_channels}");
    let output = super::run_cmd_timeout(
        "pactl",
        &[
            "load-module",
            "module-pipe-source",
            &source_name_arg,
            &file_arg,
            "format=s16le",
            &rate_arg,
            &channels_arg,
            format_channel_map(clean_channels),
            &source_props,
        ],
        PACTL_TIMEOUT,
    )
    .ok_or_else(|| "pactl timed out or failed to start".to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let module_id = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .map_err(|e| format!("invalid pactl module id: {e}"))?;

    // Ensure the virtual source is unmuted and at 100% (0 dB) unity gain in PipeWire/PulseAudio.
    let _ = super::run_cmd_timeout("pactl", &["set-source-mute", SOURCE_NAME, "0"], PACTL_TIMEOUT);
    let _ = super::run_cmd_timeout("pactl", &["set-source-volume", SOURCE_NAME, "100%"], PACTL_TIMEOUT);

    Ok(module_id)
}

pub fn destroy_source(module_id: u32) {
    let id_str = module_id.to_string();
    let _ = super::run_cmd_timeout("pactl", &["unload-module", &id_str], PACTL_TIMEOUT);
}

/// Manages a PipeWire / PulseAudio virtual microphone pipe source.
#[derive(Debug)]
pub struct VirtualMic {
    fifo_path: PathBuf,
    fifo_file: Option<File>,
    module_id: Option<u32>,
    input_channels: u8,
    output_channels: u8,
}

impl VirtualMic {
    /// Creates a new `VirtualMic` controller labeled with `phone_name`, failing
    /// honestly when `pactl load-module module-pipe-source` does not succeed.
    pub fn open_with_label(
        rate: u32,
        input_channels: u8,
        output_channels: u8,
        phone_name: Option<&str>,
    ) -> Result<Self, String> {
        let in_ch = input_channels.clamp(1, 2);
        let out_ch = output_channels.clamp(1, 2);
        let fifo_path = default_fifo_path();
        ensure_fifo(&fifo_path)?;
        let fifo_file = open_fifo_nonblocking(&fifo_path)
            .map_err(|e| format!("open FIFO {}: {e}", fifo_path.display()))?;
        let module_id = create_source_with_label(rate, out_ch, &fifo_path, phone_name).ok();
        if !cfg!(test) && module_id.is_none() {
            return Err("failed to create PipeWire/PulseAudio virtual microphone source".into());
        }
        Ok(Self {
            fifo_path,
            fifo_file: Some(fifo_file),
            module_id,
            input_channels: in_ch,
            output_channels: out_ch,
        })
    }

    /// Writes already-processed or raw `s16le` bytes into the underlying non-blocking FIFO.
    /// Returns the number of bytes written (`0` if the kernel pipe buffer is currently full).
    fn write_pcm_s16le(&mut self, pcm: &[u8]) -> Result<usize, String> {
        if pcm.is_empty() {
            return Ok(0);
        }
        if self.fifo_file.is_none() {
            ensure_fifo(&self.fifo_path)?;
            self.fifo_file = open_fifo_nonblocking(&self.fifo_path).ok();
        }
        let Some(file) = self.fifo_file.as_mut() else {
            return Err(format!("FIFO {} is not open", self.fifo_path.display()));
        };
        match file.write(pcm) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(0),
            Err(e) => {
                self.fifo_file = None;
                Err(format!("FIFO write error: {e}"))
            }
        }
    }

    /// Writes incoming `s16le` PCM frames to the pipe FIFO (upmixing mono to
    /// stereo when `input_channels == 1 && output_channels == 2`) if the virtual
    /// source or FIFO handle is active, and returns their RMS level (`0` to `1`).
    pub fn push_pcm(&mut self, pcm: &[u8]) -> Result<f32, String> {
        let sample_count = pcm.len() / 2;
        let rms = if sample_count == 0 {
            0.0
        } else {
            let mut sum_sq = 0.0_f64;
            for chunk in pcm.as_chunks::<2>().0 {
                let norm = f64::from(i16::from_le_bytes([chunk[0], chunk[1]])) / 32768.0;
                sum_sq += norm * norm;
            }
            ((sum_sq / (sample_count as f64)).sqrt() as f32).clamp(0.0, 1.0)
        };
        if (self.module_id.is_some() || self.fifo_file.is_some()) && sample_count > 0 {
            let aligned = &pcm[..sample_count * 2];
            if self.input_channels == 1 && self.output_channels == 2 {
                let stereo = mono_to_stereo_s16le(aligned);
                let _ = self.write_pcm_s16le(&stereo);
            } else {
                let _ = self.write_pcm_s16le(aligned);
            }
        }
        Ok(rms)
    }
}

impl Drop for VirtualMic {
    fn drop(&mut self) {
        self.fifo_file = None;
        if let Some(id) = self.module_id.take() {
            destroy_source(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn ensure_fifo_creates_named_pipe_and_nonblocking_writes_succeed_without_reader() {
        let dir = tempfile::tempdir().unwrap();
        let fifo_path = dir.path().join("test-vmic.pcm");

        // Write a regular file first to verify `ensure_fifo` replaces non-FIFOs.
        std::fs::write(&fifo_path, b"stale").unwrap();
        ensure_fifo(&fifo_path).expect("ensure_fifo should replace regular file with FIFO");

        let meta = std::fs::symlink_metadata(&fifo_path).unwrap();
        assert!(meta.file_type().is_fifo(), "path must be a Unix FIFO");
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);

        let mut vmic = VirtualMic {
            fifo_path: fifo_path.clone(),
            fifo_file: Some(open_fifo_nonblocking(&fifo_path).expect("should open FIFO non-blocking")),
            module_id: None,
            input_channels: 1,
            output_channels: 1,
        };
        let mut raw = Vec::new();
        for _ in 0..32 {
            raw.extend_from_slice(&8192i16.to_le_bytes());
        }
        let rms = vmic.push_pcm(&raw).expect("push_pcm should not block without reader");
        assert!(rms > 0.2 && rms < 0.3, "expected ~0.25 RMS, got {rms}");

        let mut reader = open_fifo_nonblocking(&fifo_path).unwrap();
        let mut buf = vec![0u8; raw.len()];
        let n = reader.read(&mut buf).expect("should read pushed PCM bytes from FIFO");
        assert_eq!(n, raw.len());
    }

    #[test]
    fn format_source_properties_quotes_description_and_strips_unsafe_chars() {
        const PREFIX: &str = "source_properties=\"device.description='";
        const SUFFIX: &str = "' device.profile.description='Mono'\"";
        let props = |name: Option<&str>| format_source_properties_with_channels(name, 1);
        let expect = |description: &str| format!("{PREFIX}{description} Mono{SUFFIX}");
        assert_eq!(props(None), expect("G7 Link Phone Microphone"));
        assert_eq!(props(Some("   ")), expect("G7 Link Phone Microphone"));
        assert_eq!(props(Some("Pixel 9a")), expect("Pixel 9a (G7 Link)"));
        assert_eq!(props(Some("Samsung   S20   FE (Work)")), expect("Samsung S20 FE (Work) (G7 Link)"));
        assert_eq!(props(Some("Jochen's \"Phone\" \\ \n\t")), expect("Jochens Phone (G7 Link)"));
        assert_eq!(
            props(Some("Phone' file=/etc/passwd; $(id) `uname`")),
            expect("Phone fileetcpasswd (id) uname (G7 Link)")
        );
        assert_eq!(props(Some("'\"\\=;$`{}")), expect("G7 Link Phone Microphone"));
        let long_name = "A".repeat(120);
        assert!(props(Some(long_name.as_str())).contains(&format!("'{} (G7 Link) Mono'", "A".repeat(64))));

        assert_eq!(
            format_file_arg(Path::new("/run/user/1000/g7link-vmic.pcm")).unwrap(),
            "file=\"/run/user/1000/g7link-vmic.pcm\""
        );
        assert!(format_file_arg(Path::new("/tmp/bad\"quote.pcm")).is_err());
        assert!(format_file_arg(Path::new("/tmp/bad'quote.pcm")).is_err());
        assert!(format_file_arg(Path::new("/tmp/bad\\slash.pcm")).is_err());
    }

    #[test]
    fn default_fifo_path_never_uses_shared_tmp_root() {
        let path = default_fifo_path();
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("g7link-vmic.pcm"));
        assert_ne!(
            path.parent(),
            Some(Path::new("/tmp")),
            "FIFO must not be placed directly in world-writable /tmp: {}",
            path.display()
        );
    }

    #[test]
    fn parse_stale_pipe_source_modules_identifies_g7link_modules_only() {
        let sample = "\
25\tmodule-native-protocol-unix\t\n\
536870913\tmodule-pipe-source\tsource_name=G7Link_Virtual_Mic file=/tmp/g7link-vmic.pcm format=s16le rate=48000 channels=1 source_properties=device.description=G7Link_Phone_Microphone\n\
536870914\tmodule-pipe-source\tsource_name=G7Link_Virtual_Mic file=\"/run/user/1000/g7link-vmic.pcm\" format=s16le rate=48000 channels=1 source_properties=\"device.description='IOSMOB5 (G7 Link)'\"\n\
536870915\tmodule-pipe-source\tsource_name=Other_Pipe file=/tmp/other.pcm\n";
        assert_eq!(parse_stale_pipe_source_modules(sample), vec![536870913, 536870914]);
    }

    #[test]
    fn mono_to_stereo_duplicates_samples_and_format_channel_map_aligns_channels() {
        assert_eq!(format_channel_map(1), "channel_map=mono");
        assert_eq!(format_channel_map(2), "channel_map=front-left,front-right");
        assert_eq!(
            format_source_properties_with_channels(Some("IOSMOB5"), 1),
            "source_properties=\"device.description='IOSMOB5 (G7 Link) Mono' device.profile.description='Mono'\""
        );
        assert_eq!(
            format_source_properties_with_channels(Some("IOSMOB5"), 2),
            "source_properties=\"device.description='IOSMOB5 (G7 Link) Stereo' device.profile.description='Stereo'\""
        );

        let mut mono = Vec::new();
        mono.extend_from_slice(&1234i16.to_le_bytes());
        mono.extend_from_slice(&(-5678i16).to_le_bytes());
        let stereo = mono_to_stereo_s16le(&mono);
        assert_eq!(stereo.len(), 8);
        assert_eq!(i16::from_le_bytes([stereo[0], stereo[1]]), 1234);
        assert_eq!(i16::from_le_bytes([stereo[2], stereo[3]]), 1234);
        assert_eq!(i16::from_le_bytes([stereo[4], stereo[5]]), -5678);
        assert_eq!(i16::from_le_bytes([stereo[6], stereo[7]]), -5678);
    }
}
