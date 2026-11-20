// SPDX-License-Identifier: MPL-2.0
//! Keep-awake (§Q4.10): while at least one thread works (not waiting) and
//! the setting is on, the host holds one logind inhibitor (`sleep:idle`,
//! mode `block`). It is released [`api::KEEP_AWAKE_RELEASE_MS`] after the
//! last thread stopped working, at once when the setting is turned off, and
//! by its descriptor closing if the host dies. Lid-close handling is
//! logind's own and is not touched.

use std::collections::HashSet;
use std::time::Duration;

use tokio::sync::mpsc;
use zbus::zvariant::OwnedFd;

use crate::api;

pub const WHAT: &str = "sleep:idle";
/// Asked for when `sleep:idle` is refused (§Q12 #14).
pub const WHAT_FALLBACK: &str = "idle";
pub const WHO: &str = "G7 Agents";
pub const WHY: &str = "An agent is working";
pub const MODE: &str = "block";

const LOGIND: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const LOGIND_MANAGER: &str = "org.freedesktop.login1.Manager";

/// Where the inhibitor comes from.
#[derive(Debug, Clone)]
pub enum Source {
    /// logind on the system bus.
    System,
    /// A bus at this address (tests: a private bus with a stand-in logind).
    Address(String),
    /// Never taken (hosts in tests that don't test this).
    Off,
}

#[derive(Debug)]
enum Msg {
    /// A thread works (`true`) or not.
    Working(String, bool),
    Enabled(bool),
}

/// The keep-awake holder. Cheap to clone; the inhibitor lives on its task,
/// which ends (and lets go of it) when every handle is dropped.
#[derive(Debug, Clone)]
pub struct Awake {
    tx: mpsc::UnboundedSender<Msg>,
}

impl Awake {
    /// Starts the holder: `enabled` is the setting, `release_after` the
    /// delay after the last thread stopped working.
    pub fn spawn(source: Source, enabled: bool, release_after: Duration) -> Awake {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run(rx, source, enabled, release_after));
        Awake { tx }
    }

    /// The thread `thread_id` works now, or stopped working (waiting,
    /// finished, removed). Never blocks.
    pub fn working(&self, thread_id: &str, working: bool) {
        let _ = self.tx.send(Msg::Working(thread_id.to_owned(), working));
    }

    /// The setting changed.
    pub fn set_enabled(&self, enabled: bool) {
        let _ = self.tx.send(Msg::Enabled(enabled));
    }
}

struct Holder {
    source: Source,
    conn: Option<zbus::Connection>,
    held: Option<OwnedFd>,
    /// Tried once in this working period (a failure is not retried until
    /// no thread works and one starts again).
    tried: bool,
    failure_logged: bool,
}

impl Holder {
    async fn connect(&mut self) -> zbus::Result<zbus::Connection> {
        if let Some(conn) = &self.conn {
            return Ok(conn.clone());
        }
        let conn = match &self.source {
            Source::System => zbus::Connection::system().await?,
            Source::Address(address) => zbus::connection::Builder::address(address.as_str())?.build().await?,
            Source::Off => return Err(zbus::Error::Failure("keep-awake is off in this host".into())),
        };
        self.conn = Some(conn.clone());
        Ok(conn)
    }

    async fn take(&mut self) {
        if self.held.is_some() || self.tried {
            return;
        }
        self.tried = true;
        let mut taken = self.inhibit(WHAT).await;
        if let Err(zbus::Error::MethodError(..) | zbus::Error::FDO(_)) = &taken {
            taken = self.inhibit(WHAT_FALLBACK).await;
        }
        match taken {
            Ok(fd) => {
                tracing::debug!("keep-awake: inhibitor taken");
                self.held = Some(fd);
            }
            Err(e) => {
                if !self.failure_logged {
                    self.failure_logged = true;
                    tracing::info!("keep-awake: logind refused the inhibitor: {e}");
                }
            }
        }
    }

    async fn inhibit(&mut self, what: &str) -> zbus::Result<OwnedFd> {
        let conn = self.connect().await?;
        let reply = conn
            .call_method(Some(LOGIND), LOGIND_PATH, Some(LOGIND_MANAGER), "Inhibit", &(what, WHO, WHY, MODE))
            .await?;
        reply.body().deserialize::<OwnedFd>()
    }

    fn release(&mut self) {
        if self.held.take().is_some() {
            tracing::debug!("keep-awake: inhibitor released");
        }
    }
}

async fn run(
    mut rx: mpsc::UnboundedReceiver<Msg>,
    source: Source,
    mut enabled: bool,
    release_after: Duration,
) {
    let mut holder = Holder { source, conn: None, held: None, tried: false, failure_logged: false };
    let mut working: HashSet<String> = HashSet::new();
    let mut release_at: Option<tokio::time::Instant> = None;
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(Msg::Working(id, on)) => {
                    if on {
                        working.insert(id);
                    } else {
                        working.remove(&id);
                    }
                }
                Some(Msg::Enabled(on)) => enabled = on,
                None => break,
            },
            () = sleep_until(release_at), if release_at.is_some() => {
                release_at = None;
                holder.release();
            }
        }
        if enabled && !working.is_empty() {
            release_at = None;
            holder.take().await;
        } else {
            holder.tried = false;
            if !enabled {
                release_at = None;
                holder.release();
            } else if holder.held.is_some() && release_at.is_none() {
                release_at = Some(tokio::time::Instant::now() + release_after);
            }
        }
    }
    holder.release();
}

async fn sleep_until(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// The release delay hosts use.
pub fn release_delay() -> Duration {
    Duration::from_millis(api::KEEP_AWAKE_RELEASE_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::{Arc, Mutex};

    /// One `Inhibit` call's arguments.
    type Call = (String, String, String, String);

    /// A stand-in logind: records each `Inhibit` and hands out the write end
    /// of a pipe, whose read end sees EOF once the holder lets go.
    struct FakeLogind {
        calls: Arc<Mutex<Vec<Call>>>,
        released: Arc<Mutex<Vec<tokio::sync::oneshot::Receiver<()>>>>,
        /// Refuse, as logind does without the right to inhibit.
        refuse: Arc<std::sync::atomic::AtomicBool>,
    }

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl FakeLogind {
        fn inhibit(
            &self,
            what: String,
            who: String,
            why: String,
            mode: String,
        ) -> zbus::fdo::Result<OwnedFd> {
            self.calls.lock().unwrap().push((what, who, why, mode));
            if self.refuse.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(zbus::fdo::Error::AccessDenied("not allowed".into()));
            }
            let (mut reader, writer) =
                std::io::pipe().map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
            let (tx, rx) = tokio::sync::oneshot::channel();
            std::thread::spawn(move || {
                let mut buf = [0u8; 1];
                let _ = reader.read(&mut buf);
                let _ = tx.send(());
            });
            self.released.lock().unwrap().push(rx);
            Ok(OwnedFd::from(std::os::fd::OwnedFd::from(writer)))
        }
    }

    /// The private bus's daemon, stopped when the test ends (passing or not)
    /// so it never outlives the run.
    struct Bus(std::process::Child);

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// A private bus (the stock session configuration at our own address),
    /// or `None` when this system has no `dbus-daemon`.
    fn private_bus(dir: &std::path::Path) -> Option<(Bus, String)> {
        let daemon = ["/usr/bin/dbus-daemon", "/bin/dbus-daemon"]
            .into_iter()
            .find(|p| std::path::Path::new(p).exists())?;
        let socket = dir.join("bus");
        let address = format!("unix:path={}", socket.display());
        let child = std::process::Command::new(daemon)
            .args(["--session", "--nofork", &format!("--address={address}")])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let bus = Bus(child);
        for _ in 0..100 {
            if socket.exists() {
                return Some((bus, address));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    async fn settle() {
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn held_while_working_released_after_the_delay_and_when_turned_off() {
        let dir = tempfile::tempdir().unwrap();
        let Some((daemon, address)) = private_bus(dir.path()) else {
            eprintln!("no dbus-daemon here: keep-awake against a private bus not run");
            return;
        };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let released = Arc::new(Mutex::new(Vec::new()));
        let refuse = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fake = FakeLogind { calls: calls.clone(), released: released.clone(), refuse: refuse.clone() };
        let _logind = tokio::time::timeout(
            Duration::from_secs(10),
            zbus::connection::Builder::address(address.as_str())
                .unwrap()
                .name(LOGIND)
                .unwrap()
                .serve_at(LOGIND_PATH, fake)
                .unwrap()
                .build(),
        )
        .await
        .expect("the stand-in logind connects")
        .unwrap();
        let awake = Awake::spawn(Source::Address(address.clone()), true, Duration::from_millis(400));
        let next_release = || released.lock().unwrap().pop().expect("an inhibitor was taken");

        // Waiting doesn't take it; working does, once for two threads.
        awake.working("a", false);
        settle().await;
        assert!(calls.lock().unwrap().is_empty(), "not while nothing works");
        awake.working("a", true);
        awake.working("b", true);
        settle().await;
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [(WHAT.to_owned(), WHO.to_owned(), WHY.to_owned(), MODE.to_owned())]
        );
        let mut first = next_release();
        // One thread waits on the user: still held for the other.
        awake.working("a", false);
        settle().await;
        assert!(first.try_recv().is_err(), "still held while b works");
        // The last one stops: held for the delay, then let go.
        awake.working("b", false);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(first.try_recv().is_err(), "held during the delay");
        tokio::time::timeout(Duration::from_secs(5), first).await.unwrap().unwrap();

        // Working again takes a new one; turning the setting off lets go at once.
        awake.working("c", true);
        settle().await;
        assert_eq!(calls.lock().unwrap().len(), 2);
        let second = next_release();
        awake.set_enabled(false);
        tokio::time::timeout(Duration::from_millis(300), second).await.unwrap().unwrap();
        awake.working("d", true);
        settle().await;
        assert_eq!(calls.lock().unwrap().len(), 2, "not while the setting is off");

        // Refused: `sleep:idle`, then `idle`, once while threads work, and
        // again only in a later period.
        refuse.store(true, std::sync::atomic::Ordering::SeqCst);
        awake.set_enabled(true);
        awake.working("e", true);
        awake.working("f", true);
        settle().await;
        let whats: Vec<String> = calls.lock().unwrap()[2..].iter().map(|c| c.0.clone()).collect();
        assert_eq!(whats, [WHAT, WHAT_FALLBACK], "one try for this period");
        for id in ["c", "d", "e", "f"] {
            awake.working(id, false);
        }
        awake.working("g", true);
        settle().await;
        assert_eq!(calls.lock().unwrap().len(), 6, "a new period tries again");
        drop(awake);
        drop(daemon);
    }
}
