// SPDX-License-Identifier: MPL-2.0
//! Phone as touchpad, keyboard and presentation remote (`docs/protocol/remote.md`).

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
pub use g7link_protocol::messages::{
    ButtonAction, KeyMod, MouseButton, RemoteInput, SlideAction, remote_keys,
};
use g7link_protocol::{
    DeviceId, Envelope, ErrorCode,
    messages::{
        RemoteAppIcon, RemoteAppIconData, RemoteTextFocus, RemoteTextState, RemoteTextWatch, StreamHeader,
        input as remote, is_icon_key, types,
    },
    read_frame, write_frame,
};
use iroh::endpoint::{RecvStream, SendStream};

use crate::{
    Error, NodeEvent, Result, ble_link::RemotePath, dispatch::Responder, node::Shared, session::Session,
    wire_stats::Counters,
};

/// Capability ID announced by a desktop that can inject mouse, keyboard and
/// presentation input (`docs/protocol/remote.md` §1).
pub const INPUT_INJECT: &str = "input.inject";

/// Capability ID announced by a desktop that takes `Edit` and `Caret` input
/// and tells watching phones about its focused text field
/// (`remote.text_watch`, `remote.text_state`).
pub const INPUT_TEXT_MIRROR: &str = "input.text_mirror";

/// Capability ID announced by a desktop that names the focused app's icon in
/// its text states and serves it (`remote.app_icon`).
pub const INPUT_APP_ICON: &str = "input.app_icon";

/// Most app icons a PC keeps to serve.
const KEPT_ICONS: usize = 32;

/// Per-device toggle name on the PC (off by default).
pub(crate) const TOGGLE: &str = "remote_input";

const TIMEOUT: Duration = Duration::from_secs(5);

/// What a PC answers when a phone sends input faster than [`CONTROL_RATE_PER_SEC`].
const RATE_LIMITED: &str = "rate limit exceeded";
/// How long a phone keeps resending an input the PC turned away as too fast
/// (older PCs allowed only 30 at once, then 60 a second), and how long it
/// waits between tries. Inputs go one at a time, so their order is kept.
const RATE_LIMIT_RETRY_FOR: Duration = Duration::from_secs(3);
const RATE_LIMIT_RETRY_AFTER: Duration = Duration::from_millis(25);

/// Maximum rate of pointer motion (`move`, `scroll`, `laser`) run per peer,
/// after motion waiting together is added up.
const DATAGRAM_RATE_PER_SEC: f64 = 480.0;
const DATAGRAM_BURST: f64 = 120.0;
/// Motion waiting for the motion worker, at most; past that, new motion is
/// added into what waits, never dropped.
const MOTION_QUEUE: usize = 64;
/// How often an idle motion worker looks whether its session is gone.
const ABANDONED_CHECK: Duration = Duration::from_millis(500);

/// Maximum rate of incoming control-stream `remote.input` requests per peer.
/// Typing sends one request per key: when voice typing rewrites a sentence,
/// the phone sends a backspace for each character it replaces, one after the
/// other as each is answered (several hundred a second over USB). The limit
/// only stops a runaway flood.
const CONTROL_RATE_PER_SEC: f64 = 500.0;
const CONTROL_BURST: f64 = 500.0;

/// Simple token bucket for per-peer input rate limiting.
#[derive(Debug, Clone)]
pub(crate) struct TokenBucket {
    tokens: f64,
    capacity: f64,
    rate_per_sec: f64,
    last: Instant,
}

impl TokenBucket {
    pub fn new(capacity: f64, rate_per_sec: f64, now: Instant) -> Self {
        Self { tokens: capacity, capacity, rate_per_sec, last: now }
    }

    pub fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * self.rate_per_sec).min(self.capacity);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[derive(Debug)]
struct PeerLimiter {
    datagram: TokenBucket,
    control: TokenBucket,
}

impl PeerLimiter {
    fn new(now: Instant) -> Self {
        Self {
            datagram: TokenBucket::new(DATAGRAM_BURST, DATAGRAM_RATE_PER_SEC, now),
            control: TokenBucket::new(CONTROL_BURST, CONTROL_RATE_PER_SEC, now),
        }
    }
}

/// Per-node state for remote input: rate limiters and one-time prompt tracking.
#[derive(Debug, Default)]
pub(crate) struct RemoteState {
    peers: HashMap<DeviceId, PeerLimiter>,
    /// Peers that already triggered the one-time prompt while `remote_input`
    /// was off.
    prompted: HashSet<DeviceId>,
    /// PC: app icons named in text states, by key, oldest first.
    icons: Vec<(String, Arc<Vec<u8>>)>,
    /// PC: the path each watching phone's last `text_watch` came on, which
    /// its states go back on.
    watch_via: HashMap<DeviceId, RemotePath>,
    /// Phone: the focus and `rev` of the last state each PC's typing box
    /// was given. One from the path used before can arrive after a newer one.
    last_state: HashMap<DeviceId, (u32, u32)>,
}

impl RemoteState {
    pub fn allow_datagram(&mut self, peer: DeviceId, now: Instant) -> bool {
        self.peers.entry(peer).or_insert_with(|| PeerLimiter::new(now)).datagram.take(now)
    }

    pub fn allow_control(&mut self, peer: DeviceId, now: Instant) -> bool {
        self.peers.entry(peer).or_insert_with(|| PeerLimiter::new(now)).control.take(now)
    }

    /// Records that `peer` asked for remote input while the toggle was off.
    /// Returns `true` the first time for that peer.
    pub fn mark_prompted(&mut self, peer: DeviceId) -> bool {
        self.prompted.insert(peer)
    }

    /// Keeps `png` to serve as `key`, dropping the oldest past [`KEPT_ICONS`].
    pub fn keep_icon(&mut self, key: String, png: Vec<u8>) {
        self.icons.retain(|(k, _)| *k != key);
        self.icons.push((key, Arc::new(png)));
        if self.icons.len() > KEPT_ICONS {
            self.icons.remove(0);
        }
    }

    pub fn icon(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        self.icons.iter().find(|(k, _)| k == key).map(|(_, png)| png.clone())
    }

    pub fn remove_peer(&mut self, peer: &DeviceId) {
        self.peers.remove(peer);
        self.prompted.remove(peer);
        self.watch_via.remove(peer);
        self.last_state.remove(peer);
    }

    /// Whether a state from `peer` for `focus` numbered `rev` is newer than
    /// the last one given for that focus; it's recorded when it is.
    fn fresh_state(&mut self, peer: DeviceId, focus: u32, rev: u32) -> bool {
        if let Some(&(last_focus, last_rev)) = self.last_state.get(&peer)
            && last_focus == focus
            // `rev` counts on and wraps: newer is less than half the range ahead.
            && !(1..=i32::MAX as u32).contains(&rev.wrapping_sub(last_rev))
        {
            return false;
        }
        self.last_state.insert(peer, (focus, rev));
        true
    }
}

/// Phone: what the typing box was given from `peer` no longer counts (it
/// closed, or the PC is gone and may come back with its states starting over).
pub(crate) fn forget_text_state(shared: &Shared, peer: &DeviceId) {
    shared.remote.lock().unwrap_or_else(|e| e.into_inner()).last_state.remove(peer);
}

/// PC: the path `peer`'s last `text_watch` came on, while it watches.
pub(crate) fn watch_via(shared: &Shared, peer: &DeviceId) -> Option<RemotePath> {
    shared.remote.lock().unwrap_or_else(|e| e.into_inner()).watch_via.get(peer).copied()
}

fn peer_offers(shared: &Shared, peer: &DeviceId, cap: &str) -> Result<bool> {
    Ok(shared.peer_offers_cap(peer, cap))
}

fn check_allowed_with(
    shared: &Shared,
    peer: &DeviceId,
    allow_deck: bool,
) -> std::result::Result<(), (ErrorCode, &'static str)> {
    let supported =
        shared.we_offer(INPUT_INJECT) || (allow_deck && shared.we_offer(crate::deck::DECK_ACTIONS));
    if !supported {
        return Err((ErrorCode::Unsupported, "remote input is not supported here"));
    }
    if !shared.toggle_on(peer, TOGGLE) {
        let first_time = shared.remote.lock().unwrap_or_else(|e| e.into_inner()).mark_prompted(*peer);
        if first_time {
            shared.emit(NodeEvent::RemoteInputRequested { device: *peer });
        }
        return Err((ErrorCode::Denied, "remote input is turned off for this device"));
    }
    Ok(())
}

fn check_allowed(shared: &Shared, peer: &DeviceId) -> std::result::Result<(), (ErrorCode, &'static str)> {
    check_allowed_with(shared, peer, false)
}

/// PC: `peer`'s `remote.check`, from a session or the Bluetooth link: the reply.
pub(crate) fn run_check(shared: &Shared, peer: &DeviceId) -> Envelope {
    match check_allowed_with(shared, peer, true) {
        Ok(()) => Envelope::empty(types::OK),
        Err((code, msg)) => Envelope::error(code, msg),
    }
}

/// Phone: whether the PC `peer` takes input from phones.
pub(crate) fn takes_input(shared: &Shared, peer: &DeviceId) -> bool {
    peer_offers(shared, peer, INPUT_INJECT).unwrap_or(false)
}

/// Checks whether `peer` (a PC) currently accepts remote input from this phone.
pub(crate) async fn check(shared: &Arc<Shared>, session: &Arc<Session>) -> Result<()> {
    if !peer_offers(shared, &session.peer, INPUT_INJECT)?
        && !peer_offers(shared, &session.peer, crate::deck::DECK_ACTIONS)?
    {
        return Err(Error::Unsupported);
    }
    session.request(Envelope::empty(types::INPUT_CHECK), TIMEOUT).await?.expect(types::OK)?;
    Ok(())
}

/// Tells a phone whether what has the focus on this PC takes text
/// (`remote.text_focus`). Only phones that may control this PC hear it.
pub(crate) async fn text_focus(shared: &Shared, session: &Session, editable: bool) -> Result<()> {
    if !shared.toggle_on(&session.peer, TOGGLE) {
        return Ok(());
    }
    session.send(Envelope::new(types::INPUT_TEXT_FOCUS, &RemoteTextFocus { editable })?).await
}

/// Sends a phone the field that has the focus on this PC
/// (`remote.text_state`). Only phones that may control this PC hear it.
pub(crate) async fn text_state(shared: &Shared, session: &Session, state: &RemoteTextState) -> Result<()> {
    if !shared.toggle_on(&session.peer, TOGGLE) {
        return Ok(());
    }
    if !state.is_valid() {
        return Err(Error::Protocol("invalid text state".into()));
    }
    session.send(Envelope::new(types::INPUT_TEXT_STATE, state)?).await
}

/// Phone: tells the PC `peer` that its typing box opened (`on`) or closed
/// (`remote.text_watch`), when that PC mirrors its text fields.
pub(crate) async fn text_watch(shared: &Shared, session: &Session, on: bool) -> Result<()> {
    if !peer_offers(shared, &session.peer, INPUT_TEXT_MIRROR)? {
        return Err(Error::Unsupported);
    }
    session.send(Envelope::new(types::INPUT_TEXT_WATCH, &RemoteTextWatch { on })?).await
}

/// Phone: whether the PC `peer` mirrors its text fields and takes `Edit`.
pub(crate) fn mirrors_text(shared: &Shared, peer: &DeviceId) -> bool {
    peer_offers(shared, peer, INPUT_TEXT_MIRROR).unwrap_or(false)
}

/// PC: keeps an app icon to serve phones under `key` (named in text states).
pub(crate) fn keep_app_icon(shared: &Shared, key: String, png: Vec<u8>) -> Result<()> {
    let icon = RemoteAppIconData { png };
    if !is_icon_key(&key) || !icon.is_valid() {
        return Err(Error::Protocol("invalid app icon".into()));
    }
    shared.remote.lock().unwrap_or_else(|e| e.into_inner()).keep_icon(key, icon.png);
    Ok(())
}

/// Phone: fetches the app icon `key` a text state from the PC named, as a PNG.
pub(crate) async fn app_icon(shared: &Shared, session: &Session, key: String) -> Result<Vec<u8>> {
    if !peer_offers(shared, &session.peer, INPUT_APP_ICON)? {
        return Err(Error::Unsupported);
    }
    if !is_icon_key(&key) {
        return Err(Error::Protocol("invalid app icon key".into()));
    }
    let reply =
        session.request(Envelope::new(types::INPUT_APP_ICON, &RemoteAppIcon { key })?, TIMEOUT).await?;
    let icon: RemoteAppIconData = reply.expect_body(types::OK)?;
    if !icon.is_valid() {
        return Err(Error::Protocol("invalid app icon".into()));
    }
    Ok(icon.png)
}

/// PC: answers `remote.app_icon` for a phone that may control this PC.
fn serve_app_icon(shared: &Shared, peer: &DeviceId, env: &Envelope) -> Envelope {
    if !shared.we_offer(INPUT_APP_ICON) {
        return Envelope::error(ErrorCode::Unsupported, "no app icons");
    }
    if let Err((code, msg)) = check_allowed(shared, peer) {
        return Envelope::error(code, msg);
    }
    let Ok(RemoteAppIcon { key }) = env.body::<RemoteAppIcon>() else {
        return Envelope::error(ErrorCode::BadMessage, "invalid app icon request");
    };
    let png = shared.remote.lock().unwrap_or_else(|e| e.into_inner()).icon(&key);
    match png {
        Some(png) => Envelope::new(types::OK, &RemoteAppIconData { png: png.to_vec() })
            .unwrap_or_else(|_| Envelope::error(ErrorCode::Internal, "app icon")),
        None => Envelope::error(ErrorCode::NotFound, "no such app icon"),
    }
}

/// Sends a remote input event to `peer` (a PC).
///
/// Discrete actions travel on the control stream. If `input` is
/// [`RemoteInput::Move`], permission is checked first and the motion itself is
/// sent as a QUIC datagram, never on the control stream.
pub(crate) async fn input(shared: &Arc<Shared>, session: &Arc<Session>, event: RemoteInput) -> Result<()> {
    if !event.is_valid() {
        return Err(Error::Protocol("invalid remote input".into()));
    }
    if !peer_offers(shared, &session.peer, INPUT_INJECT)? {
        return Err(Error::Unsupported);
    }
    if let RemoteInput::Move { dx, dy } = event {
        check(shared, session).await?;
        return move_pointer(shared, session, dx, dy).await;
    }
    let give_up = Instant::now() + RATE_LIMIT_RETRY_FOR;
    loop {
        let env = Envelope::new(types::INPUT_EVENT, &event)?;
        let reply = session.request(env, TIMEOUT).await?;
        match reply.expect(types::OK) {
            Err(g7link_protocol::ProtocolError::Remote { code: ErrorCode::Denied, msg })
                if msg == RATE_LIMITED && Instant::now() < give_up =>
            {
                tokio::time::sleep(RATE_LIMIT_RETRY_AFTER).await;
            }
            done => return Ok(done?),
        }
    }
}

/// Sends relative pointer motion `(dx, dy)` in a QUIC datagram (or a dedicated
/// `remote/motion` stream if datagrams are unavailable), never on the control
/// stream.
pub(crate) async fn move_pointer(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    dx: f32,
    dy: f32,
) -> Result<()> {
    let event = RemoteInput::Move { dx, dy };
    if !event.is_valid() {
        return Err(Error::Protocol("pointer motion out of bounds".into()));
    }
    send_datagram(shared, session, &event).await
}

/// Sends smooth scroll `(dx, dy)` in a QUIC datagram.
pub(crate) async fn scroll_fast(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    dx: f32,
    dy: f32,
) -> Result<()> {
    let event = RemoteInput::Scroll { dx, dy };
    if !event.is_valid() {
        return Err(Error::Protocol("scroll delta out of bounds".into()));
    }
    send_datagram(shared, session, &event).await
}

/// Sends a laser pointer update. While `on` is `true`, positions travel in
/// QUIC datagrams; when `on` is `false`, a datagram and a control-stream
/// `remote.input` are both sent so the dot reliably hides.
pub(crate) async fn laser(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    on: bool,
    x: f32,
    y: f32,
) -> Result<()> {
    let event = RemoteInput::Laser { on, x, y };
    if !event.is_valid() {
        return Err(Error::Protocol("laser coordinates out of bounds".into()));
    }
    if on {
        send_datagram(shared, session, &event).await
    } else {
        let _ = send_datagram(shared, session, &event).await;
        input(shared, session, event).await
    }
}

async fn send_datagram(shared: &Arc<Shared>, session: &Arc<Session>, event: &RemoteInput) -> Result<()> {
    if !peer_offers(shared, &session.peer, INPUT_INJECT)? {
        return Err(Error::Unsupported);
    }
    let bytes = Bytes::from(event.to_datagram());
    if session.send_datagram(bytes).is_ok() {
        return Ok(());
    }
    // Fallback if QUIC datagrams are disabled on this transport: open a
    // dedicated stream (still never the control stream).
    let (mut send, _recv) = session.open_bi().await?;
    let header =
        StreamHeader { svc: remote::SERVICE.into(), op: remote::OP_MOTION.into(), v: remote::VERSION };
    let header = Envelope::new(types::STREAM, &header)?;
    write_frame(&mut send, &header.to_cbor()).await?;
    let frame = Envelope::new(types::INPUT_EVENT, event)?;
    write_frame(&mut send, &frame.to_cbor()).await?;
    let _ = send.finish();
    Ok(())
}

/// The typing box's one-way messages from `peer`, from a session or the
/// Bluetooth link (`via`): whether `env` was one.
pub(crate) fn incoming_one_way(
    shared: &Shared,
    peer: DeviceId,
    env: &Envelope,
    via: RemotePath,
) -> Result<bool> {
    match env.t.as_str() {
        // A PC's text focus: heard only where input isn't injected.
        types::INPUT_TEXT_FOCUS => {
            if !shared.we_offer(INPUT_INJECT) {
                let RemoteTextFocus { editable } = env.body()?;
                shared.emit(NodeEvent::PcTextFocus { device: peer, editable });
            }
        }
        // The PC's focused field: heard only where input isn't injected, and
        // dropped when it isn't newer than one already given.
        types::INPUT_TEXT_STATE => {
            if !shared.we_offer(INPUT_INJECT) {
                let state: RemoteTextState = env.body()?;
                let fresh = state.is_valid()
                    && shared.remote.lock().unwrap_or_else(|e| e.into_inner()).fresh_state(
                        peer,
                        state.focus,
                        state.rev,
                    );
                if fresh {
                    shared.emit(NodeEvent::PcTextState { device: peer, state });
                }
            }
        }
        // A phone's typing box opened or closed: only phones that may
        // control this PC are heard. Its states go on the path this came on.
        types::INPUT_TEXT_WATCH => {
            if shared.we_offer(INPUT_TEXT_MIRROR) {
                let RemoteTextWatch { on } = env.body()?;
                if !on {
                    shared.remote.lock().unwrap_or_else(|e| e.into_inner()).watch_via.remove(&peer);
                    shared.emit(NodeEvent::TextWatch { device: peer, on });
                } else if check_allowed(shared, &peer).is_ok() {
                    shared.remote.lock().unwrap_or_else(|e| e.into_inner()).watch_via.insert(peer, via);
                    shared.emit(NodeEvent::TextWatch { device: peer, on });
                }
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Handles incoming `remote.*` requests on the control stream.
pub(crate) async fn handle(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<bool> {
    if incoming_one_way(shared, session.peer, env, RemotePath::Session)? {
        return Ok(true);
    }
    let reply = match env.t.as_str() {
        types::INPUT_CHECK => run_check(shared, &session.peer),
        types::INPUT_APP_ICON => serve_app_icon(shared, &session.peer, env),
        types::INPUT_EVENT => match env.body::<RemoteInput>() {
            Ok(event) => run_control(shared, session.peer, event).await,
            Err(_) => Envelope::error(ErrorCode::BadMessage, "invalid remote input"),
        },
        _ => return Ok(false),
    };
    rsp.send(reply).await?;
    Ok(true)
}

/// Runs a discrete input event (a click, a key, text...) `peer` asked for, on a
/// session or the Bluetooth link: the reply.
pub(crate) async fn run_control(shared: &Arc<Shared>, peer: DeviceId, event: RemoteInput) -> Envelope {
    if let Err((code, msg)) = check_allowed(shared, &peer) {
        return Envelope::error(code, msg);
    }
    if !event.is_lane_allowed() || !event.is_valid() {
        return Envelope::error(ErrorCode::BadMessage, "invalid remote input");
    }
    let allowed = shared.remote.lock().unwrap_or_else(|e| e.into_inner()).allow_control(peer, Instant::now());
    if !allowed {
        return Envelope::error(ErrorCode::Denied, RATE_LIMITED);
    }
    let platform = shared.platform.clone();
    match tokio::task::spawn_blocking(move || platform.remote_input(&peer, event)).await {
        Ok(Ok(())) => Envelope::empty(types::OK),
        Ok(Err(e)) => Envelope::error(ErrorCode::Internal, e),
        Err(_) => Envelope::error(ErrorCode::Internal, "input task failed"),
    }
}

/// Runs pointer motion, a scroll or the laser from `peer`, as a datagram is:
/// dropped when not allowed or over the rate.
pub(crate) fn run_motion(shared: &Arc<Shared>, peer: DeviceId, event: RemoteInput) {
    if check_allowed(shared, &peer).is_err() || !event.is_datagram_allowed() || !event.is_valid() {
        return;
    }
    let allowed =
        shared.remote.lock().unwrap_or_else(|e| e.into_inner()).allow_datagram(peer, Instant::now());
    if allowed {
        let _ = shared.platform.remote_input(&peer, event);
    }
}

/// Pointer motion from one peer waiting to be run (`plans/protocol-v1-contract.md`
/// §2.6): the session's datagrams and its `input/motion` stream put it in, and
/// the session's motion worker takes it out on a blocking thread, so a slow
/// platform never holds up the session.
#[derive(Debug, Default)]
pub(crate) struct MotionQueue {
    state: std::sync::Mutex<MotionState>,
    ready: std::sync::Condvar,
}

#[derive(Debug, Default)]
struct MotionState {
    items: VecDeque<RemoteInput>,
    closed: bool,
}

impl MotionQueue {
    fn state(&self) -> std::sync::MutexGuard<'_, MotionState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queues `event`; with the queue full, adds it into the last motion of
    /// its kind instead.
    pub(crate) fn push(&self, event: RemoteInput, counters: &Counters) {
        Counters::add(&counters.motion_received, 1);
        let mut state = self.state();
        if state.closed {
            return;
        }
        if state.items.len() >= MOTION_QUEUE && merge_into_last(&mut state.items, &event) {
            Counters::add(&counters.motion_summed, 1);
        } else {
            state.items.push_back(event);
        }
        drop(state);
        self.ready.notify_one();
    }

    /// Ends the worker once it has run what waits.
    pub(crate) fn close(&self) {
        self.state().closed = true;
        self.ready.notify_all();
    }

    /// Everything waiting, once there's something; `None` once closed, or
    /// once nothing else holds the queue (the session was dropped without
    /// ending, as when its runtime goes): a blocking thread left waiting
    /// would keep the runtime from shutting down.
    fn take(self: &Arc<Self>) -> Option<Vec<RemoteInput>> {
        let mut state = self.state();
        loop {
            if !state.items.is_empty() {
                return Some(state.items.drain(..).collect());
            }
            if state.closed || Arc::strong_count(self) == 1 {
                return None;
            }
            state = self.ready.wait_timeout(state, ABANDONED_CHECK).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

/// Adds `event` into the last waiting motion of its kind; false if none waits.
fn merge_into_last(items: &mut VecDeque<RemoteInput>, event: &RemoteInput) -> bool {
    let same_kind = |e: &RemoteInput| std::mem::discriminant(e) == std::mem::discriminant(event);
    let Some(last) = items.iter_mut().rev().find(|e| same_kind(e)) else { return false };
    match (last, event) {
        (RemoteInput::Move { dx, dy }, RemoteInput::Move { dx: x, dy: y })
        | (RemoteInput::Scroll { dx, dy }, RemoteInput::Scroll { dx: x, dy: y }) => {
            *dx += x;
            *dy += y;
            true
        }
        (last @ RemoteInput::Laser { .. }, RemoteInput::Laser { .. }) => {
            *last = event.clone();
            true
        }
        _ => false,
    }
}

/// `batch` with consecutive moves, and consecutive scrolls, added up, and
/// sums too large for one event split back into events that fit.
fn coalesce(batch: Vec<RemoteInput>, counters: &Counters) -> Vec<RemoteInput> {
    let mut out: Vec<RemoteInput> = Vec::with_capacity(batch.len());
    for event in batch {
        match (out.last_mut(), &event) {
            (Some(RemoteInput::Move { dx, dy }), RemoteInput::Move { dx: x, dy: y })
            | (Some(RemoteInput::Scroll { dx, dy }), RemoteInput::Scroll { dx: x, dy: y }) => {
                *dx += x;
                *dy += y;
                Counters::add(&counters.motion_summed, 1);
            }
            _ => out.push(event),
        }
    }
    out.into_iter().flat_map(split_motion).collect()
}

/// `event`, as as many events as it takes to stay within the motion limits.
fn split_motion(event: RemoteInput) -> Vec<RemoteInput> {
    let (dx, dy, max, scroll) = match event {
        RemoteInput::Move { dx, dy } => (dx, dy, remote::MAX_MOVE_DELTA, false),
        RemoteInput::Scroll { dx, dy } => (dx, dy, remote::MAX_SCROLL_DELTA, true),
        other => return vec![other],
    };
    let largest = dx.abs().max(dy.abs());
    if !largest.is_finite() {
        return Vec::new();
    }
    let pieces = (largest / max).ceil().max(1.0) as usize;
    let (dx, dy) = (dx / pieces as f32, dy / pieces as f32);
    let piece = if scroll { RemoteInput::Scroll { dx, dy } } else { RemoteInput::Move { dx, dy } };
    vec![piece; pieces]
}

/// The session's motion worker: runs `peer`'s queued motion on this blocking
/// thread until the queue closes.
pub(crate) fn run_motion_worker(
    shared: std::sync::Weak<Shared>,
    peer: DeviceId,
    queue: Arc<MotionQueue>,
    counters: Arc<Counters>,
) {
    let mut inject_max = Duration::ZERO;
    let mut reported = Instant::now();
    while let Some(batch) = queue.take() {
        let Some(shared) = shared.upgrade() else { break };
        if check_allowed(&shared, &peer).is_err() {
            Counters::add(&counters.motion_refused, batch.len() as u64);
            continue;
        }
        for event in coalesce(batch, &counters) {
            if !event.is_datagram_allowed() || !event.is_valid() {
                Counters::add(&counters.motion_refused, 1);
                continue;
            }
            let allowed =
                shared.remote.lock().unwrap_or_else(|e| e.into_inner()).allow_datagram(peer, Instant::now());
            if !allowed {
                Counters::add(&counters.motion_refused, 1);
                continue;
            }
            let started = Instant::now();
            let _ = shared.platform.remote_input(&peer, event);
            inject_max = inject_max.max(started.elapsed());
        }
        if reported.elapsed() >= Duration::from_secs(1) {
            let counted = counters.snapshot();
            crate::wire_stats::motion(
                counted.motion_received,
                counted.motion_summed,
                counted.motion_refused,
                inject_max,
            );
            (reported, inject_max) = (Instant::now(), Duration::ZERO);
        }
    }
}

/// Handles a fallback `remote/motion` stream when QUIC datagrams aren't used.
pub(crate) async fn receive_motion(
    shared: Arc<Shared>,
    peer: DeviceId,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    while let Ok(Some(bytes)) = read_frame(&mut recv).await {
        if check_allowed(&shared, &peer).is_err() {
            break;
        }
        let Ok(env) = Envelope::from_cbor(&bytes) else { break };
        let Ok(event) = env.expect_body::<RemoteInput>(types::INPUT_EVENT) else { break };
        if !event.is_datagram_allowed() || !event.is_valid() {
            break;
        }
        // Run by the session's motion worker, as datagrams are.
        let Some(session) = shared.session(&peer) else { break };
        session.motion.push(event, &session.counters);
    }
    let _ = send.finish();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_bucket_enforces_burst_and_refills() {
        let start = Instant::now();
        let mut b = TokenBucket::new(3.0, 10.0, start);
        assert!(b.take(start));
        assert!(b.take(start));
        assert!(b.take(start));
        assert!(!b.take(start), "burst exhausted");

        // 100 ms later at 10/s refills 1 token.
        let later = start + Duration::from_millis(100);
        assert!(b.take(later));
        assert!(!b.take(later));
    }

    #[test]
    fn prompt_is_emitted_once_per_peer() {
        let mut state = RemoteState::default();
        let a = DeviceId([1; 32]);
        let b = DeviceId([2; 32]);
        assert!(state.mark_prompted(a));
        assert!(!state.mark_prompted(a), "second ask from same peer does not re-prompt");
        assert!(state.mark_prompted(b));
        state.remove_peer(&a);
        assert!(state.mark_prompted(a), "unpairing resets prompt state");
    }

    #[test]
    fn a_state_that_isnt_newer_for_its_focus_is_dropped() {
        let mut state = RemoteState::default();
        let pc = DeviceId([1; 32]);
        assert!(state.fresh_state(pc, 7, 5));
        assert!(!state.fresh_state(pc, 7, 4), "older, from the path used before");
        assert!(!state.fresh_state(pc, 7, 5), "the same one twice");
        assert!(state.fresh_state(pc, 7, 6));
        assert!(state.fresh_state(pc, 8, 1), "another focus starts over");
        assert!(state.fresh_state(DeviceId([2; 32]), 8, 0), "each PC its own");
        for rev in [0x7FFF_FFFF, 0xF000_0000, u32::MAX, 0] {
            assert!(state.fresh_state(pc, 8, rev), "rev counts on and wraps: {rev:#x}");
        }
        assert!(!state.fresh_state(pc, 8, u32::MAX), "just before the wrap");
        state.last_state.remove(&pc);
        assert!(state.fresh_state(pc, 8, 0), "forgotten");
    }
}
