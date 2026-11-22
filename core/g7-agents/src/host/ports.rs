// SPDX-License-Identifier: MPL-2.0
//! Dev servers on the phone (phase 5, §V9.2): the TCP ports a thread's agent,
//! and the processes it started, listen on; and from phase 6 (§X3.5) its
//! script runs' shells and theirs. While one of those runs the host looks
//! every 5 s for a minute after a command or run starts, else every 30 s;
//! what it finds rides `thread.state` as `ThreadInfo.ports`, oldest first,
//! at most [`MAX_PORTS`], until the thread has no root left.
//!
//! Only `/proc` is read: each of this user's processes' `stat` (its parent
//! and start time), the `fd` links of at most 64 processes in the agent's
//! tree (256 descriptors each, which sockets they hold), and `net/tcp` and
//! `net/tcp6` (at most 1 MiB each). A process's start time is read with its
//! pid and again after its descriptors, so a pid used again in between counts
//! for nothing. Never `environ`, `cmdline` or any other file of a process.
//! Nothing is probed or connected to, and nothing is stored.
//!
//! A server in another network namespace (a container) isn't seen.
//!
//! [`scan`] blocks; the host calls it from `spawn_blocking`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustix::fs::{Mode, OFlags};

use crate::api::*;
use crate::host::Host;
use crate::host::sessions::procs;
use crate::host::store::now_ms;
use crate::host::threads::Entry;

/// How often a thread is looked at for a minute after a command starts.
pub const FAST_EVERY: Duration = Duration::from_secs(5);
pub const FAST_FOR: Duration = Duration::from_secs(60);
/// How often otherwise, while its agent's process runs.
pub const SLOW_EVERY: Duration = Duration::from_secs(30);
/// Processes of one tree looked at, the agent's own included.
pub const MAX_TREE: usize = 64;
/// Descriptors of one process looked at.
pub const MAX_FDS: usize = 256;
/// What is read of `net/tcp` and of `net/tcp6`.
pub const MAX_NET_FILE: u64 = 1 << 20;
/// `/proc` entries listed to find a tree: as many processes as the session
/// scan looks at, and the few dozen other names before the pids.
const MAX_LISTED: usize = MAX_PROC_SCAN + 256;
const MAX_STAT: u64 = 4096;
/// `st` of a listening socket.
const LISTEN: &str = "0A";

/// A process, by its pid and its start time (clock ticks after boot, `stat`
/// field 22), read together: a later process with the same pid differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Root {
    pub pid: u32,
    pub start_ticks: u64,
}

impl Root {
    /// `pid` as it is now. Read a thread's agent while the host still holds
    /// its process (not yet reaped), so the pid can't be another's yet.
    pub fn read(proc_root: &Path, pid: u32) -> Option<Root> {
        let stat = read_stat(&proc_root.join(pid.to_string()))?;
        Some(Root { pid, start_ticks: stat.start_ticks })
    }
}

/// Where and as whom [`scan`] looks.
#[derive(Debug, Clone)]
pub struct Look {
    /// `/proc`, or a fake tree in tests.
    pub proc_root: PathBuf,
    /// This user: only its processes and sockets count.
    pub euid: u32,
    /// Whose a `/proc/<pid>` folder is.
    pub owner: fn(&Path) -> Option<u32>,
}

impl Look {
    pub fn new(proc_root: PathBuf) -> Look {
        Look { proc_root, euid: crate::client::euid(), owner: procs::owner }
    }

    fn ours(&self, dir: &Path) -> bool {
        (self.owner)(dir) == Some(self.euid)
    }
}

/// The ports one tree listens on (each at least [`MIN_PORT`]), and where.
pub type Found = BTreeMap<u16, Reach>;

/// Opens `path` read-only: not through a link, without blocking, and only
/// when it is a regular file.
fn open_regular(path: &Path) -> Option<File> {
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()?;
    let file = File::from(fd);
    file.metadata().ok()?.is_file().then_some(file)
}

/// At most `limit` bytes of `path`, read from the opened file.
fn read_head(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    open_regular(path)?.take(limit).read_to_end(&mut out).ok()?;
    Some(out)
}

fn read_stat(dir: &Path) -> Option<procs::Stat> {
    let bytes = read_head(&dir.join("stat"), MAX_STAT)?;
    procs::parse_stat(&String::from_utf8_lossy(&bytes))
}

/// The `/proc` entries that are processes, at most [`MAX_PROC_SCAN`].
fn pid_dirs(proc_root: &Path) -> Vec<(u32, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(proc_root) else { return Vec::new() };
    entries
        .flatten()
        .take(MAX_LISTED)
        .filter_map(|e| {
            let name = e.file_name();
            let name = name.to_str()?;
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some((name.parse().ok()?, e.path()))
        })
        .take(MAX_PROC_SCAN)
        .collect()
}

/// This user's processes by parent, each with its start time.
fn children(look: &Look) -> HashMap<u32, Vec<Root>> {
    let mut out: HashMap<u32, Vec<Root>> = HashMap::new();
    for (pid, dir) in pid_dirs(&look.proc_root) {
        if !look.ours(&dir) {
            continue;
        }
        let Some(stat) = read_stat(&dir) else { continue };
        out.entry(stat.ppid).or_default().push(Root { pid, start_ticks: stat.start_ticks });
    }
    for kids in out.values_mut() {
        kids.sort_by_key(|k| k.pid);
    }
    out
}

/// `root` and its descendants among `children`, `root` first, at most
/// [`MAX_TREE`]. A process that started before its parent is not its child:
/// its pid was another's.
fn tree(root: Root, children: &HashMap<u32, Vec<Root>>) -> Vec<Root> {
    let mut out = vec![root];
    let mut seen = HashSet::from([root.pid]);
    let mut at = 0;
    while at < out.len() && out.len() < MAX_TREE {
        let parent = out[at];
        for kid in children.get(&parent.pid).into_iter().flatten() {
            if out.len() == MAX_TREE {
                break;
            }
            if kid.start_ticks >= parent.start_ticks && seen.insert(kid.pid) {
                out.push(*kid);
            }
        }
        at += 1;
    }
    out
}

/// The inode of a descriptor's link target `socket:[<inode>]`.
fn socket_inode(target: &Path) -> Option<u64> {
    target.to_str()?.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok()
}

/// The sockets `proc` holds, from at most [`MAX_FDS`] descriptors; none when
/// its start time doesn't read the same afterwards (it was another process).
fn sockets(look: &Look, proc: Root) -> Vec<u64> {
    let dir = look.proc_root.join(proc.pid.to_string());
    let Ok(entries) = std::fs::read_dir(dir.join("fd")) else { return Vec::new() };
    let inodes: Vec<u64> = entries
        .flatten()
        .take(MAX_FDS)
        .filter_map(|e| std::fs::read_link(e.path()).ok())
        .filter_map(|target| socket_inode(&target))
        .collect();
    let same = read_stat(&dir).is_some_and(|s| s.start_ticks == proc.start_ticks);
    if same && look.ours(&dir) { inodes } else { Vec::new() }
}

/// A listening row of `net/tcp` or `net/tcp6`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Listening {
    port: u16,
    loopback: bool,
    uid: u32,
    inode: u64,
}

/// One 32-bit word of an address as the kernel prints it: hex, in this
/// machine's byte order.
fn word(hex: &str) -> Option<[u8; 4]> {
    (hex.len() == 8).then_some(())?;
    Some(u32::from_str_radix(hex, 16).ok()?.to_ne_bytes())
}

fn parse_addr(hex: &str) -> Option<IpAddr> {
    if !hex.is_ascii() {
        return None;
    }
    match hex.len() {
        8 => Some(IpAddr::V4(Ipv4Addr::from(word(hex)?))),
        32 => {
            let mut bytes = [0u8; 16];
            for (n, chunk) in bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                *chunk = word(&hex[n * 8..n * 8 + 8])?;
            }
            Some(IpAddr::V6(Ipv6Addr::from(bytes)))
        }
        _ => None,
    }
}

/// A row in `LISTEN`; the header and anything else is `None`.
fn parse_row(line: &str) -> Option<Listening> {
    let fields: Vec<&str> = line.split_whitespace().take(10).collect();
    if fields.len() < 10 || fields[3] != LISTEN {
        return None;
    }
    let (addr, port) = fields[1].split_once(':')?;
    let ip = parse_addr(addr)?;
    Some(Listening {
        port: u16::from_str_radix(port, 16).ok()?,
        loopback: ip.to_canonical().is_loopback(),
        uid: fields[7].parse().ok()?,
        inode: fields[9].parse().ok()?,
    })
}

/// The ports each of `roots`' trees listens on, in their order: `None` for
/// a root that is gone, was used again, or isn't this user's. One pass over
/// `/proc` for all of them.
pub fn scan(look: &Look, roots: &[Root]) -> Vec<Option<Found>> {
    let live: Vec<bool> = roots
        .iter()
        .map(|root| {
            let dir = look.proc_root.join(root.pid.to_string());
            look.ours(&dir) && read_stat(&dir).is_some_and(|s| s.start_ticks == root.start_ticks)
        })
        .collect();
    if !live.contains(&true) {
        return vec![None; roots.len()];
    }
    let children = children(look);
    let mut held: Vec<Option<HashSet<u64>>> = Vec::with_capacity(roots.len());
    for (root, live) in roots.iter().zip(&live) {
        if !live {
            held.push(None);
            continue;
        }
        let inodes = tree(*root, &children).into_iter().flat_map(|p| sockets(look, p)).collect();
        held.push(Some(inodes));
    }
    let mut rows = Vec::new();
    if held.iter().flatten().any(|inodes| !inodes.is_empty()) {
        for file in ["tcp", "tcp6"] {
            let Some(bytes) = read_head(&look.proc_root.join("net").join(file), MAX_NET_FILE) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            rows.extend(
                text.lines().filter_map(parse_row).filter(|r| r.uid == look.euid && r.port >= MIN_PORT),
            );
        }
    }
    held.into_iter()
        .map(|inodes| {
            let inodes = inodes?;
            let mut found = Found::new();
            for row in rows.iter().filter(|r| inodes.contains(&r.inode)) {
                let reach = if row.loopback { Reach::Pc } else { Reach::Lan };
                let kept = found.entry(row.port).or_insert(reach);
                if reach == Reach::Lan {
                    *kept = Reach::Lan;
                }
            }
            Some(found)
        })
        .collect()
}

/// `old` brought up to date with `found` at `now_ms`: a port still there
/// keeps when it was first seen, a new one is seen now; oldest first, then by
/// number, at most [`MAX_PORTS`].
pub fn merge(old: &[Port], found: &Found, now_ms: u64) -> Vec<Port> {
    let mut ports: Vec<Port> = found
        .iter()
        .map(|(&port, &reach)| Port {
            port,
            reach,
            since_ms: old.iter().find(|p| p.port == port).map_or(now_ms, |p| p.since_ms),
        })
        .collect();
    ports.sort_by_key(|p| (p.since_ms, p.port));
    ports.truncate(MAX_PORTS);
    ports
}

/// Whose process a root is: the thread's agent (its generation), or one of
/// its script runs' shells (phase 6, §X3.5).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Owner {
    Agent(u64),
    Run(String),
}

/// One thread whose agent's process, or a run's shell, runs.
#[derive(Debug, Clone)]
struct Track {
    roots: BTreeMap<Owner, Root>,
    /// What each root's last look found.
    found: BTreeMap<Owner, Found>,
    /// Looks are [`FAST_EVERY`] until then.
    fast_until: Option<Instant>,
    next: Instant,
}

impl Track {
    fn new(now: Instant) -> Track {
        Track { roots: BTreeMap::new(), found: BTreeMap::new(), fast_until: None, next: now + SLOW_EVERY }
    }

    /// The ports every root's last look found: a port two roots listen on
    /// is the LAN's when either's is.
    fn union(&self) -> Found {
        let mut all = Found::new();
        for (&port, &reach) in self.found.values().flatten() {
            let kept = all.entry(port).or_insert(reach);
            if reach == Reach::Lan {
                *kept = Reach::Lan;
            }
        }
        all
    }

    fn fast(&mut self, now: Instant) {
        self.fast_until = Some(now + FAST_FOR);
        self.next = self.next.min(now);
    }
}

/// The threads looked at, and when each is next.
#[derive(Debug, Default)]
pub struct Watch {
    tracks: HashMap<String, Track>,
}

impl Watch {
    /// `thread`'s agent process `generation` started as `root`: looked at
    /// every [`SLOW_EVERY`] from now, unless the thread is looked at already.
    /// An older process of the agent is replaced; what it found stands until
    /// this one's first look.
    pub fn started(&mut self, thread: &str, generation: u64, root: Root, now: Instant) {
        let track = self.tracks.entry(thread.to_owned()).or_insert_with(|| Track::new(now));
        let older: Vec<Owner> =
            track.roots.keys().filter(|o| matches!(o, Owner::Agent(_))).cloned().collect();
        let mut kept = None;
        for owner in older {
            track.roots.remove(&owner);
            kept = track.found.remove(&owner).or(kept);
        }
        track.roots.insert(Owner::Agent(generation), root);
        if let Some(found) = kept {
            track.found.insert(Owner::Agent(generation), found);
        }
    }

    /// A run's shell started as `root` in `thread`: as a command starting,
    /// looked at on the next tick, then every [`FAST_EVERY`] for [`FAST_FOR`].
    pub fn run_started(&mut self, thread: &str, run: &str, root: Root, now: Instant) {
        let track = self.tracks.entry(thread.to_owned()).or_insert_with(|| Track::new(now));
        track.roots.insert(Owner::Run(run.to_owned()), root);
        track.fast(now);
    }

    /// A command started in `thread`: looked at on the next tick, then every
    /// [`FAST_EVERY`] for [`FAST_FOR`].
    pub fn command(&mut self, thread: &str, now: Instant) {
        if let Some(track) = self.tracks.get_mut(thread) {
            track.fast(now);
        }
    }

    /// `owner`'s root in `thread` is gone: no more looks at it, and what it
    /// found goes. Whether it was looked at.
    pub fn gone(&mut self, thread: &str, owner: &Owner) -> bool {
        let Some(track) = self.tracks.get_mut(thread) else { return false };
        let ours = track.roots.remove(owner).is_some();
        track.found.remove(owner);
        if track.roots.is_empty() {
            self.tracks.remove(thread);
        }
        ours
    }

    /// `thread`'s agent process `generation` is gone. Whether it was looked at.
    pub fn ended(&mut self, thread: &str, generation: u64) -> bool {
        self.gone(thread, &Owner::Agent(generation))
    }

    /// A process of `thread`'s agent newer than `generation` is looked at.
    pub fn newer(&self, thread: &str, generation: u64) -> bool {
        self.tracks
            .get(thread)
            .is_some_and(|t| t.roots.keys().any(|o| matches!(o, Owner::Agent(g) if *g > generation)))
    }

    /// The threads due a look at `now`, each with its roots; the next look
    /// of each is set.
    pub fn due(&mut self, now: Instant) -> Vec<(String, Vec<(Owner, Root)>)> {
        let mut out = Vec::new();
        for (thread, track) in &mut self.tracks {
            if now < track.next {
                continue;
            }
            let fast = track.fast_until.is_some_and(|until| now < until);
            track.next = now + if fast { FAST_EVERY } else { SLOW_EVERY };
            out.push((thread.clone(), track.roots.iter().map(|(o, r)| (o.clone(), *r)).collect()));
        }
        out
    }

    /// What a look at `thread`'s roots found: kept per root, a root found
    /// gone (`None`) dropped. The ports of every root left; `None` when the
    /// thread has none left.
    pub fn found(&mut self, thread: &str, looks: Vec<(Owner, Option<Found>)>) -> Option<Found> {
        for (owner, found) in looks {
            match found {
                Some(found) => {
                    if let Some(track) = self.tracks.get_mut(thread).filter(|t| t.roots.contains_key(&owner))
                    {
                        track.found.insert(owner, found);
                    }
                }
                None => {
                    self.gone(thread, &owner);
                }
            }
        }
        self.ports(thread)
    }

    /// The ports of `thread`'s roots as last looked at; `None` when it has none.
    pub fn ports(&self, thread: &str) -> Option<Found> {
        self.tracks.get(thread).map(Track::union)
    }

    /// Whether `thread` is looked at (tests).
    #[cfg(test)]
    pub(crate) fn watches(&self, thread: &str) -> bool {
        self.tracks.contains_key(thread)
    }
}

impl Host {
    pub(crate) fn lock_ports(&self) -> std::sync::MutexGuard<'_, Watch> {
        self.ports.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Thread `thread_id`'s agent process `generation` started as `pid`: its
    /// ports are looked at while it runs. Call before the process is reaped.
    pub(crate) fn ports_started(&self, thread_id: &str, generation: u64, pid: Option<u32>) {
        let Some(root) = pid.and_then(|pid| Root::read(&self.config.proc_root, pid)) else { return };
        self.lock_ports().started(thread_id, generation, root, Instant::now());
    }

    /// A run's shell started as `pid` in `thread_id`: its ports join the
    /// thread's while it runs. Call before the shell can be reaped.
    pub(crate) fn ports_run_started(&self, thread_id: &str, run: &str, pid: u32) {
        let Some(root) = Root::read(&self.config.proc_root, pid) else { return };
        self.lock_ports().run_started(thread_id, run, root, Instant::now());
    }

    /// A run ended: its shell's ports leave the thread's.
    pub(crate) fn ports_run_ended(&self, thread_id: &str, run: &str) {
        let mut reg = self.lock_reg();
        let Some(entry) = reg.threads.get_mut(thread_id) else {
            self.lock_ports().gone(thread_id, &Owner::Run(run.to_owned()));
            return;
        };
        if self.ports_drop(entry, &Owner::Run(run.to_owned())) {
            self.hub.broadcast(&Event::ThreadState(ThreadResult { thread: entry.info.clone() }));
        }
    }

    /// The ports' tick: looks at the threads due, in one blocking pass.
    pub(crate) async fn ports_tick(self: &Arc<Self>) {
        let due = self.lock_ports().due(Instant::now());
        if due.is_empty() {
            return;
        }
        let look = Look::new(self.config.proc_root.clone());
        let roots: Vec<Root> =
            due.iter().flat_map(|(_, roots)| roots.iter().map(|(_, root)| *root)).collect();
        let Ok(found) = tokio::task::spawn_blocking(move || scan(&look, &roots)).await else { return };
        let mut found = found.into_iter();
        for (thread_id, roots) in due {
            let looks = roots.into_iter().map(|(owner, _)| (owner, found.next().flatten())).collect();
            self.ports_found(&thread_id, looks);
        }
    }

    /// What a look at `thread_id`'s roots found: its ports change, and
    /// `thread.state` says so. `None` for a root: its process is gone. A
    /// look at an agent's process that isn't the thread's current one counts
    /// as gone.
    pub(crate) fn ports_found(&self, thread_id: &str, looks: Vec<(Owner, Option<Found>)>) {
        let mut reg = self.lock_reg();
        let Some(entry) = reg.threads.get_mut(thread_id) else {
            let mut watch = self.lock_ports();
            for (owner, _) in looks {
                watch.gone(thread_id, &owner);
            }
            return;
        };
        let current = entry.session.as_ref().map(|s| s.generation);
        let looks = looks
            .into_iter()
            .map(|(owner, found)| match owner {
                Owner::Agent(g) if current != Some(g) => (owner, None),
                _ => (owner, found),
            })
            .collect();
        let union = self.lock_ports().found(thread_id, looks);
        let ports = match union {
            Some(union) => merge(&entry.info.ports, &union, now_ms()),
            None => Vec::new(),
        };
        if entry.info.ports != ports {
            entry.info.ports = ports;
            self.hub.broadcast(&Event::ThreadState(ThreadResult { thread: entry.info.clone() }));
        }
    }

    /// `entry`'s agent process `generation` is gone or was let go: no more
    /// looks at it, and its ports go, unless a newer process of the thread
    /// is looked at (its own looks keep them). A run's ports stay.
    pub(crate) fn ports_ended(&self, entry: &mut Entry, generation: u64) {
        if self.ports_drop(entry, &Owner::Agent(generation)) {
            self.hub.broadcast(&Event::ThreadState(ThreadResult { thread: entry.info.clone() }));
        }
    }

    /// `owner`'s root of `entry` is gone: its ports leave the thread's,
    /// which empty when no root is left. Whether they changed. Nothing
    /// changes while a newer process of the agent is looked at.
    pub(crate) fn ports_drop(&self, entry: &mut Entry, owner: &Owner) -> bool {
        let ports = {
            let mut watch = self.lock_ports();
            watch.gone(&entry.info.id, owner);
            if let Owner::Agent(generation) = owner
                && watch.newer(&entry.info.id, *generation)
            {
                return false;
            }
            watch.ports(&entry.info.id)
        };
        let ports = match ports {
            Some(left) => merge(&entry.info.ports, &left, now_ms()),
            None => Vec::new(),
        };
        if entry.info.ports == ports {
            return false;
        }
        entry.info.ports = ports;
        true
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const ME: u32 = 1000;
    const START: u64 = 5000;

    /// A fake `/proc`: processes, their descriptors, and `net/tcp(6)`, all
    /// this user's.
    pub(crate) struct FakeProc {
        root: PathBuf,
        euid: u32,
        _dir: Option<tempfile::TempDir>,
    }

    impl FakeProc {
        pub(crate) fn new() -> FakeProc {
            let dir = tempfile::tempdir().unwrap();
            FakeProc::at(dir.path(), ME).keeping(dir)
        }

        /// One at `root`, whose sockets are `euid`'s.
        pub(crate) fn at(root: &Path, euid: u32) -> FakeProc {
            std::fs::create_dir_all(root.join("net")).unwrap();
            let header = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";
            for file in ["tcp", "tcp6"] {
                std::fs::write(root.join("net").join(file), header).unwrap();
            }
            FakeProc { root: root.to_owned(), euid, _dir: None }
        }

        fn keeping(self, dir: tempfile::TempDir) -> FakeProc {
            FakeProc { _dir: Some(dir), ..self }
        }

        pub(crate) fn root(&self) -> &Path {
            &self.root
        }

        pub(crate) fn process(&self, pid: u32, ppid: u32, start: u64) {
            let dir = self.root().join(pid.to_string());
            std::fs::create_dir_all(dir.join("fd")).unwrap();
            let stat = format!(
                "{pid} (node) S {ppid} {pid} {pid} 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 {start} 1000 100 0"
            );
            std::fs::write(dir.join("stat"), stat).unwrap();
            let _ = std::os::unix::fs::symlink("/dev/null", dir.join("fd").join("0"));
        }

        pub(crate) fn socket(&self, pid: u32, fd: u32, inode: u64) {
            let link = self.root().join(pid.to_string()).join("fd").join(fd.to_string());
            std::os::unix::fs::symlink(format!("socket:[{inode}]"), link).unwrap();
        }

        /// A row of `net/tcp` (IPv4) or `net/tcp6`, as the kernel writes it.
        pub(crate) fn row(&self, ip: IpAddr, port: u16, state: &str, uid: u32, inode: u64) {
            let hex = |bytes: &[u8]| -> String {
                bytes.as_chunks::<4>().0.iter().map(|c| format!("{:08X}", u32::from_ne_bytes(*c))).collect()
            };
            let (file, addr, zero) = match ip {
                IpAddr::V4(v4) => ("tcp", hex(&v4.octets()), "00000000"),
                IpAddr::V6(v6) => ("tcp6", hex(&v6.octets()), "00000000000000000000000000000000"),
            };
            let line = format!(
                "   0: {addr}:{port:04X} {zero}:0000 {state} 00000000:00000000 00:00000000 00000000  {uid:>4}        0 {inode} 1 0000000000000000 100 0 0 10 0\n"
            );
            let path = self.root().join("net").join(file);
            let mut text = std::fs::read_to_string(&path).unwrap();
            text.push_str(&line);
            std::fs::write(path, text).unwrap();
        }

        pub(crate) fn listen(&self, ip: &str, port: u16, inode: u64) {
            self.row(ip.parse().unwrap(), port, LISTEN, self.euid, inode);
        }

        pub(crate) fn look(&self) -> Look {
            Look { proc_root: self.root().to_owned(), euid: self.euid, owner: |_| Some(ME) }
        }
    }

    fn found(entries: &[(u16, Reach)]) -> Option<Found> {
        Some(entries.iter().copied().collect())
    }

    fn agent() -> Root {
        Root { pid: 100, start_ticks: START }
    }

    #[test]
    fn a_kernel_line_reads_as_it_was_written() {
        let fake = FakeProc::new();
        fake.listen("127.0.0.1", 5173, 42);
        fake.listen("::", 8080, 43);
        let v4 = std::fs::read_to_string(fake.root().join("net/tcp")).unwrap();
        let row = v4.lines().find_map(parse_row).unwrap();
        assert_eq!(row, Listening { port: 5173, loopback: true, uid: ME, inode: 42 });
        let v6 = std::fs::read_to_string(fake.root().join("net/tcp6")).unwrap();
        assert_eq!(v6.lines().find_map(parse_row).unwrap().port, 8080);
        assert_eq!(parse_row(v4.lines().next().unwrap()), None, "the header");
        assert_eq!(parse_addr("0100007F\u{e9}"), None);
        assert_eq!(parse_addr("01000"), None);
        assert_eq!(socket_inode(Path::new("socket:[12345]")), Some(12345));
        assert_eq!(socket_inode(Path::new("pipe:[12345]")), None);
        assert_eq!(socket_inode(Path::new("/home/me/socket:[1]")), None);
    }

    #[cfg(target_endian = "little")]
    #[test]
    fn rows_copied_from_a_real_kernel() {
        let v4 = "   1: 0100007F:94DD 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 9561390 1 0000000000000000 100 0 0 10 0";
        assert_eq!(
            parse_row(v4),
            Some(Listening { port: 0x94DD, loopback: true, uid: 1000, inode: 9561390 })
        );
        let v6 = "   0: 00000000000000000000000001000000:1B9E 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 16923353 1 0000000000000000 100 0 0 10 0";
        assert_eq!(parse_row(v6), Some(Listening { port: 7070, loopback: true, uid: 0, inode: 16923353 }));
        let mapped = "   0: 0000000000000000FFFF00000100007F:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 7 1 0000000000000000 100 0 0 10 0";
        assert!(parse_row(mapped).unwrap().loopback, "::ffff:127.0.0.1");
        let wild = "   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 8 1 0000000000000000 100 0 0 10 0";
        assert!(!parse_row(wild).unwrap().loopback);
        let established = wild.replace(" 0A ", " 01 ");
        assert_eq!(parse_row(&established), None, "only listening sockets");
    }

    #[test]
    fn loopback_is_the_pc_and_every_address_is_the_lan() {
        let fake = FakeProc::new();
        fake.process(100, 1, START);
        fake.process(101, 100, START + 10);
        fake.process(102, 101, START + 20);
        fake.socket(100, 3, 10);
        fake.socket(101, 4, 11);
        fake.socket(102, 5, 12);
        fake.socket(102, 6, 13);
        fake.listen("127.0.0.1", 5173, 10);
        fake.listen("0.0.0.0", 3000, 11);
        fake.listen("::", 8080, 12);
        fake.listen("::1", 8080, 13);
        // The same port on loopback and on every address: the phone can open it.
        assert_eq!(
            scan(&fake.look(), &[agent()]),
            [found(&[(3000, Reach::Lan), (5173, Reach::Pc), (8080, Reach::Lan)])]
        );
        let only_v6_loopback = FakeProc::new();
        only_v6_loopback.process(100, 1, START);
        only_v6_loopback.socket(100, 3, 20);
        only_v6_loopback.listen("::1", 4321, 20);
        assert_eq!(scan(&only_v6_loopback.look(), &[agent()]), [found(&[(4321, Reach::Pc)])]);
    }

    #[test]
    fn only_this_trees_listening_sockets_count() {
        let fake = FakeProc::new();
        fake.process(100, 1, START);
        fake.process(101, 100, START + 10);
        // Another tree, and a pid that names the agent as its parent but
        // started before it: its parent was an earlier process.
        fake.process(200, 1, START);
        fake.process(300, 100, START - 1);
        fake.socket(101, 3, 10);
        fake.socket(101, 4, 11);
        fake.socket(200, 3, 20);
        fake.socket(300, 3, 30);
        fake.listen("127.0.0.1", 4000, 20);
        fake.listen("127.0.0.1", 4001, 30);
        fake.listen("127.0.0.1", 80, 10);
        // A connection the tree holds, not a listener.
        fake.row("0.0.0.0".parse().unwrap(), 4002, "01", ME, 11);
        // A listener nobody holds.
        fake.listen("0.0.0.0", 4003, 99);
        assert_eq!(scan(&fake.look(), &[agent()]), [found(&[])], "and 80 is under 1024");
    }

    #[test]
    fn another_users_process_and_socket_are_ignored() {
        let fake = FakeProc::new();
        fake.process(100, 1, START);
        fake.process(101, 100, START + 10);
        fake.process(102, 101, START + 20);
        fake.socket(101, 3, 10);
        fake.socket(102, 3, 11);
        fake.socket(100, 3, 12);
        fake.listen("0.0.0.0", 5000, 10);
        fake.listen("0.0.0.0", 5001, 11);
        fake.row("0.0.0.0".parse().unwrap(), 5002, LISTEN, 0, 12);
        let look = Look { owner: |dir| Some(if dir.ends_with("101") { 0 } else { ME }), ..fake.look() };
        assert_eq!(
            scan(&look, &[agent()]),
            [found(&[])],
            "101 is root's, so its socket and its child's go uncounted; 5002's socket is root's"
        );
        let all_mine = fake.look();
        assert_eq!(
            scan(&all_mine, &[agent()]),
            [found(&[(5000, Reach::Lan), (5001, Reach::Lan)])],
            "5002 still another user's socket"
        );
        let not_ours = Look { owner: |_| Some(0), ..fake.look() };
        assert_eq!(scan(&not_ours, &[agent()]), [None], "the agent itself isn't this user's");
    }

    #[test]
    fn a_reused_pid_counts_for_nothing() {
        let fake = FakeProc::new();
        fake.process(100, 1, START);
        fake.socket(100, 3, 10);
        fake.listen("0.0.0.0", 5173, 10);
        assert_eq!(scan(&fake.look(), &[agent()]), [found(&[(5173, Reach::Lan)])]);
        // The agent ended and a later process got its pid.
        fake.process(100, 1, START + 900);
        assert_eq!(scan(&fake.look(), &[agent()]), [None]);
        std::fs::remove_dir_all(fake.root().join("100")).unwrap();
        assert_eq!(scan(&fake.look(), &[agent()]), [None], "gone");
        assert_eq!(Root::read(fake.root(), 100), None);
        fake.process(100, 1, START);
        assert_eq!(Root::read(fake.root(), 100), Some(agent()));
    }

    #[test]
    fn several_agents_in_one_pass() {
        let fake = FakeProc::new();
        fake.process(100, 1, START);
        fake.process(200, 1, START);
        fake.socket(100, 3, 10);
        fake.socket(200, 3, 20);
        fake.listen("127.0.0.1", 5000, 10);
        fake.listen("0.0.0.0", 6000, 20);
        let other = Root { pid: 200, start_ticks: START };
        let gone = Root { pid: 300, start_ticks: START };
        assert_eq!(
            scan(&fake.look(), &[agent(), gone, other]),
            [found(&[(5000, Reach::Pc)]), None, found(&[(6000, Reach::Lan)])]
        );
    }

    #[test]
    fn a_tree_is_cut_at_64_processes() {
        let fake = FakeProc::new();
        // A chain: 100 → 101 → … → 169, each listening on its own port.
        for n in 0..70u32 {
            let pid = 100 + n;
            fake.process(pid, if n == 0 { 1 } else { pid - 1 }, START + u64::from(n));
            fake.socket(pid, 3, u64::from(pid));
            fake.listen("127.0.0.1", 10_000 + u16::try_from(n).unwrap(), u64::from(pid));
        }
        let found = scan(&fake.look(), &[agent()]).pop().unwrap().unwrap();
        assert_eq!(found.len(), MAX_TREE);
        assert_eq!(found.keys().last(), Some(&(10_000 + 63)));
    }

    #[test]
    fn a_net_file_is_read_to_1_mib() {
        let fake = FakeProc::new();
        fake.process(100, 1, START);
        fake.socket(100, 3, 10);
        let path = fake.root().join("net/tcp");
        let mut text = std::fs::read_to_string(&path).unwrap();
        let filler = "   0: 0100007F:0001 00000000:0000 01 00000000:00000000 00:00000000 00000000  1000        0 1 1 0000000000000000 100 0 0 10 0\n";
        while text.len() < usize::try_from(MAX_NET_FILE).unwrap() {
            text.push_str(filler);
        }
        std::fs::write(&path, text).unwrap();
        fake.listen("0.0.0.0", 5173, 10);
        assert_eq!(scan(&fake.look(), &[agent()]), [found(&[])], "past the first MiB");
        fake.listen("::", 5174, 10);
        assert_eq!(scan(&fake.look(), &[agent()]), [found(&[(5174, Reach::Lan)])], "tcp6 has its own MiB");
    }

    #[test]
    fn a_net_file_that_isnt_regular_is_not_read() {
        let fake = FakeProc::new();
        fake.process(100, 1, START);
        fake.socket(100, 3, 10);
        fake.listen("0.0.0.0", 5173, 10);
        let real = fake.root().join("net/tcp");
        let moved = fake.root().join("tcp-elsewhere");
        std::fs::rename(&real, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &real).unwrap();
        assert_eq!(scan(&fake.look(), &[agent()]), [found(&[])], "a link is not followed");
    }

    #[test]
    fn five_ports_keep_the_four_oldest() {
        let mut found = Found::new();
        for port in [3000, 3001, 3002, 3003, 3004] {
            found.insert(port, Reach::Lan);
        }
        let first = merge(&[], &found, 10);
        assert_eq!(first.iter().map(|p| p.port).collect::<Vec<_>>(), [3000, 3001, 3002, 3003]);
        assert!(first.iter().all(|p| p.since_ms == 10));
        // 3000 goes; a new one arrives; the rest keep when they were first seen.
        found.remove(&3000);
        found.insert(2000, Reach::Pc);
        let later = merge(&first, &found, 20);
        let ports: Vec<(u16, u64)> = later.iter().map(|p| (p.port, p.since_ms)).collect();
        assert_eq!(ports, [(3001, 10), (3002, 10), (3003, 10), (2000, 20)]);
        // A port that moves from loopback to every address keeps its time.
        found.insert(3001, Reach::Pc);
        let moved = merge(&later, &found, 30);
        assert_eq!(moved[0], Port { port: 3001, reach: Reach::Pc, since_ms: 10 });
        assert!(merge(&moved, &Found::new(), 40).is_empty());
    }

    #[test]
    fn looks_are_fast_for_a_minute_after_a_command_then_slow() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_secs(n);
        let mut watch = Watch::default();
        let thread = "a".repeat(32);
        watch.command(&thread, t0);
        assert!(watch.due(s(100)).is_empty(), "nothing started");
        watch.started(&thread, 7, agent(), t0);
        assert!(watch.due(s(29)).is_empty());
        assert_eq!(watch.due(s(30)), [(thread.clone(), vec![(Owner::Agent(7), agent())])]);
        assert!(watch.due(s(31)).is_empty());
        watch.command(&thread, s(31));
        assert_eq!(watch.due(s(31)).len(), 1, "at once");
        assert!(watch.due(s(35)).is_empty());
        assert_eq!(watch.due(s(36)).len(), 1);
        assert_eq!(watch.due(s(41)).len(), 1);
        assert_eq!(watch.due(s(90)).len(), 1, "still in the minute");
        assert_eq!(watch.due(s(95)).len(), 1, "the minute is up at 91: the next is 30 s on");
        assert!(watch.due(s(100)).is_empty());
        assert!(watch.due(s(124)).is_empty());
        assert_eq!(watch.due(s(125)).len(), 1);
        assert!(!watch.ended(&thread, 6), "another process");
        assert!(watch.ended(&thread, 7));
        assert!(watch.due(s(1000)).is_empty());
        assert!(!watch.ended(&thread, 7));
    }

    #[test]
    fn a_runs_shell_is_a_second_root_looked_at_fast() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_secs(n);
        let mut watch = Watch::default();
        let thread = "a".repeat(32);
        let shell = Root { pid: 200, start_ticks: START };
        watch.started(&thread, 7, agent(), t0);
        watch.run_started(&thread, "r1", shell, s(1));
        let due = watch.due(s(1));
        assert_eq!(
            due,
            [(thread.clone(), vec![(Owner::Agent(7), agent()), (Owner::Run("r1".into()), shell)])]
        );
        assert_eq!(watch.due(s(6)).len(), 1, "fast for a minute");
        // Each root keeps what it found; the thread's ports are both's, the LAN winning.
        let union = watch.found(
            &thread,
            vec![
                (Owner::Agent(7), Some(Found::from([(3000, Reach::Pc), (8080, Reach::Pc)]))),
                (Owner::Run("r1".into()), Some(Found::from([(3000, Reach::Lan), (5173, Reach::Pc)]))),
            ],
        );
        assert_eq!(union, Some(Found::from([(3000, Reach::Lan), (5173, Reach::Pc), (8080, Reach::Pc)])));
        // The agent's process ends: the run's ports stay, and so do the looks.
        assert!(watch.ended(&thread, 7));
        assert_eq!(watch.ports(&thread), Some(Found::from([(3000, Reach::Lan), (5173, Reach::Pc)])));
        assert!(watch.watches(&thread));
        // A look that finds the shell gone drops it; then nothing is left.
        assert_eq!(watch.found(&thread, vec![(Owner::Run("r1".into()), None)]), None);
        assert!(!watch.watches(&thread));
        // A newer agent takes over what the older one found until its own look.
        watch.started(&thread, 8, agent(), s(2));
        watch.found(&thread, vec![(Owner::Agent(8), Some(Found::from([(4000, Reach::Pc)])))]);
        watch.started(&thread, 9, agent(), s(3));
        assert_eq!(watch.ports(&thread), Some(Found::from([(4000, Reach::Pc)])));
        assert!(!watch.ended(&thread, 8) && watch.newer(&thread, 8));
    }

    #[test]
    fn ports_are_never_taken_up_again_from_the_store() {
        let port = Port { port: 5173, reach: Reach::Lan, since_ms: 1 };
        let info = ThreadInfo { ports: vec![port], ..crate::host::threads::test_info(AgentId::Claude) };
        let stored = serde_json::to_value(&info).unwrap();
        assert_eq!(stored["ports"], serde_json::json!([{"port": 5173, "reach": "lan", "since_ms": 1}]));
        let loaded = crate::host::db::LoadedThread {
            info: serde_json::from_value(stored).unwrap(),
            cols: crate::host::db::ThreadCols { next_seq: 1, next_turn: 1, ..Default::default() },
            prior_native_id: None,
            max_turn: 0,
            users: 0,
            plan: None,
        };
        let entry = crate::host::threads::Registry::entry_from(loaded, &[]);
        assert!(entry.info.ports.is_empty(), "its process is gone");
        // A reach a later host writes is left out, not an error.
        let mut later = serde_json::to_value(&info).unwrap();
        later["ports"] = serde_json::json!([{"port": 1, "reach": "tunnel", "since_ms": 1}, port]);
        let read: ThreadInfo = serde_json::from_value(later).unwrap();
        assert_eq!(read.ports, [port]);
    }

    fn port_states(lines: Vec<String>, thread_id: &str) -> Vec<Vec<Port>> {
        lines
            .iter()
            .filter_map(|line| {
                let ServerLine::Event(line) = ServerLine::parse(line).ok()? else { return None };
                match Event::from_line(&line)? {
                    Event::ThreadState(r) if r.thread.id == thread_id => Some(r.thread.ports),
                    _ => None,
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn a_threads_ports_ride_its_state_and_go_with_its_process() {
        let dir = tempfile::tempdir().unwrap();
        let host = crate::host::test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let client = host.hub.test_subscriber(crate::host::Caps::default());
        let (thread_id, generation, _cmds) = host.test_thread(AgentId::Claude);
        // The test host's `/proc`, whose folders are this user's.
        let fake = FakeProc::at(&host.config.proc_root, crate::client::euid());
        fake.process(100, 1, START);
        fake.socket(100, 3, 10);
        fake.listen("0.0.0.0", 5173, 10);

        host.ports_started(&thread_id, generation, Some(100));
        assert!(host.lock_ports().watches(&thread_id));
        host.ports_tick().await;
        assert!(port_states(host.hub.test_lines(client), &thread_id).is_empty(), "not due for 30 s");
        host.lock_ports().command(&thread_id, Instant::now());
        host.ports_tick().await;
        let states = port_states(host.hub.test_lines(client), &thread_id);
        let [ports] = states.as_slice() else { panic!("{states:?}") };
        assert_eq!(ports.iter().map(|p| (p.port, p.reach)).collect::<Vec<_>>(), [(5173, Reach::Lan)]);
        assert_eq!(&host.test_state(&thread_id).ports, ports);

        // Nothing changed: no state. The socket closed: the port goes.
        host.ports_found(
            &thread_id,
            vec![(Owner::Agent(generation), Some(Found::from([(5173, Reach::Lan)])))],
        );
        assert!(port_states(host.hub.test_lines(client), &thread_id).is_empty());
        host.ports_found(&thread_id, vec![(Owner::Agent(generation), Some(Found::new()))]);
        assert_eq!(port_states(host.hub.test_lines(client), &thread_id), [Vec::<Port>::new()]);

        // A look from an earlier process changes nothing and stops.
        host.ports_found(
            &thread_id,
            vec![(Owner::Agent(generation - 1), Some(Found::from([(4000, Reach::Pc)])))],
        );
        assert!(host.test_state(&thread_id).ports.is_empty());

        // The process ends: the ports go with it, and so do the looks.
        host.ports_found(
            &thread_id,
            vec![(Owner::Agent(generation), Some(Found::from([(8080, Reach::Pc)])))],
        );
        assert_eq!(port_states(host.hub.test_lines(client), &thread_id).len(), 1);
        host.on_agent_event(
            &thread_id,
            generation,
            crate::host::agents::AgentEvent::Exited { code: Some(0) },
        );
        let states = port_states(host.hub.test_lines(client), &thread_id);
        assert_eq!(states.last(), Some(&Vec::new()), "{states:?}");
        assert!(!host.lock_ports().watches(&thread_id));

        // An idle process the host lets go: its ports go when it ends, or
        // at its next look, whichever is first.
        let (idle, idle_generation, _cmds) = host.test_thread(AgentId::Claude);
        host.ports_started(&idle, idle_generation, Some(100));
        host.ports_found(
            &idle,
            vec![(Owner::Agent(idle_generation), Some(Found::from([(3000, Reach::Lan)])))],
        );
        assert_eq!(port_states(host.hub.test_lines(client), &idle).len(), 1);
        host.test_entry(&idle, |entry| entry.close_session(&mut std::collections::HashMap::new()));
        host.on_agent_event(
            &idle,
            idle_generation,
            crate::host::agents::AgentEvent::Exited { code: Some(0) },
        );
        assert_eq!(port_states(host.hub.test_lines(client), &idle), [Vec::<Port>::new()]);
        assert!(!host.lock_ports().watches(&idle));
        let (idle, idle_generation, _cmds) = host.test_thread(AgentId::Codex);
        host.ports_started(&idle, idle_generation, Some(100));
        host.ports_found(
            &idle,
            vec![(Owner::Agent(idle_generation), Some(Found::from([(3000, Reach::Lan)])))],
        );
        host.test_entry(&idle, |entry| entry.close_session(&mut std::collections::HashMap::new()));
        host.ports_found(
            &idle,
            vec![(Owner::Agent(idle_generation), Some(Found::from([(3000, Reach::Lan)])))],
        );
        assert_eq!(port_states(host.hub.test_lines(client), &idle).last(), Some(&Vec::new()));
        assert!(!host.lock_ports().watches(&idle));
    }

    #[tokio::test]
    async fn a_runs_port_joins_the_threads_outlives_the_agent_and_goes_with_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let host = crate::host::test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let client = host.hub.test_subscriber(crate::host::Caps::default());
        let (thread_id, generation, _cmds) = host.test_thread(AgentId::Claude);
        let fake = FakeProc::at(&host.config.proc_root, crate::client::euid());
        fake.process(100, 1, START);
        fake.socket(100, 3, 10);
        fake.listen("127.0.0.1", 8080, 10);
        // The run's shell, and the dev server it started.
        fake.process(200, 1, START);
        fake.process(201, 200, START + 1);
        fake.socket(201, 4, 20);
        fake.listen("0.0.0.0", 5173, 20);

        host.ports_started(&thread_id, generation, Some(100));
        host.ports_run_started(&thread_id, "0123456789abcdef", 200);
        host.ports_tick().await;
        let states = port_states(host.hub.test_lines(client), &thread_id);
        let ports: Vec<(u16, Reach)> = states.last().unwrap().iter().map(|p| (p.port, p.reach)).collect();
        assert_eq!(ports, [(5173, Reach::Lan), (8080, Reach::Pc)]);

        // The agent's process ends: the run's port stays.
        host.on_agent_event(
            &thread_id,
            generation,
            crate::host::agents::AgentEvent::Exited { code: Some(0) },
        );
        assert_eq!(host.test_state(&thread_id).ports.iter().map(|p| p.port).collect::<Vec<_>>(), [5173]);
        assert!(host.lock_ports().watches(&thread_id));
        // The run ends: the port goes, and so do the looks.
        host.ports_run_ended(&thread_id, "0123456789abcdef");
        assert!(host.test_state(&thread_id).ports.is_empty());
        assert_eq!(port_states(host.hub.test_lines(client), &thread_id).last(), Some(&Vec::new()));
        assert!(!host.lock_ports().watches(&thread_id));
    }
}
