// SPDX-License-Identifier: MPL-2.0
//! One Bluetooth link's end, without I/O: the platform hands it each value
//! the other device wrote or notified ([`BleEndpoint::receive`]) and sends
//! what [`BleEndpoint::next_outbound`] gives it, in order. It runs the
//! handshake, then seals and opens records, each a CBOR [`Envelope`].

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use g7link_protocol::{
    DeviceId, Envelope,
    ble::{self, LinkError, MAX_HANDSHAKE_RECORD, MAX_PLAIN, MAX_RECORD, RECORD_OVERHEAD, Reassembler},
    messages::{RemoteInput, remote as remote_limits, types},
};

use super::crypto::{Cipher, CryptoError, Initiator, Responder};
use crate::{Error, Result, events::NodeEvent, node::Shared, remote::TokenBucket};

/// A message's plain bytes at most: sealed, it fills a record.
pub const MAX_MESSAGE: usize = MAX_PLAIN;
/// Fragments waiting to go out at most; past that, sending fails.
const MAX_OUTBOX: usize = 2048;
/// Records from the other device: a burst of this many, then this many a second.
const RECORD_BURST: f64 = 20.0;
const RECORDS_PER_SEC: f64 = 2.0;
/// Keys, clicks and edits from the phone, which typing sends several of a
/// second, and what comes with them (the typing box's watch, the hold's
/// renewals, pings): a budget of their own, so they never starve the rest.
const INPUT_BURST: f64 = 60.0;
const INPUTS_PER_SEC: f64 = 25.0;
/// The PC's text states and focus: it sends at most 10 a second.
const STATE_BURST: f64 = 20.0;
const STATES_PER_SEC: f64 = 12.0;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

enum Phase {
    /// PC: waiting for the phone's first message.
    AwaitMsg1,
    /// Phone: waiting for the PC's answer.
    AwaitMsg2(Box<Initiator>),
    /// PC: waiting for the phone's proof.
    AwaitMsg3(Box<Responder>),
    Up {
        peer: DeviceId,
        cipher: Box<Cipher>,
    },
    Closed,
}

struct Inner {
    phase: Phase,
    att_mtu: u16,
    rx: Reassembler,
    tx_seq: u8,
    outbox: VecDeque<Vec<u8>>,
    incoming: TokenBucket,
    input: TokenBucket,
    state: TokenBucket,
    /// Touchpad input not sent yet: added up while the radio is busy, and
    /// sealed only when there's room, so it never queues up behind itself.
    motion: Motion,
    /// The newest text state not sent yet (plain CBOR): a newer one replaces
    /// it, and it's sealed only when there's room, as motion is.
    pending_state: Option<Vec<u8>>,
}

/// Which budget a message from the other device draws from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Budget {
    /// Pointer motion, held to the datagram limit where it's run, and
    /// replies, which come as fast as this side asks (one to a request no
    /// longer waited for is dropped at once).
    None,
    Records,
    Input,
    State,
}

fn budget_for(env: &Envelope) -> Budget {
    if env.re.is_some() {
        return Budget::None;
    }
    match env.t.as_str() {
        types::REMOTE_INPUT if env.id.is_none() => Budget::None,
        types::REMOTE_INPUT | types::REMOTE_TEXT_WATCH | types::BLE_USE | types::PING => Budget::Input,
        types::REMOTE_TEXT_STATE | types::REMOTE_TEXT_FOCUS => Budget::State,
        _ => Budget::Records,
    }
}

/// Pointer motion, scrolling and the laser waiting to go out.
#[derive(Debug, Default)]
struct Motion {
    moved: Option<(f32, f32)>,
    scrolled: Option<(f32, f32)>,
    laser: Option<(f32, f32)>,
}

impl Motion {
    fn add(&mut self, event: &RemoteInput) -> bool {
        fn sum(slot: &mut Option<(f32, f32)>, dx: f32, dy: f32, max: f32) {
            let (x, y) = slot.unwrap_or((0.0, 0.0));
            *slot = Some(((x + dx).clamp(-max, max), (y + dy).clamp(-max, max)));
        }
        match *event {
            RemoteInput::Move { dx, dy } => sum(&mut self.moved, dx, dy, remote_limits::MAX_MOVE_DELTA),
            RemoteInput::Scroll { dx, dy } => {
                sum(&mut self.scrolled, dx, dy, remote_limits::MAX_SCROLL_DELTA)
            }
            RemoteInput::Laser { on: true, x, y } => self.laser = Some((x, y)),
            _ => return false,
        }
        true
    }

    fn take(&mut self) -> Option<RemoteInput> {
        if let Some((dx, dy)) = self.moved.take() {
            return Some(RemoteInput::Move { dx, dy });
        }
        if let Some((dx, dy)) = self.scrolled.take() {
            return Some(RemoteInput::Scroll { dx, dy });
        }
        self.laser.take().map(|(x, y)| RemoteInput::Laser { on: true, x, y })
    }
}

/// What a record that came in asks of the endpoint, done once its lock is let go.
enum Then {
    Nothing,
    LinkUp(DeviceId),
    Message(DeviceId, Envelope),
}

/// One end of a Bluetooth link with a paired device.
pub struct BleEndpoint {
    id: u64,
    shared: Weak<Shared>,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for BleEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BleEndpoint").field("id", &self.id).field("peer", &self.peer()).finish()
    }
}

impl BleEndpoint {
    fn new(shared: Weak<Shared>, phase: Phase) -> Self {
        let now = Instant::now();
        BleEndpoint {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            shared,
            inner: Mutex::new(Inner {
                phase,
                att_mtu: ble::MIN_ATT_MTU,
                rx: Reassembler::default(),
                tx_seq: 0,
                outbox: VecDeque::new(),
                incoming: TokenBucket::new(RECORD_BURST, RECORDS_PER_SEC, now),
                input: TokenBucket::new(INPUT_BURST, INPUTS_PER_SEC, now),
                state: TokenBucket::new(STATE_BURST, STATES_PER_SEC, now),
                motion: Motion::default(),
                pending_state: None,
            }),
        }
    }

    /// The PC's end: waits for a phone to start.
    pub(crate) fn accept(shared: &Arc<Shared>) -> Arc<Self> {
        Arc::new(Self::new(Arc::downgrade(shared), Phase::AwaitMsg1))
    }

    /// The phone's end: starts the handshake with `pc`.
    pub(crate) fn connect(
        shared: &Arc<Shared>,
        pc: DeviceId,
        link_secret: [u8; ble::LINK_SECRET_LEN],
    ) -> Result<Arc<Self>> {
        let (initiator, msg1) = Initiator::start(shared.id, pc, link_secret).map_err(internal)?;
        let endpoint = Arc::new(Self::new(Arc::downgrade(shared), Phase::AwaitMsg2(Box::new(initiator))));
        endpoint.queue(&msg1)?;
        Ok(endpoint)
    }

    /// The ID this device gave the link, as [`NodeEvent::BleOutbound`] names it.
    pub fn id(&self) -> u64 {
        self.id
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The link's ATT MTU, once known.
    pub fn set_mtu(&self, att_mtu: u16) {
        self.lock().att_mtu = att_mtu.clamp(ble::MIN_ATT_MTU, ble::MAX_ATT_MTU);
    }

    /// The link's ATT MTU (the smallest until the platform says).
    pub fn att_mtu(&self) -> u16 {
        self.lock().att_mtu
    }

    /// The other device, once it has proved who it is.
    pub fn peer(&self) -> Option<DeviceId> {
        match &self.lock().phase {
            Phase::Up { peer, .. } => Some(*peer),
            _ => None,
        }
    }

    pub fn is_up(&self) -> bool {
        self.peer().is_some()
    }

    /// The next value to write or notify, sized for the link; `None` when
    /// there's nothing. Touchpad input waiting is sealed now, added up, and
    /// then the newest text state.
    pub fn next_outbound(&self) -> Option<Vec<u8>> {
        let mut inner = self.lock();
        if inner.outbox.is_empty() && matches!(inner.phase, Phase::Up { .. }) {
            let plain = match inner.motion.take() {
                Some(event) => Envelope::new(types::REMOTE_INPUT, &event).ok().map(|env| env.to_cbor()),
                None => inner.pending_state.take(),
            };
            if let Some(plain) = plain
                && let Err(error) = self.seal_locked(&mut inner, &plain, false)
            {
                tracing::debug!(%error, "dropped a message the link had no room for");
            }
        }
        inner.outbox.pop_front()
    }

    /// Queues touchpad input (motion, a scroll, the laser) to go when the
    /// link has room, added to what's waiting.
    pub(crate) fn send_motion(&self, event: &RemoteInput) -> Result<()> {
        let mut inner = self.lock();
        if !matches!(inner.phase, Phase::Up { .. }) {
            return Err(Error::Offline);
        }
        if !inner.motion.add(event) {
            return Err(Error::Protocol("not touchpad input".into()));
        }
        if inner.outbox.is_empty()
            && let Some(shared) = self.shared.upgrade()
        {
            shared.emit(NodeEvent::BleOutbound { endpoint: self.id });
        }
        Ok(())
    }

    /// Queues a text state (`env`) to go when the link has room, replacing
    /// one still waiting: a slow link carries fewer, newer states.
    pub(crate) fn send_state(&self, env: &Envelope) -> Result<()> {
        let plain = env.to_cbor();
        if plain.len() > MAX_MESSAGE {
            return Err(Error::TooLarge);
        }
        let mut inner = self.lock();
        if !matches!(inner.phase, Phase::Up { .. }) {
            return Err(Error::Offline);
        }
        inner.pending_state = Some(plain);
        if inner.outbox.is_empty()
            && let Some(shared) = self.shared.upgrade()
        {
            shared.emit(NodeEvent::BleOutbound { endpoint: self.id });
        }
        Ok(())
    }

    /// Queues `record`'s fragments.
    fn queue(&self, record: &[u8]) -> Result<()> {
        let mut inner = self.lock();
        self.queue_locked(&mut inner, record)
    }

    fn queue_locked(&self, inner: &mut Inner, record: &[u8]) -> Result<()> {
        self.queue_fragments(inner, record, true)
    }

    /// Queues `record`'s fragments; with `notify`, tells the platform when
    /// there's something to send again.
    fn queue_fragments(&self, inner: &mut Inner, record: &[u8], notify: bool) -> Result<()> {
        let mut seq = inner.tx_seq;
        let fragments = ble::fragments(record, inner.att_mtu, &mut seq).map_err(link)?;
        if inner.outbox.len() + fragments.len() > MAX_OUTBOX {
            return Err(Error::Offline);
        }
        inner.tx_seq = seq;
        let was_empty = inner.outbox.is_empty();
        inner.outbox.extend(fragments);
        if notify
            && was_empty
            && let Some(shared) = self.shared.upgrade()
        {
            shared.emit(NodeEvent::BleOutbound { endpoint: self.id });
        }
        Ok(())
    }

    /// Seals `plain` as the next record and queues it. Whether it fits, and
    /// whether the outbox has room, is checked first: a record sealed and
    /// then not sent would leave the other device a counter behind, and the
    /// link would end at the next one.
    fn seal_locked(&self, inner: &mut Inner, plain: &[u8], notify: bool) -> Result<()> {
        let len = plain.len() + RECORD_OVERHEAD;
        if plain.len() > MAX_MESSAGE || len > MAX_RECORD {
            return Err(Error::TooLarge);
        }
        if inner.outbox.len() + ble::fragment_count(len, inner.att_mtu) > MAX_OUTBOX {
            return Err(Error::Offline);
        }
        let Phase::Up { cipher, .. } = &mut inner.phase else {
            return Err(Error::Offline);
        };
        let record = cipher.seal(plain);
        self.queue_fragments(inner, &record, notify)
    }

    /// Sends `env` on the link, once it's up.
    pub(crate) fn send(&self, env: &Envelope) -> Result<()> {
        let plain = env.to_cbor();
        if plain.len() > MAX_MESSAGE {
            return Err(Error::TooLarge);
        }
        let mut inner = self.lock();
        self.seal_locked(&mut inner, &plain, true)
    }

    /// One value from the other device. An error means the link is done: the
    /// platform disconnects and drops this end.
    pub fn receive(self: &Arc<Self>, fragment: &[u8]) -> Result<()> {
        let then = self.take(fragment, Instant::now());
        match then {
            Err(error) => {
                self.close();
                Err(error)
            }
            Ok(Then::Nothing) => Ok(()),
            Ok(Then::LinkUp(peer)) => {
                if let Some(shared) = self.shared.upgrade() {
                    shared.ble_link_up(peer, self);
                }
                Ok(())
            }
            Ok(Then::Message(peer, env)) => {
                if let Some(shared) = self.shared.upgrade() {
                    shared.ble_message(peer, self, env);
                }
                Ok(())
            }
        }
    }

    fn take(&self, fragment: &[u8], now: Instant) -> Result<Then> {
        let mut inner = self.lock();
        let limit = match inner.phase {
            Phase::Up { .. } => MAX_RECORD,
            Phase::Closed => return Err(Error::Offline),
            _ => MAX_HANDSHAKE_RECORD,
        };
        let Some(record) = inner.rx.push(fragment, limit).map_err(link)? else {
            return Ok(Then::Nothing);
        };
        match std::mem::replace(&mut inner.phase, Phase::Closed) {
            Phase::AwaitMsg1 => {
                let shared = self.shared.upgrade().ok_or(Error::Offline)?;
                let candidates = shared.ble_candidates();
                let sign = |message: &[u8]| shared.sign(message);
                let (responder, msg2) =
                    Responder::read_msg1(shared.id, &record, &candidates, &sign).map_err(crypto)?;
                inner.phase = Phase::AwaitMsg3(Box::new(responder));
                self.queue_locked(&mut inner, &msg2)?;
                Ok(Then::Nothing)
            }
            Phase::AwaitMsg2(initiator) => {
                let shared = self.shared.upgrade().ok_or(Error::Offline)?;
                let sign = |message: &[u8]| shared.sign(message);
                let pc = initiator.pc();
                let (cipher, msg3) = initiator.read_msg2(&record, &sign).map_err(crypto)?;
                inner.phase = Phase::Up { peer: pc, cipher: Box::new(cipher) };
                self.queue_locked(&mut inner, &msg3)?;
                Ok(Then::LinkUp(pc))
            }
            Phase::AwaitMsg3(responder) => {
                let phone = responder.phone();
                let cipher = responder.read_msg3(&record).map_err(crypto)?;
                inner.phase = Phase::Up { peer: phone, cipher: Box::new(cipher) };
                Ok(Then::LinkUp(phone))
            }
            Phase::Up { peer, mut cipher } => {
                let plain = cipher.open(&record).map_err(crypto)?;
                inner.phase = Phase::Up { peer, cipher };
                let env = Envelope::from_cbor(&plain).map_err(|e| Error::Protocol(e.to_string()))?;
                // Touchpad input has its own limit, where it's run; keys and
                // clicks have theirs, and the PC's text states theirs.
                let allowed = match budget_for(&env) {
                    Budget::None => true,
                    Budget::Records => inner.incoming.take(now),
                    Budget::Input => inner.input.take(now),
                    Budget::State => inner.state.take(now),
                };
                if !allowed {
                    drop(inner);
                    if env.id.is_some() {
                        let busy = Envelope::error(g7link_protocol::ErrorCode::Busy, "too many messages");
                        self.send(&busy.reply_to(env.id))?;
                    }
                    return Ok(Then::Nothing);
                }
                Ok(Then::Message(peer, env))
            }
            Phase::Closed => Err(Error::Offline),
        }
    }

    /// Ends the link: nothing more goes in or out.
    pub fn close(self: &Arc<Self>) {
        let peer = {
            let mut inner = self.lock();
            let peer = match &inner.phase {
                Phase::Up { peer, .. } => Some(*peer),
                _ => None,
            };
            inner.phase = Phase::Closed;
            inner.outbox.clear();
            inner.pending_state = None;
            peer
        };
        if let (Some(peer), Some(shared)) = (peer, self.shared.upgrade()) {
            shared.ble_link_down(peer, self);
        }
    }

    pub fn is_closed(&self) -> bool {
        matches!(self.lock().phase, Phase::Closed)
    }
}

fn link(error: LinkError) -> Error {
    Error::Protocol(format!("Bluetooth link: {error}"))
}

fn crypto(error: CryptoError) -> Error {
    match error {
        CryptoError::UnknownPeer => Error::NotPaired,
        error => Error::Protocol(format!("Bluetooth link: {error}")),
    }
}

fn internal(error: CryptoError) -> Error {
    Error::Internal(format!("Bluetooth link: {error}"))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use g7link_protocol::messages::{RemoteTextWatch, remote as limits};

    use super::*;

    /// The phone's and the PC's ends of a link that is up, with no node
    /// behind them: what comes in is taken by hand with [`deliver`].
    fn linked() -> (Arc<BleEndpoint>, Arc<BleEndpoint>) {
        let (on_phone, on_pc, phone, pc) = super::super::crypto::linked_ciphers();
        let end = |peer, cipher| {
            Arc::new(BleEndpoint::new(Weak::new(), Phase::Up { peer, cipher: Box::new(cipher) }))
        };
        (end(pc, on_phone), end(phone, on_pc))
    }

    /// Carries everything `from` has to `to` at `now`: the messages it took.
    fn deliver(from: &BleEndpoint, to: &BleEndpoint, now: Instant) -> Result<Vec<Envelope>> {
        let mut got = Vec::new();
        while let Some(fragment) = from.next_outbound() {
            if let Then::Message(_, env) = to.take(&fragment, now)? {
                got.push(env);
            }
        }
        Ok(got)
    }

    /// An envelope whose CBOR is exactly `len` bytes.
    fn sized(len: usize) -> Envelope {
        let bare = Envelope::empty("x.test").to_cbor().len();
        assert!(len >= bare + 3, "too short to carry a body");
        let mut n = len - bare - 3;
        loop {
            let env = Envelope::new("x.test", &"x".repeat(n)).unwrap();
            match env.to_cbor().len() {
                l if l == len => return env,
                l if l < len => n += 1,
                _ => n -= 1,
            }
        }
    }

    #[test]
    fn a_message_too_long_to_seal_is_refused_and_the_link_carries_on() {
        let (phone, pc) = linked();
        phone.set_mtu(ble::MAX_ATT_MTU);
        let now = Instant::now();
        let longest = sized(MAX_MESSAGE);
        assert_eq!(MAX_MESSAGE, 4095);
        phone.send(&longest).unwrap();
        assert_eq!(deliver(&phone, &pc, now).unwrap(), [longest]);

        // One byte more doesn't fit a record: refused before it's sealed.
        assert!(matches!(phone.send(&sized(MAX_MESSAGE + 1)), Err(Error::TooLarge)));
        assert!(matches!(phone.send_state(&sized(MAX_MESSAGE + 1)), Err(Error::TooLarge)));
        let next = sized(40);
        phone.send(&next).unwrap();
        assert_eq!(deliver(&phone, &pc, now).unwrap(), [next], "the next record still opens");
    }

    #[test]
    fn a_full_outbox_refuses_a_message_and_the_link_carries_on() {
        let (phone, pc) = linked();
        let now = Instant::now();
        let big = sized(MAX_MESSAGE);
        let mut sent = 0;
        while phone.send(&big).is_ok() {
            sent += 1;
            assert!(sent < 100, "the outbox fills");
        }
        assert!(matches!(phone.send(&big), Err(Error::Offline)));
        let got = deliver(&phone, &pc, now).unwrap();
        assert_eq!(got.len(), sent);
        let next = sized(20);
        phone.send(&next).unwrap();
        assert_eq!(deliver(&phone, &pc, now).unwrap(), [next]);
    }

    fn state(rev: u32, before: usize) -> Envelope {
        let state = g7link_protocol::messages::RemoteTextState {
            focus: 1,
            field: 0,
            epoch: 0,
            rev,
            applied: 0,
            kind: g7link_protocol::messages::TextFieldKind::Mirror,
            app: None,
            label: None,
            window: false,
            multiline: false,
            start: 0,
            before: "x".repeat(before),
            after: String::new(),
            anchor: None,
            more_before: false,
            more_after: false,
            icon: None,
        };
        Envelope::new(types::REMOTE_TEXT_STATE, &state).unwrap()
    }

    #[test]
    fn a_newer_text_state_replaces_one_waiting_and_replies_go_first() {
        let (phone, pc) = linked();
        let now = Instant::now();
        for rev in 1..=30 {
            pc.send_state(&state(rev, limits::MAX_BLE_STATE_BEFORE)).unwrap();
        }
        let ok = Envelope::empty(types::OK).reply_to(Some(9));
        pc.send(&ok).unwrap();
        let got = deliver(&pc, &phone, now).unwrap();
        assert_eq!(
            got,
            [ok, state(30, limits::MAX_BLE_STATE_BEFORE)],
            "the reply, then only the newest state"
        );
        assert!(deliver(&pc, &phone, now).unwrap().is_empty());
    }

    #[test]
    fn text_states_have_a_budget_of_their_own() {
        let (phone, pc) = linked();
        let start = Instant::now();
        for rev in 0..25 {
            pc.send(&state(rev, 0)).unwrap();
        }
        assert_eq!(deliver(&pc, &phone, start).unwrap().len(), STATE_BURST as usize);
        // A second later, twelve more.
        for rev in 0..25 {
            pc.send(&state(rev, 0)).unwrap();
        }
        let later = start + Duration::from_secs(1);
        assert_eq!(deliver(&pc, &phone, later).unwrap().len(), STATES_PER_SEC as usize);
        // The other budgets aren't touched.
        pc.send(&sized(20)).unwrap();
        assert_eq!(deliver(&pc, &phone, later).unwrap().len(), 1);
    }

    #[test]
    fn the_typing_box_and_the_hold_draw_from_the_input_budget() {
        let (phone, pc) = linked();
        let now = Instant::now();
        let watch = Envelope::new(types::REMOTE_TEXT_WATCH, &RemoteTextWatch { on: true }).unwrap();
        for _ in 0..25 {
            phone.send(&watch).unwrap();
        }
        assert_eq!(deliver(&phone, &pc, now).unwrap().len(), 25, "more than the 20 other records may burst");
        for _ in 0..25 {
            phone.send(&sized(20)).unwrap();
        }
        assert_eq!(deliver(&phone, &pc, now).unwrap().len(), RECORD_BURST as usize);
        assert_eq!(budget_for(&Envelope::empty(types::BLE_USE).with_id(1)), Budget::Input);
        assert_eq!(budget_for(&Envelope::empty(types::PING).with_id(1)), Budget::Input);
        assert_eq!(budget_for(&Envelope::empty(types::REMOTE_TEXT_FOCUS)), Budget::State);
        assert_eq!(budget_for(&Envelope::empty(types::CLIP_SET).with_id(1)), Budget::Records);
        assert_eq!(budget_for(&Envelope::empty(types::REMOTE_INPUT)), Budget::None);
        assert_eq!(budget_for(&Envelope::empty(types::OK).reply_to(Some(1))), Budget::None);
    }
}
