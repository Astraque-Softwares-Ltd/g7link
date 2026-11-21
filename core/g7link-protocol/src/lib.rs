// SPDX-License-Identifier: MPL-2.0
//! The G7 Link wire protocol, version 1.
//!
//! This crate is pure protocol logic with no networking: device IDs, message
//! types and the lane each travels on, the CBOR envelope, length-prefixed
//! framing, datagrams and the pairing primitives. It is specified in
//! `docs/protocol/v1.md`.

pub mod ble;
pub mod chat_identity;
mod datagram;
mod envelope;
mod error;
mod frame;
mod id;
pub mod lanes;
pub mod messages;
pub mod pairing;
mod video;

pub use datagram::{
    DATAGRAM_INPUT, DATAGRAM_SOUND, MAX_SOUND_PAYLOAD, SOUND_CODEC_PCM_S16LE, SOUND_HEADER_LEN, SoundDatagram,
};
pub use envelope::Envelope;
pub use error::{ErrorCode, ProtocolError};
pub use frame::{MAX_FRAME_LEN, decode_frame, encode_frame, read_frame, write_frame};
pub use id::{DeviceId, ParseDeviceIdError};
pub use video::{MAX_VIDEO_PACKET, PacketKind, VideoPacket, read_video_packet, video_packet_header};

/// ALPN for sessions between paired devices.
pub const ALPN_SESSION: &[u8] = b"g7link/1";
/// The ALPN of protocol v0. A device still accepts it, only to tell a paired
/// peer that it needs an update: it reads nothing and closes the connection
/// with `VERSION_TOO_OLD` (an unpaired one with `UNPAIRED`).
pub const ALPN_SESSION_V0: &[u8] = b"g7link/0";
/// ALPN for the pairing ceremony.
pub const ALPN_PAIR: &[u8] = b"g7link-pair/0";

/// Protocol version spoken by this implementation. A `hello` with any other
/// `proto` is answered `error VERSION_TOO_OLD` and the connection closed.
pub const PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{DeviceInfo, DeviceKind, Hello, PowerLevel, types};

    #[test]
    fn the_session_speaks_v1_and_keeps_v0_only_to_turn_it_away() {
        assert_eq!(ALPN_SESSION, b"g7link/1");
        assert_eq!(ALPN_SESSION_V0, b"g7link/0");
        assert_eq!(ALPN_PAIR, b"g7link-pair/0", "pairing keeps its own version");
        assert_eq!(PROTOCOL_VERSION, 1);
    }

    #[test]
    fn a_hello_carries_its_version_and_no_minimum() {
        let hello = Hello {
            proto: PROTOCOL_VERSION,
            app: "0.1.18".into(),
            device: DeviceInfo {
                name: "Pixel".into(),
                kind: DeviceKind::Phone,
                os: "android".into(),
                os_ver: "16".into(),
                model: None,
                accent: None,
                screen: None,
                install_id: None,
            },
            caps: vec!["core.ping".into()],
            power: PowerLevel::Assist,
            localsend: None,
        };
        let env = Envelope::new(types::HELLO, &hello).unwrap();
        let ciborium::Value::Map(fields) = env.b.clone().unwrap() else { panic!("a map") };
        let keys: Vec<&str> = fields.iter().filter_map(|(k, _)| k.as_text()).collect();
        assert_eq!(keys, ["proto", "app", "device", "caps", "power"]);
        let back: Hello = Envelope::from_cbor(&env.to_cbor()).unwrap().expect_body(types::HELLO).unwrap();
        assert_eq!(back, hello);
        // A body with a `min` (as v0 sent) still reads; the field means nothing.
        let mut old = fields.clone();
        old.push((ciborium::Value::Text("min".into()), ciborium::Value::Integer(0.into())));
        let old = Envelope { b: Some(ciborium::Value::Map(old)), ..env };
        assert_eq!(old.body::<Hello>().unwrap(), hello);
    }
}
