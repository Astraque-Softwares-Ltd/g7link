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
    ble::{self, LinkError, MAX_HANDSHAKE_RECORD, MAX_RECORD, Reassembler},
    messages::{RemoteInput, remote as remote_limits, types},
};

use super::crypto::{Cipher, CryptoError, Initiator, Responder};
use crate::{Error, Result, events::NodeEvent, node::Shared, remote::TokenBucket};

/// A message's plain bytes at most.
pub const MAX_MESSAGE: usize = 4096;
/// Fragments waiting to go out at most; past that, sending fails.
const MAX_OUTBOX: usize = 2048;
/// Records from the other device: a burst of this many, then this many a second.
const RECORD_BURST: f64 = 20.0;
const RECORDS_PER_SEC: f64 = 2.0;
/// Keys, clicks and edits from the phone, which typing sends several of a
/// second: a budget of their own.
const INPUT_BURST: f64 = 60.0;
const INPUTS_PER_SEC: f64 = 25.0;

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
    /// Touchpad input not sent yet: added up while the radio is busy, and
    /// sealed only when there's room, so it never queues up behind itself.
    motion: Motion,
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
    fn new(shared: &Arc<Shared>, phase: Phase) -> Self {
        BleEndpoint {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            shared: Arc::downgrade(shared),
            inner: Mutex::new(Inner {
                phase,
                att_mtu: ble::MIN_ATT_MTU,
                rx: Reassembler::default(),
                tx_seq: 0,
                outbox: VecDeque::new(),
                incoming: TokenBucket::new(RECORD_BURST, RECORDS_PER_SEC, Instant::now()),
                input: TokenBucket::new(INPUT_BURST, INPUTS_PER_SEC, Instant::now()),
                motion: Motion::default(),
            }),
        }
    }

    /// The PC's end: waits for a phone to start.
    pub(crate) fn accept(shared: &Arc<Shared>) -> Arc<Self> {
        Arc::new(Self::new(shared, Phase::AwaitMsg1))
    }

    /// The phone's end: starts the handshake with `pc`.
    pub(crate) fn connect(
        shared: &Arc<Shared>,
        pc: DeviceId,
        link_secret: [u8; ble::LINK_SECRET_LEN],
    ) -> Result<Arc<Self>> {
        let (initiator, msg1) = Initiator::start(shared.id, pc, link_secret).map_err(internal)?;
        let endpoint = Arc::new(Self::new(shared, Phase::AwaitMsg2(Box::new(initiator))));
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
    /// there's nothing. Touchpad input waiting is sealed now, added up.
    pub fn next_outbound(&self) -> Option<Vec<u8>> {
        let mut inner = self.lock();
        if inner.outbox.is_empty()
            && let Some(event) = inner.motion.take()
            && let Ok(env) = Envelope::new(types::REMOTE_INPUT, &event)
            && let Phase::Up { cipher, .. } = &mut inner.phase
        {
            let record = cipher.seal(&env.to_cbor());
            let mut seq = inner.tx_seq;
            if let Ok(fragments) = ble::fragments(&record, inner.att_mtu, &mut seq) {
                inner.tx_seq = seq;
                inner.outbox.extend(fragments);
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

    /// Queues `record`'s fragments.
    fn queue(&self, record: &[u8]) -> Result<()> {
        let mut inner = self.lock();
        self.queue_locked(&mut inner, record)
    }

    fn queue_locked(&self, inner: &mut Inner, record: &[u8]) -> Result<()> {
        let mut seq = inner.tx_seq;
        let fragments = ble::fragments(record, inner.att_mtu, &mut seq).map_err(link)?;
        if inner.outbox.len() + fragments.len() > MAX_OUTBOX {
            return Err(Error::Offline);
        }
        inner.tx_seq = seq;
        let was_empty = inner.outbox.is_empty();
        inner.outbox.extend(fragments);
        if was_empty && let Some(shared) = self.shared.upgrade() {
            shared.emit(NodeEvent::BleOutbound { endpoint: self.id });
        }
        Ok(())
    }

    /// Sends `env` on the link, once it's up.
    pub(crate) fn send(&self, env: &Envelope) -> Result<()> {
        let plain = env.to_cbor();
        if plain.len() > MAX_MESSAGE {
            return Err(Error::TooLarge);
        }
        let mut inner = self.lock();
        let Phase::Up { cipher, .. } = &mut inner.phase else {
            return Err(Error::Offline);
        };
        let record = cipher.seal(&plain);
        self.queue_locked(&mut inner, &record)
    }

    /// One value from the other device. An error means the link is done: the
    /// platform disconnects and drops this end.
    pub fn receive(self: &Arc<Self>, fragment: &[u8]) -> Result<()> {
        let then = self.take(fragment);
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

    fn take(&self, fragment: &[u8]) -> Result<Then> {
        let shared = self.shared.upgrade().ok_or(Error::Offline)?;
        let mut inner = self.lock();
        let limit = match inner.phase {
            Phase::Up { .. } => MAX_RECORD,
            Phase::Closed => return Err(Error::Offline),
            _ => MAX_HANDSHAKE_RECORD,
        };
        let Some(record) = inner.rx.push(fragment, limit).map_err(link)? else {
            return Ok(Then::Nothing);
        };
        let sign = |message: &[u8]| shared.sign(message);
        match std::mem::replace(&mut inner.phase, Phase::Closed) {
            Phase::AwaitMsg1 => {
                let candidates = shared.ble_candidates();
                let (responder, msg2) =
                    Responder::read_msg1(shared.id, &record, &candidates, &sign).map_err(crypto)?;
                inner.phase = Phase::AwaitMsg3(Box::new(responder));
                self.queue_locked(&mut inner, &msg2)?;
                Ok(Then::Nothing)
            }
            Phase::AwaitMsg2(initiator) => {
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
                // clicks have theirs.
                let motion = env.t == types::REMOTE_INPUT && env.id.is_none();
                let input = env.t == types::REMOTE_INPUT && env.id.is_some();
                let budget = if input { &mut inner.input } else { &mut inner.incoming };
                if !motion && !budget.take(Instant::now()) {
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
