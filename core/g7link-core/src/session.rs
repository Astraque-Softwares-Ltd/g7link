// SPDX-License-Identifier: MPL-2.0
//! One authenticated session with a paired device: the hello handshake, the
//! control stream, request/response correlation and the heartbeat.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use g7link_protocol::{
    DeviceId, Envelope, ErrorCode, PROTOCOL_VERSION, ProtocolError, SoundDatagram,
    lanes::{self, Lane, lane_of},
    messages::*,
    read_frame, write_frame,
};
use iroh::endpoint::{Connection, RecvStream, SendStream, VarInt};
use tokio::{
    sync::{Notify, Semaphore, mpsc, oneshot},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Error, Result,
    dispatch::Responder,
    error::Side,
    events::{ConnectionPath, NodeEvent},
    lane::{CONTROL_PRIORITY, LARGE_FRAME, LaneSet},
    node::Shared,
    rpc::MAX_INCOMING_RPCS,
    wire_stats::{self, Counters, LaneKind},
};

#[path = "wake.rs"]
pub(crate) mod wake;

pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How often each side pings the other for the round-trip time and to catch
/// a peer that stopped answering. A link that dropped is QUIC's to notice
/// (`IDLE_TIMEOUT` in node.rs); pinging every 2.5 s kept phones awake.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// How often while the user has the app open (`Node::set_interactive`), so a
/// link that died shows within seconds instead of after sends wait on it.
const INTERACTIVE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_millis(3000);
const HEARTBEAT_RETRY_TIMEOUT: Duration = Duration::from_millis(1500);
/// Control messages waiting to be written at most.
const OUTBOX_CAPACITY: usize = 256;
/// Store work from control messages waiting at most (`hello.update`,
/// `device.info`, `pair.revoke`, …), run in order off the control reader.
const CORE_QUEUE: usize = 64;
/// How long the session's tasks get to finish once it ends, before they're stopped.
const WIND_DOWN: Duration = Duration::from_secs(2);

/// QUIC close code used when a duplicate connection is dropped.
pub(crate) const CLOSE_DUPLICATE: u32 = 100;
/// QUIC close code for a normal shutdown.
pub(crate) const CLOSE_NORMAL: u32 = 0;

/// Returns true if `ip` is a local/private LAN or loopback address, or a
/// Tailscale tailnet address (`100.64.0.0/10`, `fd7a:115c:a1e0::/48`).
/// Global internet addresses (public IPv4, `2000::/3`) are rejected.
///
/// Tailnet addresses count as local: Tailscale's WireGuard network is private
/// to the user's devices, and it's how a PC is reached from another network
/// without relays. Carrier CGNAT shares `100.64.0.0/10`, but a direct path
/// still needs the paired device's key, and relays stay off.
pub fn is_lan_ip(ip: std::net::IpAddr) -> bool {
    match ip.to_canonical() {
        std::net::IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_link_local()
                || v4.is_loopback()
                || is_tailnet_ip(std::net::IpAddr::V4(v4))
        }
        std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.is_unicast_link_local() || v6.is_unique_local(),
    }
}

/// Returns true if `ip` reaches a peer on this network: a LAN address
/// (`is_lan_ip`), or one the peer announced over mDNS here (`announced`).
/// mDNS doesn't cross routers, so the second covers home networks numbered
/// outside the private ranges (#131).
pub(crate) fn is_lan_peer_ip(ip: std::net::IpAddr, announced: &[std::net::SocketAddr]) -> bool {
    let ip = ip.to_canonical();
    is_lan_ip(ip) || announced.iter().any(|a| a.ip().to_canonical() == ip)
}

/// Returns true if `ip` is in Tailscale's ranges: IPv4 `100.64.0.0/10` (the
/// shared address space it assigns from) or IPv6 `fd7a:115c:a1e0::/48`.
pub fn is_tailnet_ip(ip: std::net::IpAddr) -> bool {
    match ip.to_canonical() {
        std::net::IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            a == 100 && (b & 0xC0) == 64
        }
        std::net::IpAddr::V6(v6) => {
            let seg = v6.segments();
            seg[0] == 0xfd7a && seg[1] == 0x115c && seg[2] == 0xa1e0
        }
    }
}

/// A session's selected path (or its first, with none selected), as
/// [`path_of`] names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Picked {
    Relay,
    /// A direct path to `ip`; `usb` when it's the USB bridge's port.
    Ip {
        ip: std::net::IpAddr,
        usb: bool,
    },
    /// Anything else a transport may add.
    Other,
}

/// What a session's path is called, from its selected path and whether any
/// of its paths is the USB bridge's: the relay; USB; a direct path to a
/// tailnet address (`Tailscale`); any other direct path (`Lan`). Naming
/// only: which paths are allowed or dialed doesn't depend on it.
fn path_of(selected: Option<Picked>, any_usb: bool) -> ConnectionPath {
    match selected {
        Some(Picked::Relay) => ConnectionPath::Relay,
        Some(Picked::Ip { usb: true, .. }) => ConnectionPath::Usb,
        _ if any_usb => ConnectionPath::Usb,
        Some(Picked::Ip { ip, .. }) if is_tailnet_ip(ip) => ConnectionPath::Tailscale,
        _ => ConnectionPath::Lan,
    }
}

/// Returns true if two IPv6 addresses share the same `/64` subnet prefix.
#[cfg(test)]
pub(crate) fn is_same_ipv6_subnet(a: std::net::Ipv6Addr, b: std::net::Ipv6Addr) -> bool {
    a.segments()[..4] == b.segments()[..4]
}

/// Builds this device's hello from the shared local state.
pub(crate) fn local_hello(shared: &Shared) -> Hello {
    let local = shared.local.read().unwrap_or_else(|e| e.into_inner());
    Hello {
        proto: PROTOCOL_VERSION,
        app: local.app_version.clone(),
        device: local.device.clone(),
        caps: local.capabilities(),
        power: local.power,
        // Public (LocalSend announces it), and only told to paired devices.
        localsend: Some(shared.localsend.fingerprint().to_owned()),
    }
}

/// Runs the dialer side of the handshake on a fresh connection.
pub(crate) async fn handshake_dialer(
    conn: &Connection,
    hello: &Hello,
) -> Result<(SendStream, RecvStream, Hello)> {
    let run = async {
        let (mut send, mut recv) = conn.open_bi().await.map_err(crate::error::net)?;
        write_frame(&mut send, &Envelope::new(types::HELLO, hello)?.to_cbor()).await?;
        let remote = read_hello(&mut recv).await?;
        check_versions(conn, &mut send, hello, &remote).await?;
        Ok((send, recv, remote))
    };
    tokio::time::timeout(HANDSHAKE_TIMEOUT, run).await.map_err(|_| Error::Timeout)?
}

/// Runs the listener side of the handshake on an accepted connection.
pub(crate) async fn handshake_listener(
    conn: &Connection,
    hello: &Hello,
) -> Result<(SendStream, RecvStream, Hello)> {
    let run = async {
        let (mut send, mut recv) = conn.accept_bi().await.map_err(crate::error::net)?;
        let remote = read_hello(&mut recv).await?;
        check_versions(conn, &mut send, hello, &remote).await?;
        write_frame(&mut send, &Envelope::new(types::HELLO, hello)?.to_cbor()).await?;
        Ok((send, recv, remote))
    };
    tokio::time::timeout(HANDSHAKE_TIMEOUT, run).await.map_err(|_| Error::Timeout)?
}

async fn read_hello(recv: &mut RecvStream) -> Result<Hello> {
    let frame = read_frame(recv).await?.ok_or(Error::Offline)?;
    Ok(Envelope::from_cbor(&frame)?.expect_body(types::HELLO)?)
}

async fn check_versions(
    conn: &Connection,
    send: &mut SendStream,
    local: &Hello,
    remote: &Hello,
) -> Result<()> {
    if remote.proto == local.proto {
        return Ok(());
    }
    // Tell the peer why before closing; best effort.
    let msg = format!("protocol {} is not compatible with {}", remote.proto, local.proto);
    let _ = write_frame(send, &Envelope::error(ErrorCode::VersionTooOld, msg).to_cbor()).await;
    // A moment for the reason to reach the peer before the close discards it.
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_millis(500), send.stopped()).await;
    let side = if remote.proto < local.proto { Side::Remote } else { Side::Local };
    conn.close(VarInt::from_u32(ErrorCode::VersionTooOld.close_code()), b"version");
    Err(Error::VersionTooOld { side })
}

/// A live session. Cheap to share; dropping the last handle does not close
/// the connection (call [`Session::close`]).
pub(crate) struct Session {
    pub peer: DeviceId,
    /// Tells this session from any other this process had.
    pub(crate) serial: u64,
    /// Who opened the connection; used to resolve duplicate connections.
    pub dialer: DeviceId,
    pub conn: Connection,
    pub(crate) ring_command: tokio::sync::Mutex<()>,
    /// Control messages to write, in order.
    outbox: mpsc::Sender<Envelope>,
    /// Control requests waiting for their answer, by `id`.
    pending: Mutex<HashMap<u64, oneshot::Sender<Envelope>>>,
    next_id: AtomicU64,
    /// Store work from control messages, run in order.
    core: mpsc::Sender<Envelope>,
    pub(crate) lanes: Arc<LaneSet>,
    /// Turns for RPCs this side opens, first come first served.
    pub(crate) rpc_out: Arc<Semaphore>,
    pub(crate) rpc_limit: usize,
    /// Slots for RPCs, and for media and bulk streams, the peer opens.
    rpc_in: Arc<Semaphore>,
    streams_in: Arc<Semaphore>,
    pub(crate) counters: Arc<Counters>,
    /// Where the session's tasks run, also when a message is queued from
    /// outside the runtime.
    pub(crate) runtime: tokio::runtime::Handle,
    rtt_ms: AtomicU32,
    pub(crate) started: Instant,
    last_rx_ms: AtomicU64,
    usb_bridge_ports: Arc<Mutex<HashMap<u16, DeviceId>>>,
    usb_peers: Arc<Mutex<HashMap<DeviceId, (std::net::SocketAddr, CancellationToken)>>>,
    /// Apps whose notification icon this peer already got.
    sent_icons: crate::notifications::SentIcons,
    /// Serializes media snapshots and remembers the artwork queued for this device.
    pub(crate) sent_art: tokio::sync::Mutex<HashSet<String>>,
    /// This peer asked for no notifications for now. Ends with the session: a
    /// PC that's still locked says so again when it reconnects.
    notifications_held: std::sync::atomic::AtomicBool,
    /// Numbers and times the PC sound sent to this peer.
    pub(crate) sound_clock: Mutex<crate::sound::SoundClock>,
    /// PC sound that datagrams couldn't carry, on its stream.
    pub(crate) sound_out: Arc<crate::sound::SoundStream>,
    /// The PC sound that came from this peer, to drop what's late.
    sound_in: Mutex<crate::sound::SoundReceived>,
    /// Whether this device plays the peer's sound (`media::TOGGLE`).
    sound_toggle: Arc<CachedToggle>,
    /// Pointer motion from the peer, for the session's motion worker.
    pub(crate) motion: Arc<crate::remote::MotionQueue>,
    /// The address this peer was reached at on a LAN path that is also this
    /// device's default gateway: the peer is our router (its hotspot).
    reached_at_gateway: Mutex<Option<std::net::IpAddr>>,
    /// Set once the node has registered the session and said it's online:
    /// what the peer sends is handled from then on.
    ready: AtomicBool,
    ready_notify: Notify,
    /// Wakes the heartbeat to ping now, and when it last was.
    ping_now: Notify,
    heartbeat_woken: Mutex<Option<Instant>>,
    pub(crate) cancel: CancellationToken,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("peer", &self.peer)
            .field("dialer", &self.dialer)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Starts the session's tasks. `on_end` runs once when the session ends,
    /// for any reason.
    pub(crate) fn spawn(
        shared: &Arc<Shared>,
        conn: Connection,
        dialer: DeviceId,
        (send, recv): (SendStream, RecvStream),
        on_end: impl FnOnce(Arc<Session>) + Send + 'static,
    ) -> Arc<Session> {
        let peer = crate::device_id(&conn.remote_id());
        // Start from QUIC's estimate (from the handshake) until the first
        // heartbeat measures the round trip end to end.
        let initial_rtt = conn
            .paths()
            .iter()
            .find(|p| p.is_selected())
            .map_or(0, |p| p.rtt().as_millis().min(u128::from(u32::MAX)) as u32);
        let (outbox, outbox_rx) = mpsc::channel(OUTBOX_CAPACITY);
        let (core, core_rx) = mpsc::channel(CORE_QUEUE);
        let rpc_limit = shared.outgoing_rpc_limit.load(Ordering::Relaxed).max(1);
        static SERIAL: AtomicU64 = AtomicU64::new(1);
        let session = Arc::new(Session {
            peer,
            serial: SERIAL.fetch_add(1, Ordering::Relaxed),
            dialer,
            conn,
            ring_command: Default::default(),
            outbox,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            core,
            lanes: Default::default(),
            rpc_out: Arc::new(Semaphore::new(rpc_limit)),
            rpc_limit,
            rpc_in: Arc::new(Semaphore::new(MAX_INCOMING_RPCS)),
            streams_in: Arc::new(Semaphore::new(MAX_INCOMING_STREAMS)),
            counters: Default::default(),
            runtime: tokio::runtime::Handle::current(),
            rtt_ms: AtomicU32::new(initial_rtt),
            started: Instant::now(),
            last_rx_ms: AtomicU64::new(0),
            usb_bridge_ports: shared.usb_bridge_ports.clone(),
            usb_peers: shared.usb_peers.clone(),
            sent_icons: Default::default(),
            sent_art: Default::default(),
            notifications_held: Default::default(),
            sound_clock: Default::default(),
            sound_out: Default::default(),
            sound_in: Default::default(),
            sound_toggle: Default::default(),
            motion: Default::default(),
            reached_at_gateway: Mutex::new(None),
            ready: AtomicBool::new(false),
            ready_notify: Notify::new(),
            ping_now: Notify::new(),
            heartbeat_woken: Mutex::new(None),
            cancel: shared.cancel.child_token(),
        });

        let weak = Arc::downgrade(shared);
        let count = shared.session_tasks.clone();
        let mut tasks = JoinSet::new();
        let s = &session;
        tasks.spawn(counted(&count, control_writer(send, outbox_rx)));
        tasks.spawn(counted(&count, control_reader(s.clone(), recv, weak.clone())));
        tasks.spawn(counted(&count, core_queue(s.clone(), core_rx, weak.clone())));
        tasks.spawn(counted(&count, heartbeat(s.clone(), weak.clone())));
        tasks.spawn(counted(&count, acceptor(s.clone(), weak.clone())));
        tasks.spawn(counted(&count, datagrams(s.clone(), weak.clone())));
        let closed = s.conn.clone();
        tasks.spawn(counted(&count, async move {
            closed.closed().await;
        }));
        let (queue, counters, live) = (s.motion.clone(), s.counters.clone(), Live::new(&count));
        tasks.spawn_blocking(move || {
            let _live = live;
            crate::remote::run_motion_worker(weak, peer, queue, counters);
        });
        tokio::spawn(supervise(session.clone(), tasks, on_end));
        session
    }

    /// Up for longer than two dials racing take, and heard from within a
    /// heartbeat period: a session a new connection mustn't replace.
    pub(crate) fn is_settled_and_heard(&self) -> bool {
        const SETTLED: Duration = Duration::from_secs(3);
        let now = Instant::now();
        self.is_alive()
            && self.started.elapsed() >= SETTLED
            && self.rx_since(now.checked_sub(INTERACTIVE_HEARTBEAT_INTERVAL).unwrap_or(self.started))
    }

    pub(crate) fn note_reached_at_gateway(&self, ip: std::net::IpAddr) {
        *self.reached_at_gateway.lock().unwrap_or_else(|e| e.into_inner()) = Some(ip);
    }

    pub(crate) fn reached_at_gateway(&self) -> Option<std::net::IpAddr> {
        *self.reached_at_gateway.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// QUIC's own round-trip estimate on the selected path.
    pub(crate) fn path_rtt(&self) -> Duration {
        self.conn.paths().iter().find(|p| p.is_selected()).map_or(Duration::ZERO, |p| p.rtt())
    }

    pub fn rtt_ms(&self) -> u32 {
        self.rtt_ms.load(Ordering::Relaxed)
    }

    pub(crate) fn touch_rx(&self) {
        let elapsed = self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        self.last_rx_ms.store(elapsed, Ordering::Relaxed);
    }

    pub(crate) fn rx_since(&self, since: Instant) -> bool {
        let since_ms = since
            .checked_duration_since(self.started)
            .map_or(0, |d| d.as_millis().min(u128::from(u64::MAX)) as u64);
        self.last_rx_ms.load(Ordering::Relaxed) > since_ms
    }

    fn is_usb_addr(&self, addr: &std::net::SocketAddr) -> bool {
        addr.ip().to_canonical().is_loopback()
            && self.usb_bridge_ports.lock().unwrap_or_else(|e| e.into_inner()).get(&addr.port())
                == Some(&self.peer)
    }

    fn has_usb_tunnel(&self) -> bool {
        self.usb_peers.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&self.peer)
    }

    pub(crate) fn uses_usb_bridge_port(&self, port: u16) -> bool {
        self.usb_peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&self.peer)
            .is_some_and(|(addr, _)| addr.port() == port)
            || self.conn.paths().iter().any(|p| {
                matches!(
                    p.remote_addr(),
                    iroh::TransportAddr::Ip(addr) if addr.port() == port && self.is_usb_addr(addr)
                )
            })
    }

    pub(crate) fn relay_url(&self) -> Option<iroh::RelayUrl> {
        if self.has_usb_tunnel() {
            return None;
        }
        let paths = self.conn.paths();
        let path = paths.iter().find(|p| p.is_selected()).or_else(|| paths.iter().next())?;
        match path.remote_addr() {
            iroh::TransportAddr::Relay(url) => Some(url.clone()),
            _ => None,
        }
    }

    /// The path the connection currently uses.
    pub fn path(&self) -> ConnectionPath {
        if self.has_usb_tunnel() {
            return ConnectionPath::Usb;
        }
        let paths = self.conn.paths();
        let selected = paths.iter().find(|p| p.is_selected()).or_else(|| paths.iter().next());
        let picked = selected.map(|p| match p.remote_addr() {
            _ if p.is_relay() => Picked::Relay,
            iroh::TransportAddr::Ip(addr) => Picked::Ip { ip: addr.ip(), usb: self.is_usb_addr(addr) },
            _ => Picked::Other,
        });
        let any_usb = paths
            .iter()
            .any(|p| matches!(p.remote_addr(), iroh::TransportAddr::Ip(addr) if self.is_usb_addr(addr)));
        path_of(picked, any_usb)
    }

    /// The peer's direct IP addresses currently in use, for reconnecting later.
    pub fn remote_ip_addrs(&self) -> Vec<std::net::SocketAddr> {
        self.conn
            .paths()
            .iter()
            .filter_map(|p| match p.remote_addr() {
                iroh::TransportAddr::Ip(addr) if !self.is_usb_addr(addr) && !addr.ip().is_loopback() => {
                    Some(*addr)
                }
                _ => None,
            })
            .collect()
    }

    pub(crate) fn sent_icons(&self) -> std::sync::MutexGuard<'_, std::collections::HashSet<String>> {
        self.sent_icons.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn notifications_held(&self) -> bool {
        self.notifications_held.load(Ordering::Relaxed)
    }

    pub(crate) fn hold_notifications(&self, held: bool) {
        self.notifications_held.store(held, Ordering::Relaxed);
    }

    pub fn is_alive(&self) -> bool {
        !self.cancel.is_cancelled() && self.conn.close_reason().is_none()
    }

    /// Sends a QUIC datagram (`docs/protocol/v1.md` §5).
    pub fn send_datagram(&self, data: bytes::Bytes) -> Result<()> {
        self.conn.send_datagram(data).map_err(crate::error::net)
    }

    /// Opens a bidirectional stream on the session connection.
    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream)> {
        self.conn.open_bi().await.map_err(crate::error::net)
    }

    pub(crate) fn next_request_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Sends a one-way message on the stream its type goes on: control, or
    /// its service's lane (`docs/protocol/v1.md` §4).
    pub async fn send(&self, env: Envelope) -> Result<()> {
        match lane_of(&env.t) {
            Some(Lane::Control) => self.send_control(env).await,
            Some(Lane::Service(svc)) => self.lane_send(svc, env).await,
            _ => Err(Error::Protocol(format!("{} isn't sent one way", env.t))),
        }
    }

    /// Queues a one-way message without waiting, so messages queued one after
    /// another from outside the runtime keep their order. Fails when the
    /// connection is gone, or with [`Error::QueueFull`] when its stream has
    /// a backlog.
    pub(crate) fn send_now(&self, env: Envelope) -> Result<()> {
        match lane_of(&env.t) {
            Some(Lane::Control) => self.outbox.try_send(env).map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => Error::QueueFull("the control stream is full".into()),
                mpsc::error::TrySendError::Closed(_) => Error::Offline,
            }),
            Some(Lane::Service(svc)) => self.lane_send_now(svc, env),
            _ => Err(Error::Protocol(format!("{} isn't sent one way", env.t))),
        }
    }

    /// Queues a loss-tolerant lane message (a text delta, a text state that
    /// a newer one replaces) without waiting; dropped while its lane is stuck.
    pub(crate) fn send_lossy(&self, env: Envelope) -> Result<()> {
        match lane_of(&env.t) {
            Some(Lane::Service(svc)) => self.lane_send_lossy(svc, env),
            _ => self.send_now(env),
        }
    }

    /// Writes a control message (or an answer on control).
    pub(crate) async fn send_control(&self, env: Envelope) -> Result<()> {
        wire_stats::control_depth(OUTBOX_CAPACITY.saturating_sub(self.outbox.capacity()));
        self.outbox.send(env).await.map_err(|_| Error::Offline)
    }

    /// Sends a request on the stream its type goes on, and waits up to
    /// `timeout` for the answer: the `ok` or `error` (or, on control, the
    /// typed reply). A paged RPC goes through [`Session::request_paged`].
    pub async fn request(&self, env: Envelope, timeout: Duration) -> Result<Envelope> {
        self.request_tracked(env, timeout).await.0
    }

    /// Like [`request`](Self::request), and also says whether the request was
    /// sent. A request that went out may have run on the peer even if no
    /// reply came back, so only an unsent one is safe to send again.
    pub async fn request_tracked(&self, env: Envelope, timeout: Duration) -> (Result<Envelope>, bool) {
        match lane_of(&env.t) {
            Some(Lane::Rpc { .. }) => {
                let Some(op) = lanes::rpc_of(&env.t) else {
                    return (Err(Error::Internal("an RPC without an op".into())), false);
                };
                let (answer, sent) = self.rpc_tracked(op, env, timeout).await;
                let answer = answer.and_then(|answer| {
                    if answer.parts.is_empty() {
                        Ok(answer.last)
                    } else {
                        Err(Error::Protocol(format!("{} answered in parts", op.t)))
                    }
                });
                (answer, sent)
            }
            Some(Lane::Service(svc)) => self.lane_request(svc, env, timeout).await,
            Some(Lane::Control) => self.control_request(env, timeout).await,
            _ => (Err(Error::Protocol(format!("{} isn't a request", env.t))), false),
        }
    }

    async fn control_request(&self, env: Envelope, timeout: Duration) -> (Result<Envelope>, bool) {
        let started = Instant::now();
        let t = env.t.clone();
        let id = self.next_request_id();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
        let mut sent = false;
        let exchange = async {
            self.send_control(env.with_id(id)).await?;
            sent = true;
            rx.await.map_err(|_| Error::Offline)
        };
        let result = tokio::select! {
            r = tokio::time::timeout(timeout, exchange) => r.unwrap_or(Err(Error::Timeout)),
            () = self.cancel.cancelled() => Err(Error::Offline),
        };
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        if t != types::PING {
            wire_stats::request(wire_stats::RequestStats {
                svc: "control",
                op: &t,
                lane: LaneKind::Control,
                took: started.elapsed(),
                outcome: wire_stats::Outcome::of(&result),
                parts: 0,
                bytes: 0,
            });
        }
        (result, sent)
    }

    /// Closes the connection with an application code.
    pub fn close(&self, code: u32, reason: &[u8]) {
        self.conn.close(VarInt::from_u32(code), reason);
        self.cancel.cancel();
    }

    /// Hands a control answer to the request waiting for it, if one is.
    fn complete(&self, env: Envelope) -> std::result::Result<(), Envelope> {
        let Some(re) = env.re else { return Err(env) };
        match self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&re) {
            Some(tx) => {
                let _ = tx.send(env);
                Ok(())
            }
            None => Err(env),
        }
    }

    /// Fails every request still waiting, on control, lanes and RPCs.
    fn fail_waiting(&self) {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.lanes.fail_all();
        self.rpc_out.close();
    }

    /// The session's wire counters (`plans/protocol-v1-contract.md` §3).
    pub(crate) fn wire_counters(&self) -> wire_stats::WireCounters {
        self.counters.snapshot()
    }
}

/// A session task, counted while it runs, so ending sessions can be seen to
/// leave none behind.
struct Live(Arc<AtomicUsize>);

impl Live {
    fn new(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::Relaxed);
        Live(count.clone())
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn counted<F>(count: &Arc<AtomicUsize>, task: F) -> impl std::future::Future<Output = ()> + Send + 'static
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let live = Live::new(count);
    async move {
        let _live = live;
        task.await
    }
}

/// Waits for the first of the session's tasks to end (or the session to be
/// cancelled), then winds the session down (`plans/protocol-v1-contract.md`
/// §2.1): every waiting request fails `Offline`, the lanes go, `on_end`
/// runs once, the tasks get a moment to finish, and the connection closes.
async fn supervise(session: Arc<Session>, mut tasks: JoinSet<()>, on_end: impl FnOnce(Arc<Session>)) {
    tokio::select! {
        () = session.cancel.cancelled() => {}
        _ = tasks.join_next() => {}
    }
    session.cancel.cancel();
    session.fail_waiting();
    session.motion.close();
    tracing::debug!(peer = %session.peer.short(), "session ended");
    on_end(session.clone());
    let finished =
        tokio::time::timeout(WIND_DOWN, async { while tasks.join_next().await.is_some() {} }).await;
    if finished.is_err() {
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
    // Streams the peer opened run on tasks of their own, holding the
    // connection: closing it ends them with the session.
    if session.conn.close_reason().is_none() {
        session.conn.close(VarInt::from_u32(CLOSE_NORMAL), b"session ended");
    }
}

/// A device toggle read per datagram: from memory, read again on a blocking
/// thread at most once a second.
#[derive(Debug, Default)]
pub(crate) struct CachedToggle {
    on: AtomicBool,
    /// When it was last read, in ms since the session started, plus one (0: never).
    read_at: AtomicU64,
    reading: AtomicBool,
}

impl CachedToggle {
    const FRESH_MS: u64 = 1000;

    fn get(self: &Arc<Self>, shared: &Arc<Shared>, session: &Session, toggle: &'static str) -> bool {
        let now = session.started.elapsed().as_millis().min(u128::from(u64::MAX - 1)) as u64 + 1;
        let read_at = self.read_at.load(Ordering::Relaxed);
        if read_at == 0 {
            // The first time only: one read here.
            let on = shared.toggle_on(&session.peer, toggle);
            self.on.store(on, Ordering::Relaxed);
            self.read_at.store(now, Ordering::Relaxed);
            return on;
        }
        if now.saturating_sub(read_at) >= Self::FRESH_MS && !self.reading.swap(true, Ordering::AcqRel) {
            let (cached, shared, peer) = (self.clone(), shared.clone(), session.peer);
            tokio::task::spawn_blocking(move || {
                cached.on.store(shared.toggle_on(&peer, toggle), Ordering::Relaxed);
                cached.read_at.store(now, Ordering::Relaxed);
                cached.reading.store(false, Ordering::Release);
            });
        }
        self.on.load(Ordering::Relaxed)
    }
}

/// PC sound in a datagram from the peer of `session`.
fn sound_datagram(shared: &Arc<Shared>, session: &Session, bytes: &[u8]) {
    // A rate or layout out of range would play at the wrong speed: dropped.
    let Ok(sound) = SoundDatagram::decode(bytes) else { return };
    emit_sound(shared, session, sound);
}

/// Hands PC sound from the peer of `session` to the app, unless it's older
/// than what came last.
pub(crate) fn emit_sound(shared: &Arc<Shared>, session: &Session, sound: SoundDatagram) {
    if !session.sound_toggle.get(shared, session, crate::media::TOGGLE) {
        return;
    }
    if !session.sound_in.lock().unwrap_or_else(|e| e.into_inner()).take(sound.seq, &session.counters) {
        return;
    }
    shared.emit(NodeEvent::PcAudioFrame {
        device: session.peer,
        sample_rate: sound.rate,
        channels: sound.channels,
        seq: sound.seq,
        ts_ms: sound.ts_ms,
        pcm: sound.payload,
    });
}

/// Decodes a frame's envelope, on a blocking thread when it's large.
pub(crate) async fn decode_frame(frame: Vec<u8>) -> std::result::Result<Envelope, ProtocolError> {
    if frame.len() < LARGE_FRAME {
        return Envelope::from_cbor(&frame);
    }
    tokio::task::spawn_blocking(move || Envelope::from_cbor(&frame))
        .await
        .map_err(|e| ProtocolError::BadMessage(e.to_string()))?
}

/// Writes the control outbox, in order. Waits on nothing but the stream.
async fn control_writer(mut send: SendStream, mut outbox: mpsc::Receiver<Envelope>) {
    let _ = send.set_priority(CONTROL_PRIORITY);
    while let Some(env) = outbox.recv().await {
        if let Err(e) = write_frame(&mut send, &env.to_cbor()).await {
            tracing::debug!(error = %e, "control stream write failed");
            return;
        }
    }
    let _ = send.finish();
}

/// Reads control: answers go to the requests waiting for them, `ping` and
/// a few events are handled here, and everything that touches the store
/// goes to the core queue. Never waits on a platform call or the disk.
async fn control_reader(session: Arc<Session>, mut recv: RecvStream, shared: Weak<Shared>) {
    session.wait_ready().await;
    loop {
        let frame = match read_frame(&mut recv).await {
            Ok(Some(frame)) => {
                session.touch_rx();
                frame
            }
            Ok(None) => return,
            Err(ProtocolError::FrameTooLarge(len)) => {
                tracing::warn!(peer = %session.peer.short(), len, "peer sent an oversized frame");
                session.close(ErrorCode::FrameTooLarge.close_code(), b"frame too large");
                return;
            }
            Err(e) => {
                tracing::debug!(peer = %session.peer.short(), error = %e, "control stream read failed");
                return;
            }
        };
        let env = match decode_frame(frame).await {
            Ok(env) => env,
            Err(e) => {
                tracing::warn!(peer = %session.peer.short(), error = %e, "dropping malformed message");
                continue;
            }
        };
        if env.re.is_some() {
            if let Some(id) = env.id {
                let refusal = Envelope::error(ErrorCode::BadMessage, "a request can't also be an answer");
                let _ = session.send_control(refusal.reply_to(Some(id))).await;
                continue;
            }
            if let Err(late) = session.complete(env) {
                wire_stats::late_reply(&session.counters, LaneKind::Control, &late.t);
            }
            continue;
        }
        let Some(shared) = shared.upgrade() else { return };
        on_control(&shared, &session, env).await;
    }
}

/// A control message that isn't an answer.
async fn on_control(shared: &Arc<Shared>, session: &Arc<Session>, env: Envelope) {
    let peer = session.peer;
    if lane_of(&env.t) != Some(Lane::Control) {
        wire_stats::wrong_lane(&session.counters, LaneKind::Control, &env.t);
        if env.id.is_some() {
            let reply = Envelope::error(ErrorCode::BadMessage, format!("{} doesn't go on control", env.t));
            let _ = session.send_control(reply.reply_to(env.id)).await;
        }
        return;
    }
    match env.t.as_str() {
        types::PING => match env.body::<Ping>() {
            Ok(ping) => {
                // Never waits: a pong held up behind a full outbox is a missed heartbeat.
                if let Ok(pong) = Envelope::new(types::PONG, &ping)
                    && session.outbox.try_send(pong.reply_to(env.id)).is_err()
                {
                    tracing::debug!(peer = %peer.short(), "the control outbox is full; no pong");
                }
            }
            Err(_) => tracing::debug!(peer = %peer.short(), "ignored a malformed ping"),
        },
        types::DEVICE_BATTERY => match env.body::<Battery>() {
            Ok(battery) => shared.emit(NodeEvent::Battery { device: peer, battery: battery.sanitized() }),
            Err(_) => tracing::debug!(peer = %peer.short(), "ignored a malformed device.battery"),
        },
        types::DEVICE_UPDATE => match env.body::<UpdateHint>().ok().and_then(UpdateHint::sanitized) {
            Some(hint) => shared.emit(NodeEvent::UpdateHint {
                device: peer,
                channel: hint.channel,
                version: hint.version,
            }),
            None => tracing::debug!(peer = %peer.short(), "ignored a malformed update hint"),
        },
        types::DEVICE_RING_STOPPED => match env.body::<RingStopped>() {
            Ok(stopped) => shared.clear_remote_ring(peer, Some(stopped.token)),
            Err(_) => tracing::debug!(peer = %peer.short(), "ignored a malformed device.ring_stopped"),
        },
        types::HELLO => tracing::debug!(peer = %peer.short(), "ignored a second hello"),
        types::PONG => {}
        _ => {
            // Store and platform work, in order, off this reader.
            let id = env.id;
            if let Err(e) = session.core.try_send(env) {
                tracing::warn!(peer = %peer.short(), "control work is backed up; refusing a message");
                if id.is_some() {
                    let busy = Envelope::error(ErrorCode::Busy, "too much at once");
                    let _ = session.send_control(busy.reply_to(id)).await;
                }
                drop(e);
            }
        }
    }
}

/// Runs the store and platform work of control messages, one at a time, in
/// the order they came.
async fn core_queue(session: Arc<Session>, mut queue: mpsc::Receiver<Envelope>, shared: Weak<Shared>) {
    while let Some(env) = queue.recv().await {
        let Some(shared) = shared.upgrade() else { return };
        let rsp = Responder::control(&session, env.id);
        let started = Instant::now();
        if let Err(e) = on_core(&shared, &session, &env, &rsp).await {
            tracing::warn!(peer = %session.peer.short(), t = %env.t, error = %e, "failed to handle message");
            if !rsp.answered() {
                let _ = rsp.send(crate::dispatch::error_reply(&e)).await;
            }
        }
        wire_stats::handler("core", &env.t, LaneKind::Control, started.elapsed());
    }
}

async fn on_core(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    env: &Envelope,
    rsp: &Responder<'_>,
) -> Result<()> {
    let peer = session.peer;
    match env.t.as_str() {
        types::DEVICE_INFO => {
            let info = env.body::<DeviceInfo>()?.sanitized();
            let effective = store_blocking(shared, move |shared| update_info(shared, peer, info)).await?;
            shared.emit(NodeEvent::PeerInfoChanged { device: peer, info: effective });
        }
        types::HELLO_UPDATE => {
            let update: HelloUpdate = env.body()?;
            if let Some(info) = update.device.map(DeviceInfo::sanitized) {
                let effective = store_blocking(shared, move |shared| update_info(shared, peer, info)).await?;
                shared.emit(NodeEvent::PeerInfoChanged { device: peer, info: effective });
            }
            let caps = update.caps.map(crate::features::sanitize_capabilities);
            let power = update.power.map(PowerLevel::effective);
            let had_caps = caps.is_some();
            if caps.is_some() || power.is_some() {
                store_blocking(shared, move |shared| {
                    shared.store.update_capabilities(&peer, caps.as_ref(), power)?;
                    shared.refresh_peer_caps(&peer);
                    Ok(())
                })
                .await?;
            }
            if let Some(power) = power {
                shared.emit(NodeEvent::PeerPowerChanged { device: peer, power });
            }
            shared.refresh_capabilities(&peer);
            if had_caps {
                shared.send_toggles_state(session).await;
                crate::agents::peer_caps_changed(shared, &peer);
                crate::agents::refresh_catalogs(shared, Some(peer)).await;
            }
        }
        types::DEVICE_RING => {
            let ring: Ring = env.body()?;
            let alive = Arc::downgrade(session);
            shared.receive_ring(peer, move || alive.upgrade().is_some_and(|s| s.is_alive()), ring).await;
            rsp.ok().await?;
        }
        types::PAIR_REVOKE => {
            tracing::info!(peer = %peer.short(), "peer unpaired this device");
            // Forgetting the peer ends this session, and this task with it:
            // the cleanup runs on a task of its own.
            let shared = shared.clone();
            tokio::spawn(async move { shared.forget_peer(&peer).await })
                .await
                .map_err(|err| Error::Internal(err.to_string()))??;
        }
        types::DEVICE_REACH => match env.body::<DeviceReach>() {
            Ok(reach) if reach.is_valid() => {
                store_blocking(shared, move |shared| shared.store_reach(&peer, &reach)).await?;
            }
            _ => tracing::debug!(peer = %peer.short(), "ignored a malformed device.reach"),
        },
        types::BLE_KEY => match env.body::<BleKey>() {
            Ok(body) => {
                store_blocking(shared, move |shared| shared.ble_key_from_peer(peer, body)).await?;
            }
            Err(_) => tracing::debug!(peer = %peer.short(), "ignored a malformed ble.key"),
        },
        types::BLE_USE => {
            let (shared, env) = (shared.clone(), env.clone());
            let reply = tokio::task::spawn_blocking(move || shared.ble_use_reply(peer, &env))
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
            rsp.send(reply).await?;
        }
        other => {
            rsp.error(ErrorCode::Unsupported, format!("unknown message type {other}")).await?;
        }
    }
    Ok(())
}

/// Runs store work on a blocking thread: a busy database or a slow disk then
/// holds up that thread, not one of the runtime's workers (two on Android),
/// which every session's heartbeat needs.
pub(crate) async fn store_blocking<T: Send + 'static>(
    shared: &Arc<Shared>,
    work: impl FnOnce(&Shared) -> Result<T> + Send + 'static,
) -> Result<T> {
    let shared = shared.clone();
    tokio::task::spawn_blocking(move || work(&shared)).await.map_err(|e| Error::Internal(e.to_string()))?
}

/// Stores what `peer` says about itself; what the apps show (the stored info).
fn update_info(shared: &Shared, peer: DeviceId, info: DeviceInfo) -> Result<DeviceInfo> {
    shared.store.update_info(&peer, &info)?;
    Ok(shared.store.get_peer(&peer).ok().flatten().map(|p| p.info).unwrap_or(info))
}

/// Concurrent incoming media and bulk streams allowed from one peer at a time.
pub(crate) const MAX_INCOMING_STREAMS: usize = 32;
/// The message of the BUSY a stream past [`MAX_INCOMING_STREAMS`] is
/// answered with: a sender tells it from a full disk by it, and tries again.
pub(crate) const TOO_MANY_STREAMS: &str = "too many streams at once";

/// How long a webcam or microphone stream may take to say its format.
pub(crate) const FIRST_CONFIG_TIMEOUT: Duration = Duration::from_secs(30);

/// Accepts the streams the peer opens and hands each to a task of its own:
/// lanes and RPCs end with the session, media and bulk streams with the
/// connection.
async fn acceptor(session: Arc<Session>, shared: Weak<Shared>) {
    session.wait_ready().await;
    let mut streams = JoinSet::new();
    loop {
        tokio::select! {
            accepted = session.conn.accept_bi() => {
                let Ok((send, recv)) = accepted else { break };
                session.touch_rx();
                let Some(shared) = shared.upgrade() else { break };
                streams.spawn(route_stream(shared, session.clone(), send, recv));
            }
            Some(_) = streams.join_next(), if !streams.is_empty() => {}
        }
    }
}

/// Reads a stream's header and serves it as what it says it is.
async fn route_stream(
    shared: Arc<Shared>,
    session: Arc<Session>,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let Some(header) = crate::transfer::read_header(&mut recv).await else {
        crate::transfer::refuse_unknown(send).await;
        return;
    };
    if header.op == lanes::OP_LANE {
        match lanes::LANE_SERVICES.iter().find(|svc| **svc == header.svc) {
            Some(svc) if header.v == lanes::VERSION => {
                crate::lane::serve(shared, session, svc, send, recv).await
            }
            _ => crate::transfer::refuse_unknown(send).await,
        }
        return;
    }
    if let Some(op) = lanes::rpc_by_header(&header.svc, &header.op) {
        if header.v != lanes::VERSION {
            return crate::transfer::refuse_unknown(send).await;
        }
        match session.rpc_in.clone().try_acquire_owned() {
            Ok(slot) => crate::rpc::serve(shared, session, op, send, recv, slot).await,
            Err(_) => crate::rpc::refuse_busy(&session, op, send).await,
        }
        return;
    }
    let Ok(slot) = session.streams_in.clone().try_acquire_owned() else {
        wire_stats::busy(&session.counters, &header.svc, &header.op, LaneKind::Stream, true);
        let busy = Envelope::error(ErrorCode::Busy, TOO_MANY_STREAMS);
        let _ = write_frame(&mut send, &busy.to_cbor()).await;
        let _ = send.finish();
        return;
    };
    // Media and bulk streams hold the connection and end with it, not with
    // the session's tasks: a transfer cleans up after itself when its stream fails.
    let peer = session.peer;
    let counters = session.counters.clone();
    let conn = session.conn.clone();
    tokio::spawn(async move {
        if counters.streams_in.fetch_add(1, Ordering::Relaxed) == 0 {
            tokio::spawn(report_conn(conn, counters.clone()));
        }
        let _ = send.set_priority(crate::lane::priority_of(&header.svc, &header.op));
        crate::transfer::serve_stream(shared, peer, header, send, recv).await;
        counters.streams_in.fetch_sub(1, Ordering::Relaxed);
        drop(slot);
    });
    drop(session);
}

/// Every 5 s while a media or bulk stream from the peer runs: how the
/// connection carries it (`plans/protocol-v1-contract.md` §3, `conn`).
async fn report_conn(conn: Connection, counters: Arc<Counters>) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if counters.streams_in.load(Ordering::Relaxed) == 0 || conn.close_reason().is_some() {
            return;
        }
        let stats = conn.stats();
        let rtt = conn.paths().iter().find(|p| p.is_selected()).map_or(Duration::ZERO, |p| p.rtt());
        wire_stats::conn(stats.lost_packets, 0, rtt, stats.udp_tx.bytes);
    }
}

/// Reads datagrams: pointer motion and PC sound. Calls nothing that waits.
async fn datagrams(session: Arc<Session>, shared: Weak<Shared>) {
    session.wait_ready().await;
    loop {
        match session.conn.read_datagram().await {
            Ok(bytes) => {
                session.touch_rx();
                let Some(shared) = shared.upgrade() else { return };
                match bytes.first() {
                    Some(&g7link_protocol::DATAGRAM_SOUND) => sound_datagram(&shared, &session, &bytes),
                    Some(&g7link_protocol::DATAGRAM_INPUT) => match RemoteInput::from_datagram(&bytes) {
                        Ok(event) => session.motion.push(event, &session.counters),
                        Err(_) => Counters::add(&session.counters.unknown_datagrams, 1),
                    },
                    _ => Counters::add(&session.counters.unknown_datagrams, 1),
                }
            }
            Err(_) => {
                // If datagrams aren't available on this connection, stay parked
                // until the session ends rather than tearing it down.
                std::future::pending::<()>().await;
            }
        }
    }
}

/// How long the heartbeat waits between pings.
fn heartbeat_period(interactive: bool) -> Duration {
    if interactive { INTERACTIVE_HEARTBEAT_INTERVAL } else { HEARTBEAT_INTERVAL }
}

/// Whether the user has the app open (`Node::set_interactive`): heartbeats
/// then ping every 5 s instead of every 15 s.
#[derive(Debug, Default)]
pub(crate) struct Interactive {
    on: AtomicBool,
    /// Wakes heartbeats waiting out a background period when the app opens.
    opened: Notify,
}

impl Interactive {
    pub(crate) fn set(&self, on: bool) {
        if !self.on.swap(on, Ordering::SeqCst) && on {
            self.opened.notify_waiters();
        }
    }

    pub(crate) fn get(&self) -> bool {
        self.on.load(Ordering::SeqCst)
    }

    /// Waits until the next ping is due: one period for the current mode, or
    /// until the app opens, so opening it checks the link right away.
    async fn wait_for_next_ping(&self) {
        // Created before reading the mode, so an opening in between isn't missed.
        let opened = self.opened.notified();
        tokio::select! {
            _ = tokio::time::sleep(heartbeat_period(self.get())) => {}
            _ = opened => {}
        }
    }
}

/// The heartbeat's view of whether the session still has a direct path this
/// device allows (`plans/disconnects.md` cause 2). Paths come and go while
/// QUIC moves between them, so one look without one is no reason to close:
/// it takes two heartbeats and at least [`PathWatch::CLOSE_AFTER`].
#[derive(Debug, Default)]
pub(crate) struct PathWatch {
    missing_since: Option<Instant>,
    misses: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathVerdict {
    Allowed,
    /// The first look without an allowed path: worth asking QUIC to look again.
    FirstMiss,
    Missing,
    Close,
}

impl PathWatch {
    pub(crate) const CLOSE_AFTER: Duration = Duration::from_secs(10);

    pub(crate) fn observe(&mut self, allowed: bool, now: Instant) -> PathVerdict {
        if allowed {
            *self = PathWatch::default();
            return PathVerdict::Allowed;
        }
        self.misses += 1;
        let since = *self.missing_since.get_or_insert(now);
        if self.misses >= 2 && now.saturating_duration_since(since) >= Self::CLOSE_AFTER {
            PathVerdict::Close
        } else if self.misses == 1 {
            PathVerdict::FirstMiss
        } else {
            PathVerdict::Missing
        }
    }

    fn missing(&self) -> bool {
        self.missing_since.is_some()
    }
}

/// The heartbeat's two strikes (`plans/disconnects.md` cause 3): a round
/// with no answer and nothing received is a strike, and the session closes
/// only when the next round also gets nothing, with no bytes in between.
#[derive(Debug, Default)]
pub(crate) struct Strikes {
    /// The bytes received when the strike was given.
    struck_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Liveness {
    Alive,
    Strike,
    Dead,
}

impl Strikes {
    /// After a round of pings: whether anything answered, and the bytes
    /// received on the connection so far.
    pub(crate) fn round(&mut self, answered: bool, received: u64) -> Liveness {
        if answered {
            self.struck_at = None;
            return Liveness::Alive;
        }
        match self.struck_at {
            Some(before) if received <= before => Liveness::Dead,
            _ => {
                self.struck_at = Some(received);
                Liveness::Strike
            }
        }
    }

    fn struck(&self) -> bool {
        self.struck_at.is_some()
    }
}

/// How soon the heartbeat pings again after a strike.
const STRIKE_RETRY: Duration = Duration::from_secs(2);
/// How soon it looks again while no allowed path is left.
const PATH_RECHECK: Duration = Duration::from_secs(5);

impl Session {
    /// The node registered the session: what the peer sends may be handled.
    pub(crate) fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
        self.ready_notify.notify_waiters();
    }

    /// Waits until [`Session::mark_ready`], so the apps hear the peer is
    /// online before they hear anything from it.
    async fn wait_ready(&self) {
        loop {
            let notified = self.ready_notify.notified();
            if self.ready.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    /// Has the heartbeat ping now (a network change here). A wake while a
    /// ping is out is absorbed, and wakes come at most once per
    /// [`WAKE_EVERY`] per session, so a burst of changes costs one ping.
    pub(crate) fn wake_heartbeat(&self) {
        let now = Instant::now();
        let mut last = self.heartbeat_woken.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| now.saturating_duration_since(at) < WAKE_EVERY) {
            return;
        }
        *last = Some(now);
        drop(last);
        self.ping_now.notify_waiters();
    }
}

/// How often a network change may wake one session's heartbeat, at most.
const WAKE_EVERY: Duration = Duration::from_secs(5);

/// One round of pings: whether the peer answered, or anything at all came
/// from it while it ran. The pong's round trip, when one came.
async fn ping_round(session: &Session, started: Instant) -> (bool, Option<Duration>) {
    let sent_at = Instant::now();
    let received = received_bytes(session);
    let ts = sent_at.duration_since(started).as_millis() as u64;
    let Ok(ping) = Envelope::new(types::PING, &Ping { ts }) else { return (true, None) };
    Counters::add(&session.counters.heartbeat_pings, 1);
    match session.request(ping, HEARTBEAT_TIMEOUT).await {
        Ok(reply) if reply.t == types::PONG => return (true, Some(sent_at.elapsed())),
        Ok(_) => return (true, None),
        Err(_) => {}
    }
    // No answer, but the peer's packets arriving still count: a slow link is
    // full of what's being sent, and the answer is queued behind it.
    if session.rx_since(sent_at) || received_bytes(session) > received {
        return (true, None);
    }
    let retry_ts = started.elapsed().as_millis() as u64;
    Counters::add(&session.counters.heartbeat_pings, 1);
    if let Ok(retry) = Envelope::new(types::PING, &Ping { ts: retry_ts })
        && matches!(session.request(retry, HEARTBEAT_RETRY_TIMEOUT).await, Ok(r) if r.t == types::PONG)
    {
        return (true, None);
    }
    (session.rx_since(sent_at) || received_bytes(session) > received, None)
}

async fn heartbeat(session: Arc<Session>, shared: Weak<Shared>) {
    let (session, shared) = (&session, &shared);
    let started = Instant::now();
    let interactive = shared.upgrade().map(|s| s.interactive.clone()).unwrap_or_default();
    let mut paths = PathWatch::default();
    let mut strikes = Strikes::default();
    // The first ping goes out right away, for the round-trip time.
    let mut first = true;
    loop {
        if !first {
            let woken = session.ping_now.notified();
            if strikes.struck() {
                tokio::time::sleep(STRIKE_RETRY).await;
            } else if paths.missing() {
                tokio::time::sleep(PATH_RECHECK.min(heartbeat_period(interactive.get()))).await;
            } else {
                tokio::select! {
                    () = interactive.wait_for_next_ping() => {}
                    () = woken => {}
                }
            }
        }
        first = false;
        if let Some(shared) = shared.upgrade() {
            let allowed = shared.session_path_allowed(session) && !shared.debug_paths_refused();
            if allowed == paths.missing() {
                let all = session.conn.paths();
                let selected = match all.iter().find(|p| p.is_selected()) {
                    Some(p) if p.is_relay() => "relay",
                    Some(_) => "direct",
                    None => "none",
                };
                wire_stats::path_policy(allowed, all.len(), selected);
            }
            match paths.observe(allowed, Instant::now()) {
                PathVerdict::Allowed | PathVerdict::Missing => {}
                PathVerdict::FirstMiss => {
                    tracing::debug!(peer = %session.peer.short(), "no allowed path for now; looking again");
                    shared.endpoint.network_change().await;
                }
                PathVerdict::Close => {
                    tracing::info!(peer = %session.peer.short(), "closing session on disallowed non-LAN path");
                    wire_stats::liveness("path", session.started.elapsed());
                    session.close(CLOSE_NORMAL, b"away mode disabled");
                    return;
                }
            }
        }
        let (answered, rtt) = ping_round(session, started).await;
        if let Some(shared) = shared.upgrade() {
            let mut moved = false;
            if let Some(rtt) = rtt {
                let rtt_ms = rtt.as_millis().min(u128::from(u32::MAX)) as u32;
                let path = session.path_rtt();
                wire_stats::rtt(rtt, path);
                let previous = session.rtt_ms.swap(rtt_ms, Ordering::Relaxed);
                moved = previous.abs_diff(rtt_ms) >= 5;
            }
            // A move between paths (Wi-Fi to the tailnet) is published by the
            // next heartbeat even when the round trip stays about the same.
            if moved || shared.path_moved(session) {
                shared.publish_online(session);
            }
        }
        match strikes.round(answered, received_bytes(session)) {
            Liveness::Alive => {}
            Liveness::Strike => {
                tracing::debug!(peer = %session.peer.short(), "no answer to a heartbeat; one more try");
            }
            Liveness::Dead => {
                tracing::info!(peer = %session.peer.short(), "heartbeat timed out; closing dead session");
                wire_stats::liveness("heartbeat", session.started.elapsed());
                session.close(CLOSE_NORMAL, b"heartbeat timeout");
                return;
            }
        }
    }
}

/// All that has arrived on this connection, acknowledgements included:
/// nothing does from a peer that's gone.
fn received_bytes(session: &Session) -> u64 {
    session.conn.stats().udp_rx.bytes
}

#[cfg(test)]
mod tests {
    use super::{
        Interactive, Liveness, PathVerdict, PathWatch, Picked, Strikes, heartbeat_period, is_lan_ip,
        is_lan_peer_ip, is_same_ipv6_subnet, is_tailnet_ip, path_of,
    };
    use crate::ConnectionPath;
    use std::{
        net::{IpAddr, Ipv6Addr, SocketAddr},
        sync::Arc,
        time::Duration,
    };

    #[test]
    fn lan_ip_accepts_private_local_and_tailnet_and_rejects_wan() {
        for ok in [
            "127.0.0.1",
            "192.168.1.42",
            "10.0.0.15",
            "172.16.5.2",
            "169.254.10.20",
            "::1",
            "fe80::1234",
            "fd12:3456:789a::1",
            "100.64.0.1",
            "100.101.102.103",
            "100.127.255.254",
            "fd7a:115c:a1e0::1",
        ] {
            let ip: IpAddr = ok.parse().unwrap();
            assert!(is_lan_ip(ip), "expected {ok} to be treated as LAN IP");
        }

        for bad in
            ["8.8.8.8", "100.63.255.255", "100.128.0.1", "2001:4860:4860::8888", "2401:4900:1c2a:8b00::1"]
        {
            let ip: IpAddr = bad.parse().unwrap();
            assert!(!is_lan_ip(ip), "expected {bad} to be rejected as non-LAN IP");
        }
    }

    #[test]
    fn a_public_address_the_peer_announced_over_mdns_counts_as_lan() {
        let announced: Vec<SocketAddr> = vec!["192.1.1.208:49806".parse().unwrap()];
        // The peer's own address, on any port and IPv4-mapped too.
        for ok in ["192.1.1.208", "::ffff:192.1.1.208"] {
            assert!(is_lan_peer_ip(ok.parse().unwrap(), &announced), "expected {ok} to be LAN");
        }
        // Private addresses need no announcement.
        assert!(is_lan_peer_ip("192.168.1.42".parse().unwrap(), &[]));
        // Other public addresses, even next to it, stay out.
        for bad in ["192.1.1.209", "8.8.8.8", "2001:4860:4860::8888"] {
            assert!(!is_lan_peer_ip(bad.parse().unwrap(), &announced), "expected {bad} not to be LAN");
        }
        assert!(!is_lan_peer_ip("192.1.1.208".parse().unwrap(), &[]));
    }

    #[test]
    fn tailnet_ip_matches_only_tailscale_ranges() {
        for ok in [
            "100.64.0.0",
            "100.100.100.100",
            "100.127.255.255",
            "fd7a:115c:a1e0:ab12::1",
            "::ffff:100.64.1.2",
        ] {
            assert!(is_tailnet_ip(ok.parse().unwrap()), "expected {ok} to be a tailnet IP");
        }
        for bad in ["100.63.0.1", "100.128.0.1", "192.168.1.1", "fd7a:115c:a1e1::1", "fd12::1"] {
            assert!(!is_tailnet_ip(bad.parse().unwrap()), "expected {bad} not to be a tailnet IP");
        }
    }

    #[test]
    fn a_direct_path_to_a_tailnet_address_is_named_tailscale() {
        let ip = |text: &str, usb: bool| Some(Picked::Ip { ip: text.parse().unwrap(), usb });
        for tailnet in ["100.64.0.1", "100.101.102.103", "fd7a:115c:a1e0::1", "::ffff:100.100.1.2"] {
            assert_eq!(path_of(ip(tailnet, false), false), ConnectionPath::Tailscale, "{tailnet}");
        }
        for lan in ["192.168.1.42", "10.0.0.15", "fe80::1", "fd12::1", "100.63.0.1", "100.128.0.1", "8.8.8.8"]
        {
            assert_eq!(path_of(ip(lan, false), false), ConnectionPath::Lan, "{lan}");
        }
        // The relay and USB keep their names; a tailnet address doesn't change them.
        assert_eq!(path_of(Some(Picked::Relay), false), ConnectionPath::Relay);
        assert_eq!(path_of(Some(Picked::Relay), true), ConnectionPath::Relay);
        assert_eq!(path_of(ip("127.0.0.1", true), false), ConnectionPath::Usb);
        assert_eq!(path_of(ip("100.64.0.1", false), true), ConnectionPath::Usb, "the USB bridge has a path");
        // No path known yet, or one this build can't place: as before, `Lan`.
        assert_eq!(path_of(None, false), ConnectionPath::Lan);
        assert_eq!(path_of(Some(Picked::Other), false), ConnectionPath::Lan);
        assert_eq!(path_of(None, true), ConnectionPath::Usb);
        assert!(ConnectionPath::Tailscale.is_direct_ip() && ConnectionPath::Lan.is_direct_ip());
        assert!(!ConnectionPath::Relay.is_direct_ip() && !ConnectionPath::Usb.is_direct_ip());
    }

    #[test]
    fn ipv6_same_subnet_matches_dual_stack_router_prefix_and_rejects_cellular() {
        let pc_wifi: Ipv6Addr = "2001:db8:2042:8b43:894c:6df4:ca1c:7561".parse().unwrap();
        let phone_wifi: Ipv6Addr = "2001:db8:2042:8b43:a9eb:bf3f:78e8:35bf".parse().unwrap();
        let phone_lte: Ipv6Addr = "2001:db8:1c2a:9911:1122:3344:5566:7788".parse().unwrap();
        assert!(is_same_ipv6_subnet(pc_wifi, phone_wifi));
        assert!(!is_same_ipv6_subnet(pc_wifi, phone_lte));
    }

    #[test]
    fn no_allowed_path_must_last_to_close() {
        let start = std::time::Instant::now();
        let at = |s: u64| start + Duration::from_secs(s);
        let mut watch = PathWatch::default();
        assert_eq!(watch.observe(false, at(0)), PathVerdict::FirstMiss);
        assert_eq!(watch.observe(false, at(5)), PathVerdict::Missing, "two misses, but only 5 s");
        assert_eq!(watch.observe(true, at(8)), PathVerdict::Allowed, "a path came back");
        assert_eq!(watch.observe(false, at(9)), PathVerdict::FirstMiss, "counting starts over");
        assert_eq!(watch.observe(false, at(14)), PathVerdict::Missing);
        assert_eq!(watch.observe(false, at(19)), PathVerdict::Close, "10 s across two heartbeats");
        // One long look alone isn't two heartbeats.
        let mut watch = PathWatch::default();
        assert_eq!(watch.observe(false, at(0)), PathVerdict::FirstMiss);
        assert_eq!(watch.observe(false, at(30)), PathVerdict::Close);
    }

    #[test]
    fn one_missed_heartbeat_round_is_a_strike_not_a_close() {
        let mut strikes = Strikes::default();
        assert_eq!(strikes.round(false, 100), Liveness::Strike);
        assert_eq!(strikes.round(true, 100), Liveness::Alive, "answered: the strike is gone");
        assert_eq!(strikes.round(false, 100), Liveness::Strike);
    }

    #[test]
    fn two_missed_rounds_with_no_bytes_close() {
        let mut strikes = Strikes::default();
        assert_eq!(strikes.round(false, 100), Liveness::Strike);
        assert_eq!(strikes.round(false, 100), Liveness::Dead);
    }

    #[test]
    fn bytes_arriving_clear_the_strike() {
        let mut strikes = Strikes::default();
        assert_eq!(strikes.round(false, 100), Liveness::Strike);
        assert_eq!(strikes.round(false, 180), Liveness::Strike, "bytes came between the rounds");
        assert_eq!(strikes.round(false, 180), Liveness::Dead);
    }

    #[test]
    fn heartbeat_pings_every_5_s_with_the_app_open_and_every_15_s_otherwise() {
        assert_eq!(heartbeat_period(true), Duration::from_secs(5));
        assert_eq!(heartbeat_period(false), Duration::from_secs(15));
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeat_waits_out_the_period_for_the_current_mode() {
        let interactive = Interactive::default();
        let started = tokio::time::Instant::now();
        interactive.wait_for_next_ping().await;
        let waited = started.elapsed();
        assert!(waited >= Duration::from_secs(15) && waited < Duration::from_millis(15_100), "{waited:?}");

        interactive.set(true);
        let started = tokio::time::Instant::now();
        interactive.wait_for_next_ping().await;
        let waited = started.elapsed();
        assert!(waited >= Duration::from_secs(5) && waited < Duration::from_millis(5_100), "{waited:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn opening_the_app_ends_a_background_wait_right_away() {
        let interactive = Arc::new(Interactive::default());
        let waiting = tokio::spawn({
            let interactive = interactive.clone();
            async move { interactive.wait_for_next_ping().await }
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(!waiting.is_finished(), "a background wait lasts 15 s");

        let opened_at = tokio::time::Instant::now();
        interactive.set(true);
        waiting.await.expect("the wait finishes");
        assert!(opened_at.elapsed() < Duration::from_secs(1), "woke after {:?}", opened_at.elapsed());
    }

    #[tokio::test(start_paused = true)]
    async fn closing_the_app_doesnt_cut_a_wait_short() {
        let interactive = Arc::new(Interactive::default());
        interactive.set(true);
        let waiting = tokio::spawn({
            let interactive = interactive.clone();
            async move { interactive.wait_for_next_ping().await }
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        interactive.set(false);
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        tokio::time::advance(Duration::from_secs(4)).await;
        waiting.await.expect("the wait finishes");
    }
}
