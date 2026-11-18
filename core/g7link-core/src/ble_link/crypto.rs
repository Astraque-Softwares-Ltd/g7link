// SPDX-License-Identifier: MPL-2.0
//! The Bluetooth channel's handshake and records ("G7BLE/1",
//! docs/protocol/bluetooth.md): ephemeral X25519 on both sides, keys salted
//! with the pair's link secret, and each side signing the transcript with its
//! device key. Records are ChaCha20-Poly1305 with a counter per direction that
//! must arrive in order. Nothing here does I/O.

use g7link_protocol::{DeviceId, ble::LINK_SECRET_LEN};
use ring::{
    aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey},
    agreement::{self, EphemeralPrivateKey, UnparsedPublicKey, X25519},
    digest, hkdf, hmac,
    rand::SystemRandom,
};

const PROTOCOL: &[u8] = b"G7BLE/1";
const HINT_LABEL: &[u8] = b"g7ble hint";
const KEYS_LABEL: &[u8] = b"G7BLE/1 keys";
const RESPONDER_LABEL: &[u8] = b"G7BLE/1 responder";
const INITIATOR_LABEL: &[u8] = b"G7BLE/1 initiator";

pub(crate) const MSG1: u8 = 0x01;
pub(crate) const MSG2: u8 = 0x02;
pub(crate) const MSG3: u8 = 0x03;
pub(crate) const RECORD: u8 = 0x10;

const KEY_LEN: usize = 32;
const HINT_LEN: usize = 8;
const SIG_LEN: usize = 64;
const TAG_LEN: usize = 16;
pub(crate) const MSG1_LEN: usize = 1 + KEY_LEN + HINT_LEN;
pub(crate) const MSG2_LEN: usize = 1 + KEY_LEN + SIG_LEN + TAG_LEN;
pub(crate) const MSG3_LEN: usize = 1 + SIG_LEN + TAG_LEN;

/// Why a handshake or a record failed. The link ends either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CryptoError {
    #[error("malformed handshake message")]
    Malformed,
    #[error("no paired device shares this secret")]
    UnknownPeer,
    #[error("the other device isn't who it says")]
    BadSignature,
    #[error("a record didn't open")]
    BadRecord,
    #[error("no randomness")]
    Random,
}

/// Signs with this device's key.
pub(crate) type Sign<'a> = &'a dyn Fn(&[u8]) -> [u8; SIG_LEN];

struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn hint(link_secret: &[u8; LINK_SECRET_LEN], e_i: &[u8]) -> [u8; HINT_LEN] {
    let key = hmac::Key::new(hmac::HMAC_SHA256, link_secret);
    let mut ctx = hmac::Context::with_key(&key);
    ctx.update(HINT_LABEL);
    ctx.update(e_i);
    let tag = ctx.sign();
    let mut out = [0; HINT_LEN];
    out.copy_from_slice(&tag.as_ref()[..HINT_LEN]);
    out
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn transcript(e_i: &[u8], hint: &[u8], e_r: &[u8], id_i: &DeviceId, id_r: &DeviceId) -> [u8; 32] {
    let mut ctx = digest::Context::new(&digest::SHA256);
    for part in [PROTOCOL, e_i, hint, e_r, id_i.as_bytes(), id_r.as_bytes()] {
        ctx.update(part);
    }
    let mut out = [0; 32];
    out.copy_from_slice(ctx.finish().as_ref());
    out
}

fn verify(id: &DeviceId, message: &[u8], signature: &[u8]) -> Result<(), CryptoError> {
    let signature: &[u8; SIG_LEN] = signature.try_into().map_err(|_| CryptoError::Malformed)?;
    let key = iroh::PublicKey::from_bytes(id.as_bytes()).map_err(|_| CryptoError::BadSignature)?;
    key.verify(message, &iroh::Signature::from_bytes(signature)).map_err(|_| CryptoError::BadSignature)
}

fn signed(label: &[u8], th: &[u8; 32]) -> Vec<u8> {
    [label, th.as_slice()].concat()
}

/// The two directions' keys, from the shared secret and the transcript.
fn keys(
    link_secret: &[u8; LINK_SECRET_LEN],
    my_key: EphemeralPrivateKey,
    their_key: &[u8],
    th: &[u8; 32],
) -> Result<([u8; KEY_LEN], [u8; KEY_LEN]), CryptoError> {
    let peer = UnparsedPublicKey::new(&X25519, their_key);
    let okm = agreement::agree_ephemeral(my_key, &peer, |dh| {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, link_secret).extract(dh);
        let info = [KEYS_LABEL, th.as_slice()];
        let okm = prk.expand(&info, Len(2 * KEY_LEN)).ok()?;
        let mut out = [0u8; 2 * KEY_LEN];
        okm.fill(&mut out).ok()?;
        Some(out)
    })
    .ok()
    .flatten()
    .ok_or(CryptoError::Malformed)?;
    let (i2r, r2i) = okm.split_at(KEY_LEN);
    Ok((
        i2r.try_into().map_err(|_| CryptoError::Malformed)?,
        r2i.try_into().map_err(|_| CryptoError::Malformed)?,
    ))
}

fn nonce(counter: u64) -> Nonce {
    let mut bytes = [0u8; 12];
    bytes[4..].copy_from_slice(&counter.to_be_bytes());
    Nonce::assume_unique_for_key(bytes)
}

fn aead(key: &[u8; KEY_LEN]) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).expect("32 bytes is a ChaCha20 key"))
}

fn seal_with(key: &LessSafeKey, counter: u64, plain: &[u8]) -> Vec<u8> {
    let mut buf = plain.to_vec();
    key.seal_in_place_append_tag(nonce(counter), Aad::empty(), &mut buf)
        .expect("in-place sealing doesn't fail");
    buf
}

fn open_with(key: &LessSafeKey, counter: u64, sealed: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let mut buf = sealed.to_vec();
    let plain =
        key.open_in_place(nonce(counter), Aad::empty(), &mut buf).map_err(|_| CryptoError::BadRecord)?;
    let len = plain.len();
    buf.truncate(len);
    Ok(buf)
}

/// Sealed and opened records, once the handshake is done. Counters start at
/// 1 (the handshake used 0) and must follow each other exactly.
pub(crate) struct Cipher {
    send: LessSafeKey,
    recv: LessSafeKey,
    sent: u64,
    received: u64,
}

impl std::fmt::Debug for Cipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cipher").field("sent", &self.sent).field("received", &self.received).finish()
    }
}

impl Cipher {
    fn new(send: &[u8; KEY_LEN], recv: &[u8; KEY_LEN]) -> Self {
        Cipher { send: aead(send), recv: aead(recv), sent: 0, received: 0 }
    }

    /// `plain` as the next record: [`RECORD`] and the sealed bytes.
    pub(crate) fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        self.sent += 1;
        let mut out = vec![RECORD];
        out.extend(seal_with(&self.send, self.sent, plain));
        out
    }

    /// The next record's plain bytes.
    pub(crate) fn open(&mut self, record: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let sealed = record.strip_prefix(&[RECORD]).ok_or(CryptoError::BadRecord)?;
        let next = self.received + 1;
        let plain = open_with(&self.recv, next, sealed)?;
        self.received = next;
        Ok(plain)
    }
}

/// The phone's side, between sending msg1 and reading msg2.
pub(crate) struct Initiator {
    me: DeviceId,
    pc: DeviceId,
    link_secret: [u8; LINK_SECRET_LEN],
    key: EphemeralPrivateKey,
    e_i: [u8; KEY_LEN],
    hint: [u8; HINT_LEN],
}

impl std::fmt::Debug for Initiator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Initiator").field("pc", &self.pc.short()).finish_non_exhaustive()
    }
}

impl Initiator {
    /// Starts a handshake with `pc`: the state, and msg1 to send.
    pub(crate) fn start(
        me: DeviceId,
        pc: DeviceId,
        link_secret: [u8; LINK_SECRET_LEN],
    ) -> Result<(Self, Vec<u8>), CryptoError> {
        let key =
            EphemeralPrivateKey::generate(&X25519, &SystemRandom::new()).map_err(|_| CryptoError::Random)?;
        let public = key.compute_public_key().map_err(|_| CryptoError::Random)?;
        let e_i: [u8; KEY_LEN] = public.as_ref().try_into().map_err(|_| CryptoError::Random)?;
        let hint = hint(&link_secret, &e_i);
        let msg1 = [&[MSG1], e_i.as_slice(), hint.as_slice()].concat();
        Ok((Initiator { me, pc, link_secret, key, e_i, hint }, msg1))
    }

    /// The PC this handshake is with.
    pub(crate) fn pc(&self) -> DeviceId {
        self.pc
    }

    /// Reads msg2: checks it's `pc` that answered, and gives the cipher and msg3.
    pub(crate) fn read_msg2(self, msg2: &[u8], sign: Sign<'_>) -> Result<(Cipher, Vec<u8>), CryptoError> {
        if msg2.len() != MSG2_LEN || msg2[0] != MSG2 {
            return Err(CryptoError::Malformed);
        }
        let (e_r, sealed) = msg2[1..].split_at(KEY_LEN);
        let th = transcript(&self.e_i, &self.hint, e_r, &self.me, &self.pc);
        let (i2r, r2i) = keys(&self.link_secret, self.key, e_r, &th)?;
        let signature = open_with(&aead(&r2i), 0, sealed).map_err(|_| CryptoError::BadSignature)?;
        verify(&self.pc, &signed(RESPONDER_LABEL, &th), &signature)?;
        let mine = sign(&signed(INITIATOR_LABEL, &th));
        let msg3 = [&[MSG3], seal_with(&aead(&i2r), 0, &mine).as_slice()].concat();
        Ok((Cipher::new(&i2r, &r2i), msg3))
    }
}

/// The PC's side, between sending msg2 and reading msg3.
pub(crate) struct Responder {
    phone: DeviceId,
    th: [u8; 32],
    i2r: [u8; KEY_LEN],
    r2i: [u8; KEY_LEN],
}

impl std::fmt::Debug for Responder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Responder").field("phone", &self.phone.short()).finish_non_exhaustive()
    }
}

impl Responder {
    /// Reads msg1: finds which of `candidates` (paired phones and their link
    /// secrets) sent it, and gives the state and msg2.
    pub(crate) fn read_msg1(
        me: DeviceId,
        msg1: &[u8],
        candidates: &[(DeviceId, [u8; LINK_SECRET_LEN])],
        sign: Sign<'_>,
    ) -> Result<(Self, Vec<u8>), CryptoError> {
        if msg1.len() != MSG1_LEN || msg1[0] != MSG1 {
            return Err(CryptoError::Malformed);
        }
        let (e_i, their_hint) = msg1[1..].split_at(KEY_LEN);
        let (phone, link_secret) = candidates
            .iter()
            .find(|(_, secret)| same(&hint(secret, e_i), their_hint))
            .ok_or(CryptoError::UnknownPeer)?;
        let key =
            EphemeralPrivateKey::generate(&X25519, &SystemRandom::new()).map_err(|_| CryptoError::Random)?;
        let public = key.compute_public_key().map_err(|_| CryptoError::Random)?;
        let e_r = public.as_ref().to_vec();
        let th = transcript(e_i, their_hint, &e_r, phone, &me);
        let (i2r, r2i) = keys(link_secret, key, e_i, &th)?;
        let mine = sign(&signed(RESPONDER_LABEL, &th));
        let msg2 = [&[MSG2], e_r.as_slice(), seal_with(&aead(&r2i), 0, &mine).as_slice()].concat();
        Ok((Responder { phone: *phone, th, i2r, r2i }, msg2))
    }

    /// The phone that started the handshake (not yet proved).
    pub(crate) fn phone(&self) -> DeviceId {
        self.phone
    }

    /// Reads msg3: checks the phone is who it says, and gives the cipher.
    pub(crate) fn read_msg3(self, msg3: &[u8]) -> Result<Cipher, CryptoError> {
        if msg3.len() != MSG3_LEN || msg3[0] != MSG3 {
            return Err(CryptoError::Malformed);
        }
        let signature = open_with(&aead(&self.i2r), 0, &msg3[1..]).map_err(|_| CryptoError::BadSignature)?;
        verify(&self.phone, &signed(INITIATOR_LABEL, &self.th), &signature)?;
        Ok(Cipher::new(&self.r2i, &self.i2r))
    }
}

/// Both ends' ciphers of a finished handshake between two test devices: the
/// phone's, the PC's, and the two devices.
#[cfg(test)]
pub(crate) fn linked_ciphers() -> (Cipher, Cipher, DeviceId, DeviceId) {
    let (phone_key, pc_key) = (iroh::SecretKey::from_bytes(&[1; 32]), iroh::SecretKey::from_bytes(&[2; 32]));
    let (phone, pc) = (DeviceId(*phone_key.public().as_bytes()), DeviceId(*pc_key.public().as_bytes()));
    let secret = [3; LINK_SECRET_LEN];
    let (initiator, msg1) = Initiator::start(phone, pc, secret).expect("a handshake starts");
    let (responder, msg2) =
        Responder::read_msg1(pc, &msg1, &[(phone, secret)], &|m| pc_key.sign(m).to_bytes()).expect("msg1");
    let (on_phone, msg3) = initiator.read_msg2(&msg2, &|m| phone_key.sign(m).to_bytes()).expect("msg2");
    let on_pc = responder.read_msg3(&msg3).expect("msg3");
    (on_phone, on_pc, phone, pc)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Device {
        key: iroh::SecretKey,
        id: DeviceId,
    }

    fn device(seed: u8) -> Device {
        let key = iroh::SecretKey::from_bytes(&[seed; 32]);
        let id = DeviceId(*key.public().as_bytes());
        Device { key, id }
    }

    fn signer(key: &iroh::SecretKey) -> impl Fn(&[u8]) -> [u8; SIG_LEN] + '_ {
        move |m| key.sign(m).to_bytes()
    }

    /// A full handshake between `phone` and `pc`: both ciphers.
    fn handshake(
        phone: &Device,
        pc: &Device,
        secret: [u8; 32],
        candidates: &[(DeviceId, [u8; 32])],
    ) -> Result<(Cipher, Cipher), CryptoError> {
        let (initiator, msg1) = Initiator::start(phone.id, pc.id, secret)?;
        let (responder, msg2) = Responder::read_msg1(pc.id, &msg1, candidates, &signer(&pc.key))?;
        assert_eq!(responder.phone(), phone.id);
        let (phone_cipher, msg3) = initiator.read_msg2(&msg2, &signer(&phone.key))?;
        let pc_cipher = responder.read_msg3(&msg3)?;
        Ok((phone_cipher, pc_cipher))
    }

    #[test]
    fn a_handshake_gives_both_ends_the_same_records() {
        let (phone, pc) = (device(1), device(2));
        let secret = [9; 32];
        let (mut on_phone, mut on_pc) = handshake(&phone, &pc, secret, &[(phone.id, secret)]).unwrap();
        for n in 0..3u8 {
            let record = on_phone.seal(&[n; 40]);
            assert_eq!(on_pc.open(&record).unwrap(), [n; 40]);
            let back = on_pc.seal(b"pong");
            assert_eq!(on_phone.open(&back).unwrap(), b"pong");
        }
    }

    #[test]
    fn the_pc_finds_the_phone_among_several() {
        let (phone, pc) = (device(1), device(2));
        let candidates = [(device(3).id, [1; 32]), (phone.id, [2; 32]), (device(4).id, [3; 32])];
        assert!(handshake(&phone, &pc, [2; 32], &candidates).is_ok());
    }

    #[test]
    fn a_wrong_secret_is_refused_at_once() {
        let (phone, pc) = (device(1), device(2));
        let (_, msg1) = Initiator::start(phone.id, pc.id, [5; 32]).unwrap();
        let refused = Responder::read_msg1(pc.id, &msg1, &[(phone.id, [6; 32])], &signer(&pc.key));
        assert_eq!(refused.err(), Some(CryptoError::UnknownPeer));
    }

    #[test]
    fn the_right_secret_without_the_right_key_fails() {
        let (phone, pc, impostor) = (device(1), device(2), device(3));
        let secret = [7; 32];
        // A PC that has the secret but not the PC's key: the phone refuses msg2.
        let (initiator, msg1) = Initiator::start(phone.id, pc.id, secret).unwrap();
        let (_, msg2) =
            Responder::read_msg1(pc.id, &msg1, &[(phone.id, secret)], &signer(&impostor.key)).unwrap();
        assert_eq!(initiator.read_msg2(&msg2, &signer(&phone.key)).err(), Some(CryptoError::BadSignature));
        // A phone that has the secret but not the phone's key: the PC refuses msg3.
        let (initiator, msg1) = Initiator::start(phone.id, pc.id, secret).unwrap();
        let (responder, msg2) =
            Responder::read_msg1(pc.id, &msg1, &[(phone.id, secret)], &signer(&pc.key)).unwrap();
        let (_, msg3) = initiator.read_msg2(&msg2, &signer(&impostor.key)).unwrap();
        assert_eq!(responder.read_msg3(&msg3).err(), Some(CryptoError::BadSignature));
    }

    #[test]
    fn an_answer_from_another_handshake_fails() {
        let (phone, pc) = (device(1), device(2));
        let secret = [8; 32];
        let candidates = [(phone.id, secret)];
        let (first, _) = Initiator::start(phone.id, pc.id, secret).unwrap();
        let (_, other_msg1) = Initiator::start(phone.id, pc.id, secret).unwrap();
        let (_, msg2) = Responder::read_msg1(pc.id, &other_msg1, &candidates, &signer(&pc.key)).unwrap();
        assert!(first.read_msg2(&msg2, &signer(&phone.key)).is_err());
    }

    #[test]
    fn replayed_reordered_or_altered_records_fail() {
        let (phone, pc) = (device(1), device(2));
        let secret = [4; 32];
        let (mut on_phone, mut on_pc) = handshake(&phone, &pc, secret, &[(phone.id, secret)]).unwrap();
        let first = on_phone.seal(b"one");
        let second = on_phone.seal(b"two");
        assert!(on_pc.open(&second).is_err(), "out of order");

        let (mut on_phone, mut on_pc) = handshake(&phone, &pc, secret, &[(phone.id, secret)]).unwrap();
        let first_again = on_phone.seal(b"one");
        assert!(on_pc.open(&first).is_err(), "from another link");
        assert_eq!(on_pc.open(&first_again).unwrap(), b"one");
        assert!(on_pc.open(&first_again).is_err(), "replayed");

        let mut altered = on_phone.seal(b"three");
        altered[3] ^= 1;
        assert!(on_pc.open(&altered).is_err());
        assert!(on_pc.open(b"").is_err());
    }

    #[test]
    fn malformed_messages_are_refused() {
        let (phone, pc) = (device(1), device(2));
        let candidates = [(phone.id, [1; 32])];
        let sign = signer(&pc.key);
        assert_eq!(
            Responder::read_msg1(pc.id, &[MSG1], &candidates, &sign).err(),
            Some(CryptoError::Malformed)
        );
        assert_eq!(
            Responder::read_msg1(pc.id, &[MSG2; MSG1_LEN], &candidates, &sign).err(),
            Some(CryptoError::Malformed)
        );
        let (initiator, _) = Initiator::start(phone.id, pc.id, [1; 32]).unwrap();
        assert_eq!(initiator.read_msg2(&[MSG2; 3], &signer(&phone.key)).err(), Some(CryptoError::Malformed));
    }
}
