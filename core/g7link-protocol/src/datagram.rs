// SPDX-License-Identifier: MPL-2.0
//! QUIC datagrams (`docs/protocol/v1.md` §5): one tag byte, then the
//! channel's bytes. A receiver drops a datagram whose tag it doesn't know.

use crate::ProtocolError;

/// Pointer motion, scrolling and the laser: a CBOR `RemoteInput`
/// ([`crate::messages::input`]), at most 256 bytes after the tag.
pub const DATAGRAM_INPUT: u8 = 0x01;
/// PC sound: a [`SoundDatagram`].
pub const DATAGRAM_SOUND: u8 = 0x02;

/// `codec` of 16-bit signed little-endian PCM, channels interleaved. The
/// other values are reserved; a receiver drops sound in a codec it doesn't know.
pub const SOUND_CODEC_PCM_S16LE: u8 = 0;
/// A sound datagram's bytes after its tag and before its payload:
/// `seq` (2), `ts_ms` (4), `codec` (1), `channels` (1), `rate` (4).
pub const SOUND_HEADER_LEN: usize = 12;
/// Most payload bytes in one sound datagram: 5 ms of 48 kHz stereo.
pub const MAX_SOUND_PAYLOAD: usize = 960;

/// One piece of PC sound, in a datagram tagged [`DATAGRAM_SOUND`], or as a
/// frame of the `media/sound` stream without the tag
/// ([`crate::messages::media_sound`]). Integers are little endian.
#[derive(Clone, PartialEq, Eq)]
pub struct SoundDatagram {
    /// Counts up by one per datagram in a sound session, and wraps.
    pub seq: u16,
    /// When the first sample was captured, in ms since the sound session began.
    pub ts_ms: u32,
    pub codec: u8,
    /// 1 or 2.
    pub channels: u8,
    /// Samples a second per channel, 8000 to 96000.
    pub rate: u32,
    /// Whole frames, at most [`MAX_SOUND_PAYLOAD`] bytes.
    pub payload: Vec<u8>,
}

/// Never prints the sound itself.
impl std::fmt::Debug for SoundDatagram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoundDatagram")
            .field("seq", &self.seq)
            .field("ts_ms", &self.ts_ms)
            .field("codec", &self.codec)
            .field("channels", &self.channels)
            .field("rate", &self.rate)
            .field("bytes", &self.payload.len())
            .finish()
    }
}

impl SoundDatagram {
    /// Whether a player can take it: a known codec, a sane rate, mono or
    /// stereo, and whole frames of 16-bit samples. A rate or layout out of
    /// range would play at the wrong speed, and a ragged length makes a
    /// misaligned write to the audio device.
    pub fn is_valid(&self) -> bool {
        self.codec == SOUND_CODEC_PCM_S16LE
            && (8_000..=96_000).contains(&self.rate)
            && (1..=2).contains(&self.channels)
            && (1..=MAX_SOUND_PAYLOAD).contains(&self.payload.len())
            && self.payload.len().is_multiple_of(2 * usize::from(self.channels))
    }

    /// The datagram: [`DATAGRAM_SOUND`], then [`Self::encode_body`].
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + SOUND_HEADER_LEN + self.payload.len());
        out.push(DATAGRAM_SOUND);
        self.write_body(&mut out);
        out
    }

    /// A frame of the `media/sound` stream: the datagram without its tag.
    pub fn encode_body(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SOUND_HEADER_LEN + self.payload.len());
        self.write_body(&mut out);
        out
    }

    fn write_body(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.ts_ms.to_le_bytes());
        out.push(self.codec);
        out.push(self.channels);
        out.extend_from_slice(&self.rate.to_le_bytes());
        out.extend_from_slice(&self.payload);
    }

    /// Reads and checks a datagram tagged [`DATAGRAM_SOUND`].
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        match bytes.split_first() {
            Some((&DATAGRAM_SOUND, body)) => Self::decode_body(body),
            _ => Err(ProtocolError::BadMessage("not a sound datagram".into())),
        }
    }

    /// Reads and checks a frame of the `media/sound` stream.
    pub fn decode_body(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (header, payload) = bytes
            .split_first_chunk::<SOUND_HEADER_LEN>()
            .ok_or_else(|| ProtocolError::BadMessage("short sound datagram".into()))?;
        if payload.len() > MAX_SOUND_PAYLOAD {
            return Err(ProtocolError::FrameTooLarge(payload.len()));
        }
        let [s0, s1, t0, t1, t2, t3, codec, channels, r0, r1, r2, r3] = *header;
        let sound = SoundDatagram {
            seq: u16::from_le_bytes([s0, s1]),
            ts_ms: u32::from_le_bytes([t0, t1, t2, t3]),
            codec,
            channels,
            rate: u32::from_le_bytes([r0, r1, r2, r3]),
            payload: payload.to_vec(),
        };
        if !sound.is_valid() {
            return Err(ProtocolError::BadMessage("unplayable sound datagram".into()));
        }
        Ok(sound)
    }

    /// Whether sound numbered `seq` comes after `last`, the last one played:
    /// counting on from `last` modulo 2^16, it's less than 2^15 ahead. One
    /// that doesn't is old, or a repeat, and is dropped.
    pub fn is_after(seq: u16, last: u16) -> bool {
        (1..0x8000).contains(&seq.wrapping_sub(last))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound() -> SoundDatagram {
        SoundDatagram {
            seq: 0xFFFE,
            ts_ms: 123_456,
            codec: SOUND_CODEC_PCM_S16LE,
            channels: 2,
            rate: 48_000,
            payload: (0..MAX_SOUND_PAYLOAD).map(|i| i as u8).collect(),
        }
    }

    #[test]
    fn a_sound_datagram_round_trips_in_its_published_layout() {
        let s = sound();
        assert!(s.is_valid());
        let bytes = s.encode();
        assert_eq!(bytes.len(), 1 + SOUND_HEADER_LEN + MAX_SOUND_PAYLOAD);
        assert_eq!(
            &bytes[..13],
            &[0x02, 0xFE, 0xFF, 0x40, 0xE2, 0x01, 0x00, 0x00, 0x02, 0x80, 0xBB, 0x00, 0x00]
        );
        assert_eq!(SoundDatagram::decode(&bytes).unwrap(), s);
        assert_eq!(&bytes[1..], s.encode_body().as_slice());
        assert_eq!(SoundDatagram::decode_body(&s.encode_body()).unwrap(), s);
        let mono = SoundDatagram { channels: 1, rate: 8_000, payload: vec![1, 2], ..s };
        assert_eq!(SoundDatagram::decode(&mono.encode()).unwrap(), mono);
        assert!(!format!("{mono:?}").contains("[1, 2]"));
    }

    #[test]
    fn short_truncated_and_unplayable_sound_is_refused() {
        let bytes = sound().encode();
        for len in 0..=1 + SOUND_HEADER_LEN {
            assert!(SoundDatagram::decode(&bytes[..len]).is_err(), "{len} bytes");
        }
        assert!(SoundDatagram::decode(&bytes[..bytes.len() - 1]).is_err(), "a ragged frame");
        assert!(SoundDatagram::decode(&bytes[..bytes.len() - 2]).is_err(), "half a stereo frame");
        assert!(SoundDatagram::decode(&bytes[..bytes.len() - 4]).is_ok(), "one frame fewer");
        let mut long = bytes.clone();
        long.extend_from_slice(&[0; 4]);
        assert!(matches!(SoundDatagram::decode(&long), Err(ProtocolError::FrameTooLarge(964))));
        let bad = |change: fn(&mut SoundDatagram)| {
            let mut s = sound();
            change(&mut s);
            SoundDatagram::decode(&s.encode()).is_err()
        };
        assert!(bad(|s| s.codec = 1), "a codec this build doesn't know");
        assert!(bad(|s| s.channels = 0));
        assert!(bad(|s| s.channels = 3));
        assert!(bad(|s| s.rate = 7_999));
        assert!(bad(|s| s.rate = 96_001));
        assert!(bad(|s| s.payload.clear()));
    }

    #[test]
    fn another_tag_is_not_sound() {
        let mut bytes = sound().encode();
        bytes[0] = DATAGRAM_INPUT;
        assert!(SoundDatagram::decode(&bytes).is_err());
        bytes[0] = 0x03;
        assert!(SoundDatagram::decode(&bytes).is_err());
        assert!(
            SoundDatagram::decode(b"NLPA\x80\xbb\x00\x00\x02\x00\x01\x02\x03\x04").is_err(),
            "v0's sound"
        );
    }

    #[test]
    fn newer_sound_is_found_across_the_wrap() {
        assert!(SoundDatagram::is_after(1, 0));
        assert!(SoundDatagram::is_after(0, 0xFFFF), "wraps");
        assert!(SoundDatagram::is_after(0x7FFF, 0));
        assert!(!SoundDatagram::is_after(0x8000, 0), "half the window back is old");
        assert!(!SoundDatagram::is_after(5, 5), "a repeat");
        assert!(!SoundDatagram::is_after(4, 5));
        assert!(!SoundDatagram::is_after(0xFFFF, 0), "just before the wrap");
    }
}
