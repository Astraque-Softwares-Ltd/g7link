// SPDX-License-Identifier: MPL-2.0
//! The thread store (§U3): one SQLite database, `$XDG_DATA_HOME/g7-agents/agents.db`,
//! holding threads, their items, turns, answered approvals, message keys and
//! the sessions found on disk, with full-text indexes over messages and titles.
//!
//! One writer: the store thread owns [`Db`] and its connection, so writes keep
//! their order and never block the async runtime. Item, thread, turn and
//! approval writes are batched: one transaction per [`BATCH_WINDOW`] or
//! [`BATCH_OPS`] writes, whichever comes first. The store thread waits on its
//! queue until [`Db::flush_deadline`] and then calls [`Db::tick`];
//! [`Db::flush`] commits at once. Writes that must land together (a rewind's
//! cut, a fork's copy, a delete, a message key) first commit the batch and
//! then run in a transaction of their own.
//!
//! [`Reader`] is a second, read-only connection for `thread.read`,
//! `thread.turns` and search on the blocking pool. It sees what the writer
//! has committed.
//!
//! The file store of 0.1.18 (`threads/`) is imported once ([`import_v1`]). A
//! database that fails its check is moved aside as `agents.db.unreadable-<ms>`
//! and never overwritten.

mod found;
pub mod import_v1;
mod items;
mod schema;
mod search;
mod threads;
mod turns;

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rusqlite::Connection;

use crate::api::{Item, ThreadInfo, TurnFile};
pub use crate::host::store::ApprovalRecord;
use crate::host::store::now_ms;

pub use found::{FoundFile, FoundPage, FoundQuery, FoundRow};
pub use import_v1::ImportReport;
pub use search::{Interrupt, SearchPage, SearchQuery, fold_text, searchable};
pub use threads::{LoadedThread, ThreadCols};
pub use turns::{CutRange, MAX_SKIPPED, TurnRow, turn_refs, undo_ref};

/// The database's file name in the data directory.
pub const DB_FILE: &str = "agents.db";
/// A batch commits after this many writes…
pub const BATCH_OPS: u32 = 64;
/// …or this long after its first write, whichever comes first.
pub const BATCH_WINDOW: Duration = Duration::from_millis(100);
/// An item's JSON is at most this long (the file store's line limit).
pub const MAX_ITEM_JSON: usize = 1 << 20;

/// `store.info`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoreInfo {
    pub threads: u32,
    pub archived: u32,
    /// Items that are not cut.
    pub items: u64,
    /// The database file and its write-ahead log.
    pub bytes: u64,
    pub found: u32,
}

pub(crate) fn sql(e: rusqlite::Error) -> io::Error {
    match e {
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::OperationInterrupted => {
            io::Error::new(io::ErrorKind::Interrupted, "the search was stopped")
        }
        e => io::Error::other(e),
    }
}

/// What a store operation can fail with, inside the module.
#[derive(Debug)]
pub(crate) enum Fail {
    Sql(rusqlite::Error),
    Io(io::Error),
}

pub(crate) type Res<T> = Result<T, Fail>;

impl From<rusqlite::Error> for Fail {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}

impl From<io::Error> for Fail {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for Fail {
    fn from(e: serde_json::Error) -> Self {
        Self::Io(io::Error::other(e))
    }
}

impl From<Fail> for io::Error {
    fn from(e: Fail) -> Self {
        match e {
            Fail::Sql(e) => sql(e),
            Fail::Io(e) => e,
        }
    }
}

pub(crate) fn invalid(message: &'static str) -> Fail {
    Fail::Io(io::Error::new(io::ErrorKind::InvalidInput, message))
}

pub(crate) fn not_found(message: &'static str) -> Fail {
    Fail::Io(io::Error::new(io::ErrorKind::NotFound, message))
}

pub(crate) fn interrupted() -> Fail {
    Fail::Io(io::Error::new(io::ErrorKind::Interrupted, "the search was stopped"))
}

/// A `u64` as SQLite's `INTEGER`.
pub(crate) fn int(v: u64) -> rusqlite::Result<i64> {
    i64::try_from(v).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn from_int<T: TryFrom<i64>>(idx: usize, v: i64) -> rusqlite::Result<T>
where
    T::Error: std::error::Error + Send + Sync + 'static,
{
    T::try_from(v).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Integer, Box::new(e))
    })
}

/// An `INTEGER` column as a `u64`.
pub(crate) fn uint(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<u64> {
    from_int(idx, row.get::<_, i64>(idx)?)
}

/// A nullable `INTEGER` column as a `u64`.
pub(crate) fn opt_uint(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<Option<u64>> {
    row.get::<_, Option<i64>>(idx)?.map(|v| from_int(idx, v)).transpose()
}

/// An `INTEGER` column as a `u32`.
pub(crate) fn uint32(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<u32> {
    from_int(idx, row.get::<_, i64>(idx)?)
}

/// A nullable `INTEGER` column as a `u32`.
pub(crate) fn opt_uint32(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<Option<u32>> {
    row.get::<_, Option<i64>>(idx)?.map(|v| from_int(idx, v)).transpose()
}

/// Whether the error says the file isn't a database we can read.
fn is_unreadable(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, _)
            if matches!(f.code, rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt)
    )
}

/// Creates `path` empty and 0600 when it doesn't exist, and makes it 0600 when it does.
pub(crate) fn private_file(path: &Path) -> io::Result<()> {
    match OpenOptions::new().write(true).create_new(true).mode(0o600).open(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let meta = fs::symlink_metadata(path)?;
            if !meta.is_file() {
                return Err(io::Error::other("the thread store is not a regular file"));
            }
            if meta.permissions().mode() & 0o077 != 0 {
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub(crate) fn sync_dir(dir: &Path) {
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
}

/// `<path><suffix>`.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Moves an unreadable database (and its write-ahead log) aside so nothing
/// overwrites it. Returns where it went.
fn set_aside(path: &Path) -> io::Result<PathBuf> {
    let ms = now_ms();
    let mut aside = with_suffix(path, &format!(".unreadable-{ms}"));
    let mut n = 1;
    while fs::symlink_metadata(&aside).is_ok() {
        aside = with_suffix(path, &format!(".unreadable-{ms}-{n}"));
        n += 1;
    }
    fs::rename(path, &aside)?;
    let wal = with_suffix(path, "-wal");
    if fs::symlink_metadata(&wal).is_ok() {
        fs::rename(&wal, with_suffix(&aside, "-wal"))?;
    }
    let _ = fs::remove_file(with_suffix(path, "-shm"));
    if let Some(dir) = path.parent() {
        sync_dir(dir);
    }
    tracing::warn!("moved an unreadable thread store aside");
    Ok(aside)
}

/// Opens the write connection: settings, a quick check, the schema. The flag
/// says the file can't be read as a database.
fn open_writer(path: &Path) -> Result<Connection, (bool, io::Error)> {
    let fail = |e: rusqlite::Error| (is_unreadable(&e), sql(e));
    private_file(path).map_err(|e| (false, e))?;
    let conn = Connection::open(path).map_err(fail)?;
    schema::configure_writer(&conn).map_err(fail)?;
    let check: String = conn.query_row("PRAGMA quick_check(1)", [], |r| r.get(0)).map_err(fail)?;
    if check != "ok" {
        return Err((true, io::Error::other("the thread store failed its check")));
    }
    match schema::migrate(&conn) {
        Ok(()) => Ok(conn),
        Err(schema::MigrateError::Newer(v)) => {
            Err((false, io::Error::other(format!("the thread store was written by a newer G7 (schema {v})"))))
        }
        Err(schema::MigrateError::Sql(e)) => Err(fail(e)),
    }
}

/// The batch transaction that is open.
#[derive(Debug)]
struct Batch {
    started: Instant,
    ops: u32,
}

/// The store's write side. Owned by the store thread.
#[derive(Debug)]
pub struct Db {
    conn: Connection,
    path: PathBuf,
    batch: Option<Batch>,
    /// Items per thread, counted on first use, for the [`crate::api::MAX_ITEMS_STORED`] rule.
    counts: HashMap<String, usize>,
    /// Where an unreadable database was moved at this open.
    set_aside: Option<PathBuf>,
    /// What the import of the file store did at this open.
    imported: Option<ImportReport>,
}

impl Db {
    /// Opens `root/agents.db`, importing `root/threads/` first when the
    /// database doesn't exist yet. A database that can't be read is moved
    /// aside ([`Db::set_aside`]) and a new one is made.
    pub fn open(root: &Path) -> io::Result<Db> {
        fs::DirBuilder::new().recursive(true).mode(0o700).create(root)?;
        let path = root.join(DB_FILE);
        import_v1::discard_half_made(root)?;
        let mut imported = None;
        if fs::symlink_metadata(&path).is_err() {
            if import_v1::has_file_store(root) {
                let report = import_v1::build(root)?;
                import_v1::finish(root)?;
                imported = Some(report);
            }
        } else {
            import_v1::retire(root)?;
        }
        let (conn, aside) = match open_writer(&path) {
            Ok(conn) => (conn, None),
            Err((true, e)) => {
                tracing::warn!("the thread store can't be read: {e}");
                let aside = set_aside(&path)?;
                let conn = open_writer(&path).map_err(|(_, e)| e)?;
                (conn, Some(aside))
            }
            Err((false, e)) => return Err(e),
        };
        Ok(Db { conn, path, batch: None, counts: HashMap::new(), set_aside: aside, imported })
    }

    /// The database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where an unreadable database was moved when this one was opened.
    pub fn set_aside(&self) -> Option<&Path> {
        self.set_aside.as_deref()
    }

    /// What the one-time import of the file store did, when it ran at this open.
    pub fn imported(&self) -> Option<&ImportReport> {
        self.imported.as_ref()
    }

    /// A read-only connection to the same database.
    pub fn reader(&self) -> io::Result<Reader> {
        Reader::open(&self.path)
    }

    // ---- Batching ----

    /// Runs `f` inside the open batch (opening one if needed) as one unit: if
    /// it fails, none of its changes stay. Commits when the batch is full.
    fn write<T>(&mut self, f: impl FnOnce(&Connection) -> Res<T>) -> io::Result<T> {
        // SQLite ends a transaction itself on some errors (a full disk).
        if self.batch.is_some() && self.conn.is_autocommit() {
            self.batch = None;
            self.counts.clear();
        }
        if self.batch.is_none() {
            self.conn.execute_batch("BEGIN IMMEDIATE").map_err(sql)?;
            self.batch = Some(Batch { started: Instant::now(), ops: 0 });
        }
        self.conn.execute_batch("SAVEPOINT op").map_err(sql)?;
        let result = f(&self.conn);
        let end = if result.is_ok() { "RELEASE op" } else { "ROLLBACK TO op; RELEASE op" };
        if let Err(e) = self.conn.execute_batch(end) {
            if self.conn.is_autocommit() {
                self.batch = None;
                self.counts.clear();
            }
            return Err(sql(e));
        }
        let value = result?;
        let full = self.batch.as_mut().is_some_and(|b| {
            b.ops += 1;
            b.ops >= BATCH_OPS
        });
        if full {
            self.flush()?;
        }
        Ok(value)
    }

    /// Commits the batch, then runs `f` in a transaction of its own.
    fn exclusive<T>(&mut self, f: impl FnOnce(&Connection) -> Res<T>) -> io::Result<T> {
        self.flush()?;
        let tx = self.conn.unchecked_transaction().map_err(sql)?;
        let value = f(&tx)?;
        tx.commit().map_err(sql)?;
        Ok(value)
    }

    /// Commits the open batch now.
    pub fn flush(&mut self) -> io::Result<()> {
        if self.batch.take().is_none() {
            return Ok(());
        }
        if let Err(e) = self.conn.execute_batch("COMMIT") {
            // No transaction is left open behind a failed commit; its writes are lost.
            if !self.conn.is_autocommit() {
                let _ = self.conn.execute_batch("ROLLBACK");
            }
            self.counts.clear();
            return Err(sql(e));
        }
        Ok(())
    }

    /// When the open batch must be committed, if one is open.
    pub fn flush_deadline(&self) -> Option<Instant> {
        self.batch.as_ref().map(|b| b.started + BATCH_WINDOW)
    }

    /// Commits the batch if its time is up.
    pub fn tick(&mut self) -> io::Result<()> {
        match self.flush_deadline() {
            Some(due) if Instant::now() >= due => self.flush(),
            _ => Ok(()),
        }
    }

    // ---- Threads ----

    /// Inserts or replaces a thread.
    pub fn save_thread(&mut self, info: &ThreadInfo, cols: &ThreadCols) -> io::Result<()> {
        if !crate::api::is_valid_id(&info.id) {
            return Err(invalid("bad thread id").into());
        }
        self.write(|c| threads::upsert(c, info, cols).map(|_| ()))
    }

    /// Every thread, with what the host needs to take it up again.
    pub fn load_threads(&self) -> io::Result<Vec<LoadedThread>> {
        Ok(threads::load(&self.conn, None)?)
    }

    /// One thread, as [`Db::load_threads`] gives it.
    pub fn load_thread(&self, thread: &str) -> io::Result<Option<LoadedThread>> {
        Ok(threads::load(&self.conn, Some(thread))?.pop())
    }

    /// Deletes a thread with its items, turns, approvals and keys.
    pub fn delete_thread(&mut self, thread: &str) -> io::Result<()> {
        self.counts.remove(thread);
        self.exclusive(|c| threads::delete(c, thread))
    }

    /// Makes `new` a copy of `from` through turn `through_turn` (§U6.5): the
    /// newest [`crate::api::FORK_ITEMS`] items of those turns with new `seq`s
    /// and the same turn numbers, their turns (marks kept, no restore points)
    /// and their answered approvals. `new` must not exist yet; its `seq`s go
    /// on from [`ThreadCols::next_seq`] of [`Db::load_thread`].
    pub fn copy_thread(&mut self, from: &str, through_turn: u32, new: &ThreadInfo) -> io::Result<()> {
        if !crate::api::is_valid_id(&new.id) {
            return Err(invalid("bad thread id").into());
        }
        self.exclusive(|c| threads::copy(c, from, through_turn, new))
    }

    /// Removes the threads past [`crate::api::MAX_THREADS_STORED`]: archived
    /// unpinned ones first, then idle unpinned ones, oldest first; never one
    /// that is working, waiting or pinned. Returns their ids.
    pub fn prune_threads(&mut self) -> io::Result<Vec<String>> {
        let ids = threads::prunable(&self.conn, crate::api::MAX_THREADS_STORED)?;
        if ids.is_empty() {
            return Ok(ids);
        }
        for id in &ids {
            self.counts.remove(id);
        }
        self.exclusive(|c| ids.iter().try_for_each(|id| threads::delete(c, id)))?;
        Ok(ids)
    }

    /// Takes a message key for a thread (§U8.3): `true` the first time within
    /// [`crate::api::SEND_KEY_MS`], `false` for a duplicate. The thread must exist.
    pub fn take_send_key(&mut self, thread: &str, key: &str, now_ms: u64) -> io::Result<bool> {
        if !crate::api::is_valid_send_key(key) {
            return Err(invalid("bad message key").into());
        }
        self.exclusive(|c| threads::take_send_key(c, thread, key, now_ms))
    }

    /// Gives a message key back, for a send that was refused after taking it
    /// (`BUSY`, a full queue), so the sender's retry is not a duplicate.
    pub fn release_send_key(&mut self, thread: &str, key: &str) -> io::Result<()> {
        self.exclusive(|c| threads::release_send_key(c, thread, key))
    }

    // ---- Items ----

    /// Inserts an item, or replaces the one with its `seq` unless that one has
    /// a newer `rev`. Finished messages go into the search index. The thread
    /// must have been saved first.
    pub fn append_item(&mut self, thread: &str, item: &Item) -> io::Result<()> {
        let body = serde_json::to_string(item).map_err(io::Error::other)?;
        if body.len() > MAX_ITEM_JSON {
            return Err(invalid("item too large").into());
        }
        let inserted = self.write(|c| items::upsert(c, thread, item, &body))?;
        if inserted {
            self.count_one(thread)?;
        }
        Ok(())
    }

    /// One more item in `thread`; past the limit, the oldest go.
    fn count_one(&mut self, thread: &str) -> io::Result<()> {
        let count = match self.counts.get(thread) {
            Some(&n) => n + 1,
            None => items::count(&self.conn, thread)?,
        };
        let count = if count > crate::api::MAX_ITEMS_STORED {
            self.write(|c| items::trim(c, thread, crate::api::MAX_ITEMS_STORED))?
        } else {
            count
        };
        self.counts.insert(thread.to_owned(), count);
        Ok(())
    }

    /// The newest `limit` items before `before` (from the end when absent),
    /// oldest first. Cut items are never returned.
    pub fn read_items(&self, thread: &str, before: Option<u64>, limit: u32) -> io::Result<Vec<Item>> {
        Ok(items::read_before(&self.conn, thread, before, limit)?)
    }

    /// The oldest `limit` items after `after`, oldest first.
    pub fn read_items_after(&self, thread: &str, after: u64, limit: u32) -> io::Result<Vec<Item>> {
        Ok(items::read_after(&self.conn, thread, after, limit)?)
    }

    /// Every item that is not cut, by `seq` (what an export reads).
    pub fn all_items(&self, thread: &str) -> io::Result<BTreeMap<u64, Item>> {
        Ok(items::read_all(&self.conn, thread)?)
    }

    pub fn append_approval(&mut self, thread: &str, record: &ApprovalRecord) -> io::Result<()> {
        let text = serde_json::to_string(record).map_err(io::Error::other)?;
        self.write(|c| items::insert_approval(c, thread, record.ts_ms, &text))
    }

    /// The thread's answered approvals, oldest first.
    pub fn read_approvals(&self, thread: &str) -> io::Result<Vec<ApprovalRecord>> {
        Ok(items::read_approvals(&self.conn, thread)?)
    }

    // ---- Turns ----

    /// Inserts or replaces a turn; `files` (the first [`crate::api::MAX_TURN_FILES`])
    /// replaces its file list when given.
    pub fn save_turn(&mut self, thread: &str, turn: &TurnRow, files: Option<&[TurnFile]>) -> io::Result<()> {
        self.write(|c| turns::upsert(c, thread, turn, files))
    }

    /// The newest `limit` turns before `before` (from the end when absent),
    /// newest first. Cut turns are left out.
    pub fn turns(&self, thread: &str, before: Option<u32>, limit: u32) -> io::Result<Vec<TurnRow>> {
        Ok(turns::read(&self.conn, thread, before, limit)?)
    }

    /// One turn, cut or not.
    pub fn turn(&self, thread: &str, turn: u32) -> io::Result<Option<TurnRow>> {
        Ok(turns::read_one(&self.conn, thread, turn)?)
    }

    /// A turn's files as recorded.
    pub fn turn_files(&self, thread: &str, turn: u32) -> io::Result<Vec<TurnFile>> {
        Ok(turns::read_files(&self.conn, thread, turn)?)
    }

    /// What [`Db::cut`] would cut, changing nothing.
    pub fn cut_preview(&self, thread: &str, from_turn: u32) -> io::Result<CutRange> {
        Ok(turns::preview_cut(&self.conn, thread, from_turn)?)
    }

    /// Cuts turn `from_turn` and everything after it (§U6.3 step 4) in one
    /// transaction: the items and turns get `cut` and leave the search index,
    /// and the thread continues from `native` (empty: a new session at its
    /// next message), its old native id kept for [`Db::uncut`]. Refused while
    /// an earlier cut can still be undone: [`Db::purge_cut`] that first.
    pub fn cut(&mut self, thread: &str, from_turn: u32, native: &str) -> io::Result<CutRange> {
        self.exclusive(|c| turns::cut(c, thread, from_turn, native))
    }

    /// Undoes the cut: the items and turns come back (and into the index),
    /// the notices written since are removed, the native id goes back.
    pub fn uncut(&mut self, thread: &str) -> io::Result<CutRange> {
        self.counts.remove(thread);
        self.exclusive(|c| turns::uncut(c, thread))
    }

    /// Deletes what was cut, for good. Returns the refs of their restore
    /// points and of the rewind's undo point, to delete.
    pub fn purge_cut(&mut self, thread: &str) -> io::Result<Vec<String>> {
        self.counts.remove(thread);
        self.exclusive(|c| turns::purge(c, thread))
    }

    /// Marks the restore points of all but the newest `keep` turns that have
    /// one `expired`. Returns their refs, to delete.
    pub fn expire_turns(&mut self, thread: &str, keep: u32) -> io::Result<Vec<String>> {
        self.exclusive(|c| turns::expire(c, thread, keep))
    }

    // ---- Search and the sessions found ----

    /// `threads.search` (§U3.4); `stop` ends it early with [`io::ErrorKind::Interrupted`].
    pub fn search(&self, q: &SearchQuery, stop: &Interrupt) -> io::Result<SearchPage> {
        Ok(search::search(&self.conn, q, stop)?)
    }

    /// Inserts or updates sessions found on disk; each row's files replace
    /// the ones stored for it. A row's link to a thread is kept.
    pub fn upsert_found(&mut self, rows: &[FoundRow]) -> io::Result<()> {
        self.exclusive(|c| found::upsert(c, rows))
    }

    pub fn found(&self, q: &FoundQuery) -> io::Result<FoundPage> {
        Ok(found::query(&self.conn, q)?)
    }

    pub fn found_row(&self, id: &str) -> io::Result<Option<FoundRow>> {
        Ok(found::one(&self.conn, id)?)
    }

    /// Every stored file of every session found, with its session's id.
    pub fn found_files(&self) -> io::Result<Vec<(String, FoundFile)>> {
        Ok(found::all_files(&self.conn)?)
    }

    /// Drops `(session id, path)` files that are no longer on disk; their sessions stay.
    pub fn forget_found_files(&mut self, files: &[(String, String)]) -> io::Result<()> {
        self.exclusive(|c| found::forget_files(c, files))
    }

    /// Links a session found to the thread brought in from it, or unlinks it.
    pub fn set_found_thread(&mut self, found_id: &str, thread: Option<&str>) -> io::Result<()> {
        self.exclusive(|c| found::set_thread(c, found_id, thread))
    }

    /// `store.info`.
    pub fn info(&self) -> io::Result<StoreInfo> {
        Ok(info(&self.conn, &self.path)?)
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        if let Err(e) = self.flush() {
            tracing::warn!("couldn't save the last thread changes: {e}");
        }
    }
}

fn info(conn: &Connection, path: &Path) -> Res<StoreInfo> {
    let (threads, archived): (u32, u32) =
        conn.query_row("SELECT COUNT(*), COUNT(archived_ms) FROM threads", [], |r| {
            Ok((uint32(r, 0)?, uint32(r, 1)?))
        })?;
    let items = conn.query_row("SELECT COUNT(*) FROM items WHERE cut = 0", [], |r| uint(r, 0))?;
    let found = conn.query_row("SELECT COUNT(*) FROM found", [], |r| uint32(r, 0))?;
    let size = |p: &Path| fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    Ok(StoreInfo { threads, archived, items, bytes: size(path) + size(&with_suffix(path, "-wal")), found })
}

/// A read-only connection, for the blocking pool. One call runs at a time.
#[derive(Debug)]
pub struct Reader {
    conn: Mutex<Connection>,
    path: PathBuf,
}

impl Reader {
    fn open(path: &Path) -> io::Result<Reader> {
        let conn = schema::open_reader(path).map_err(sql)?;
        Ok(Reader { conn: Mutex::new(conn), path: path.to_owned() })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn read_items(&self, thread: &str, before: Option<u64>, limit: u32) -> io::Result<Vec<Item>> {
        Ok(items::read_before(&self.conn(), thread, before, limit)?)
    }

    pub fn read_items_after(&self, thread: &str, after: u64, limit: u32) -> io::Result<Vec<Item>> {
        Ok(items::read_after(&self.conn(), thread, after, limit)?)
    }

    pub fn all_items(&self, thread: &str) -> io::Result<BTreeMap<u64, Item>> {
        Ok(items::read_all(&self.conn(), thread)?)
    }

    pub fn read_approvals(&self, thread: &str) -> io::Result<Vec<ApprovalRecord>> {
        Ok(items::read_approvals(&self.conn(), thread)?)
    }

    pub fn turns(&self, thread: &str, before: Option<u32>, limit: u32) -> io::Result<Vec<TurnRow>> {
        Ok(turns::read(&self.conn(), thread, before, limit)?)
    }

    pub fn turn(&self, thread: &str, turn: u32) -> io::Result<Option<TurnRow>> {
        Ok(turns::read_one(&self.conn(), thread, turn)?)
    }

    pub fn turn_files(&self, thread: &str, turn: u32) -> io::Result<Vec<TurnFile>> {
        Ok(turns::read_files(&self.conn(), thread, turn)?)
    }

    pub fn cut_preview(&self, thread: &str, from_turn: u32) -> io::Result<CutRange> {
        Ok(turns::preview_cut(&self.conn(), thread, from_turn)?)
    }

    /// `threads.search`. A newer search from the same client stops this one
    /// through its [`Interrupt`]; it then fails with [`io::ErrorKind::Interrupted`].
    pub fn search(&self, q: &SearchQuery, stop: &Interrupt) -> io::Result<SearchPage> {
        if stop.is_stopped() {
            return Err(interrupted().into());
        }
        Ok(search::search(&self.conn(), q, stop)?)
    }

    pub fn found(&self, q: &FoundQuery) -> io::Result<FoundPage> {
        Ok(found::query(&self.conn(), q)?)
    }

    pub fn found_row(&self, id: &str) -> io::Result<Option<FoundRow>> {
        Ok(found::one(&self.conn(), id)?)
    }

    pub fn info(&self) -> io::Result<StoreInfo> {
        Ok(info(&self.conn(), &self.path)?)
    }
}
