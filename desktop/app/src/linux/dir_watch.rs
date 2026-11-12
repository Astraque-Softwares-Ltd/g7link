// SPDX-License-Identifier: GPL-3.0-or-later
//! Watching folders for files that are written, moved or deleted (inotify).

use std::{
    collections::HashSet,
    ffi::CString,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
    time::Duration,
};

/// How often folders that don't exist yet are looked for again (ms).
const RESCAN_MS: i32 = 5000;
/// A file is often written in several steps: wait for them to settle.
const SETTLE: Duration = Duration::from_millis(100);
/// Files written, moved in or out, made or deleted.
const EVENTS: u32 =
    libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_MOVED_FROM | libc::IN_CREATE | libc::IN_DELETE;

/// Calls `on_change`, on a thread of its own, whenever a file in one of
/// `dirs()` changes or one of those folders appears. The folders are asked
/// for again every few seconds, so ones made later are watched too. Runs for
/// the life of the app.
pub fn watch(
    name: &str,
    dirs: impl Fn() -> Vec<PathBuf> + Send + 'static,
    on_change: impl Fn() + Send + 'static,
) {
    let spawned = std::thread::Builder::new().name(name.into()).spawn(move || run(&dirs, &on_change));
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "can't start watching folders");
    }
}

fn run(dirs: &dyn Fn() -> Vec<PathBuf>, on_change: &dyn Fn()) {
    // SAFETY: `inotify_init1` takes only flags.
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
    if fd < 0 {
        tracing::warn!(error = %std::io::Error::last_os_error(), "can't watch folders");
        return;
    }
    // SAFETY: `fd` is a new descriptor that nothing else owns.
    let inotify = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut watched = HashSet::new();
    let mut buf = [0u8; 4096];
    loop {
        let appeared = add_watches(&inotify, &dirs(), &mut watched);
        let mut pfd = [libc::pollfd { fd: inotify.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        // SAFETY: `pfd` is a valid one-element pollfd array.
        let ready = unsafe { libc::poll(pfd.as_mut_ptr(), 1, RESCAN_MS) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            tracing::warn!(%error, "stopped watching folders");
            return;
        }
        if ready > 0 {
            std::thread::sleep(SETTLE);
            drain(&inotify, &mut buf);
        }
        if ready > 0 || appeared {
            on_change();
        }
    }
}

/// Watches each folder that exists; whether one wasn't watched before.
fn add_watches(inotify: &OwnedFd, dirs: &[PathBuf], watched: &mut HashSet<i32>) -> bool {
    let now: HashSet<i32> = dirs.iter().filter_map(|dir| add_watch(inotify, dir)).collect();
    let appeared = !now.is_subset(watched);
    *watched = now;
    appeared
}

fn add_watch(inotify: &OwnedFd, dir: &Path) -> Option<i32> {
    let path = CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is a NUL-terminated string that outlives the call.
    let wd = unsafe { libc::inotify_add_watch(inotify.as_raw_fd(), path.as_ptr(), EVENTS) };
    (wd >= 0).then_some(wd)
}

/// Reads the pending events: which file changed doesn't matter, only that one did.
fn drain(inotify: &OwnedFd, buf: &mut [u8]) {
    loop {
        // SAFETY: `buf` is a live, writable buffer of `buf.len()` bytes.
        let n = unsafe { libc::read(inotify.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            break;
        }
    }
}
