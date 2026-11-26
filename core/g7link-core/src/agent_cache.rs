// SPDX-License-Identifier: MPL-2.0
//! The phone's saved agent threads and its held messages
//! (`docs/protocol/agents.md` §6): what a PC's threads looked like when it
//! was last reachable, so they read at once and offline, and the messages
//! written while it wasn't, sent once it's back.
//!
//! Its own SQLite file beside the core's store, opened only on a device that
//! follows a PC's agents. It's a cache: a file that can't be read is put
//! aside and started again, and nothing here needs a migration. Bodies are
//! the wire structs in CBOR. Kept: each PC's newest 200 threads, and items
//! for the 20 threads opened last, newest 400 each, 64 MiB in all, the
//! least recently opened going first; and each kept thread's last
//! source-control strip (§8).

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use g7link_protocol::{
    DeviceId, Envelope,
    messages::{AgentGitChanged, AgentItem, AgentItems, AgentThread, types},
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{Error, Result};

/// The cache's file, in the core's data directory.
pub(crate) const FILE: &str = "agents-cache.db";
/// Threads kept per PC: the newest.
pub(crate) const MAX_THREADS: usize = 200;
/// Threads whose items are kept per PC: the ones opened last.
pub(crate) const MAX_OPENED: usize = 20;
/// Items kept per thread: the newest.
pub(crate) const MAX_ITEMS: usize = 400;
/// What the saved bodies may take in all.
pub(crate) const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Held messages per PC.
pub const MAX_HELD: usize = 20;
/// How long a held message may wait before it's never sent by itself.
pub const HELD_FOR_MS: u64 = 24 * 60 * 60 * 1000;
/// The schema this build writes.
const VERSION: i64 = 1;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS threads (
        pc TEXT NOT NULL, id TEXT NOT NULL, updated_ms INTEGER NOT NULL, saved_ms INTEGER NOT NULL,
        body BLOB NOT NULL, PRIMARY KEY (pc, id)
    ) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS items (
        pc TEXT NOT NULL, thread TEXT NOT NULL, seq INTEGER NOT NULL, body BLOB NOT NULL,
        PRIMARY KEY (pc, thread, seq)
    ) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS opened (
        pc TEXT NOT NULL, thread TEXT NOT NULL, at_ms INTEGER NOT NULL, saved_ms INTEGER NOT NULL,
        PRIMARY KEY (pc, thread)
    ) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS git (
        pc TEXT NOT NULL, thread TEXT NOT NULL, saved_ms INTEGER NOT NULL, body BLOB NOT NULL,
        PRIMARY KEY (pc, thread)
    ) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS outbox (
        n INTEGER PRIMARY KEY AUTOINCREMENT, pc TEXT NOT NULL, key TEXT NOT NULL, thread TEXT NOT NULL,
        text TEXT NOT NULL, created_ms INTEGER NOT NULL, state TEXT NOT NULL, why TEXT,
        tries INTEGER NOT NULL DEFAULT 0, UNIQUE (pc, key)
    );
";

/// Where a held message stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldState {
    /// Waiting for the PC: "Sends when <PC> is back" / "Waiting to send".
    Waiting,
    /// On its way now: "Sending".
    Sending,
    /// The PC is back but can't take it yet (`why` says why); it's tried
    /// again when the thread changes.
    Blocked,
    /// It won't go by itself (`why` says why): "Not sent", with Send and Discard.
    NotSent,
}

impl HeldState {
    fn word(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Sending => "sending",
            Self::Blocked => "blocked",
            Self::NotSent => "not_sent",
        }
    }

    fn from_word(word: &str) -> Self {
        match word {
            "sending" => Self::Sending,
            "blocked" => Self::Blocked,
            "not_sent" => Self::NotSent,
            _ => Self::Waiting,
        }
    }
}

/// Why a held message isn't on its way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldWhy {
    /// The thread waits for an answer: "Answer the request first".
    AnswerFirst,
    /// The thread's queue on the PC is full: "Waiting: the queue on <PC> is full".
    QueueFull,
    /// The thread is gone from the PC.
    Gone,
    /// The thread can be read but not continued.
    ReadOnly,
    /// It waited more than a day: an old instruction isn't sent unannounced.
    TooOld,
    /// The PC refused it.
    Refused,
}

impl HeldWhy {
    fn word(self) -> &'static str {
        match self {
            Self::AnswerFirst => "answer_first",
            Self::QueueFull => "queue_full",
            Self::Gone => "gone",
            Self::ReadOnly => "read_only",
            Self::TooOld => "too_old",
            Self::Refused => "refused",
        }
    }

    fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "answer_first" => Self::AnswerFirst,
            "queue_full" => Self::QueueFull,
            "gone" => Self::Gone,
            "read_only" => Self::ReadOnly,
            "too_old" => Self::TooOld,
            "refused" => Self::Refused,
            _ => return None,
        })
    }
}

/// A message written while its PC couldn't take it.
#[derive(Clone, PartialEq, Eq)]
pub struct AgentHeld {
    pub pc: DeviceId,
    /// Sent with it, so the PC takes it once however often it's tried.
    pub key: String,
    pub thread: String,
    pub text: String,
    /// When it was written (or the user last tapped Send), Unix milliseconds.
    pub created_ms: u64,
    pub state: HeldState,
    pub why: Option<HeldWhy>,
}

/// Never prints the text.
impl std::fmt::Debug for AgentHeld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHeld")
            .field("thread", &self.thread)
            .field("bytes", &self.text.len())
            .field("state", &self.state)
            .field("why", &self.why)
            .finish_non_exhaustive()
    }
}

/// What the cache had, and when it was saved (Unix milliseconds); `None`
/// when it had nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached<T> {
    pub value: T,
    pub saved_ms: Option<u64>,
}

/// A thread's saved items, and the thread as last seen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CachedItems {
    pub thread: Option<AgentThread>,
    /// Oldest first.
    pub items: Vec<AgentItem>,
}

pub(crate) struct AgentCache {
    conn: Mutex<Connection>,
}

fn encode<T: serde::Serialize>(t: &str, body: &T) -> Result<Vec<u8>> {
    Ok(Envelope::new(t, body)?.to_cbor())
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    Envelope::from_cbor(bytes).ok()?.body().ok()
}

fn ms(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn unms(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// Puts an unreadable cache aside (one copy is kept) so a new one can start.
fn put_aside(path: &Path) {
    let mut aside = PathBuf::from(path);
    aside.set_extension("db.bad");
    let _ = std::fs::rename(path, &aside);
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(side));
    }
}

fn prepare(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != 0 && version != VERSION {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if version == 0 {
        conn.execute_batch("PRAGMA auto_vacuum = INCREMENTAL;")?;
    }
    conn.execute_batch(SCHEMA)?;
    conn.execute_batch(&format!("PRAGMA user_version = {VERSION};"))?;
    Ok(())
}

impl AgentCache {
    /// Opens the cache in `dir`, starting a new one when the file can't be read.
    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join(FILE);
        let conn = match Connection::open(&path).and_then(|c| prepare(&c).map(|()| c)) {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!(error = %e, "the saved agent threads can't be read; starting again");
                put_aside(&path);
                let conn = Connection::open(&path)?;
                prepare(&conn)?;
                conn
            }
        };
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether threads are kept on this phone ("Keep threads on this phone").
    pub fn keeps(&self) -> Result<bool> {
        let conn = self.lock();
        let keep: Option<i64> =
            conn.query_row("SELECT value FROM meta WHERE key = 'keep'", [], |r| r.get(0)).optional()?;
        Ok(keep.is_none_or(|k| k != 0))
    }

    /// Turns keeping threads on or off; off clears what was kept.
    pub fn set_keeps(&self, keep: bool) -> Result<()> {
        {
            let conn = self.lock();
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('keep', ?1)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![i64::from(keep)],
            )?;
        }
        if !keep {
            self.clear()?;
        }
        Ok(())
    }

    /// Saves a PC's threads as a list answered them.
    pub fn save_threads(&self, pc: &DeviceId, threads: &[AgentThread], now_ms: u64) -> Result<()> {
        if !self.keeps()? {
            return Ok(());
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        for thread in threads {
            upsert_thread(&tx, pc, thread, now_ms)?;
        }
        tx.commit()?;
        drop(conn);
        self.trim(pc)
    }

    /// Saves one thread as it changed.
    pub fn save_thread(&self, pc: &DeviceId, thread: &AgentThread, now_ms: u64) -> Result<()> {
        self.save_threads(pc, std::slice::from_ref(thread), now_ms)
    }

    /// Saves a page of a thread: the thread, its items, and (`opened`) that
    /// it was just opened.
    pub fn save_page(&self, pc: &DeviceId, page: &AgentItems, now_ms: u64, opened: bool) -> Result<()> {
        if !self.keeps()? {
            return Ok(());
        }
        let key = pc.to_string();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        upsert_thread(&tx, pc, &page.thread, now_ms)?;
        if opened {
            tx.execute(
                "INSERT INTO opened (pc, thread, at_ms, saved_ms) VALUES (?1, ?2, ?3, ?3)
                 ON CONFLICT (pc, thread) DO UPDATE SET at_ms = excluded.at_ms, saved_ms = excluded.saved_ms",
                params![key, page.thread.id, ms(now_ms)],
            )?;
        }
        let kept: bool = tx
            .query_row(
                "SELECT 1 FROM opened WHERE pc = ?1 AND thread = ?2",
                params![key, page.thread.id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if kept {
            for item in &page.items {
                upsert_item(&tx, &key, &page.thread.id, item)?;
            }
            tx.execute(
                "UPDATE opened SET saved_ms = ?3 WHERE pc = ?1 AND thread = ?2",
                params![key, page.thread.id, ms(now_ms)],
            )?;
        }
        tx.commit()?;
        drop(conn);
        self.trim(pc)
    }

    /// Saves an item of a thread whose items are kept (one opened lately).
    pub fn save_item(&self, pc: &DeviceId, thread: &str, item: &AgentItem) -> Result<()> {
        let key = pc.to_string();
        let conn = self.lock();
        let kept = conn
            .query_row("SELECT 1 FROM opened WHERE pc = ?1 AND thread = ?2", params![key, thread], |_| Ok(()))
            .optional()?
            .is_some();
        if kept {
            upsert_item(&conn, &key, thread, item)?;
        }
        Ok(())
    }

    /// Forgets a thread the PC removed.
    pub fn remove_thread(&self, pc: &DeviceId, thread: &str) -> Result<()> {
        let key = pc.to_string();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM threads WHERE pc = ?1 AND id = ?2", params![key, thread])?;
        tx.execute("DELETE FROM items WHERE pc = ?1 AND thread = ?2", params![key, thread])?;
        tx.execute("DELETE FROM opened WHERE pc = ?1 AND thread = ?2", params![key, thread])?;
        tx.execute("DELETE FROM git WHERE pc = ?1 AND thread = ?2", params![key, thread])?;
        tx.commit()?;
        Ok(())
    }

    /// Saves a thread's strip as `agent.git` said it.
    pub fn save_git(&self, pc: &DeviceId, change: &AgentGitChanged, now_ms: u64) -> Result<()> {
        if !self.keeps()? {
            return Ok(());
        }
        self.lock().execute(
            "INSERT INTO git (pc, thread, saved_ms, body) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (pc, thread) DO UPDATE SET saved_ms = excluded.saved_ms, body = excluded.body",
            params![pc.to_string(), change.thread, ms(now_ms), encode(types::AGENT_GIT, change)?],
        )?;
        Ok(())
    }

    /// Saves a thread's strip as a status read it: a status says nothing of
    /// the pull request, so the one saved stays.
    pub fn save_git_status(&self, pc: &DeviceId, strip: &AgentGitChanged, now_ms: u64) -> Result<()> {
        let mut strip = strip.clone();
        if strip.pr.is_none() {
            strip.pr = self.git(pc, &strip.thread)?.value.and_then(|saved| saved.pr);
        }
        self.save_git(pc, &strip, now_ms)
    }

    /// A thread's strip as last saved, and when.
    pub fn git(&self, pc: &DeviceId, thread: &str) -> Result<Cached<Option<AgentGitChanged>>> {
        let row: Option<(Vec<u8>, i64)> = self
            .lock()
            .query_row(
                "SELECT body, saved_ms FROM git WHERE pc = ?1 AND thread = ?2",
                params![pc.to_string(), thread],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let value = row
            .as_ref()
            .and_then(|(body, _)| decode::<AgentGitChanged>(body))
            .and_then(AgentGitChanged::sanitized)
            .filter(|strip| strip.thread == thread);
        let saved_ms = value.as_ref().and(row.map(|(_, at)| unms(at)));
        Ok(Cached { value, saved_ms })
    }

    /// Drops items `from` to `to` of a thread (a rewind left them out).
    pub fn cut(&self, pc: &DeviceId, thread: &str, from: u64, to: u64) -> Result<()> {
        self.lock().execute(
            "DELETE FROM items WHERE pc = ?1 AND thread = ?2 AND seq BETWEEN ?3 AND ?4",
            params![pc.to_string(), thread, ms(from), ms(to)],
        )?;
        Ok(())
    }

    /// A PC's saved threads, newest first, and when the list was saved.
    pub fn threads(&self, pc: &DeviceId) -> Result<Cached<Vec<AgentThread>>> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT body, saved_ms FROM threads WHERE pc = ?1 ORDER BY updated_ms DESC, id")?;
        let rows =
            stmt.query_map(params![pc.to_string()], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)))?;
        let mut threads = Vec::new();
        let mut saved = None::<u64>;
        for row in rows {
            let (body, at) = row?;
            if let Some(thread) = decode::<AgentThread>(&body).and_then(AgentThread::sanitized) {
                threads.push(thread);
                saved = Some(saved.map_or(unms(at), |s| s.max(unms(at))));
            }
        }
        Ok(Cached { value: threads, saved_ms: saved })
    }

    /// A thread's saved items, oldest first, the thread as last seen, and
    /// when its items were saved.
    pub fn items(&self, pc: &DeviceId, thread: &str) -> Result<Cached<CachedItems>> {
        let key = pc.to_string();
        let conn = self.lock();
        let body: Option<Vec<u8>> = conn
            .query_row("SELECT body FROM threads WHERE pc = ?1 AND id = ?2", params![key, thread], |r| {
                r.get(0)
            })
            .optional()?;
        let saved: Option<i64> = conn
            .query_row(
                "SELECT saved_ms FROM opened WHERE pc = ?1 AND thread = ?2",
                params![key, thread],
                |r| r.get(0),
            )
            .optional()?;
        let mut stmt = conn.prepare("SELECT body FROM items WHERE pc = ?1 AND thread = ?2 ORDER BY seq")?;
        let items = stmt
            .query_map(params![key, thread], |r| r.get::<_, Vec<u8>>(0))?
            .filter_map(|row| row.ok().and_then(|b| decode::<AgentItem>(&b)).and_then(AgentItem::sanitized))
            .collect();
        let thread = body.and_then(|b| decode::<AgentThread>(&b)).and_then(AgentThread::sanitized);
        Ok(Cached { value: CachedItems { thread, items }, saved_ms: saved.map(unms) })
    }

    /// Forgets everything of a PC, its held messages too (it was unpaired,
    /// or turned agents off for this phone).
    pub fn drop_pc(&self, pc: &DeviceId) -> Result<()> {
        let key = pc.to_string();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        for table in ["threads", "items", "opened", "git", "outbox"] {
            tx.execute(&format!("DELETE FROM {table} WHERE pc = ?1"), params![key])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Forgets the PCs that aren't in `paired`.
    pub fn keep_only(&self, paired: &[DeviceId]) -> Result<()> {
        let known: Vec<String> = paired.iter().map(DeviceId::to_string).collect();
        let pcs: Vec<String> = {
            let conn = self.lock();
            let mut stmt = conn.prepare(
                "SELECT pc FROM threads UNION SELECT pc FROM opened UNION SELECT pc FROM git
                 UNION SELECT pc FROM outbox",
            )?;
            stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?
        };
        for pc in pcs.iter().filter(|pc| !known.contains(pc)) {
            let mut conn = self.lock();
            let tx = conn.transaction()?;
            for table in ["threads", "items", "opened", "git", "outbox"] {
                tx.execute(&format!("DELETE FROM {table} WHERE pc = ?1"), params![pc])?;
            }
            tx.commit()?;
        }
        Ok(())
    }

    /// Forgets every saved thread ("Clear saved threads"). Held messages stay.
    pub fn clear(&self) -> Result<()> {
        let conn = self.lock();
        conn.execute_batch(
            "BEGIN; DELETE FROM threads; DELETE FROM items; DELETE FROM opened; DELETE FROM git; COMMIT;
             PRAGMA incremental_vacuum;",
        )?;
        Ok(())
    }

    /// What the cache takes on disk.
    pub fn bytes(&self) -> Result<u64> {
        let conn = self.lock();
        let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let free: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
        let size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok(unms((pages - free).max(0).saturating_mul(size)))
    }

    /// Keeps a PC's newest threads, the items of the threads opened last,
    /// each thread's newest items, and all of it within [`MAX_BYTES`].
    fn trim(&self, pc: &DeviceId) -> Result<()> {
        let key = pc.to_string();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM threads WHERE pc = ?1 AND id NOT IN
             (SELECT id FROM threads WHERE pc = ?1 ORDER BY updated_ms DESC, id LIMIT ?2)",
            params![key, MAX_THREADS as i64],
        )?;
        tx.execute(
            "DELETE FROM opened WHERE pc = ?1 AND (thread NOT IN (SELECT id FROM threads WHERE pc = ?1)
             OR thread NOT IN (SELECT thread FROM opened WHERE pc = ?1 ORDER BY at_ms DESC, thread LIMIT ?2))",
            params![key, MAX_OPENED as i64],
        )?;
        tx.execute(
            "DELETE FROM items WHERE pc = ?1 AND thread NOT IN (SELECT thread FROM opened WHERE pc = ?1)",
            params![key],
        )?;
        tx.execute(
            "DELETE FROM git WHERE pc = ?1 AND thread NOT IN (SELECT id FROM threads WHERE pc = ?1)",
            params![key],
        )?;
        tx.execute(
            "DELETE FROM items WHERE pc = ?1 AND seq < COALESCE((SELECT seq FROM items AS newer
             WHERE newer.pc = items.pc AND newer.thread = items.thread ORDER BY seq DESC LIMIT 1 OFFSET ?2), -1)",
            params![key, (MAX_ITEMS - 1) as i64],
        )?;
        loop {
            let total: i64 = tx.query_row(
                "SELECT COALESCE((SELECT SUM(LENGTH(body)) FROM threads), 0)
                      + COALESCE((SELECT SUM(LENGTH(body)) FROM items), 0)",
                [],
                |r| r.get(0),
            )?;
            if unms(total) <= MAX_BYTES {
                break;
            }
            let oldest: Option<(String, String)> = tx
                .query_row("SELECT pc, thread FROM opened ORDER BY at_ms, thread LIMIT 1", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            let Some((pc, thread)) = oldest else { break };
            tx.execute("DELETE FROM items WHERE pc = ?1 AND thread = ?2", params![pc, thread])?;
            tx.execute("DELETE FROM opened WHERE pc = ?1 AND thread = ?2", params![pc, thread])?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---- Held messages ----

    /// Holds a message for `thread` of `pc`; `Err(Error::QueueFull)` past
    /// [`MAX_HELD`] for that PC.
    pub fn hold(&self, pc: &DeviceId, key: &str, thread: &str, text: &str, now_ms: u64) -> Result<AgentHeld> {
        let pc_key = pc.to_string();
        let conn = self.lock();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM outbox WHERE pc = ?1 AND state != 'not_sent'",
            params![pc_key],
            |r| r.get(0),
        )?;
        if unms(count) >= MAX_HELD as u64 {
            return Err(Error::QueueFull(format!("at most {MAX_HELD} messages wait for a PC")));
        }
        conn.execute(
            "INSERT INTO outbox (pc, key, thread, text, created_ms, state) VALUES (?1, ?2, ?3, ?4, ?5, 'waiting')",
            params![pc_key, key, thread, text, ms(now_ms)],
        )?;
        Ok(AgentHeld {
            pc: *pc,
            key: key.to_owned(),
            thread: thread.to_owned(),
            text: text.to_owned(),
            created_ms: now_ms,
            state: HeldState::Waiting,
            why: None,
        })
    }

    /// A PC's held messages in the order they were written.
    pub fn held(&self, pc: &DeviceId) -> Result<Vec<AgentHeld>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT key, thread, text, created_ms, state, why FROM outbox WHERE pc = ?1 ORDER BY n",
        )?;
        let rows = stmt.query_map(params![pc.to_string()], |r| {
            Ok(AgentHeld {
                pc: *pc,
                key: r.get(0)?,
                thread: r.get(1)?,
                text: r.get(2)?,
                created_ms: unms(r.get(3)?),
                state: HeldState::from_word(&r.get::<_, String>(4)?),
                why: r.get::<_, Option<String>>(5)?.as_deref().and_then(HeldWhy::from_word),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Sets where a held message stands; whether it's still held.
    pub fn set_held(&self, pc: &DeviceId, key: &str, state: HeldState, why: Option<HeldWhy>) -> Result<bool> {
        let tries = i64::from(state == HeldState::Sending);
        let changed = self.lock().execute(
            "UPDATE outbox SET state = ?3, why = ?4, tries = tries + ?5 WHERE pc = ?1 AND key = ?2",
            params![pc.to_string(), key, state.word(), why.map(HeldWhy::word), tries],
        )?;
        Ok(changed > 0)
    }

    /// The user asked to send a message that wasn't sent: it waits again, and
    /// its day starts now. Whether it's held.
    pub fn retry(&self, pc: &DeviceId, key: &str, now_ms: u64) -> Result<bool> {
        let changed = self.lock().execute(
            "UPDATE outbox SET state = 'waiting', why = NULL, created_ms = ?3 WHERE pc = ?1 AND key = ?2",
            params![pc.to_string(), key, ms(now_ms)],
        )?;
        Ok(changed > 0)
    }

    /// Forgets a held message (sent, or discarded); whether it was held.
    pub fn take(&self, pc: &DeviceId, key: &str) -> Result<bool> {
        let changed = self
            .lock()
            .execute("DELETE FROM outbox WHERE pc = ?1 AND key = ?2", params![pc.to_string(), key])?;
        Ok(changed > 0)
    }

    /// Messages held for more than [`HELD_FOR_MS`] stop waiting: "Not
    /// sent". Whether any did.
    pub fn expire(&self, pc: &DeviceId, now_ms: u64) -> Result<bool> {
        let changed = self.lock().execute(
            "UPDATE outbox SET state = 'not_sent', why = 'too_old'
             WHERE pc = ?1 AND state != 'not_sent' AND created_ms < ?2",
            params![pc.to_string(), ms(now_ms.saturating_sub(HELD_FOR_MS))],
        )?;
        Ok(changed > 0)
    }

    /// Whether `thread` of `pc` has a message waiting to go.
    pub fn holds_for(&self, pc: &DeviceId, thread: &str) -> Result<bool> {
        Ok(self
            .lock()
            .query_row(
                "SELECT 1 FROM outbox WHERE pc = ?1 AND thread = ?2 AND state != 'not_sent' LIMIT 1",
                params![pc.to_string(), thread],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }
}

fn upsert_thread(conn: &Connection, pc: &DeviceId, thread: &AgentThread, now_ms: u64) -> Result<()> {
    conn.execute(
        "INSERT INTO threads (pc, id, updated_ms, saved_ms, body) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (pc, id) DO UPDATE SET updated_ms = excluded.updated_ms, saved_ms = excluded.saved_ms,
         body = excluded.body",
        params![
            pc.to_string(),
            thread.id,
            ms(thread.updated),
            ms(now_ms),
            encode(types::AGENT_THREAD, thread)?
        ],
    )?;
    Ok(())
}

fn upsert_item(conn: &Connection, pc: &str, thread: &str, item: &AgentItem) -> Result<()> {
    conn.execute(
        "INSERT INTO items (pc, thread, seq, body) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (pc, thread, seq) DO UPDATE SET body = excluded.body",
        params![pc, thread, ms(item.seq), encode(types::AGENT_EVENT, item)?],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc(n: u8) -> DeviceId {
        DeviceId([n; 32])
    }

    fn thread(i: u32, updated: u64) -> AgentThread {
        AgentThread {
            id: format!("{i:032x}"),
            agent: "claude".into(),
            folder: "g7linux".into(),
            title: format!("Thread {i}"),
            state: "done".into(),
            mode: "ask".into(),
            updated,
            ..Default::default()
        }
    }

    fn item(seq: u64, bytes: usize) -> AgentItem {
        AgentItem {
            seq,
            id: format!("m:{seq}"),
            kind: "assistant".into(),
            when: 1,
            text: Some("x".repeat(bytes)),
            ..Default::default()
        }
    }

    fn page(thread: AgentThread, items: Vec<AgentItem>) -> AgentItems {
        AgentItems { thread, items, ..Default::default() }
    }

    #[test]
    fn what_is_saved_reads_back_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        {
            let cache = AgentCache::open(dir.path()).unwrap();
            cache.save_threads(&pc(1), &[thread(1, 10), thread(2, 20)], 100).unwrap();
            cache.save_page(&pc(1), &page(thread(1, 10), vec![item(1, 3), item(2, 3)]), 200, true).unwrap();
            cache.hold(&pc(1), "0123456789abcdef", &thread(1, 10).id, "Then the tests.", 300).unwrap();
        }
        let cache = AgentCache::open(dir.path()).unwrap();
        let threads = cache.threads(&pc(1)).unwrap();
        assert_eq!(threads.value.iter().map(|t| t.updated).collect::<Vec<_>>(), [20, 10]);
        assert_eq!(threads.saved_ms, Some(200));
        let items = cache.items(&pc(1), &thread(1, 10).id).unwrap();
        assert_eq!(items.value.items.iter().map(|i| i.seq).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(items.saved_ms, Some(200));
        assert_eq!(items.value.thread.map(|t| t.id), Some(thread(1, 10).id));
        let held = cache.held(&pc(1)).unwrap();
        assert_eq!(held.len(), 1);
        assert_eq!((held[0].text.as_str(), held[0].state), ("Then the tests.", HeldState::Waiting));
        assert!(cache.threads(&pc(2)).unwrap().value.is_empty(), "another PC's are its own");
    }

    #[test]
    fn items_are_kept_only_for_threads_opened_lately() {
        let dir = tempfile::tempdir().unwrap();
        let cache = AgentCache::open(dir.path()).unwrap();
        let closed = thread(1, 1);
        cache.save_page(&pc(1), &page(closed.clone(), vec![item(1, 1)]), 1, false).unwrap();
        cache.save_item(&pc(1), &closed.id, &item(2, 1)).unwrap();
        assert!(cache.items(&pc(1), &closed.id).unwrap().value.items.is_empty());
        for i in 0..=MAX_OPENED as u32 {
            cache
                .save_page(&pc(1), &page(thread(10 + i, 5), vec![item(1, 1)]), 10 + u64::from(i), true)
                .unwrap();
        }
        assert!(
            cache.items(&pc(1), &thread(10, 5).id).unwrap().value.items.is_empty(),
            "the 21st opened last goes"
        );
        assert_eq!(cache.items(&pc(1), &thread(11, 5).id).unwrap().value.items.len(), 1);
        cache.save_item(&pc(1), &thread(30, 5).id, &item(9, 1)).unwrap();
        assert_eq!(cache.items(&pc(1), &thread(30, 5).id).unwrap().value.items.len(), 2);
    }

    #[test]
    fn the_caps_hold() {
        let dir = tempfile::tempdir().unwrap();
        let cache = AgentCache::open(dir.path()).unwrap();
        let many: Vec<AgentThread> = (0..MAX_THREADS as u32 + 5).map(|i| thread(i, u64::from(i))).collect();
        cache.save_threads(&pc(1), &many, 1).unwrap();
        let kept = cache.threads(&pc(1)).unwrap().value;
        assert_eq!(kept.len(), MAX_THREADS);
        assert_eq!(kept.last().map(|t| t.updated), Some(5), "the oldest five went");

        let busy = thread(500, 1_000);
        let items: Vec<AgentItem> = (1..=MAX_ITEMS as u64 + 10).map(|s| item(s, 1)).collect();
        cache.save_page(&pc(1), &page(busy.clone(), items), 2, true).unwrap();
        let kept = cache.items(&pc(1), &busy.id).unwrap().value.items;
        assert_eq!(kept.len(), MAX_ITEMS);
        assert_eq!(kept.first().map(|i| i.seq), Some(11), "the newest 400");

        // 64 MiB in all: the thread opened longest ago loses its items first.
        let big = 16_000;
        let per_thread = 400;
        for i in 0..12u32 {
            let items: Vec<AgentItem> = (1..=per_thread).map(|s| item(s, big)).collect();
            cache.save_page(&pc(2), &page(thread(700 + i, 9_000), items), 100 + u64::from(i), true).unwrap();
        }
        assert!(cache.items(&pc(2), &thread(700, 9_000).id).unwrap().value.items.is_empty());
        assert!(!cache.items(&pc(2), &thread(711, 9_000).id).unwrap().value.items.is_empty());
        let conn = cache.lock();
        let total: i64 = conn.query_row("SELECT SUM(LENGTH(body)) FROM items", [], |r| r.get(0)).unwrap();
        assert!(unms(total) <= MAX_BYTES, "{total}");
    }

    #[test]
    fn a_cut_drops_its_range() {
        let dir = tempfile::tempdir().unwrap();
        let cache = AgentCache::open(dir.path()).unwrap();
        let t = thread(1, 1);
        cache.save_page(&pc(1), &page(t.clone(), (1..=6).map(|s| item(s, 1)).collect()), 1, true).unwrap();
        cache.cut(&pc(1), &t.id, 3, 5).unwrap();
        let seqs: Vec<u64> = cache.items(&pc(1), &t.id).unwrap().value.items.iter().map(|i| i.seq).collect();
        assert_eq!(seqs, [1, 2, 6]);
        cache.remove_thread(&pc(1), &t.id).unwrap();
        assert!(cache.threads(&pc(1)).unwrap().value.is_empty());
        assert!(cache.items(&pc(1), &t.id).unwrap().value.items.is_empty());
    }

    #[test]
    fn a_pc_dropped_or_unpaired_leaves_nothing_and_clearing_keeps_held_messages() {
        let dir = tempfile::tempdir().unwrap();
        let cache = AgentCache::open(dir.path()).unwrap();
        for n in [1, 2, 3] {
            cache.save_page(&pc(n), &page(thread(1, 1), vec![item(1, 1)]), 1, true).unwrap();
            cache.hold(&pc(n), "0123456789abcdef", &thread(1, 1).id, "hi", 1).unwrap();
        }
        cache.drop_pc(&pc(1)).unwrap();
        assert!(cache.threads(&pc(1)).unwrap().value.is_empty());
        assert!(cache.held(&pc(1)).unwrap().is_empty());
        cache.keep_only(&[pc(3)]).unwrap();
        assert!(cache.threads(&pc(2)).unwrap().value.is_empty());
        assert!(cache.held(&pc(2)).unwrap().is_empty());
        assert_eq!(cache.held(&pc(3)).unwrap().len(), 1);
        cache.clear().unwrap();
        assert!(cache.threads(&pc(3)).unwrap().value.is_empty());
        assert_eq!(
            cache.held(&pc(3)).unwrap().len(),
            1,
            "clearing saved threads keeps what waits to be sent"
        );
        assert!(cache.bytes().unwrap() > 0);
    }

    #[test]
    fn a_threads_strip_is_kept_with_its_thread_and_a_status_keeps_the_pull_request() {
        use g7link_protocol::messages::AgentPrBrief;
        let dir = tempfile::tempdir().unwrap();
        let cache = AgentCache::open(dir.path()).unwrap();
        let t = thread(1, 1);
        cache.save_thread(&pc(1), &t, 1).unwrap();
        let pr = AgentPrBrief { number: 212, state: "open".into(), pass: 7, ..Default::default() };
        let strip = AgentGitChanged {
            thread: t.id.clone(),
            branch: Some("feat/x".into()),
            changed: 4,
            ahead: Some(2),
            pr: Some(pr.clone()),
            ..Default::default()
        };
        cache.save_git(&pc(1), &strip, 10).unwrap();
        let read = AgentGitChanged { changed: 1, pr: None, ..strip.clone() };
        cache.save_git_status(&pc(1), &read, 20).unwrap();
        let saved = cache.git(&pc(1), &t.id).unwrap();
        assert_eq!(saved.saved_ms, Some(20));
        assert_eq!(saved.value.as_ref().map(|s| (s.changed, s.pr.clone())), Some((1, Some(pr))));
        assert_eq!(cache.git(&pc(2), &t.id).unwrap().value, None, "another PC's are its own");
        cache.remove_thread(&pc(1), &t.id).unwrap();
        assert_eq!(cache.git(&pc(1), &t.id).unwrap(), Cached { value: None, saved_ms: None });
        cache.save_thread(&pc(1), &t, 1).unwrap();
        cache.save_git(&pc(1), &strip, 30).unwrap();
        cache.drop_pc(&pc(1)).unwrap();
        assert_eq!(cache.git(&pc(1), &t.id).unwrap().value, None);
        cache.set_keeps(false).unwrap();
        cache.save_git(&pc(1), &strip, 40).unwrap();
        assert_eq!(cache.git(&pc(1), &t.id).unwrap().value, None, "nothing kept");
    }

    #[test]
    fn keeping_off_saves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cache = AgentCache::open(dir.path()).unwrap();
        cache.save_thread(&pc(1), &thread(1, 1), 1).unwrap();
        cache.set_keeps(false).unwrap();
        assert!(cache.threads(&pc(1)).unwrap().value.is_empty());
        cache.save_thread(&pc(1), &thread(2, 1), 1).unwrap();
        assert!(cache.threads(&pc(1)).unwrap().value.is_empty());
        drop(cache);
        let cache = AgentCache::open(dir.path()).unwrap();
        assert!(!cache.keeps().unwrap(), "the choice survives a restart");
        cache.set_keeps(true).unwrap();
        cache.save_thread(&pc(1), &thread(2, 1), 1).unwrap();
        assert_eq!(cache.threads(&pc(1)).unwrap().value.len(), 1);
    }

    #[test]
    fn held_messages_are_bounded_ordered_and_expire() {
        let dir = tempfile::tempdir().unwrap();
        let cache = AgentCache::open(dir.path()).unwrap();
        let key = |i: usize| format!("{i:016x}");
        for i in 0..MAX_HELD {
            cache.hold(&pc(1), &key(i), "t", &format!("m{i}"), 1_000 + i as u64).unwrap();
        }
        assert!(matches!(cache.hold(&pc(1), &key(99), "t", "one more", 2_000), Err(Error::QueueFull(_))));
        let held = cache.held(&pc(1)).unwrap();
        assert_eq!(held.iter().map(|h| h.text.clone()).collect::<Vec<_>>()[..3], ["m0", "m1", "m2"]);
        assert!(cache.set_held(&pc(1), &key(0), HeldState::Blocked, Some(HeldWhy::AnswerFirst)).unwrap());
        assert_eq!(cache.held(&pc(1)).unwrap()[0].why, Some(HeldWhy::AnswerFirst));
        assert!(cache.holds_for(&pc(1), "t").unwrap());

        assert!(!cache.expire(&pc(1), 1_000 + HELD_FOR_MS).unwrap(), "not yet a day");
        assert!(cache.expire(&pc(1), 1_005 + HELD_FOR_MS).unwrap());
        let held = cache.held(&pc(1)).unwrap();
        assert_eq!(held[0].state, HeldState::NotSent);
        assert_eq!(held[0].why, Some(HeldWhy::TooOld));
        assert_eq!(held[10].state, HeldState::Waiting);

        // A message not sent doesn't count against the twenty.
        cache.hold(&pc(1), &key(99), "t", "one more", 2_000).unwrap();
        assert!(cache.retry(&pc(1), &key(0), 9_000_000_000).unwrap());
        let first = &cache.held(&pc(1)).unwrap()[0];
        assert_eq!(
            (first.key.as_str(), first.state, first.created_ms),
            (key(0).as_str(), HeldState::Waiting, 9_000_000_000)
        );
        assert!(cache.take(&pc(1), &key(0)).unwrap());
        assert!(!cache.take(&pc(1), &key(0)).unwrap());
    }

    #[test]
    fn an_unreadable_file_is_put_aside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), b"this is not a database at all, not even close....").unwrap();
        let cache = AgentCache::open(dir.path()).unwrap();
        cache.save_thread(&pc(1), &thread(1, 1), 1).unwrap();
        assert_eq!(cache.threads(&pc(1)).unwrap().value.len(), 1);
        assert!(dir.path().join("agents-cache.db.bad").exists());
    }

    #[test]
    fn a_later_schema_starts_again() {
        let dir = tempfile::tempdir().unwrap();
        {
            let conn = Connection::open(dir.path().join(FILE)).unwrap();
            conn.execute_batch("PRAGMA user_version = 9; CREATE TABLE other (x);").unwrap();
        }
        let cache = AgentCache::open(dir.path()).unwrap();
        cache.save_thread(&pc(1), &thread(1, 1), 1).unwrap();
        assert_eq!(cache.threads(&pc(1)).unwrap().value.len(), 1);
    }

    #[test]
    fn a_held_message_never_prints_its_text() {
        let held = AgentHeld {
            pc: pc(1),
            key: "0123456789abcdef".into(),
            thread: "t".into(),
            text: "secret plan".into(),
            created_ms: 1,
            state: HeldState::Waiting,
            why: None,
        };
        let shown = format!("{held:?}");
        assert!(!shown.contains("secret") && !shown.contains("0123456789abcdef"), "{shown}");
    }
}
