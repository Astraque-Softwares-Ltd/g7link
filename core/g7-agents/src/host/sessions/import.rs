// SPDX-License-Identifier: MPL-2.0
//! Bringing sessions in (§U4.2), and looking at one first (`session.preview`).
//!
//! A session is read from its first line through the line mappers live
//! following uses, decompressing `.zst` rollouts as they stream, at most
//! [`zst::MAX_DECODED`] bytes per file and [`MAX_LINES`] lines in all (past
//! that the oldest lines are skipped unread). Only the newest
//! [`IMPORT_KEPT_MESSAGES`] messages, the tool steps between them and the
//! first prompt are kept in memory.
//!
//! A Codex thread can be several rollout files. Each later file's
//! `session_meta.history_base` names the file its earlier history is in
//! (a part's own UUID, or the thread's for the first part) and how many
//! bytes of it, decompressed: on the spike's laptop every link pointed at
//! the part before, cut where that part went on differently. So a thread is
//! read from its newest part back through those links, never by joining
//! parts: the lines a later part replaced are left out. A fork's history is
//! in another thread's file, named the same way. At most [`MAX_BASES`] links
//! are followed, and only to files the scan found.
//!
//! Nothing is written to the session's files. Brought in, a session becomes
//! a thread in the store ([`super::FoundStore::bring_in`]); sending to it
//! later starts a copy (§U4.2), which is the host's to do.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::claude_files::ClaudeTail;
use super::codex_files::{self, CodexTail};
use super::paths::{self, Roots};
use super::tail;
use super::zst::{self, ZstReader};
use super::{FoundStore, MAX_META_LINE, OpenQuery, collect, first_line, folder_of, last_component, open_now};
use crate::api::*;
use crate::host::Host;
use crate::host::db::{FoundFile, FoundRow, TurnRow};
use crate::host::store::now_ms;
use crate::host::threads::{UNFINISHED_OUTPUT, random_hex, subdir_of};

/// Lines read from one session at most; past it the oldest are skipped.
pub const MAX_LINES: u64 = 100_000;
/// `history_base` links followed at most.
pub const MAX_BASES: usize = 8;
/// Items a session brought in keeps at most, tool steps included.
pub const MAX_KEPT_ITEMS: usize = FORK_ITEMS;
/// A line longer than this is skipped, as live following skips it.
const MAX_LINE: u64 = SESSION_TAIL_MAX;
/// When the oldest lines are skipped, the first prompt is looked for in at
/// most this much of the start.
const FIRST_PROMPT_SCAN: u64 = 4 << 20;

pub const HISTORY_GONE: &str = "Earlier messages are in a session that is no longer on this PC.";
pub const LEFT_OUT: &str = "Earlier messages were left out.";
pub const STILL_OPEN: &str = "Brought in from a terminal session that is still open.";

fn left_out(n: usize) -> String {
    if n == 1 {
        "1 earlier message was left out.".to_owned()
    } else {
        format!("{n} earlier messages were left out.")
    }
}

// ---- Reading files ----

/// A session file from its start, decompressed when it is `.zst`. Opened
/// without following a link, and only when it is a regular file.
pub fn open(path: &Path, packed: bool) -> io::Result<Box<dyn Read + Send>> {
    let (file, _) = tail::open_regular(path)?;
    if packed {
        Ok(Box::new(ZstReader::new(BufReader::new(file), zst::MAX_DECODED)?))
    } else {
        Ok(Box::new(file))
    }
}

/// Whole lines from a reader: where each starts and its bytes without the
/// `\n`. A line past `max` bytes is skipped, a blank one too; a last line
/// without its `\n` (still being written, or cut at `end`) is not given.
pub struct LineReader<R: Read> {
    src: BufReader<R>,
    pos: u64,
    end: Option<u64>,
    max: u64,
}

impl<R: Read> std::fmt::Debug for LineReader<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LineReader").field("pos", &self.pos).field("end", &self.end).finish_non_exhaustive()
    }
}

impl<R: Read> LineReader<R> {
    /// Lines of `src`, up to byte `end` when given.
    pub fn new(src: R, end: Option<u64>, max: u64) -> Self {
        Self { src: BufReader::with_capacity(65_536, src), pos: 0, end, max }
    }

    /// Bytes consumed so far.
    pub fn pos(&self) -> u64 {
        self.pos
    }

    /// Skips to just past the next `\n`: for a reader that starts mid-line.
    fn skip_partial(&mut self) -> io::Result<()> {
        loop {
            let avail = self.src.fill_buf()?;
            if avail.is_empty() {
                return Ok(());
            }
            match avail.iter().position(|&b| b == b'\n') {
                Some(nl) => {
                    self.src.consume(nl + 1);
                    self.pos += nl as u64 + 1;
                    return Ok(());
                }
                None => {
                    let n = avail.len();
                    self.src.consume(n);
                    self.pos += n as u64;
                }
            }
        }
    }

    /// The next line into `out`; its offset, or `None` at the end.
    pub fn next_line(&mut self, out: &mut Vec<u8>) -> io::Result<Option<u64>> {
        out.clear();
        let mut start = self.pos;
        let mut skipping = false;
        loop {
            let room = self.end.map_or(u64::MAX, |end| end.saturating_sub(self.pos));
            if room == 0 {
                return Ok(None);
            }
            let avail = self.src.fill_buf()?;
            if avail.is_empty() {
                return Ok(None);
            }
            let chunk = &avail[..avail.len().min(usize::try_from(room).unwrap_or(usize::MAX))];
            match chunk.iter().position(|&b| b == b'\n') {
                Some(nl) => {
                    if !skipping {
                        out.extend_from_slice(&chunk[..nl]);
                    }
                    self.src.consume(nl + 1);
                    self.pos += nl as u64 + 1;
                    if skipping || out.len() as u64 > self.max || out.iter().all(u8::is_ascii_whitespace) {
                        skipping = false;
                        out.clear();
                        start = self.pos;
                        continue;
                    }
                    return Ok(Some(start));
                }
                None => {
                    let n = chunk.len();
                    if !skipping {
                        out.extend_from_slice(chunk);
                        if out.len() as u64 > self.max {
                            skipping = true;
                            out.clear();
                        }
                    }
                    self.src.consume(n);
                    self.pos += n as u64;
                }
            }
        }
    }
}

/// One file in a session's reading order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub path: PathBuf,
    pub packed: bool,
    /// Read up to this many bytes (decompressed): where the next piece's
    /// history ends. `None`: to the end.
    pub end: Option<u64>,
    pub mtime_ms: u64,
}

impl Piece {
    fn of(file: &FoundFile, end: Option<u64>) -> Self {
        Self { path: PathBuf::from(&file.path), packed: file.packed, end, mtime_ms: file.mtime_ms }
    }

    /// Its lines from `start` on, plain files only; a `.zst` file always
    /// reads from its start.
    fn lines(&self, start: u64) -> io::Result<LineReader<Box<dyn Read + Send>>> {
        if self.packed || start == 0 {
            return Ok(LineReader::new(open(&self.path, self.packed)?, self.end, MAX_LINE));
        }
        // One byte early, so a start right after a `\n` keeps its line.
        let (mut file, _) = tail::open_regular(&self.path)?;
        file.seek(SeekFrom::Start(start - 1))?;
        let mut lines = LineReader::new(Box::new(file) as Box<dyn Read + Send>, self.end, MAX_LINE);
        lines.pos = start - 1;
        lines.skip_partial()?;
        Ok(lines)
    }

    /// Where reading starts: a plain file longer than the limit is read from
    /// its last [`zst::MAX_DECODED`] bytes, at the next line.
    fn start(&self) -> u64 {
        if self.packed {
            return 0;
        }
        let size = tail::stamp(&self.path).map_or(0, |s| s.size);
        self.end.unwrap_or(size).min(size).saturating_sub(zst::MAX_DECODED)
    }
}

/// The id other rollouts' `history_base` names a file by.
fn segment_of(path: &str) -> Option<&str> {
    let name = Path::new(path).file_name()?.to_str()?;
    paths::rollout_name(name).map(|n| n.part.unwrap_or(n.thread))
}

/// Rollout files the scan found, by the id their history links use; the
/// whole list is read from the store only when a link leaves the session.
pub struct Segments {
    store: Arc<dyn FoundStore>,
    all: Option<HashMap<String, FoundFile>>,
}

impl std::fmt::Debug for Segments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Segments").field("loaded", &self.all.as_ref().map(HashMap::len)).finish()
    }
}

impl Segments {
    pub fn new(store: Arc<dyn FoundStore>) -> Self {
        Self { store, all: None }
    }

    fn find(&mut self, row: &FoundRow, segment: &str) -> io::Result<Option<FoundFile>> {
        if let Some(file) = row.files.iter().find(|f| segment_of(&f.path) == Some(segment)) {
            return Ok(Some(file.clone()));
        }
        if self.all.is_none() {
            let mut all = HashMap::new();
            for (_, file) in self.store.found_files()? {
                if let Some(segment) = segment_of(&file.path) {
                    all.insert(segment.to_owned(), file);
                }
            }
            self.all = Some(all);
        }
        Ok(self.all.as_ref().and_then(|all| all.get(segment).cloned()))
    }
}

/// A rollout's first line, as the history links need it.
fn first_meta(path: &Path, packed: bool) -> io::Result<Option<codex_files::CodexMeta>> {
    let mut lines = LineReader::new(open(path, packed)?, None, MAX_META_LINE);
    let mut buf = Vec::new();
    match lines.next_line(&mut buf)? {
        Some(0) => Ok(codex_files::parse_meta(&buf)),
        _ => Ok(None),
    }
}

/// The files a session's conversation is in, oldest first, and whether its
/// history starts in a file that isn't here any more.
pub fn pieces(row: &FoundRow, segments: &mut Segments) -> io::Result<(Vec<Piece>, bool)> {
    let gone = || io::Error::new(io::ErrorKind::NotFound, "the session's files are gone");
    if row.agent == AgentId::Claude {
        let file = row.files.first().ok_or_else(gone)?;
        return Ok((vec![Piece::of(file, None)], false));
    }
    let newest = row
        .files
        .iter()
        .filter(|f| {
            Path::new(&f.path)
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(paths::rollout_name)
                .is_some_and(|n| n.thread == row.native_id)
        })
        .max_by(|a, b| (&a.stamp, &a.path).cmp(&(&b.stamp, &b.path)))
        .ok_or_else(gone)?;
    let mut chain = vec![Piece::of(newest, None)];
    let mut seen: HashSet<String> = HashSet::from([newest.path.clone()]);
    let mut missing = false;
    while let Some(current) = chain.last() {
        let meta = first_meta(&current.path, current.packed)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a rollout without session_meta"))?;
        if chain.len() == 1 && meta.id.as_deref() != Some(row.native_id.as_str()) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a rollout of another thread"));
        }
        let Some(base) = meta.history_base else { break };
        if chain.len() > MAX_BASES {
            missing = true;
            break;
        }
        match segments.find(row, &base.segment)? {
            Some(file) if seen.insert(file.path.clone()) => chain.push(Piece::of(&file, Some(base.end_byte))),
            _ => {
                missing = true;
                break;
            }
        }
    }
    chain.reverse();
    Ok((chain, missing))
}

/// What a read keeps.
#[derive(Debug, Clone, Copy)]
pub struct Keep {
    /// The newest messages (user and assistant items).
    pub messages: usize,
    /// Items in all, tool steps included.
    pub items: usize,
    /// Keep the first prompt when it falls out.
    pub first_user: bool,
}

impl Keep {
    pub fn import() -> Self {
        Self { messages: IMPORT_KEPT_MESSAGES, items: MAX_KEPT_ITEMS, first_user: true }
    }

    pub fn preview(limit: usize) -> Self {
        Self { messages: usize::MAX, items: limit.max(1), first_user: false }
    }
}

fn is_message(item: &Item) -> bool {
    matches!(item.kind, ItemKind::User | ItemKind::Assistant)
}

/// The newest items while a session streams past.
#[derive(Debug, Default)]
struct Kept {
    items: BTreeMap<u64, Item>,
    messages: usize,
    /// Items below this `seq` fell out; a late change to one is dropped.
    floor: u64,
    left_out: usize,
    first_user: Option<Item>,
}

impl Kept {
    fn add(&mut self, item: Item, keep: Keep) {
        if item.seq < self.floor {
            return;
        }
        if keep.first_user && self.first_user.is_none() && item.kind == ItemKind::User {
            self.first_user = Some(item.clone());
        }
        let message = is_message(&item);
        if self.items.insert(item.seq, item).is_none() && message {
            self.messages += 1;
        }
        while self.messages > keep.messages || self.items.len() > keep.items {
            let Some((seq, old)) = self.items.pop_first() else { break };
            self.floor = seq + 1;
            if is_message(&old) {
                self.messages -= 1;
                if self.first_user.as_ref().is_none_or(|f| f.seq != seq) {
                    self.left_out += 1;
                }
            }
        }
    }

    /// The items oldest first; the first prompt when it fell out of them.
    fn finish(mut self) -> (Vec<Item>, Option<Item>, usize) {
        if self.floor > 0 {
            // Tool steps before the oldest message kept belong to messages left out.
            while self.items.first_key_value().is_some_and(|(_, i)| !is_message(i)) {
                self.items.pop_first();
            }
        }
        let first_user = self.first_user.filter(|f| f.seq < self.floor);
        (self.items.into_values().collect(), first_user, self.left_out)
    }
}

/// How many messages a read left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftOut {
    Count(usize),
    /// Lines were skipped unread: how many messages they held isn't known.
    Uncounted,
}

/// A session read.
#[derive(Debug, Clone)]
pub struct Reading {
    /// Oldest first, with the `seq`s the read gave them.
    pub items: Vec<Item>,
    /// The first prompt, when it isn't among `items`.
    pub first_user: Option<Item>,
    pub left_out: LeftOut,
    /// Its history starts in a file that isn't here.
    pub history_gone: bool,
}

/// One mapper for the whole read.
enum Mapper {
    Claude(ClaudeTail),
    Codex(CodexTail),
}

impl Mapper {
    fn new(agent: AgentId, cwd: Option<&str>) -> Self {
        let cwd = cwd.map(PathBuf::from);
        match agent {
            AgentId::Claude => Self::Claude(ClaudeTail::new(cwd)),
            AgentId::Codex => Self::Codex(CodexTail::new(cwd)),
        }
    }

    fn feed(&mut self, v: &Value, offset: u64, mtime_ms: u64) -> Vec<Item> {
        match self {
            Self::Claude(t) => t.feed_value(v, offset, mtime_ms),
            Self::Codex(t) => t.feed_value(v, offset, mtime_ms),
        }
    }
}

fn stopped() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "stopped")
}

/// Reads a found session whole (within the limits) and keeps what `keep` says.
pub fn read_session(
    row: &FoundRow,
    segments: &mut Segments,
    keep: Keep,
    stop: &AtomicBool,
) -> io::Result<Reading> {
    let (pieces, history_gone) = pieces(row, segments)?;
    let starts: Vec<u64> = pieces.iter().map(Piece::start).collect();
    // First pass: count the lines (this also finds a damaged file before
    // anything is kept).
    let mut total = 0u64;
    let mut buf = Vec::new();
    for (piece, start) in pieces.iter().zip(&starts) {
        let mut lines = piece.lines(*start)?;
        while lines.next_line(&mut buf)?.is_some() {
            total += 1;
            if total.is_multiple_of(4096) && stop.load(Ordering::SeqCst) {
                return Err(stopped());
            }
        }
    }
    let skip = total.saturating_sub(MAX_LINES);
    let cut_front = skip > 0 || starts.iter().any(|s| *s > 0);
    let mut mapper = Mapper::new(row.agent, row.cwd.as_deref());
    let mut kept = Kept::default();
    let mut line_no = 0u64;
    let mut base = 0u64;
    let mut last_ordinal: Option<u64> = None;
    for (piece, start) in pieces.iter().zip(&starts) {
        let mut lines = piece.lines(*start)?;
        while let Some(offset) = lines.next_line(&mut buf)? {
            line_no += 1;
            if line_no.is_multiple_of(4096) && stop.load(Ordering::SeqCst) {
                return Err(stopped());
            }
            if line_no <= skip {
                continue;
            }
            let Ok(v) = serde_json::from_slice::<Value>(&buf) else { continue };
            if let Some(ordinal) = codex_files::ordinal_of(&v) {
                if last_ordinal.is_some_and(|last| ordinal <= last) {
                    continue;
                }
                last_ordinal = Some(ordinal);
            }
            for item in mapper.feed(&v, base + offset, piece.mtime_ms) {
                kept.add(item, keep);
            }
        }
        base += lines.pos() + 1;
    }
    let (items, mut first_user, left) = kept.finish();
    if cut_front && keep.first_user {
        first_user = first_prompt(row, &pieces)?.filter(|f| items.first().is_none_or(|i| f.seq < i.seq));
    }
    Ok(Reading {
        items,
        first_user,
        left_out: if cut_front { LeftOut::Uncounted } else { LeftOut::Count(left) },
        history_gone,
    })
}

/// The first prompt, from the start of the oldest piece.
fn first_prompt(row: &FoundRow, pieces: &[Piece]) -> io::Result<Option<Item>> {
    let Some(first) = pieces.first() else { return Ok(None) };
    let piece = Piece { end: Some(first.end.unwrap_or(u64::MAX).min(FIRST_PROMPT_SCAN)), ..first.clone() };
    let mut lines = piece.lines(0)?;
    let mut mapper = Mapper::new(row.agent, row.cwd.as_deref());
    let mut buf = Vec::new();
    while let Some(offset) = lines.next_line(&mut buf)? {
        let Ok(v) = serde_json::from_slice::<Value>(&buf) else { continue };
        if let Some(item) =
            mapper.feed(&v, offset, piece.mtime_ms).into_iter().find(|i| i.kind == ItemKind::User)
        {
            return Ok(Some(item));
        }
    }
    Ok(None)
}

// ---- A session as a thread ----

/// A session ready to become a thread: everything the host needs to make it
/// ([`super::FoundStore::bring_in`]).
#[derive(Debug, Clone, PartialEq)]
pub struct BroughtIn {
    /// The `found` row it comes from (`ThreadCols.found_id`).
    pub found_id: String,
    pub agent: AgentId,
    /// The session's UUID: Claude's session id, Codex's thread id.
    pub native_id: String,
    pub title: String,
    /// The agent folder holding its working directory, and the sub-folder
    /// inside it. `None`: outside every agent folder, so the thread is
    /// read-only (`readonly: outside_folders`) until a folder holding
    /// `cwd` is added.
    pub folder: Option<(String, Option<String>)>,
    /// Its folder's label, else the last component of `cwd`.
    pub folder_label: String,
    /// Its working directory. Never leaves the PC; kept in the thread's row
    /// (`ThreadCols.cwd`) only when `folder` is `None`.
    pub cwd: Option<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub origin: Origin,
    /// `seq` 1, 2, … oldest first, notices included. Nothing is running
    /// and every answer is finished (`end_ms`), so all of it is indexed.
    pub items: Vec<Item>,
    /// One per prompt: `checkpoint: missing`, `why: before`, no mark.
    pub turns: Vec<TurnRow>,
}

impl BroughtIn {
    /// `ThreadCols.next_seq`.
    pub fn next_seq(&self) -> u64 {
        self.items.len() as u64 + 1
    }

    /// `ThreadCols.next_turn`.
    pub fn next_turn(&self) -> u32 {
        u32::try_from(self.turns.len()).unwrap_or(u32::MAX).saturating_add(1)
    }

    /// `ThreadInfo.last_seq` and `ThreadCols.read_seq`: it isn't unread.
    pub fn last_seq(&self) -> u64 {
        self.items.len() as u64
    }
}

fn notice(text: &str, ts_ms: u64) -> Item {
    Item { text: Some(text.to_owned()), ..Item::new(0, "notice", ItemKind::Notice, ts_ms) }
}

/// Where a session's working directory is among the agent folders:
/// `(folder id, sub-folder)` and the label to show.
pub fn place(
    cwd: Option<&str>,
    folders: &[(String, String, PathBuf)],
) -> (Option<(String, Option<String>)>, String) {
    let Some(cwd) = cwd else { return (None, String::new()) };
    let (canonical, folder) = folder_of(cwd, folders);
    match folder.and_then(|(id, label, path)| Some((id, label, subdir_of(&canonical, path).ok()?))) {
        Some((id, label, subdir)) => (Some((id.clone(), subdir)), label.clone()),
        None => (None, last_component(cwd)),
    }
}

/// The thread a session becomes.
pub fn brought_in(
    row: &FoundRow,
    reading: Reading,
    folders: &[(String, String, PathBuf)],
    origin: Origin,
    still_open: bool,
) -> BroughtIn {
    let Reading { items, first_user, left_out: left, history_gone } = reading;
    let first_ts = first_user.as_ref().or(items.first()).map_or(row.updated_ms, |i| i.ts_ms);
    let created_ms = row.first_ms.unwrap_or(first_ts);
    let updated_ms = row.updated_ms.max(created_ms);
    let title = Some(row.title.trim())
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            first_user
                .as_ref()
                .or_else(|| items.iter().find(|i| i.kind == ItemKind::User))?
                .text
                .as_deref()
                .and_then(first_line)
        })
        .map(|t| clean_cut(&t, MAX_TITLE).0)
        .unwrap_or_else(|| row.agent.display_name().to_owned());
    let mut all: Vec<Item> = Vec::with_capacity(items.len() + 4);
    if history_gone {
        all.push(notice(HISTORY_GONE, created_ms));
    }
    match left {
        LeftOut::Count(0) => {}
        LeftOut::Count(n) => all.push(notice(&left_out(n), created_ms)),
        LeftOut::Uncounted => all.push(notice(LEFT_OUT, created_ms)),
    }
    all.extend(first_user);
    all.extend(items);
    if still_open {
        all.push(notice(STILL_OPEN, updated_ms));
    }
    let mut turns: Vec<TurnRow> = Vec::new();
    for (n, item) in all.iter_mut().enumerate() {
        let seq = n as u64 + 1;
        item.seq = seq;
        item.rev = 0;
        if item.kind == ItemKind::Notice {
            item.id = format!("notice-{seq}");
        }
        if let Some(tool) = item.tool.as_mut()
            && tool.status == ToolStatus::Running
        {
            tool.status = ToolStatus::Error;
            tool.output = Some(UNFINISHED_OUTPUT.to_owned());
        }
        if item.kind == ItemKind::Assistant && item.end_ms.is_none() {
            item.end_ms = Some(item.ts_ms);
        }
        if item.kind == ItemKind::User {
            let number = u32::try_from(turns.len()).unwrap_or(u32::MAX).saturating_add(1);
            let mut turn = TurnRow::new(number, seq, item.ts_ms);
            turn.checkpoint = Checkpoint::Missing;
            turn.why = Some(CheckpointWhy::Before);
            turns.push(turn);
        }
        if let Some(turn) = turns.last_mut()
            && item.kind != ItemKind::Notice
        {
            item.turn = Some(turn.turn);
            turn.last_seq = Some(seq);
            turn.ended_ms = Some(item.ts_ms.max(turn.started_ms));
            turn.ended = Some(TurnEnd::Done);
        }
    }
    let (folder, folder_label) = place(row.cwd.as_deref(), folders);
    BroughtIn {
        found_id: row.id.clone(),
        agent: row.agent,
        native_id: row.native_id.clone(),
        title,
        folder,
        folder_label,
        cwd: row.cwd.clone(),
        created_ms,
        updated_ms,
        origin,
        items: all,
        turns,
    }
}

// ---- The job ----

/// A `sessions.import` job started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportJob {
    /// [`IMPORT_JOB_LEN`] lowercase hex digits.
    pub job: String,
    pub total: u32,
}

/// The running job, in [`super::Sessions`].
#[derive(Debug)]
pub(crate) struct JobSlot {
    id: String,
    cancel: Arc<AtomicBool>,
}

impl JobSlot {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

fn bad(message: &str) -> RpcError {
    RpcError::new(ErrorCode::BadRequest, message)
}

/// `sessions.import` (§U4.2): checks the request, starts the job and
/// answers at once; `import.progress` follows. One job at a time.
pub async fn import(host: &Arc<Host>, ids: Vec<String>, by: Origin) -> Result<ImportJob, RpcError> {
    if ids.is_empty() || ids.len() > MAX_IMPORT_IDS {
        return Err(bad("Choose between 1 and 50 sessions."));
    }
    if !ids.iter().all(|id| is_valid_session_id(id)) {
        return Err(bad("Bad session id."));
    }
    if !by.is_valid() {
        return Err(bad("Bad origin."));
    }
    let mut seen = HashSet::new();
    let ids: Vec<String> = ids.into_iter().filter(|id| seen.insert(id.clone())).collect();
    let store = host.sessions.found_store()?;
    let cancel = Arc::new(AtomicBool::new(false));
    let job = random_hex(IMPORT_JOB_LEN / 2);
    {
        let mut slot = host.sessions.job.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return Err(RpcError::new(
                ErrorCode::Busy,
                "Sessions are being brought in. Wait until that finishes.",
            ));
        }
        *slot = Some(JobSlot { id: job.clone(), cancel: cancel.clone() });
    }
    let total = u32::try_from(ids.len()).unwrap_or(u32::MAX);
    let run = Run {
        host: Arc::downgrade(host),
        store,
        job: job.clone(),
        roots: host.sessions.roots.clone(),
        folders: host.session_folders(),
        by,
        cancel,
    };
    tokio::spawn(async move {
        let host = run.host.clone();
        let id = run.job.clone();
        let _ = tokio::task::spawn_blocking(move || run.go(ids)).await;
        if let Some(host) = host.upgrade() {
            let mut slot = host.sessions.job.lock().unwrap_or_else(|e| e.into_inner());
            if slot.as_ref().is_some_and(|s| s.id == id) {
                *slot = None;
            }
        }
    });
    Ok(ImportJob { job, total })
}

/// `import.cancel`: the job stops after the session it is on; what was
/// brought in stays. Its last `import.progress` has `total = done + failed`.
pub fn cancel(host: &Host, job: &str) -> Result<(), RpcError> {
    let slot = host.sessions.job.lock().unwrap_or_else(|e| e.into_inner());
    match slot.as_ref() {
        Some(running) if running.id == job => {
            running.cancel();
            Ok(())
        }
        _ => Err(RpcError::new(ErrorCode::NotFound, "No such import.")),
    }
}

/// One job, on the blocking pool.
struct Run {
    host: Weak<Host>,
    store: Arc<dyn FoundStore>,
    job: String,
    roots: Roots,
    folders: Vec<(String, String, PathBuf)>,
    by: Origin,
    cancel: Arc<AtomicBool>,
}

/// How one session went.
enum Outcome {
    /// Brought in now, or before: its thread.
    Thread(String),
    Failed,
    /// The job was cancelled while it was read: not counted.
    Stopped,
}

impl Run {
    fn go(self, ids: Vec<String>) {
        let mut segments = Segments::new(self.store.clone());
        let (mut done, mut failed) = (0u32, 0u32);
        let mut total = u32::try_from(ids.len()).unwrap_or(u32::MAX);
        let mut thread_id = None;
        let mut last: Option<Instant> = None;
        for (n, id) in ids.iter().enumerate() {
            if self.cancel.load(Ordering::SeqCst) {
                total = done + failed;
                break;
            }
            match self.one(id, &mut segments) {
                Outcome::Thread(thread) => {
                    done += 1;
                    thread_id = Some(thread);
                }
                Outcome::Failed => failed += 1,
                Outcome::Stopped => break,
            }
            let finished = n + 1 == ids.len();
            if !finished && last.is_some_and(|at| at.elapsed() < Duration::from_millis(IMPORT_EVENT_MS)) {
                continue;
            }
            if !finished {
                last = Some(Instant::now());
                self.progress(done, failed, total, thread_id.clone());
            }
        }
        if self.cancel.load(Ordering::SeqCst) {
            total = done + failed;
        }
        self.progress(done, failed, total, thread_id);
    }

    fn progress(&self, done: u32, failed: u32, total: u32, thread_id: Option<String>) {
        if let Some(host) = self.host.upgrade() {
            host.hub.broadcast(&Event::ImportProgress(ImportProgressEvent {
                job: self.job.clone(),
                done,
                failed,
                total,
                thread_id,
            }));
        }
    }

    fn one(&self, id: &str, segments: &mut Segments) -> Outcome {
        let row = match self.store.found_row(id) {
            Ok(Some(row)) => row,
            Ok(None) => return Outcome::Failed,
            Err(e) => {
                tracing::warn!("a session found couldn't be read from the store: {}", e.kind());
                return Outcome::Failed;
            }
        };
        if let Some(thread) = row.thread_id {
            return Outcome::Thread(thread);
        }
        if row.damaged {
            return Outcome::Failed;
        }
        let reading = match read_session(&row, segments, Keep::import(), &self.cancel) {
            Ok(reading) => reading,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => return Outcome::Stopped,
            Err(e) => {
                tracing::info!("a session couldn't be brought in: {}", e.kind());
                return Outcome::Failed;
            }
        };
        let open = open_now(
            &self.roots,
            crate::client::euid(),
            std::process::id(),
            OpenQuery {
                agent: row.agent,
                uuid: &row.native_id,
                path: row.files.iter().max_by_key(|f| f.mtime_ms).map(|f| PathBuf::from(&f.path)),
                cwd: row.cwd.as_deref(),
                ours_until: None,
            },
            now_ms(),
        );
        let thread =
            brought_in(&row, reading, &self.folders, self.by.clone(), open == Some(SessionSource::Terminal));
        match self.store.bring_in(thread) {
            Ok(thread) => Outcome::Thread(thread),
            Err(e) => {
                tracing::info!("a session couldn't be saved as a thread: {}", e.code.as_str());
                Outcome::Failed
            }
        }
    }
}

// ---- Preview ----

fn unreadable() -> RpcError {
    RpcError::new(ErrorCode::Unavailable, "This session can't be read.")
}

/// `session.preview`: the session's newest `limit` items (at most
/// [`MAX_PREVIEW_LIMIT`]), read now and kept nowhere, within
/// `session.read`'s reply size.
pub async fn preview(host: &Host, id: &str, limit: u32) -> Result<Vec<Item>, RpcError> {
    if !is_valid_session_id(id) {
        return Err(super::not_found());
    }
    let store = host.sessions.found_store()?;
    let limit = usize::try_from(limit.clamp(1, MAX_PREVIEW_LIMIT)).unwrap_or(1);
    let id = id.to_owned();
    let items = tokio::task::spawn_blocking(move || -> Result<Vec<Item>, RpcError> {
        let row = store
            .found_row(&id)
            .map_err(|e| {
                tracing::warn!("a session found couldn't be read from the store: {}", e.kind());
                unreadable()
            })?
            .ok_or_else(super::not_found)?;
        if row.damaged {
            return Err(unreadable());
        }
        let mut segments = Segments::new(store);
        let (pieces, gone) = pieces(&row, &mut segments).map_err(|e| {
            tracing::info!("a session couldn't be previewed: {}", e.kind());
            unreadable()
        })?;
        // One plain file: read back from its end, as `session.read` does.
        if let [piece] = pieces.as_slice()
            && !piece.packed
            && !gone
        {
            return collect(row.agent, &piece.path, row.cwd.as_deref().map(PathBuf::from), None, limit)
                .map(|(items, _)| items)
                .map_err(|_| unreadable());
        }
        let never = AtomicBool::new(false);
        read_session(&row, &mut segments, Keep::preview(limit), &never).map(|r| r.items).map_err(|e| {
            tracing::info!("a session couldn't be previewed: {}", e.kind());
            unreadable()
        })
    })
    .await
    .map_err(|_| RpcError::new(ErrorCode::Internal, "The read failed."))??;
    Ok(within_reply(items))
}

/// The newest items that fit a reply.
fn within_reply(mut items: Vec<Item>) -> Vec<Item> {
    let mut budget = MAX_READ_RESULT.saturating_sub(64);
    let mut keep = items.len();
    for (i, item) in items.iter().enumerate().rev() {
        let size = serde_json::to_vec(item).map_or(0, |v| v.len() + 1);
        if size > budget {
            break;
        }
        budget -= size;
        keep = i;
    }
    items.drain(..keep);
    items
}

/// A [`FoundStore`] over a database the caller owns, for tests: the store
/// side of what the host does, without its registry or events.
#[cfg(any(test, feature = "fake-cli"))]
pub mod testing {
    use std::io;
    use std::sync::Mutex;

    use super::BroughtIn;
    use crate::api::*;
    use crate::host::db::{Db, FoundFile, FoundPage, FoundQuery, FoundRow, ThreadCols};
    use crate::host::sessions::FoundStore;
    use crate::host::threads::random_hex;

    #[derive(Debug)]
    pub struct DbStore {
        pub db: Mutex<Db>,
    }

    impl DbStore {
        pub fn new(db: Db) -> Self {
            Self { db: Mutex::new(db) }
        }

        pub fn db(&self) -> std::sync::MutexGuard<'_, Db> {
            self.db.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    /// The thread a [`BroughtIn`] makes, as the host would show it.
    pub fn thread_info(id: &str, b: &BroughtIn) -> ThreadInfo {
        let mut info: ThreadInfo = serde_json::from_value(serde_json::json!({
            "id": id, "agent": b.agent, "folder_id": b.folder.as_ref().map_or("", |f| f.0.as_str()),
            "folder_label": b.folder_label, "title": b.title, "state": "done", "mode": "supervised",
            "origin": b.origin, "created_ms": b.created_ms, "updated_ms": b.updated_ms,
            "last_seq": b.last_seq(), "pending": 0
        }))
        .unwrap_or_else(|e| panic!("a thread info: {e}"));
        info.native_id = Some(b.native_id.clone());
        info.subdir = b.folder.as_ref().and_then(|f| f.1.clone());
        info.found = true;
        info.readonly = b.folder.is_none().then_some(Readonly::OutsideFolders);
        info.turns = b.turns.last().map_or(0, |t| t.turn);
        info
    }

    impl FoundStore for DbStore {
        fn found_files(&self) -> io::Result<Vec<(String, FoundFile)>> {
            self.db().found_files()
        }

        fn found(&self, q: &FoundQuery) -> io::Result<FoundPage> {
            self.db().found(q)
        }

        fn found_row(&self, id: &str) -> io::Result<Option<FoundRow>> {
            let db = self.db();
            let mut before_ms = None;
            loop {
                let page =
                    db.found(&FoundQuery { limit: MAX_SCAN_LIMIT, before_ms, ..FoundQuery::default() })?;
                if let Some(row) = page.rows.into_iter().find(|r| r.id == id) {
                    return Ok(Some(row));
                }
                match page.next_ms {
                    Some(next) => before_ms = Some(next),
                    None => return Ok(None),
                }
            }
        }

        fn upsert_found(&self, rows: &[FoundRow]) -> io::Result<()> {
            self.db().upsert_found(rows)
        }

        fn forget_found_files(&self, files: &[(String, String)]) -> io::Result<()> {
            self.db().forget_found_files(files)
        }

        fn bring_in(&self, b: BroughtIn) -> Result<String, RpcError> {
            let failed = |_| RpcError::new(ErrorCode::Internal, "Couldn't save the thread.");
            let id = random_hex(16);
            let info = thread_info(&id, &b);
            let cols = ThreadCols {
                next_seq: b.next_seq(),
                next_turn: b.next_turn(),
                cwd: if b.folder.is_none() { b.cwd.clone() } else { None },
                read_seq: b.last_seq(),
                found_id: Some(b.found_id.clone()),
                ..ThreadCols::default()
            };
            let mut db = self.db();
            db.save_thread(&info, &cols).map_err(failed)?;
            for item in &b.items {
                db.append_item(&id, item).map_err(failed)?;
            }
            for turn in &b.turns {
                db.save_turn(&id, turn, None).map_err(failed)?;
            }
            db.flush().map_err(failed)?;
            db.set_found_thread(&b.found_id, Some(&id)).map_err(failed)?;
            Ok(id)
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::testing::DbStore;
    use super::*;
    use crate::host::db::Db;
    use crate::host::test_host;
    use serde_json::json;

    pub(crate) const THREAD: &str = "11111111-2222-4333-8444-555555555555";
    pub(crate) const PART: &str = "019a0000-1111-7222-8333-444444444444";

    /// A Claude transcript of `pairs` prompts and answers, in `cwd`.
    pub(crate) fn claude_transcript(home: &Path, uuid: &str, cwd: &str, pairs: usize) -> PathBuf {
        let folder = home.join("projects").join(cwd.replace('/', "-"));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(format!("{uuid}.jsonl"));
        let mut body = String::from("{\"type\":\"queue-operation\",\"operation\":\"enqueue\"}\n");
        for n in 0..pairs {
            let ts = format!("2026-11-19T09:{:02}:{:02}.000Z", (n / 60) % 60, n % 60);
            body.push_str(&format!(
                "{}\n{}\n",
                json!({"type": "user", "uuid": format!("u{n}"), "timestamp": ts, "cwd": cwd, "entrypoint": "cli",
                       "message": {"role": "user", "content": format!("prompt {n}")}}),
                json!({"type": "assistant", "uuid": format!("a{n}"), "timestamp": ts, "cwd": cwd,
                       "message": {"id": format!("msg{n}"), "role": "assistant", "stop_reason": "end_turn",
                                   "content": [{"type": "text", "text": format!("answer {n}")}]}}),
            ));
        }
        std::fs::write(&path, body).unwrap();
        path
    }

    /// Rollout lines: `session_meta` at ordinal `from`, then one message per
    /// entry (`true`: the user's).
    pub(crate) fn rollout(meta: serde_json::Value, from: u64, messages: &[(bool, String)]) -> Vec<u8> {
        let mut out = format!(
            "{}\n",
            json!({"timestamp": "2026-11-19T09:00:00Z", "ordinal": from, "type": "session_meta", "payload": meta})
        );
        for (n, (user, text)) in messages.iter().enumerate() {
            let ordinal = from + n as u64 + 1;
            let item = if *user {
                json!({"type": "UserMessage", "id": format!("u{ordinal}"), "content": [{"type": "text", "text": text}]})
            } else {
                json!({"type": "AgentMessage", "id": format!("a{ordinal}"), "content": [{"type": "Text", "text": text}]})
            };
            out.push_str(&format!(
                "{}\n",
                json!({"timestamp": "2026-11-19T09:00:01Z", "ordinal": ordinal, "type": "event_msg",
                       "payload": {"type": "item_completed", "item": item}})
            ));
        }
        out.into_bytes()
    }

    pub(crate) fn pack(bytes: &[u8]) -> Vec<u8> {
        ruzstd::encoding::compress_to_vec(bytes, ruzstd::encoding::CompressionLevel::Fastest)
    }

    pub(crate) fn file(path: &Path, stamp: &str) -> FoundFile {
        let s = tail::stamp(path).unwrap();
        FoundFile {
            path: path.to_string_lossy().into_owned(),
            dev: s.dev,
            ino: s.ino,
            bytes: s.size,
            mtime_ms: s.mtime_ms(),
            stamp: stamp.into(),
            packed: path.to_string_lossy().ends_with(".zst"),
        }
    }

    pub(crate) fn row(agent: AgentId, uuid: &str, files: Vec<FoundFile>) -> FoundRow {
        FoundRow {
            id: paths::session_id(agent, uuid).unwrap(),
            agent,
            native_id: uuid.into(),
            cwd: Some("/home/me/code/app".into()),
            folder_id: None,
            title: String::new(),
            first_ms: None,
            updated_ms: 1_800_000_000_000,
            bytes: 1,
            source: None,
            base_thread: None,
            base_end: None,
            damaged: false,
            thread_id: None,
            files,
        }
    }

    pub(crate) fn store(dir: &Path) -> Arc<DbStore> {
        Arc::new(DbStore::new(Db::open(&dir.join("db")).unwrap()))
    }

    fn texts(items: &[Item]) -> Vec<String> {
        items.iter().map(|i| i.text.clone().unwrap_or_default()).collect()
    }

    #[test]
    fn lines_are_whole_bounded_and_never_blank() {
        let mut body = b"one\n\n   \ntwo\n".to_vec();
        body.extend(std::iter::repeat_n(b'x', 100));
        body.extend_from_slice(b"\nthree\nfour-unfinished");
        let mut lines = LineReader::new(body.as_slice(), None, 50);
        let mut buf = Vec::new();
        let mut got = Vec::new();
        while let Some(offset) = lines.next_line(&mut buf).unwrap() {
            got.push((offset, String::from_utf8(buf.clone()).unwrap()));
        }
        assert_eq!(got, [(0, "one".to_owned()), (9, "two".to_owned()), (114, "three".to_owned())]);
        let mut lines = LineReader::new(body.as_slice(), Some(12), 50);
        let mut n = 0;
        while lines.next_line(&mut buf).unwrap().is_some() {
            n += 1;
        }
        assert_eq!(n, 1, "a line cut at the end isn't given");
    }

    #[test]
    fn a_session_keeps_its_first_prompt_the_newest_200_and_says_how_many_went() {
        let dir = tempfile::tempdir().unwrap();
        let path = claude_transcript(dir.path(), THREAD, "/home/me/code/app", 250);
        let store = store(dir.path());
        let mut segments = Segments::new(store);
        let found = row(AgentId::Claude, THREAD, vec![file(&path, "")]);
        let reading = read_session(&found, &mut segments, Keep::import(), &AtomicBool::new(false)).unwrap();
        assert_eq!(reading.items.len(), 200);
        assert_eq!(reading.first_user.as_ref().and_then(|i| i.text.as_deref()), Some("prompt 0"));
        assert_eq!(reading.left_out, LeftOut::Count(299));
        assert_eq!(reading.items[0].text.as_deref(), Some("prompt 150"));
        let folders = vec![("f1".to_owned(), "app".to_owned(), PathBuf::from("/home/me/code"))];
        let b = brought_in(&found, reading, &folders, Origin::launcher(), true);
        assert_eq!(b.items.len(), 203);
        assert_eq!(b.items[0].text.as_deref(), Some("299 earlier messages were left out."));
        assert_eq!(b.items[0].kind, ItemKind::Notice);
        assert_eq!(b.items[1].text.as_deref(), Some("prompt 0"));
        assert_eq!(b.items[2].text.as_deref(), Some("prompt 150"));
        assert_eq!(b.items.last().unwrap().text.as_deref(), Some(STILL_OPEN));
        assert!(b.items.iter().enumerate().all(|(n, i)| i.seq == n as u64 + 1 && i.rev == 0));
        assert!(b.items.iter().filter(|i| i.kind == ItemKind::Assistant).all(|i| i.end_ms.is_some()));
        assert_eq!(b.turns.len(), 101);
        assert_eq!((b.turns[0].turn, b.turns[0].prompt_seq, b.turns[0].last_seq), (1, 2, Some(2)));
        assert_eq!((b.turns[1].prompt_seq, b.turns[1].last_seq), (3, Some(4)));
        assert!(
            b.turns
                .iter()
                .all(|t| t.checkpoint == Checkpoint::Missing && t.why == Some(CheckpointWhy::Before))
        );
        assert_eq!((b.next_seq(), b.next_turn(), b.last_seq()), (204, 102, 203));
        assert_eq!(b.items[3].turn, Some(2));
        assert_eq!(b.items[0].turn, None, "a notice is in no turn");
        assert_eq!(b.title, "prompt 0");
        assert_eq!(b.folder, Some(("f1".into(), Some("app".into()))), "inside the folder `code`");
        assert_eq!(b.folder_label, "app");
    }

    #[test]
    fn a_short_session_is_kept_whole_and_outside_every_folder_it_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = claude_transcript(dir.path(), THREAD, "/somewhere/else", 3);
        let mut segments = Segments::new(store(dir.path()));
        let mut found = row(AgentId::Claude, THREAD, vec![file(&path, "")]);
        found.cwd = Some("/somewhere/else".into());
        found.title = "Named".into();
        let reading = read_session(&found, &mut segments, Keep::import(), &AtomicBool::new(false)).unwrap();
        assert_eq!(reading.left_out, LeftOut::Count(0));
        let b = brought_in(&found, reading, &[], Origin::launcher(), false);
        assert_eq!(texts(&b.items), ["prompt 0", "answer 0", "prompt 1", "answer 1", "prompt 2", "answer 2"]);
        assert_eq!(
            (b.folder.as_ref(), b.folder_label.as_str(), b.cwd.as_deref()),
            (None, "else", Some("/somewhere/else"))
        );
        assert_eq!(b.title, "Named");
    }

    #[test]
    fn a_stopped_turn_brings_in_no_prompt_or_turn_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let path = crate::host::sessions::tests::fixture_path("claude-stopped.jsonl");
        let mut segments = Segments::new(store(dir.path()));
        let found = row(AgentId::Claude, THREAD, vec![file(&path, "")]);
        let reading = read_session(&found, &mut segments, Keep::import(), &AtomicBool::new(false)).unwrap();
        let b = brought_in(&found, reading, &[], Origin::launcher(), false);
        assert_eq!(
            texts(&b.items),
            ["Run the slow suite.", "", "Just run the fast one.", "Running the fast suite.", ""]
        );
        assert_eq!(b.turns.len(), 2, "one turn per prompt");
        assert_eq!(b.items[4].turn, Some(2));
        let tool = b.items[4].tool.as_ref().unwrap();
        assert_eq!((tool.status, tool.output.as_deref()), (ToolStatus::Error, Some(UNFINISHED_OUTPUT)));
    }

    #[test]
    fn a_running_tool_is_finished_as_stopped() {
        let found = row(AgentId::Claude, THREAD, Vec::new());
        let mut tool = Item::new(5, "t", ItemKind::Tool, 9);
        tool.tool = Some(Tool {
            name: "Bash".into(),
            title: "Run command".into(),
            detail: None,
            status: ToolStatus::Running,
            output: None,
            class: None,
            path: None,
            by_rule: false,
        });
        let reading = Reading {
            items: vec![Item { text: Some("go".into()), ..Item::new(1, "u", ItemKind::User, 8) }, tool],
            first_user: None,
            left_out: LeftOut::Uncounted,
            history_gone: true,
        };
        let b = brought_in(&found, reading, &[], Origin::launcher(), false);
        assert_eq!(texts(&b.items[..2]), [HISTORY_GONE, LEFT_OUT]);
        let tool = b.items[3].tool.as_ref().unwrap();
        assert_eq!((tool.status, tool.output.as_deref()), (ToolStatus::Error, Some(UNFINISHED_OUTPUT)));
        assert_eq!(b.items[3].turn, Some(1));
    }

    /// A thread in two parts: the second goes on from ordinal 4 of the first
    /// and replaces what the first has after it.
    fn two_parts(day: &Path) -> (PathBuf, PathBuf) {
        std::fs::create_dir_all(day).unwrap();
        let meta = json!({"id": THREAD, "cwd": "/home/me/code/app", "originator": "Codex Desktop",
                          "creator_account_id": "acct-secret"});
        let first = rollout(
            meta,
            0,
            &[
                (true, "first prompt".into()),
                (false, "first answer".into()),
                (true, "second prompt".into()),
                (false, "replaced answer".into()),
                (true, "replaced prompt".into()),
            ],
        );
        // Ordinal 4 is the fifth line.
        let cut = first.split(|&b| b == b'\n').take(4).map(|l| l.len() as u64 + 1).sum::<u64>();
        let meta = json!({"id": THREAD, "cwd": "/home/me/code/app", "originator": "Codex Desktop",
                          "history_base": {"thread_id": THREAD, "end_ordinal_exclusive": 4, "end_byte_offset": cut}});
        let second = rollout(meta, 4, &[(false, "kept answer".into()), (true, "third prompt".into())]);
        let a = day.join(format!("rollout-2026-11-19T09-00-00-{THREAD}.jsonl.zst"));
        let b = day.join(format!("rollout-2026-11-19T10-00-00-{THREAD}_{PART}.jsonl.zst"));
        std::fs::write(&a, pack(&first)).unwrap();
        std::fs::write(&b, pack(&second)).unwrap();
        (a, b)
    }

    #[test]
    fn a_codex_thread_reads_back_through_its_parts() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = two_parts(&dir.path().join("2026/11/19"));
        let found = row(
            AgentId::Codex,
            THREAD,
            vec![file(&a, "2026-11-19T09-00-00"), file(&b, "2026-11-19T10-00-00")],
        );
        let mut segments = Segments::new(store(dir.path()));
        let (pieces, gone) = pieces(&found, &mut segments).unwrap();
        assert!(!gone);
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0].path, a);
        assert!(pieces[0].end.is_some());
        let reading = read_session(&found, &mut segments, Keep::import(), &AtomicBool::new(false)).unwrap();
        assert_eq!(
            texts(&reading.items),
            ["first prompt", "first answer", "second prompt", "kept answer", "third prompt"]
        );
        assert!(reading.items.windows(2).all(|w| w[0].seq < w[1].seq));
    }

    #[test]
    fn a_fork_reads_its_base_and_says_when_the_base_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("2026/11/19");
        let (a, b) = two_parts(&day);
        let store = store(dir.path());
        let mut source = row(
            AgentId::Codex,
            THREAD,
            vec![file(&a, "2026-11-19T09-00-00"), file(&b, "2026-11-19T10-00-00")],
        );
        source.title = "source".into();
        store.upsert_found(&[source]).unwrap();
        // A fork of the second part through its first answer.
        let plain_second = {
            let mut out = Vec::new();
            ZstReader::new(std::fs::read(&b).unwrap().as_slice(), zst::MAX_DECODED)
                .unwrap()
                .read_to_end(&mut out)
                .unwrap();
            out
        };
        let cut = plain_second.split(|&c| c == b'\n').take(2).map(|l| l.len() as u64 + 1).sum::<u64>();
        let fork_id = "22222222-2222-4333-8444-555555555555";
        let meta = json!({"id": fork_id, "cwd": "/home/me/code/app", "forked_from_id": THREAD,
                          "history_base": {"thread_id": PART, "end_ordinal_exclusive": 6, "end_byte_offset": cut}});
        let fork = day.join(format!("rollout-2026-11-19T11-00-00-{fork_id}.jsonl"));
        std::fs::write(&fork, rollout(meta, 6, &[(true, "fork prompt".into())])).unwrap();
        let found = row(AgentId::Codex, fork_id, vec![file(&fork, "2026-11-19T11-00-00")]);
        let mut segments = Segments::new(store.clone());
        let reading = read_session(&found, &mut segments, Keep::import(), &AtomicBool::new(false)).unwrap();
        assert!(!reading.history_gone);
        assert_eq!(
            texts(&reading.items),
            ["first prompt", "first answer", "second prompt", "kept answer", "fork prompt"]
        );
        // The base's files gone from the store: the fork says so.
        store
            .forget_found_files(&[(
                paths::session_id(AgentId::Codex, THREAD).unwrap(),
                b.to_string_lossy().into_owned(),
            )])
            .unwrap();
        let mut segments = Segments::new(store);
        let reading = read_session(&found, &mut segments, Keep::import(), &AtomicBool::new(false)).unwrap();
        assert!(reading.history_gone);
        assert_eq!(texts(&reading.items), ["fork prompt"]);
    }

    #[test]
    fn past_the_line_limit_the_oldest_lines_are_skipped_unread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut body = String::new();
        for n in 0..(MAX_LINES + 10) {
            body.push_str(&format!(
                "{{\"type\":\"user\",\"uuid\":\"u{n}\",\"message\":{{\"role\":\"user\",\"content\":\"p{n}\"}}}}\n"
            ));
        }
        std::fs::write(&path, body).unwrap();
        let found = row(AgentId::Claude, THREAD, vec![file(&path, "")]);
        let mut segments = Segments::new(store(dir.path()));
        let reading = read_session(&found, &mut segments, Keep::import(), &AtomicBool::new(false)).unwrap();
        assert_eq!(reading.left_out, LeftOut::Uncounted);
        assert_eq!(reading.first_user.as_ref().and_then(|i| i.text.as_deref()), Some("p0"));
        assert_eq!(reading.items.last().and_then(|i| i.text.as_deref()), Some("p100009"));
        assert_eq!(reading.items[0].text.as_deref(), Some("p99810"));
        let stop = AtomicBool::new(true);
        let e = read_session(&found, &mut segments, Keep::import(), &stop).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Interrupted);
    }

    /// A store whose bring-in cancels the job it runs in.
    #[derive(Debug)]
    struct Cancelling {
        inner: Arc<DbStore>,
        cancel: Arc<AtomicBool>,
    }

    impl FoundStore for Cancelling {
        fn found_files(&self) -> io::Result<Vec<(String, FoundFile)>> {
            self.inner.found_files()
        }
        fn found(&self, q: &crate::host::db::FoundQuery) -> io::Result<crate::host::db::FoundPage> {
            self.inner.found(q)
        }
        fn found_row(&self, id: &str) -> io::Result<Option<FoundRow>> {
            self.inner.found_row(id)
        }
        fn upsert_found(&self, rows: &[FoundRow]) -> io::Result<()> {
            self.inner.upsert_found(rows)
        }
        fn forget_found_files(&self, files: &[(String, String)]) -> io::Result<()> {
            self.inner.forget_found_files(files)
        }
        fn bring_in(&self, thread: BroughtIn) -> Result<String, RpcError> {
            self.cancel.store(true, Ordering::SeqCst);
            self.inner.bring_in(thread)
        }
    }

    fn progress(host: &Host, client: u64) -> Vec<ImportProgressEvent> {
        host.hub
            .test_lines(client)
            .iter()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v["event"] == "import.progress")
            .filter_map(|v| serde_json::from_value(v["params"].clone()).ok())
            .collect()
    }

    async fn final_progress(host: &Host, client: u64) -> ImportProgressEvent {
        for _ in 0..500 {
            if let Some(last) = progress(host, client).into_iter().find(|e| e.done + e.failed == e.total) {
                return last;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the job didn't finish");
    }

    #[tokio::test]
    async fn sessions_are_brought_in_once_and_again_after_a_delete() {
        let dir = tempfile::tempdir().unwrap();
        let host = test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let watcher = host.hub.test_subscriber(crate::host::Caps::default());
        let ids = ["a", "b", "c"].map(|n| format!("{}{}", n.repeat(8), &THREAD[8..]));
        for id in &ids {
            claude_transcript(&host.config.claude_home, id, "/home/me/code/app", 2);
        }
        let store = store(dir.path());
        let missing =
            import(&host, vec![paths::session_id(AgentId::Claude, &ids[0]).unwrap()], Origin::launcher())
                .await;
        assert_eq!(missing.unwrap_err().code, ErrorCode::Unavailable, "no store yet");
        host.sessions.attach_store(store.clone());
        crate::host::sessions::scan::scan(&host).finished().await;
        let found: Vec<String> = ids.iter().map(|u| paths::session_id(AgentId::Claude, u).unwrap()).collect();
        assert_eq!(
            import(&host, Vec::new(), Origin::launcher()).await.unwrap_err().code,
            ErrorCode::BadRequest
        );
        assert_eq!(
            import(&host, vec!["sc../x".into()], Origin::launcher()).await.unwrap_err().code,
            ErrorCode::BadRequest
        );
        let job = import(&host, found.clone(), Origin::launcher()).await.unwrap();
        assert_eq!((job.job.len(), job.total), (IMPORT_JOB_LEN, 3));
        let last = final_progress(&host, watcher).await;
        assert_eq!((last.job.as_str(), last.done, last.failed, last.total), (job.job.as_str(), 3, 0, 3));
        let first = store.found_row(&found[0]).unwrap().unwrap().thread_id.unwrap();
        let threads = store.db().load_threads().unwrap();
        assert_eq!(threads.len(), 3);
        let thread = threads.iter().find(|t| t.info.id == first).unwrap();
        assert!(thread.info.found && thread.info.native_id.as_deref() == Some(ids[0].as_str()));
        assert_eq!(thread.cols.found_id.as_deref(), Some(found[0].as_str()));
        // Again: skipped, counted done, its thread named.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let again = import(&host, vec![found[0].clone()], Origin::launcher()).await.unwrap();
        let last = final_progress(&host, watcher).await;
        assert_eq!((last.job, last.done, last.thread_id.as_deref()), (again.job, 1, Some(first.as_str())));
        assert_eq!(store.db().load_threads().unwrap().len(), 3);
        // Deleted, it can be brought in again.
        store.db().delete_thread(&first).unwrap();
        assert_eq!(store.found_row(&found[0]).unwrap().unwrap().thread_id, None);
        tokio::time::sleep(Duration::from_millis(20)).await;
        import(&host, vec![found[0].clone()], Origin::launcher()).await.unwrap();
        let last = final_progress(&host, watcher).await;
        assert_ne!(last.thread_id.as_deref(), Some(first.as_str()));
        assert_eq!(store.db().load_threads().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn one_job_at_a_time_and_cancel_stops_after_the_session_it_is_on() {
        let dir = tempfile::tempdir().unwrap();
        let host = test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let store = store(dir.path());
        host.sessions.attach_store(store.clone());
        *host.sessions.job.lock().unwrap() =
            Some(JobSlot { id: "0123456789abcdef".into(), cancel: Arc::new(AtomicBool::new(false)) });
        let busy = import(&host, vec![format!("sc{}", "a".repeat(32))], Origin::launcher()).await;
        assert_eq!(busy.unwrap_err().code, ErrorCode::Busy);
        assert_eq!(cancel(&host, "fedcba9876543210").unwrap_err().code, ErrorCode::NotFound);
        cancel(&host, "0123456789abcdef").unwrap();
        assert!(host.sessions.job.lock().unwrap().as_ref().unwrap().cancel.load(Ordering::SeqCst));
        *host.sessions.job.lock().unwrap() = None;

        let ids = ["a", "b", "c"].map(|n| format!("{}{}", n.repeat(8), &THREAD[8..]));
        for id in &ids {
            claude_transcript(&host.config.claude_home, id, "/home/me/code/app", 1);
        }
        crate::host::sessions::scan::scan(&host).finished().await;
        let watcher = host.hub.test_subscriber(crate::host::Caps::default());
        let flag = Arc::new(AtomicBool::new(false));
        let run = Run {
            host: Arc::downgrade(&host),
            store: Arc::new(Cancelling { inner: store.clone(), cancel: flag.clone() }),
            job: "00000000000000aa".into(),
            roots: host.sessions.roots.clone(),
            folders: Vec::new(),
            by: Origin::launcher(),
            cancel: flag,
        };
        run.go(ids.iter().map(|u| paths::session_id(AgentId::Claude, u).unwrap()).collect());
        let events = progress(&host, watcher);
        let last = events.last().unwrap();
        assert_eq!((last.done, last.failed, last.total), (1, 0, 1), "{events:?}");
        assert_eq!(store.db().load_threads().unwrap().len(), 1, "what was brought in stays");
    }

    #[tokio::test]
    async fn a_preview_reads_compressed_and_plain_sessions_and_keeps_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let host = test_host(dir.path(), APPROVAL_LIFETIME_MS);
        let store = store(dir.path());
        assert_eq!(
            preview(&host, &format!("sx{}", "a".repeat(32)), 5).await.unwrap_err().code,
            ErrorCode::Unavailable
        );
        host.sessions.attach_store(store.clone());
        two_parts(&host.config.codex_home.join("sessions/2026/11/19"));
        claude_transcript(&host.config.claude_home, PART, "/home/me/code/app", 30);
        crate::host::sessions::scan::scan(&host).finished().await;
        let codex = paths::session_id(AgentId::Codex, THREAD).unwrap();
        let items = preview(&host, &codex, 2).await.unwrap();
        assert_eq!(texts(&items), ["kept answer", "third prompt"]);
        let claude = paths::session_id(AgentId::Claude, PART).unwrap();
        let items = preview(&host, &claude, 3).await.unwrap();
        assert_eq!(texts(&items), ["answer 28", "prompt 29", "answer 29"]);
        assert_eq!(
            preview(&host, &format!("sc{}", "a".repeat(32)), 3).await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert!(store.db().load_threads().unwrap().is_empty());
    }
}
