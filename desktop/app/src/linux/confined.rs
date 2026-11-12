// SPDX-License-Identifier: GPL-3.0-or-later
//! Writing inside a folder without leaving it. Each folder below the root is
//! opened relative to the one above it and never through a symbolic link, so a
//! link inside the folder can't send a write somewhere else.

use std::{
    ffi::CString,
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::OpenOptionsExt,
    },
    path::Path,
};

const DIR_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

/// A folder opened under a root, without following a link on the way.
#[derive(Debug)]
pub struct ConfinedDir {
    fd: OwnedFd,
}

impl ConfinedDir {
    /// Opens the folder `dirs` names below `root`, outermost first, making each
    /// missing one when `create` is set. `root` may itself be a link, since the
    /// user chose it; nothing below it may be.
    pub fn open(root: &Path, dirs: &[&str], create: bool) -> io::Result<Self> {
        let root = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY).open(root)?;
        let mut dir = Self { fd: OwnedFd::from(root) };
        for name in dirs {
            dir = dir.child(name, create)?;
        }
        Ok(dir)
    }

    fn child(&self, name: &str, create: bool) -> io::Result<Self> {
        let name = plain_name(name)?;
        if create {
            // SAFETY: `name` is a NUL-terminated string that outlives the call.
            let made = unsafe { libc::mkdirat(self.fd.as_raw_fd(), name.as_ptr(), 0o777) };
            if made < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
        }
        // SAFETY: `name` is a NUL-terminated string that outlives the call.
        let fd = unsafe { libc::openat(self.fd.as_raw_fd(), name.as_ptr(), DIR_FLAGS) };
        Ok(Self { fd: owned(fd)? })
    }

    /// Creates `name` here to read and write. Fails when anything already has
    /// the name, a link included.
    pub fn create_new(&self, name: &str) -> io::Result<File> {
        let name = plain_name(name)?;
        let flags = libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let mode: libc::c_uint = 0o666;
        // SAFETY: `name` is a NUL-terminated string that outlives the call.
        let fd = unsafe { libc::openat(self.fd.as_raw_fd(), name.as_ptr(), flags, mode) };
        Ok(File::from(owned(fd)?))
    }

    /// Removes the file `name` from here. A link is removed itself, not what it
    /// points to.
    pub fn remove_file(&self, name: &str) -> io::Result<()> {
        let name = plain_name(name)?;
        // SAFETY: `name` is a NUL-terminated string that outlives the call.
        let removed = unsafe { libc::unlinkat(self.fd.as_raw_fd(), name.as_ptr(), 0) };
        if removed < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    /// Renames `from` to `to`, both in this folder, replacing what `to` named.
    pub fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        let (from, to) = (plain_name(from)?, plain_name(to)?);
        let fd = self.fd.as_raw_fd();
        // SAFETY: `from` and `to` are NUL-terminated strings that outlive the call.
        let renamed = unsafe { libc::renameat(fd, from.as_ptr(), fd, to.as_ptr()) };
        if renamed < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }
}

/// `name` for a call, if it names something in a folder rather than a path.
fn plain_name(name: &str) -> io::Result<CString> {
    let refused = || io::Error::new(io::ErrorKind::InvalidInput, "not a plain file name");
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(refused());
    }
    CString::new(name).map_err(|_| refused())
}

fn owned(fd: libc::c_int) -> io::Result<OwnedFd> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a new descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_below_the_root_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("work")).unwrap();

        assert!(ConfinedDir::open(root.path(), &["work"], false).is_err());
        assert!(ConfinedDir::open(root.path(), &["work", "new"], true).is_err());
        assert!(ConfinedDir::open(root.path(), &[".."], false).is_err());
        assert!(ConfinedDir::open(root.path(), &["a/b"], true).is_err());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);

        let dir = ConfinedDir::open(root.path(), &["made", "here"], true).unwrap();
        drop(dir.create_new("x").unwrap());
        assert!(root.path().join("made/here/x").is_file());
        assert!(dir.create_new("x").is_err());
    }

    #[test]
    fn a_linked_root_is_followed() {
        let real = tempfile::tempdir().unwrap();
        let links = tempfile::tempdir().unwrap();
        let root = links.path().join("root");
        std::os::unix::fs::symlink(real.path(), &root).unwrap();

        let dir = ConfinedDir::open(&root, &[], false).unwrap();
        drop(dir.create_new("x").unwrap());
        dir.rename("x", "y").unwrap();
        assert!(real.path().join("y").is_file());
        dir.remove_file("y").unwrap();
        assert!(!real.path().join("y").exists());
    }
}
