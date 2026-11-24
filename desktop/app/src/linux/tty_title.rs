// SPDX-License-Identifier: GPL-3.0-or-later
//! A one-time title on a terminal, for a phone's reply into a session
//! (`agents::reply`, `plans/reply-tabs-idle.md` §2): written to the
//! session's own terminal device, the output side, which the terminal draws
//! and the program on it never reads. The old title is pushed first and
//! popped after (XTWINOPS 22 and 23). Nothing here reads a terminal, and the
//! title is never logged.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};

/// The Unix98 pty slaves' major: `/dev/pts/N`.
const PTS_MAJOR: u32 = 136;
/// Most bytes in a title.
const MAX_TITLE: usize = 48;
/// Push the window and icon titles (`CSI 22 ; 0 t`), and pop them (`CSI 23 ; 0 t`).
const PUSH: &[u8] = b"\x1b[22;0t";
const POP: &[u8] = b"\x1b[23;0t";

/// Sets the title of the terminal `tty_nr` (`/proc/<pid>/stat` field 7),
/// pushing the old one. False when it isn't a pty of this user, the title
/// isn't 1 to 48 letters, digits and spaces, or the write didn't go whole.
pub fn mark(tty_nr: u32, title: &str) -> bool {
    if !allowed(title) {
        return false;
    }
    let Some(mut pts) = open(tty_nr) else { return false };
    write_once(&mut pts, &sequence(title))
}

/// Pops the title pushed by [`mark`].
pub fn unmark(tty_nr: u32) -> bool {
    let Some(mut pts) = open(tty_nr) else { return false };
    write_once(&mut pts, POP)
}

/// A `tty_nr`'s major and minor (the kernel's `new_encode_dev`).
fn decode(tty_nr: u32) -> (u32, u32) {
    ((tty_nr >> 8) & 0xfff, (tty_nr & 0xff) | ((tty_nr >> 12) & 0xfff00))
}

/// The pty slave `tty_nr` names; `None` for any other device.
fn pts_minor(tty_nr: u32) -> Option<u32> {
    let (major, minor) = decode(tty_nr);
    (major == PTS_MAJOR).then_some(minor)
}

fn allowed(title: &str) -> bool {
    (1..=MAX_TITLE).contains(&title.len()) && title.bytes().all(|b| b.is_ascii_alphanumeric() || b == b' ')
}

/// Push, then set (`OSC 0 ; title BEL`), in one write.
fn sequence(title: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(PUSH.len() + title.len() + 5);
    bytes.extend_from_slice(PUSH);
    bytes.extend_from_slice(b"\x1b]0;");
    bytes.extend_from_slice(title.as_bytes());
    bytes.push(0x07);
    bytes
}

/// The pty slave, opened for writing only: never as a controlling terminal,
/// never blocking, never through a link (std adds `O_CLOEXEC`). It must be
/// that very device and this user's.
fn open(tty_nr: u32) -> Option<File> {
    let minor = pts_minor(tty_nr)?;
    let pts = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(format!("/dev/pts/{minor}"))
        .ok()?;
    let meta = pts.metadata().ok()?;
    // SAFETY: geteuid takes no arguments and cannot fail.
    let euid = unsafe { libc::geteuid() };
    (meta.file_type().is_char_device()
        && meta.rdev() == libc::makedev(PTS_MAJOR, minor)
        && meta.uid() == euid)
        .then_some(pts)
}

/// One write, whole or nothing: a short write or a full terminal (`EAGAIN`)
/// is false, and isn't tried again.
fn write_once(pts: &mut File, bytes: &[u8]) -> bool {
    matches!(pts.write(bytes), Ok(n) if n == bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tty_nr_names_its_pty() {
        assert_eq!(decode(34816), (136, 0));
        assert_eq!(pts_minor(34816), Some(0));
        assert_eq!(pts_minor(34817), Some(1));
        // `/dev/pts/300`: the minor's high bits sit above the major.
        let tty_nr = 0x2c | (136 << 8) | (0x100 << 12);
        assert_eq!(decode(tty_nr), (136, 300));
        // A virtual console (4, 1) and a serial line (4, 64) aren't ptys.
        assert_eq!(pts_minor(0x401), None);
        assert_eq!(pts_minor(0x440), None);
        assert_eq!(pts_minor(0), None);
    }

    #[test]
    fn only_a_pty_is_written_to() {
        // Refused before anything is opened.
        assert!(!mark(0x401, "G7 0123456789abcdef"));
        assert!(!unmark(0x401));
        assert!(!mark(0, "G7 0123456789abcdef"));
    }

    #[test]
    fn a_title_is_letters_digits_and_spaces() {
        assert!(allowed("G7 0123456789abcdef"));
        assert!(allowed(&"a".repeat(48)));
        for bad in ["", "G7\x07", "G7\x1b]0;x", "G7;1", "G7\n", "G7 é", &"a".repeat(49)] {
            assert!(!allowed(bad), "{bad:?}");
            // Refused before the device is looked at, even for a real pty number.
            assert!(!mark(34816, bad), "{bad:?}");
        }
    }

    #[test]
    fn the_title_is_pushed_then_set_in_one_sequence() {
        assert_eq!(sequence("G7 0123456789abcdef"), b"\x1b[22;0t\x1b]0;G7 0123456789abcdef\x07".to_vec());
        assert_eq!(POP, b"\x1b[23;0t");
    }
}
