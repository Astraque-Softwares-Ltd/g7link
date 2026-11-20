// SPDX-License-Identifier: MPL-2.0
//! Bluetooth LE: the PC's rotating beacon, and the framing of the channel a
//! paired phone opens to the PC's GATT service.
//!
//! A PC broadcasts manufacturer data (company [`COMPANY_ID`]) made of
//! [`HEADER`] and six-byte slots. Each slot is a tag only one paired phone can
//! recognise: a truncated HMAC of the time window under a key derived from the
//! pair's link secret. The rest of the slots are random, and the slots are in
//! random order, so nothing in the beacon stays the same from one window to
//! the next, and it doesn't say how many phones are paired.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::messages::types;

type HmacSha256 = Hmac<Sha256>;

/// Bluetooth SIG company ID for testing, used by every G7 Link beacon.
pub const COMPANY_ID: u16 = 0xFFFF;
/// The first bytes of a rotating beacon's manufacturer data: 'G' and version 2.
pub const HEADER: [u8; 2] = [0x47, 0x02];
/// Bytes in a tag.
pub const TAG_LEN: usize = 6;
/// A tag is good for this long, and for one window either side.
pub const EPOCH_SECS: u64 = 900;
/// Slots in a beacon that can't be connected to (27 bytes are free).
pub const BEACON_SLOTS: usize = 4;
/// Slots in an advertisement that can be connected to (BlueZ adds flags).
pub const WINDOW_SLOTS: usize = 3;
/// Bytes in a pair's link secret.
pub const LINK_SECRET_LEN: usize = 32;

const BEACON_KEY_LABEL: &[u8] = b"g7link ble beacon v2";

/// What a tag asks of the phone that recognises it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKind {
    /// The PC is here.
    Presence,
    /// The PC can be connected to over Bluetooth now, and wants this phone to.
    Connect,
}

impl TagKind {
    fn byte(self) -> u8 {
        match self {
            TagKind::Presence => b'P',
            TagKind::Connect => b'C',
        }
    }
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

/// The key a pair's beacon tags are made with, from its link secret.
pub fn beacon_key(link_secret: &[u8; LINK_SECRET_LEN]) -> [u8; 32] {
    hmac(link_secret, &[BEACON_KEY_LABEL])
}

/// The time window `unix_secs` falls in.
pub fn epoch_at(unix_secs: u64) -> u64 {
    unix_secs / EPOCH_SECS
}

/// The tag of `kind` for window `epoch`.
pub fn tag(beacon_key: &[u8; 32], kind: TagKind, epoch: u64) -> [u8; TAG_LEN] {
    let full = hmac(beacon_key, &[&[kind.byte()], &epoch.to_be_bytes()]);
    let mut out = [0; TAG_LEN];
    out.copy_from_slice(&full[..TAG_LEN]);
    out
}

/// A beacon's manufacturer data: [`HEADER`], then `slots` slots holding
/// `tags` (as many as fit) and bytes from `filler` for the rest, placed in the
/// order `order` gives (a permutation of `0..slots`; slot `order[i]` gets the
/// `i`th). The caller supplies the randomness, `slots * TAG_LEN` filler bytes
/// at least.
pub fn payload(tags: &[[u8; TAG_LEN]], slots: usize, filler: &[u8], order: &[usize]) -> Vec<u8> {
    let mut body = vec![0u8; slots * TAG_LEN];
    for i in 0..slots {
        let at = order.get(i).copied().filter(|&o| o < slots).unwrap_or(i) * TAG_LEN;
        let value: &[u8] = match tags.get(i) {
            Some(tag) => tag,
            None => filler.get(i * TAG_LEN..(i + 1) * TAG_LEN).unwrap_or(&[0; TAG_LEN]),
        };
        body[at..at + TAG_LEN].copy_from_slice(value);
    }
    let mut out = Vec::with_capacity(HEADER.len() + body.len());
    out.extend_from_slice(&HEADER);
    out.extend_from_slice(&body);
    out
}

/// The slots of a beacon's manufacturer data, or `None` when it isn't one.
fn slots(payload: &[u8]) -> Option<&[[u8; TAG_LEN]]> {
    let (slots, rest) = payload.strip_prefix(&HEADER)?.as_chunks::<TAG_LEN>();
    (rest.is_empty() && !slots.is_empty() && slots.len() <= BEACON_SLOTS).then_some(slots)
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// What `payload` holds for the pair whose key is `beacon_key`, in window
/// `epoch` or the one either side (the clocks may differ). `Connect` wins
/// over `Presence`.
pub fn match_payload(payload: &[u8], beacon_key: &[u8; 32], epoch: u64) -> Option<TagKind> {
    let slots = slots(payload)?;
    let epochs = [epoch.checked_sub(1), Some(epoch), epoch.checked_add(1)];
    [TagKind::Connect, TagKind::Presence].into_iter().find(|&kind| {
        epochs.into_iter().flatten().any(|e| {
            let wanted = tag(beacon_key, kind, e);
            slots.iter().fold(false, |found, slot| found | same(slot, &wanted))
        })
    })
}

// ---- The Bluetooth channel: GATT service and framing ----

/// The PC's GATT service the phone connects to.
pub const SERVICE_UUID: &str = "bcac09a4-522f-43c6-9533-b30bd7428154";
/// Phone to PC: written, with a response.
pub const RX_UUID: &str = "7f2964e4-c2df-4b2e-8425-68d2be4c85ab";
/// PC to phone: notified.
pub const TX_UUID: &str = "ea1df14d-6860-4376-a3b0-64fdcb79d7f2";
/// A record at most: a 4096-byte message and its 16-byte tag.
pub const MAX_RECORD: usize = 4096 + 16;
/// A record's bytes besides its message: the record byte and the tag.
pub const RECORD_OVERHEAD: usize = 1 + 16;
/// A message's plain bytes at most, so that its record fits [`MAX_RECORD`].
pub const MAX_PLAIN: usize = MAX_RECORD - RECORD_OVERHEAD;
/// A record before the other device has proved who it is: a handshake message.
pub const MAX_HANDSHAKE_RECORD: usize = 256;
/// The smallest ATT MTU, whose values hold 20 bytes.
pub const MIN_ATT_MTU: u16 = 23;
/// The largest ATT MTU.
pub const MAX_ATT_MTU: u16 = 517;

const FIRST: u8 = 0x80;
const LAST: u8 = 0x40;
const SEQ: u8 = 0x3F;

/// Why a Bluetooth link ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("record too long")]
    TooLong,
    #[error("fragment out of order")]
    OutOfOrder,
    #[error("malformed fragment")]
    Malformed,
}

/// The bytes one value carries over a link with `att_mtu`.
fn value_len(att_mtu: u16) -> usize {
    usize::from(att_mtu.clamp(MIN_ATT_MTU, MAX_ATT_MTU)) - 3
}

/// The message types the Bluetooth channel carries (`docs/protocol/bluetooth.md`
/// §4.4); anything else is refused. There are no streams on the channel, so
/// every request keeps its `id` and every answer its `re`: `input.check`, an
/// RPC in a session, is an ordinary request here.
pub const CARRIED_TYPES: &[&str] = &[
    types::PING,
    types::PONG,
    types::OK,
    types::ERROR,
    types::DEVICE_RING,
    types::DEVICE_RING_STOPPED,
    types::CLIP_SET,
    types::BLE_OTP,
    types::TOGGLES_SET,
    types::TOGGLES_STATE,
    types::INPUT_CHECK,
    types::INPUT_EVENT,
    types::INPUT_TEXT_WATCH,
    types::INPUT_TEXT_STATE,
    types::INPUT_TEXT_FOCUS,
    types::BLE_USE,
];

/// Whether the Bluetooth channel carries messages of type `t`.
pub fn carries(t: &str) -> bool {
    CARRIED_TYPES.contains(&t)
}

/// The fragments `record` goes in over a link with `att_mtu`, numbered on from
/// `seq`, which is left at the next number. Each fragment is one value: a
/// header byte (first, last, a 6-bit sequence number), then on the first the
/// record's length (big endian, 2 bytes), then the record's bytes.
pub fn fragments(record: &[u8], att_mtu: u16, seq: &mut u8) -> Result<Vec<Vec<u8>>, LinkError> {
    let len = u16::try_from(record.len()).map_err(|_| LinkError::TooLong)?;
    if record.len() > MAX_RECORD {
        return Err(LinkError::TooLong);
    }
    let value = value_len(att_mtu);
    let mut out = Vec::with_capacity(record.len() / (value - 1) + 1);
    let mut rest = record;
    let mut first = true;
    loop {
        let room = value - 1 - if first { 2 } else { 0 };
        let (chunk, tail) = rest.split_at(rest.len().min(room));
        let last = tail.is_empty();
        let mut fragment = Vec::with_capacity(value);
        fragment.push(if first { FIRST } else { 0 } | if last { LAST } else { 0 } | (*seq & SEQ));
        *seq = seq.wrapping_add(1) & SEQ;
        if first {
            fragment.extend_from_slice(&len.to_be_bytes());
        }
        fragment.extend_from_slice(chunk);
        out.push(fragment);
        if last {
            return Ok(out);
        }
        rest = tail;
        first = false;
    }
}

/// How many fragments a record of `len` bytes goes in over a link with
/// `att_mtu`, as [`fragments`] cuts it.
pub fn fragment_count(len: usize, att_mtu: u16) -> usize {
    let value = value_len(att_mtu);
    let first = value - 3;
    if len <= first { 1 } else { 1 + (len - first).div_ceil(value - 1) }
}

/// Puts records back together from their fragments, in order, refusing
/// anything out of sequence or longer than allowed before it holds the bytes.
#[derive(Debug, Default)]
pub struct Reassembler {
    next_seq: u8,
    /// The record coming in: its announced length and what arrived of it.
    open: Option<(usize, Vec<u8>)>,
}

impl Reassembler {
    /// Takes the next fragment; a whole record once its last fragment is in.
    /// Records longer than `limit` are refused. Any error ends the link.
    pub fn push(&mut self, fragment: &[u8], limit: usize) -> Result<Option<Vec<u8>>, LinkError> {
        let (&head, body) = fragment.split_first().ok_or(LinkError::Malformed)?;
        if head & SEQ != self.next_seq {
            return Err(LinkError::OutOfOrder);
        }
        self.next_seq = (self.next_seq + 1) & SEQ;
        let body = if head & FIRST != 0 {
            if self.open.is_some() {
                return Err(LinkError::OutOfOrder);
            }
            let (len, rest) = body.split_first_chunk::<2>().ok_or(LinkError::Malformed)?;
            let len = usize::from(u16::from_be_bytes(*len));
            if len > limit.min(MAX_RECORD) {
                return Err(LinkError::TooLong);
            }
            self.open = Some((len, Vec::with_capacity(len)));
            rest
        } else {
            body
        };
        let Some((len, buf)) = self.open.as_mut() else {
            return Err(LinkError::OutOfOrder);
        };
        if buf.len() + body.len() > *len {
            return Err(LinkError::TooLong);
        }
        buf.extend_from_slice(body);
        if head & LAST == 0 {
            return Ok(None);
        }
        let (len, buf) = self.open.take().ok_or(LinkError::Malformed)?;
        if buf.len() != len {
            return Err(LinkError::Malformed);
        }
        Ok(Some(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn secret() -> [u8; 32] {
        std::array::from_fn(|i| i as u8)
    }

    #[test]
    fn matches_the_published_vectors() {
        let key = beacon_key(&secret());
        assert_eq!(hex(&key), "e76f53a6349d38f0a86e427569bec16bbfa93aed3edc8ba68685eb0a9f0e5f20");
        assert_eq!(epoch_at(1_710_000_000), 1_900_000);
        assert_eq!(hex(&tag(&key, TagKind::Presence, 1_900_000)), "01340facca29");
        assert_eq!(hex(&tag(&key, TagKind::Presence, 1_900_001)), "f236dc484abb");
        assert_eq!(hex(&tag(&key, TagKind::Connect, 1_900_000)), "adac05998b8d");
        assert_eq!(hex(&tag(&key, TagKind::Connect, 1_900_001)), "26311d70bd72");
    }

    #[test]
    fn a_tag_is_found_in_any_slot_and_the_window_either_side() {
        let key = beacon_key(&secret());
        let filler = [0xAAu8; BEACON_SLOTS * TAG_LEN];
        for at in 0..BEACON_SLOTS {
            let mut order: Vec<usize> = (0..BEACON_SLOTS).collect();
            order.swap(0, at);
            let data = payload(&[tag(&key, TagKind::Presence, 100)], BEACON_SLOTS, &filler, &order);
            assert_eq!(data.len(), HEADER.len() + BEACON_SLOTS * TAG_LEN);
            assert_eq!(&data[2 + at * TAG_LEN..2 + (at + 1) * TAG_LEN], tag(&key, TagKind::Presence, 100));
            assert_eq!(match_payload(&data, &key, 100), Some(TagKind::Presence));
            assert_eq!(match_payload(&data, &key, 99), Some(TagKind::Presence));
            assert_eq!(match_payload(&data, &key, 101), Some(TagKind::Presence));
            assert_eq!(match_payload(&data, &key, 102), None);
            assert_eq!(match_payload(&data, &key, 98), None);
        }
    }

    #[test]
    fn another_pairs_key_and_other_data_find_nothing() {
        let key = beacon_key(&secret());
        let other = beacon_key(&[7; 32]);
        let filler = [0u8; BEACON_SLOTS * TAG_LEN];
        let data = payload(&[tag(&key, TagKind::Connect, 5)], WINDOW_SLOTS, &filler, &[2, 0, 1]);
        assert_eq!(data.len(), HEADER.len() + WINDOW_SLOTS * TAG_LEN);
        assert_eq!(match_payload(&data, &key, 5), Some(TagKind::Connect));
        assert_eq!(match_payload(&data, &other, 5), None);
        // The old beacon, a wrong header, too many slots, an odd length.
        let mut legacy = b"NCTR\x01".to_vec();
        legacy.extend_from_slice(&[1; 8]);
        assert_eq!(match_payload(&legacy, &key, 5), None);
        let mut wrong = data.clone();
        wrong[1] = 0x03;
        assert_eq!(match_payload(&wrong, &key, 5), None);
        let mut long = HEADER.to_vec();
        long.extend(std::iter::repeat_n(tag(&key, TagKind::Connect, 5), BEACON_SLOTS + 1).flatten());
        assert_eq!(match_payload(&long, &key, 5), None);
        assert_eq!(match_payload(&data[..data.len() - 1], &key, 5), None);
        assert_eq!(match_payload(&HEADER, &key, 5), None);
        assert_eq!(match_payload(&[], &key, 5), None);
    }

    #[test]
    fn connect_wins_over_presence() {
        let key = beacon_key(&secret());
        let tags = [tag(&key, TagKind::Presence, 9), tag(&key, TagKind::Connect, 9)];
        let data = payload(&tags, WINDOW_SLOTS, &[0; WINDOW_SLOTS * TAG_LEN], &[0, 1, 2]);
        assert_eq!(match_payload(&data, &key, 9), Some(TagKind::Connect));
    }

    #[test]
    fn a_bad_order_or_short_filler_never_panics() {
        let data = payload(&[], BEACON_SLOTS, &[], &[9, 9]);
        assert_eq!(data.len(), HEADER.len() + BEACON_SLOTS * TAG_LEN);
    }

    #[test]
    fn records_come_back_whole_from_their_fragments() {
        for mtu in [MIN_ATT_MTU, 100, MAX_ATT_MTU] {
            for len in [0, 1, 17, 18, 19, 20, 36, 37, 300, 2922, MAX_RECORD] {
                let record: Vec<u8> = (0..len).map(|i| i as u8).collect();
                let mut seq = 60;
                let parts = fragments(&record, mtu, &mut seq).unwrap();
                assert!(parts.iter().all(|p| p.len() <= usize::from(mtu) - 3), "fits the MTU");
                assert_eq!(parts.len(), fragment_count(len, mtu), "{len} bytes at MTU {mtu}");
                let mut rx = Reassembler { next_seq: 60, open: None };
                let mut out = None;
                for (i, part) in parts.iter().enumerate() {
                    let got = rx.push(part, MAX_RECORD).unwrap();
                    assert_eq!(got.is_some(), i == parts.len() - 1);
                    out = got.or(out);
                }
                assert_eq!(out.unwrap(), record);
            }
        }
    }

    #[test]
    fn the_channel_carries_the_v1_names_and_nothing_on_a_stream_only_lane() {
        assert_eq!(CARRIED_TYPES.len(), 16);
        for t in CARRIED_TYPES {
            assert!(crate::lanes::lane_of(t).is_some(), "{t} is a known type");
        }
        assert!(carries("input.event") && carries("toggles.set") && carries("input.check"));
        for old in ["remote.input", "remote.check", "phone.toggle.set", "phone.toggles"] {
            assert!(!carries(old), "{old}");
        }
        assert!(!carries(types::PART), "no streams, so no parts");
        assert!(!carries(types::MEDIA_COMMAND));
    }

    #[test]
    fn a_largest_text_state_takes_six_notifications() {
        assert_eq!(MAX_PLAIN, 4095);
        // bluetooth.md §4.4: 2922 bytes sealed at MTU 517, 1002 at MTU 23.
        assert_eq!(fragment_count(2922, MAX_ATT_MTU), 6);
        assert_eq!(fragment_count(1002, MIN_ATT_MTU), 53);
    }

    #[test]
    fn too_long_out_of_order_or_malformed_fragments_end_the_link() {
        let mut seq = 0;
        assert_eq!(fragments(&vec![0; MAX_RECORD + 1], MIN_ATT_MTU, &mut seq), Err(LinkError::TooLong));
        // Announcing more than allowed is refused before anything is kept.
        assert_eq!(Reassembler::default().push(&[FIRST, 0x01, 0x2C, 1], 256), Err(LinkError::TooLong));
        assert_eq!(Reassembler::default().push(&[FIRST | 3, 0, 1, 1], 256), Err(LinkError::OutOfOrder));
        assert_eq!(Reassembler::default().push(&[0], 256), Err(LinkError::OutOfOrder), "no record open");
        assert_eq!(Reassembler::default().push(&[], 256), Err(LinkError::Malformed));
        assert_eq!(Reassembler::default().push(&[FIRST | LAST, 0], 256), Err(LinkError::Malformed));
        // More bytes than announced, and a first fragment inside a record.
        assert_eq!(Reassembler::default().push(&[FIRST | LAST, 0, 1, 7, 7], 256), Err(LinkError::TooLong));
        let mut rx = Reassembler::default();
        assert_eq!(rx.push(&[FIRST, 0, 4, 1], 256), Ok(None));
        assert_eq!(rx.push(&[FIRST | 1, 0, 1, 1], 256), Err(LinkError::OutOfOrder));
        // Short of what was announced.
        let mut rx = Reassembler::default();
        assert_eq!(rx.push(&[FIRST, 0, 4, 1], 256), Ok(None));
        assert_eq!(rx.push(&[LAST | 1, 2], 256), Err(LinkError::Malformed));
        // A duplicate.
        let mut seq = 0;
        let parts = fragments(&[5; 40], MIN_ATT_MTU, &mut seq).unwrap();
        let mut rx = Reassembler::default();
        rx.push(&parts[0], 256).unwrap();
        assert_eq!(rx.push(&parts[0], 256), Err(LinkError::OutOfOrder));
    }
}
