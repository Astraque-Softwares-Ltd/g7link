// SPDX-License-Identifier: MPL-2.0
//! The database's schema (§U3.2), its migrations and the connection settings.
//!
//! Versions are counted in `PRAGMA user_version`: `MIGRATIONS[n]` takes the
//! database from version `n` to `n + 1`, each in its own transaction.
//!
//! `threads.pending_fork` (not in the contract's schema) holds the encoded
//! launch hint of a Claude fork or rewind that resolves only when the
//! thread's next process starts, so it survives a host restart.
//!
//! `threads` and `found` have an explicit `rowid INTEGER PRIMARY KEY` beside
//! their text ids: the title indexes are keyed by rowid, and only an explicit
//! alias keeps rowids stable across a `VACUUM`.

use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

pub(crate) const MIGRATIONS: &[&str] = &[
    // v1
    "CREATE TABLE threads (
        rowid            INTEGER PRIMARY KEY,
        id               TEXT NOT NULL UNIQUE,
        agent            TEXT NOT NULL,
        folder_id        TEXT,
        cwd              TEXT,
        title            TEXT NOT NULL,
        state            TEXT NOT NULL,
        created_ms       INTEGER NOT NULL,
        updated_ms       INTEGER NOT NULL,
        next_seq         INTEGER NOT NULL,
        next_turn        INTEGER NOT NULL,
        native_id        TEXT,
        prior_native_id  TEXT,
        pinned           INTEGER NOT NULL DEFAULT 0,
        archived_ms      INTEGER,
        snoozed_until_ms INTEGER,
        read_seq         INTEGER NOT NULL DEFAULT 0,
        unread_mark      INTEGER NOT NULL DEFAULT 0,
        forked_from      TEXT,
        forked_turn      INTEGER,
        found_id         TEXT,
        pending_fork     TEXT,
        info             TEXT NOT NULL
    );
    CREATE INDEX threads_by_time ON threads (archived_ms, updated_ms DESC);

    CREATE TABLE items (
        rowid     INTEGER PRIMARY KEY,
        thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
        seq       INTEGER NOT NULL,
        turn      INTEGER,
        kind      TEXT NOT NULL,
        ts_ms     INTEGER NOT NULL,
        cut       INTEGER NOT NULL DEFAULT 0,
        body      TEXT NOT NULL,
        UNIQUE (thread_id, seq)
    );

    CREATE TABLE turns (
        thread_id   TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
        turn        INTEGER NOT NULL,
        prompt_seq  INTEGER NOT NULL,
        last_seq    INTEGER,
        started_ms  INTEGER NOT NULL,
        ended_ms    INTEGER,
        ended       TEXT,
        native_mark TEXT,
        checkpoint  TEXT NOT NULL,
        why         TEXT,
        pre_oid     TEXT,
        post_oid    TEXT,
        pre_head    TEXT,
        post_head   TEXT,
        files       INTEGER NOT NULL DEFAULT 0,
        adds        INTEGER NOT NULL DEFAULT 0,
        dels        INTEGER NOT NULL DEFAULT 0,
        skipped     TEXT,
        cut         INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (thread_id, turn)
    ) WITHOUT ROWID;

    CREATE TABLE turn_files (
        thread_id TEXT NOT NULL,
        turn      INTEGER NOT NULL,
        path      TEXT NOT NULL,
        status    TEXT NOT NULL,
        from_path TEXT,
        adds      INTEGER,
        dels      INTEGER,
        binary    INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (thread_id, turn, path),
        FOREIGN KEY (thread_id, turn) REFERENCES turns(thread_id, turn) ON DELETE CASCADE
    ) WITHOUT ROWID;

    CREATE TABLE approvals (
        thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
        ts_ms     INTEGER NOT NULL,
        record    TEXT NOT NULL
    );
    CREATE INDEX approvals_by_thread ON approvals (thread_id);

    CREATE TABLE send_keys (
        thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
        key       TEXT NOT NULL,
        ts_ms     INTEGER NOT NULL,
        PRIMARY KEY (thread_id, key)
    ) WITHOUT ROWID;

    CREATE TABLE found (
        rowid       INTEGER PRIMARY KEY,
        id          TEXT NOT NULL UNIQUE,
        agent       TEXT NOT NULL,
        native_id   TEXT NOT NULL,
        cwd         TEXT,
        folder_id   TEXT,
        title       TEXT NOT NULL,
        first_ms    INTEGER,
        updated_ms  INTEGER NOT NULL,
        bytes       INTEGER NOT NULL,
        source      TEXT,
        base_thread TEXT,
        base_end    INTEGER,
        damaged     INTEGER NOT NULL DEFAULT 0,
        thread_id   TEXT
    );
    CREATE INDEX found_by_time ON found (updated_ms DESC);

    CREATE TABLE found_files (
        found_id TEXT NOT NULL REFERENCES found(id) ON DELETE CASCADE,
        path     TEXT NOT NULL,
        dev      INTEGER NOT NULL,
        ino      INTEGER NOT NULL,
        bytes    INTEGER NOT NULL,
        mtime_ms INTEGER NOT NULL,
        stamp    TEXT NOT NULL,
        packed   INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (found_id, path)
    ) WITHOUT ROWID;

    CREATE VIRTUAL TABLE item_text USING fts5(
        text, tokenize = 'trigram remove_diacritics 1', content = '', contentless_delete = 1
    );
    CREATE VIRTUAL TABLE title_text USING fts5(
        title, tokenize = 'trigram remove_diacritics 1', content = '', contentless_delete = 1
    );
    CREATE VIRTUAL TABLE found_text USING fts5(
        title, folder, tokenize = 'trigram remove_diacritics 1', content = '', contentless_delete = 1
    );",
];

/// How long a statement waits for another connection's lock.
const BUSY_TIMEOUT: Duration = Duration::from_millis(2000);
/// The write-ahead log shrinks back to this after a checkpoint.
const JOURNAL_SIZE_LIMIT: i64 = 4 * 1024 * 1024;

/// The write connection's settings (§U3.1).
pub(crate) fn configure_writer(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(())
}

/// The connection an import builds the database through: no write-ahead log,
/// so once it is closed the file is the whole database.
pub(crate) fn configure_importer(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "journal_mode", "DELETE")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(())
}

/// A read-only connection to the database at `path`.
pub(crate) fn open_reader(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "query_only", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(conn)
}

/// What [`migrate`] found.
#[derive(Debug)]
pub(crate) enum MigrateError {
    /// Written by a newer G7 (its schema version).
    Newer(usize),
    Sql(rusqlite::Error),
}

impl From<rusqlite::Error> for MigrateError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}

/// Brings the schema up to date, one transaction per version.
pub(crate) fn migrate(conn: &Connection) -> Result<(), MigrateError> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let current = usize::try_from(current).unwrap_or(usize::MAX);
    if current > MIGRATIONS.len() {
        return Err(MigrateError::Newer(current));
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let version = i64::try_from(index + 1).unwrap_or(i64::MAX);
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
        tracing::debug!(version, "applied an agents store migration");
    }
    Ok(())
}
