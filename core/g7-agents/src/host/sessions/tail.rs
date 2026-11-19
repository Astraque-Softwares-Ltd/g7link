// SPDX-License-Identifier: MPL-2.0
//! Bounded reads of files other programs write: a window from the end, and a
//! follower that reads what was appended. Blocking; the host calls these
//! from `spawn_blocking`.
//!
//! A file is opened without following a link and without blocking (a FIFO
//! can't stall us), must be a regular file once open, and is read at most up
//! to a limit from the opened file, never to a size checked earlier.

use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rustix::fs::{Mode, OFlags};

use crate::api::{SESSION_READ_STEP, SESSION_TAIL, SESSION_TAIL_MAX};

#[cfg(any(test, feature = "fake-cli"))]
static OPENS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Files and folders the session reader has opened in this process (tests).
#[cfg(any(test, feature = "fake-cli"))]
pub fn opens() -> u64 {
    OPENS.load(std::sync::atomic::Ordering::SeqCst)
}

fn count_open() {
    #[cfg(any(test, feature = "fake-cli"))]
    OPENS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Opens `path` read-only: not through a link, without blocking, and only
/// when it is a regular file.
pub fn open_regular(path: &Path) -> io::Result<(File, Metadata)> {
    count_open();
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let file = File::from(fd);
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
    }
    Ok((file, meta))
}

/// At most `limit` bytes of `path` from its start.
pub fn read_head(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let (file, _) = open_regular(path)?;
    let mut out = Vec::new();
    file.take(limit).read_to_end(&mut out)?;
    Ok(out)
}

/// The entries of a folder, at most `max` of them, as names and paths.
pub fn list_dir(dir: &Path, max: usize) -> Vec<(String, PathBuf)> {
    count_open();
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries.flatten().take(max).filter_map(|e| Some((e.file_name().into_string().ok()?, e.path()))).collect()
}

/// A link's target (`/proc/<pid>/cwd`, `exe`).
pub fn read_link(path: &Path) -> Option<PathBuf> {
    count_open();
    std::fs::read_link(path).ok()
}

/// What identifies a file's content for the cache: a change in any of these
/// means it is read again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stamp {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime_ns: i128,
}

impl Stamp {
    pub fn of(meta: &Metadata) -> Stamp {
        Stamp {
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.size(),
            mtime_ns: i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()),
        }
    }

    pub fn mtime_ms(&self) -> u64 {
        u64::try_from(self.mtime_ns.max(0) / 1_000_000).unwrap_or(0)
    }

    pub fn same_file(&self, other: &Stamp) -> bool {
        self.dev == other.dev && self.ino == other.ino
    }
}

/// The stamp of `path` without opening it (not following a link).
pub fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    meta.is_file().then(|| Stamp::of(&meta))
}

/// One whole line: where its first byte is, and its bytes without `\n`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub offset: u64,
    pub bytes: Vec<u8>,
}

/// Whole lines read from a window of a file.
#[derive(Debug, Clone, Default)]
pub struct Window {
    pub lines: Vec<Line>,
    /// The first byte the window covers; lines before it weren't read.
    pub start: u64,
    /// Just past the last `\n` read: a follower continues here.
    pub end: u64,
    /// `end` is inside a line too long to keep: a follower skips to its end.
    pub in_line: bool,
    pub stamp: Stamp,
}

impl Window {
    /// Lines exist before the window.
    pub fn more_before(&self) -> bool {
        self.start > 0
    }
}

fn read_range(file: &mut File, from: u64, to: u64) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(from))?;
    let mut out = Vec::with_capacity(usize::try_from(to.saturating_sub(from)).unwrap_or(0).min(1 << 22));
    file.take(to.saturating_sub(from)).read_to_end(&mut out)?;
    Ok(out)
}

/// The whole lines in `buf`, which starts at file offset `base`. Bytes
/// before the first `\n` are dropped when `drop_first`. Returns the lines,
/// where they begin, and the offset just past the last `\n` (`None` when
/// there was none).
fn split_lines(buf: &[u8], base: u64, drop_first: bool) -> (Vec<Line>, u64, Option<u64>) {
    let mut lines = Vec::new();
    let mut pos = 0usize;
    let mut last = None;
    if drop_first {
        match buf.iter().position(|&b| b == b'\n') {
            Some(nl) => {
                pos = nl + 1;
                last = Some(base + pos as u64);
            }
            None => return (lines, base + buf.len() as u64, None),
        }
    }
    let begin = base + pos as u64;
    while let Some(nl) = buf[pos..].iter().position(|&b| b == b'\n') {
        let line = &buf[pos..pos + nl];
        if !line.iter().all(u8::is_ascii_whitespace) {
            lines.push(Line { offset: base + pos as u64, bytes: line.to_vec() });
        }
        pos += nl + 1;
        last = Some(base + pos as u64);
    }
    (lines, begin, last)
}

/// Reads the whole lines in the last [`SESSION_TAIL`] bytes before `upto`
/// (the end of the file when `None`), plus up to `lookahead` bytes after
/// `upto` so later lines can finish what the window started. The window is
/// doubled up to [`SESSION_TAIL_MAX`] while it holds no whole line before
/// `upto`; a line longer than that is skipped.
pub fn read_window(path: &Path, upto: Option<u64>, lookahead: u64) -> io::Result<Window> {
    let (mut file, meta) = open_regular(path)?;
    let stamp = Stamp::of(&meta);
    let end = upto.map_or(stamp.size, |u| u.min(stamp.size));
    let read_end = if upto.is_some() { end.saturating_add(lookahead).min(stamp.size) } else { end };
    let mut window = SESSION_TAIL;
    loop {
        let start = end.saturating_sub(window);
        // One byte early, so a window that starts right after a `\n` keeps its first line.
        let from = start.saturating_sub(1);
        let buf = read_range(&mut file, from, read_end)?;
        let (lines, begin, last) = split_lines(&buf, from, start > 0);
        if start == 0 || lines.iter().any(|l| l.offset < end) {
            return Ok(Window {
                lines,
                start: begin.min(end),
                end: last.unwrap_or(begin),
                in_line: false,
                stamp,
            });
        }
        if window >= SESSION_TAIL_MAX {
            // Inside one line longer than the maximum: it is skipped.
            return Ok(Window {
                lines: Vec::new(),
                start,
                end: last.unwrap_or(read_end),
                in_line: last.is_none(),
                stamp,
            });
        }
        window = (window * 2).min(SESSION_TAIL_MAX);
    }
}

/// What a follower found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    Unchanged,
    /// New whole lines (possibly none: a partial line is still being written).
    Lines(Vec<Line>),
    /// Replaced, truncated or rewritten: start again with a window read.
    Reset,
}

/// Follows a file someone appends to, from a line boundary.
#[derive(Debug, Clone)]
pub struct Follower {
    pub dev: u64,
    pub ino: u64,
    /// Just past the last `\n` consumed.
    pub offset: u64,
    /// Read so far (`offset` plus a partial line, or a skipped one).
    read_pos: u64,
    partial: Vec<u8>,
    skipping: bool,
}

impl Follower {
    /// Continues after a window read.
    pub fn after(window: &Window) -> Follower {
        Follower {
            dev: window.stamp.dev,
            ino: window.stamp.ino,
            offset: window.end,
            read_pos: window.end,
            partial: Vec::new(),
            skipping: window.in_line,
        }
    }

    /// Reads at most [`SESSION_READ_STEP`] new bytes and returns the whole lines.
    pub fn poll(&mut self, path: &Path) -> io::Result<Poll> {
        let Some(now) = stamp(path) else { return Ok(Poll::Reset) };
        if now.dev != self.dev || now.ino != self.ino || now.size < self.read_pos {
            return Ok(Poll::Reset);
        }
        if now.size == self.read_pos {
            return Ok(Poll::Unchanged);
        }
        let (mut file, meta) = open_regular(path)?;
        let opened = Stamp::of(&meta);
        if !opened.same_file(&now) || opened.size < self.read_pos {
            return Ok(Poll::Reset);
        }
        let to = opened.size.min(self.read_pos + SESSION_READ_STEP);
        let buf = read_range(&mut file, self.read_pos, to)?;
        let base = self.read_pos;
        self.read_pos += buf.len() as u64;
        let mut lines = Vec::new();
        let mut pos = 0usize;
        while let Some(nl) = buf[pos..].iter().position(|&b| b == b'\n') {
            if !self.skipping {
                self.partial.extend_from_slice(&buf[pos..pos + nl]);
                let bytes = std::mem::take(&mut self.partial);
                if !bytes.iter().all(u8::is_ascii_whitespace) {
                    lines.push(Line { offset: self.offset, bytes });
                }
            }
            self.partial.clear();
            self.skipping = false;
            pos += nl + 1;
            self.offset = base + pos as u64;
        }
        if !self.skipping {
            self.partial.extend_from_slice(&buf[pos..]);
            if self.partial.len() as u64 > SESSION_TAIL_MAX {
                self.partial = Vec::new();
                self.skipping = true;
            }
        }
        Ok(Poll::Lines(lines))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
    }

    fn append(path: &Path, bytes: &[u8]) {
        std::fs::OpenOptions::new().append(true).open(path).unwrap().write_all(bytes).unwrap();
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| String::from_utf8(l.bytes.clone()).unwrap()).collect()
    }

    #[test]
    fn a_small_file_is_read_whole_and_a_partial_last_line_stays_unread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        write(&path, b"one\ntwo\nthr");
        let w = read_window(&path, None, 0).unwrap();
        assert_eq!(texts(&w.lines), ["one", "two"]);
        assert_eq!((w.start, w.end), (0, 8));
        assert_eq!(w.lines[1].offset, 4);
        assert!(!w.more_before());
    }

    #[test]
    fn the_window_drops_the_partial_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        let mut body = Vec::new();
        let filler = "x".repeat(1000);
        for i in 0..400 {
            body.extend_from_slice(format!("{i:04}{filler}\n").as_bytes());
        }
        write(&path, &body);
        let w = read_window(&path, None, 0).unwrap();
        assert!(w.more_before());
        assert!(w.lines.iter().all(|l| l.bytes.len() == 1004), "no cut line");
        assert_eq!(w.lines.last().unwrap().bytes[..4], *b"0399");
        assert_eq!(w.lines[0].offset % 1005, 0, "starts on a line boundary");
        // A window that starts right after a newline keeps that line.
        let upto = 300 * 1005;
        let w2 = read_window(&path, Some(upto), 0).unwrap();
        assert_eq!(w2.lines.last().unwrap().offset, upto - 1005);
        assert!(w2.lines.iter().all(|l| l.offset < upto));
        // Lookahead reads past `upto`.
        let w3 = read_window(&path, Some(upto), 3000).unwrap();
        assert_eq!(w3.lines.last().unwrap().offset, upto + 1005, "the next line ends past the lookahead");
    }

    #[test]
    fn a_window_grows_for_a_long_line_and_skips_one_past_the_maximum() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        let mut body = b"first\n".to_vec();
        body.extend(std::iter::repeat_n(b'y', 600_000));
        body.push(b'\n');
        write(&path, &body);
        let w = read_window(&path, None, 0).unwrap();
        assert_eq!(w.lines.len(), 2, "doubled until the long line fit");
        assert_eq!(w.lines[1].bytes.len(), 600_000);

        let mut body = b"first\n".to_vec();
        body.extend(std::iter::repeat_n(b'z', 5 * 1024 * 1024));
        body.push(b'\n');
        write(&path, &body);
        let w = read_window(&path, None, 0).unwrap();
        assert!(w.lines.is_empty(), "a 5 MiB line is skipped, not an error");
        assert!(w.more_before());
    }

    #[test]
    fn a_follower_reads_appended_lines_in_steps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        write(&path, b"one\n");
        let w = read_window(&path, None, 0).unwrap();
        let mut f = Follower::after(&w);
        assert_eq!(f.poll(&path).unwrap(), Poll::Unchanged);
        append(&path, b"two\nthr");
        let Poll::Lines(lines) = f.poll(&path).unwrap() else { panic!() };
        assert_eq!(texts(&lines), ["two"]);
        assert_eq!(lines[0].offset, 4);
        append(&path, b"ee\n");
        let Poll::Lines(lines) = f.poll(&path).unwrap() else { panic!() };
        assert_eq!(texts(&lines), ["three"]);
        assert_eq!(lines[0].offset, 8);
        // More than one step's worth arrives over two polls.
        let big = "b".repeat(700_000);
        append(&path, format!("{big}\n{big}\n").as_bytes());
        let Poll::Lines(first) = f.poll(&path).unwrap() else { panic!() };
        assert_eq!(first.len(), 1);
        let Poll::Lines(second) = f.poll(&path).unwrap() else { panic!() };
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].offset, 14 + 700_001);
    }

    #[test]
    fn truncation_and_a_new_inode_reset_the_follower() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        write(&path, b"one\ntwo\n");
        let mut f = Follower::after(&read_window(&path, None, 0).unwrap());
        write(&path, b"x\n");
        assert_eq!(f.poll(&path).unwrap(), Poll::Reset, "shorter than what was read");
        let mut f = Follower::after(&read_window(&path, None, 0).unwrap());
        let other = dir.path().join("b.jsonl");
        write(&other, b"x\ny\nz\n");
        std::fs::rename(&other, &path).unwrap();
        assert_eq!(f.poll(&path).unwrap(), Poll::Reset, "another file now");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(f.poll(&path).unwrap(), Poll::Reset, "gone");
    }

    #[test]
    fn a_follower_skips_a_line_past_the_maximum() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        write(&path, b"");
        let mut f = Follower::after(&read_window(&path, None, 0).unwrap());
        let huge = vec![b'h'; 5 * 1024 * 1024];
        append(&path, &huge);
        append(&path, b"\nafter\n");
        let mut got = Vec::new();
        for _ in 0..8 {
            if let Poll::Lines(lines) = f.poll(&path).unwrap() {
                got.extend(lines);
            }
        }
        assert_eq!(texts(&got), ["after"]);
        assert_eq!(got[0].offset, 5 * 1024 * 1024 + 1);
    }

    #[test]
    fn only_regular_files_open() {
        let dir = tempfile::tempdir().unwrap();
        assert!(open_regular(dir.path()).is_err(), "a folder");
        let target = dir.path().join("t.jsonl");
        write(&target, b"x\n");
        let link = dir.path().join("l.jsonl");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(open_regular(&link).is_err(), "a link isn't followed");
        assert!(stamp(&link).is_none());
        assert!(read_window(&link, None, 0).is_err());
        assert_eq!(read_head(&target, 1).unwrap(), b"x");
    }
}
