// SPDX-License-Identifier: MPL-2.0
//! Compressed Codex rollouts (`rollout-…jsonl.zst`, §U4.1): ruzstd's frame
//! decoder behind `Read`, streaming, with the two limits a file from disk
//! needs.
//!
//! A frame whose window is over [`MAX_WINDOW`] is refused before anything is
//! allocated for it, and a read past the decoder's byte limit fails, counted
//! as the bytes stream out. Memory stays at about one window and one block,
//! whatever the file's size. Codex writes one ordinary frame per file; frames
//! in a row and skippable frames are read too.

use std::io::{self, BufRead, Read};

use ruzstd::decoding::FrameDecoder;
use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};

/// The largest window a frame may declare. Every rollout on the spike's
/// laptop has 2 MiB at most.
pub const MAX_WINDOW: u64 = 64 << 20;
/// At most this much is decompressed from one file.
pub const MAX_DECODED: u64 = 512 << 20;

/// A file named `….zst`.
pub fn is_packed(name: &str) -> bool {
    name.ends_with(".zst")
}

/// What a refused or broken file is.
fn damaged(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("a compressed file can't be read: {e}"))
}

fn header_error(e: FrameDecoderError) -> io::Error {
    match e {
        FrameDecoderError::WindowSizeTooBig { requested, .. } => io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a compressed file asks for a {requested} byte window"),
        ),
        FrameDecoderError::ReadFrameHeaderError(ReadFrameHeaderError::MagicNumberReadError(e))
            if e.kind() == io::ErrorKind::UnexpectedEof =>
        {
            io::Error::new(io::ErrorKind::UnexpectedEof, "a compressed file ends early")
        }
        e => damaged(e),
    }
}

/// Decodes zstd frames from `source` as they are read.
pub struct ZstReader<R: BufRead> {
    source: R,
    decoder: FrameDecoder,
    /// Bytes handed out so far.
    decoded: u64,
    limit: u64,
}

impl<R: BufRead> std::fmt::Debug for ZstReader<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZstReader").field("decoded", &self.decoded).field("limit", &self.limit).finish()
    }
}

impl<R: BufRead> ZstReader<R> {
    /// Reads the first frame's header now: a refused window, a file that
    /// isn't zstd or one that is empty fail here.
    pub fn new(mut source: R, limit: u64) -> io::Result<Self> {
        let mut decoder = FrameDecoder::new();
        decoder.set_max_window_size(MAX_WINDOW);
        start_frame(&mut decoder, &mut source)?;
        Ok(Self { source, decoder, decoded: 0, limit })
    }

    /// Bytes decompressed and read so far.
    pub fn decoded(&self) -> u64 {
        self.decoded
    }
}

/// Starts the next frame, stepping over skippable ones. `Ok(false)`: the
/// source ended cleanly between frames.
fn next_frame(decoder: &mut FrameDecoder, source: &mut impl BufRead) -> io::Result<bool> {
    if source.fill_buf()?.is_empty() {
        return Ok(false);
    }
    start_frame(decoder, source)?;
    Ok(true)
}

fn start_frame(decoder: &mut FrameDecoder, source: &mut impl BufRead) -> io::Result<()> {
    loop {
        match decoder.reset(&mut *source) {
            Ok(()) => return Ok(()),
            Err(FrameDecoderError::ReadFrameHeaderError(ReadFrameHeaderError::SkipFrame {
                length, ..
            })) => {
                let skipped = io::copy(&mut (&mut *source).take(u64::from(length)), &mut io::sink())?;
                if skipped < u64::from(length) {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "a compressed file ends early"));
                }
                if source.fill_buf()?.is_empty() {
                    return Err(damaged("no frame after a skippable one"));
                }
            }
            Err(e) => return Err(header_error(e)),
        }
    }
}

impl<R: BufRead> Read for ZstReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.decoder.can_collect() > 0 {
                let room = usize::try_from(self.limit.saturating_sub(self.decoded)).unwrap_or(usize::MAX);
                if room == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::FileTooLarge,
                        "a compressed file decompresses past the limit",
                    ));
                }
                let take = buf.len().min(room);
                let n = self.decoder.read(&mut buf[..take])?;
                self.decoded += n as u64;
                return Ok(n);
            }
            if self.decoder.is_finished() {
                if !next_frame(&mut self.decoder, &mut self.source)? {
                    return Ok(0);
                }
                continue;
            }
            match self
                .decoder
                .decode_blocks(&mut self.source, ruzstd::decoding::BlockDecodingStrategy::UptoBlocks(1))
            {
                Ok(_) => {}
                Err(FrameDecoderError::FailedToReadBlockHeader(e)) if is_eof(&e) => {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "a compressed file ends early"));
                }
                Err(e) => return Err(damaged(e)),
            }
        }
    }
}

fn is_eof(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if let Some(io) = err.downcast_ref::<io::Error>() {
            return io.kind() == io::ErrorKind::UnexpectedEof;
        }
        cur = err.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruzstd::encoding::{CompressionLevel, compress_to_vec};

    fn read_all(bytes: &[u8], limit: u64) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        ZstReader::new(bytes, limit)?.read_to_end(&mut out)?;
        Ok(out)
    }

    #[test]
    fn a_compressed_file_round_trips() {
        let body: Vec<u8> =
            (0..20_000).flat_map(|n| format!("{{\"ordinal\":{n},\"type\":\"x\"}}\n").into_bytes()).collect();
        let packed = compress_to_vec(body.as_slice(), CompressionLevel::Fastest);
        assert!(packed.len() < body.len() / 4);
        assert_eq!(read_all(&packed, MAX_DECODED).unwrap(), body);
        // Two frames in a row, with a skippable frame between them.
        let mut two = packed.clone();
        two.extend_from_slice(&[0x50, 0x2A, 0x4D, 0x18, 3, 0, 0, 0, 1, 2, 3]);
        two.extend_from_slice(&packed);
        let out = read_all(&two, MAX_DECODED).unwrap();
        assert_eq!(out.len(), body.len() * 2);
        assert!(out.ends_with(&body));
    }

    #[test]
    fn bad_input_is_an_error_never_a_panic() {
        assert_eq!(read_all(b"", MAX_DECODED).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(
            read_all(b"{\"type\":\"x\"}\n", MAX_DECODED).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let packed = compress_to_vec(&b"hello\n".repeat(1000)[..], CompressionLevel::Fastest);
        for cut in [5, packed.len() / 2, packed.len() - 1] {
            let e = read_all(&packed[..cut], MAX_DECODED).unwrap_err();
            assert!(
                matches!(e.kind(), io::ErrorKind::UnexpectedEof | io::ErrorKind::InvalidData),
                "cut at {cut}: {e}"
            );
        }
    }

    #[test]
    fn the_limit_counts_what_streams_out() {
        let body = vec![b'a'; 300_000];
        let packed = compress_to_vec(body.as_slice(), CompressionLevel::Fastest);
        let e = read_all(&packed, 100_000).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::FileTooLarge);
        let mut reader = ZstReader::new(packed.as_slice(), 100_000).unwrap();
        let mut got = Vec::new();
        assert!((&mut reader).take(100_000).read_to_end(&mut got).is_ok());
        assert_eq!(reader.decoded(), 100_000);
    }
}
