// SPDX-License-Identifier: MPL-2.0
//! The one-time import of 0.1.18's file store (§U3.3): `threads/<id>/` with
//! `meta.json`, `items.jsonl` and `approvals.jsonl`.
//!
//! The files are read exactly as the file store read them (the readers below
//! are its own): an unreadable line is skipped, an item of a later kind is
//! kept as it was written, each `seq` at its highest `rev`. The database is
//! built as `agents.db.importing`, checked, synced and renamed to `agents.db`;
//! then `threads/` is renamed `threads.v1-imported` and never read again. A
//! crash before the first rename leaves the files as the truth, and the
//! half-made database is deleted at the next start.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::Deserialize;

use super::threads::{self, ThreadCols, raw_kind};
use super::{ApprovalRecord, DB_FILE, Res, items, private_file, schema, sql, sync_dir, turns, with_suffix};
use crate::api::{self, Item, ItemKind, ThreadInfo, Via};
use crate::host::store::now_ms;

/// The file store's folder, and its name once imported.
pub const THREADS_DIR: &str = "threads";
pub const IMPORTED_DIR: &str = "threads.v1-imported";
/// The database while it is being built.
pub const IMPORTING_FILE: &str = "agents.db.importing";
/// A stored item line longer than this is skipped (as the file store did).
const MAX_ITEM_LINE: usize = 1 << 20;
/// `approvals.jsonl` is read up to this, from its end.
const MAX_APPROVALS_READ: u64 = 1 << 20;

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Thread folders with a readable `meta.json`.
    pub readable: u32,
    /// Threads imported, and their items.
    pub threads: u32,
    pub items: u64,
    /// Threads that couldn't be imported, by id. Their files stay in
    /// `threads.v1-imported` with the rest.
    pub failed: Vec<String>,
}

/// `meta.json`.
#[derive(Deserialize)]
struct MetaFile {
    /// Required, as the file store required it; any value.
    #[serde(rename = "version")]
    _version: u32,
    thread: ThreadInfo,
    next_seq: u64,
}

/// A thread as the file store holds it, with each line as it was written.
#[derive(Debug, Clone)]
pub struct V1Thread {
    pub info: ThreadInfo,
    pub next_seq: u64,
    /// By `seq`, at the highest `rev`: the item and its line.
    pub items: BTreeMap<u64, (Item, String)>,
    /// Oldest first: the record and its line.
    pub approvals: Vec<(ApprovalRecord, String)>,
}

/// Calls `f` with each line of `path` within [`MAX_ITEM_LINE`]; longer lines
/// are skipped. A missing file has no lines.
fn each_line(path: &Path, mut f: impl FnMut(&[u8])) -> io::Result<()> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match crate::lines::read_line_blocking(&mut reader, &mut buf, MAX_ITEM_LINE)? {
            crate::lines::LineRead::Eof => return Ok(()),
            crate::lines::LineRead::TooLong => {
                let mut rest = Vec::new();
                reader.read_until(b'\n', &mut rest)?;
            }
            crate::lines::LineRead::Line => f(&buf),
        }
    }
}

/// The items of a thread folder by `seq`, each at its highest `rev`, with
/// their lines. A line that isn't an item is skipped; an item of a kind this
/// version doesn't know reads as [`ItemKind::Unknown`] and keeps its line.
pub fn read_item_lines(dir: &Path) -> io::Result<BTreeMap<u64, (Item, String)>> {
    let mut items: BTreeMap<u64, (Item, String)> = BTreeMap::new();
    each_line(&dir.join("items.jsonl"), |buf| {
        let Ok(item) = serde_json::from_slice::<Item>(buf) else { return };
        // serde_json read it, so it is UTF-8.
        let Ok(line) = String::from_utf8(buf.to_vec()) else { return };
        match items.get(&item.seq) {
            Some((old, _)) if old.rev > item.rev => {}
            _ => {
                items.insert(item.seq, (item, line));
            }
        }
    })?;
    Ok(items)
}

/// The file store's own reader: the items of a thread folder by `seq`.
pub fn read_items(dir: &Path) -> io::Result<BTreeMap<u64, Item>> {
    Ok(read_item_lines(dir)?.into_iter().map(|(seq, (item, _))| (seq, item)).collect())
}

/// The answered approvals of a thread folder (the newest 1 MiB), oldest
/// first, with their lines.
pub fn read_approval_lines(dir: &Path) -> io::Result<Vec<(ApprovalRecord, String)>> {
    let mut file = match File::open(dir.join("approvals.jsonl")) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let size = file.metadata()?.len();
    let from = size.saturating_sub(MAX_APPROVALS_READ);
    file.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::new();
    file.take(MAX_APPROVALS_READ).read_to_end(&mut bytes)?;
    let mut lines = bytes.split(|&b| b == b'\n');
    if from > 0 {
        lines.next();
    }
    Ok(lines
        .filter_map(|l| {
            let record = serde_json::from_slice::<ApprovalRecord>(l).ok()?;
            Some((record, String::from_utf8(l.to_vec()).ok()?))
        })
        .collect())
}

/// The file store's own reader: a thread folder's answered approvals.
pub fn read_approvals(dir: &Path) -> io::Result<Vec<ApprovalRecord>> {
    Ok(read_approval_lines(dir)?.into_iter().map(|(r, _)| r).collect())
}

/// A thread folder, when its `meta.json` can be read and names it.
pub fn read_thread(dir: &Path) -> io::Result<Option<V1Thread>> {
    let id = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let meta = match fs::read(dir.join("meta.json")) {
        Ok(bytes) => serde_json::from_slice::<MetaFile>(&bytes).ok(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let Some(meta) = meta.filter(|m| m.thread.id == id) else { return Ok(None) };
    Ok(Some(V1Thread {
        info: meta.thread,
        next_seq: meta.next_seq,
        items: read_item_lines(dir)?,
        approvals: read_approval_lines(dir)?,
    }))
}

/// Whether `root` has a file store to import.
pub(crate) fn has_file_store(root: &Path) -> bool {
    fs::symlink_metadata(root.join(THREADS_DIR)).is_ok_and(|m| m.is_dir())
}

/// Deletes a database an import didn't finish.
pub(crate) fn discard_half_made(root: &Path) -> io::Result<()> {
    let path = root.join(IMPORTING_FILE);
    for p in [with_suffix(&path, "-journal"), with_suffix(&path, "-wal"), with_suffix(&path, "-shm"), path] {
        match fs::remove_file(&p) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// The thread folders under `threads/` whose names are thread ids, by name.
fn thread_dirs(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(root.join(THREADS_DIR))?
        .flatten()
        .filter(|e| {
            e.file_name().to_str().is_some_and(api::is_valid_id) && e.file_type().is_ok_and(|t| t.is_dir())
        })
        .map(|e| e.path())
        .collect();
    dirs.sort();
    Ok(dirs)
}

/// Imports one thread in one transaction; checks its item count.
fn import_thread(conn: &Connection, t: &V1Thread) -> Res<u64> {
    let tx = conn.unchecked_transaction()?;
    let id = &t.info.id;
    let max_seq = t.items.keys().next_back().copied().unwrap_or(0);
    let max_turn = t.items.values().filter_map(|(i, _)| i.turn).max().unwrap_or(0);
    let users = t
        .items
        .values()
        .filter(|(i, _)| i.kind == ItemKind::User && i.deny_of.is_none() && i.via != Some(Via::Steer))
        .count();
    let users = u32::try_from(users).unwrap_or(u32::MAX);
    let cols = ThreadCols {
        next_seq: t.next_seq.max(max_seq + 1).max(1),
        next_turn: if max_turn > 0 { max_turn } else { users }.saturating_add(1),
        // What came before is read: nothing shows as unread after the update.
        read_seq: t.info.last_seq.max(max_seq),
        ..ThreadCols::default()
    };
    threads::upsert(&tx, &t.info, &cols)?;
    for (item, line) in t.items.values() {
        items::insert(&tx, id, item, &raw_kind(item, line), line)?;
    }
    let parsed: BTreeMap<u64, Item> = t.items.iter().map(|(&seq, (item, _))| (seq, item.clone())).collect();
    for row in turns::rebuild(&parsed, t.info.state) {
        turns::upsert(&tx, id, &row, None)?;
    }
    for (record, line) in &t.approvals {
        items::insert_approval(&tx, id, record.ts_ms, line)?;
    }
    let stored = items::count(&tx, id)?;
    if stored != t.items.len() {
        return Err(io::Error::other(format!("{stored} items stored of {}", t.items.len())).into());
    }
    tx.commit()?;
    Ok(u64::try_from(stored).unwrap_or(u64::MAX))
}

/// Steps 1 to 3: builds `agents.db.importing` from `threads/` and checks it.
/// The files are not changed.
pub fn build(root: &Path) -> io::Result<ImportReport> {
    discard_half_made(root)?;
    let path = root.join(IMPORTING_FILE);
    let result = build_at(root, &path);
    if result.is_err() {
        let _ = discard_half_made(root);
    }
    result
}

fn build_at(root: &Path, path: &Path) -> io::Result<ImportReport> {
    private_file(path)?;
    let conn = Connection::open(path).map_err(sql)?;
    schema::configure_importer(&conn).map_err(sql)?;
    match schema::migrate(&conn) {
        Ok(()) => {}
        Err(schema::MigrateError::Sql(e)) => return Err(sql(e)),
        Err(schema::MigrateError::Newer(_)) => return Err(io::Error::other("unexpected schema")),
    }
    let mut report = ImportReport::default();
    for dir in thread_dirs(root)? {
        let id = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        let thread = match read_thread(&dir) {
            Ok(Some(thread)) => thread,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(thread = %id, "couldn't read a thread to import: {e}");
                continue;
            }
        };
        report.readable += 1;
        match import_thread(&conn, &thread) {
            Ok(items) => {
                report.threads += 1;
                report.items += items;
            }
            Err(e) => {
                tracing::warn!(thread = %id, "couldn't import a thread: {}", io::Error::from(e));
                report.failed.push(id);
            }
        }
    }
    let stored: i64 = conn.query_row("SELECT COUNT(*) FROM threads", [], |r| r.get(0)).map_err(sql)?;
    let expected = report.readable - u32::try_from(report.failed.len()).unwrap_or(u32::MAX);
    if stored != i64::from(expected) || report.threads != expected {
        return Err(io::Error::other(format!("the import stored {stored} threads of {expected}")));
    }
    conn.close().map_err(|(_, e)| sql(e))?;
    File::open(path)?.sync_all()?;
    tracing::info!(
        threads = report.threads,
        items = report.items,
        failed = report.failed.len(),
        "imported the thread files into the thread store"
    );
    Ok(report)
}

/// Step 4: `agents.db.importing` becomes `agents.db`, then the file store is retired.
pub fn finish(root: &Path) -> io::Result<()> {
    fs::rename(root.join(IMPORTING_FILE), root.join(DB_FILE))?;
    sync_dir(root);
    retire(root)
}

/// Renames a `threads/` left beside the database (`threads.v1-imported`, or
/// with the time when that exists); an empty one is removed.
pub(crate) fn retire(root: &Path) -> io::Result<()> {
    let dir = root.join(THREADS_DIR);
    if !has_file_store(root) {
        return Ok(());
    }
    if fs::read_dir(&dir)?.next().is_none() {
        fs::remove_dir(&dir)?;
    } else {
        let mut to = root.join(IMPORTED_DIR);
        if fs::symlink_metadata(&to).is_ok() {
            to = root.join(format!("{IMPORTED_DIR}-{}", now_ms()));
        }
        fs::rename(&dir, &to)?;
    }
    sync_dir(root);
    Ok(())
}
