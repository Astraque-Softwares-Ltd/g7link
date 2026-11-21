// SPDX-License-Identifier: MPL-2.0
//! The sessions found on disk (§U4, reconcile §6): one row per agent thread
//! however many files hold it, its files, and an index over titles and
//! folder names.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::search::{has_all, match_expr};
use super::{Res, int, not_found, opt_uint, uint};
use crate::api::{AgentId, FoundShow, MAX_SCAN_LIMIT, MIN_INDEXED_TERM, SessionSource};

/// One file of a session found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundFile {
    pub path: String,
    pub dev: u64,
    pub ino: u64,
    pub bytes: u64,
    pub mtime_ms: u64,
    /// The `<ts>` of its name: the order of a thread's parts.
    pub stamp: String,
    /// `.zst`.
    pub packed: bool,
}

/// A session found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundRow {
    /// `"sc"`/`"sx"` and the thread's uuid without dashes.
    pub id: String,
    pub agent: AgentId,
    pub native_id: String,
    /// Never leaves the PC.
    pub cwd: Option<String>,
    pub folder_id: Option<String>,
    pub title: String,
    pub first_ms: Option<u64>,
    pub updated_ms: u64,
    pub bytes: u64,
    pub source: Option<SessionSource>,
    /// Codex: the thread a fork's history starts in, and where in it.
    pub base_thread: Option<String>,
    pub base_end: Option<u64>,
    pub damaged: bool,
    /// The thread brought in from it ([`super::Db::set_found_thread`]); not
    /// written by [`super::Db::upsert_found`].
    pub thread_id: Option<String>,
    /// Its files, oldest part first.
    pub files: Vec<FoundFile>,
}

impl FoundRow {
    /// Some part of it is kept compressed.
    pub fn packed(&self) -> bool {
        self.files.iter().any(|f| f.packed)
    }

    /// The last component of its working directory, what the index calls its folder.
    pub fn folder_name(&self) -> String {
        self.cwd
            .as_deref()
            .and_then(|c| Path::new(c).file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// A `sessions.scan` request.
#[derive(Debug, Clone, Default)]
pub struct FoundQuery {
    /// Matches titles and folder names; under 3 characters, the start of a title.
    pub query: Option<String>,
    pub agent: Option<AgentId>,
    pub show: FoundShow,
    /// Sessions last changed before this.
    pub before_ms: Option<u64>,
    /// At most [`MAX_SCAN_LIMIT`].
    pub limit: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FoundPage {
    /// Newest first.
    pub rows: Vec<FoundRow>,
    /// `before_ms` for the next page; absent: this was the last.
    pub next_ms: Option<u64>,
    /// Matching sessions in all.
    pub total: u32,
}

/// A `u64` stored bit for bit (device and inode numbers use the whole range).
fn bits(v: u64) -> i64 {
    i64::from_ne_bytes(v.to_ne_bytes())
}

fn unbits(v: i64) -> u64 {
    u64::from_ne_bytes(v.to_ne_bytes())
}

fn word<T: serde::Serialize>(v: &T) -> Option<String> {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => Some(s),
        _ => None,
    }
}

fn parse<T: serde::de::DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).ok()
}

pub(crate) fn upsert(conn: &Connection, rows: &[FoundRow]) -> Res<()> {
    let mut row_stmt = conn.prepare_cached(
        "INSERT INTO found (id, agent, native_id, cwd, folder_id, title, first_ms, updated_ms, bytes, source,
             base_thread, base_end, damaged)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT (id) DO UPDATE SET agent = excluded.agent, native_id = excluded.native_id,
             cwd = excluded.cwd, folder_id = excluded.folder_id, title = excluded.title,
             first_ms = excluded.first_ms, updated_ms = excluded.updated_ms, bytes = excluded.bytes,
             source = excluded.source, base_thread = excluded.base_thread, base_end = excluded.base_end,
             damaged = excluded.damaged
         RETURNING rowid",
    )?;
    let mut clear = conn.prepare_cached("DELETE FROM found_files WHERE found_id = ?1")?;
    let mut file_stmt = conn.prepare_cached(
        "INSERT OR REPLACE INTO found_files (found_id, path, dev, ino, bytes, mtime_ms, stamp, packed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    let mut index =
        conn.prepare_cached("INSERT OR REPLACE INTO found_text (rowid, title, folder) VALUES (?1, ?2, ?3)")?;
    for row in rows {
        let rowid: i64 = row_stmt.query_row(
            params![
                row.id,
                row.agent.as_str(),
                row.native_id,
                row.cwd,
                row.folder_id,
                row.title,
                row.first_ms.map(int).transpose()?,
                int(row.updated_ms)?,
                int(row.bytes)?,
                row.source.as_ref().and_then(word),
                row.base_thread,
                row.base_end.map(int).transpose()?,
                row.damaged,
            ],
            |r| r.get(0),
        )?;
        clear.execute(params![row.id])?;
        for f in &row.files {
            file_stmt.execute(params![
                row.id,
                f.path,
                bits(f.dev),
                bits(f.ino),
                int(f.bytes)?,
                int(f.mtime_ms)?,
                f.stamp,
                f.packed
            ])?;
        }
        index.execute(params![rowid, row.title, row.folder_name()])?;
    }
    Ok(())
}

const COLUMNS: &str =
    "f.id, f.agent, f.native_id, f.cwd, f.folder_id, f.title, f.first_ms, f.updated_ms, f.bytes,
     f.source, f.base_thread, f.base_end, f.damaged, f.thread_id";

fn row_of(r: &Row<'_>) -> rusqlite::Result<Option<FoundRow>> {
    let agent: String = r.get(1)?;
    let source: Option<String> = r.get(9)?;
    let Some(agent) = parse::<AgentId>(&agent) else { return Ok(None) };
    Ok(Some(FoundRow {
        id: r.get(0)?,
        agent,
        native_id: r.get(2)?,
        cwd: r.get(3)?,
        folder_id: r.get(4)?,
        title: r.get(5)?,
        first_ms: opt_uint(r, 6)?,
        updated_ms: uint(r, 7)?,
        bytes: uint(r, 8)?,
        source: source.as_deref().and_then(parse),
        base_thread: r.get(10)?,
        base_end: opt_uint(r, 11)?,
        damaged: r.get(12)?,
        thread_id: r.get(13)?,
        files: Vec::new(),
    }))
}

fn file_of(r: &Row<'_>, from: usize) -> rusqlite::Result<FoundFile> {
    Ok(FoundFile {
        path: r.get(from)?,
        dev: unbits(r.get(from + 1)?),
        ino: unbits(r.get(from + 2)?),
        bytes: uint(r, from + 3)?,
        mtime_ms: uint(r, from + 4)?,
        stamp: r.get(from + 5)?,
        packed: r.get(from + 6)?,
    })
}

fn files_of(conn: &Connection, id: &str) -> Res<Vec<FoundFile>> {
    Ok(conn
        .prepare_cached(
            "SELECT path, dev, ino, bytes, mtime_ms, stamp, packed FROM found_files WHERE found_id = ?1
             ORDER BY stamp, path",
        )?
        .query_map(params![id], |r| file_of(r, 0))?
        .collect::<Result<_, _>>()?)
}

/// One session found on disk, by id, whatever its thread link; `None` if there is no such row.
pub(crate) fn one(conn: &Connection, id: &str) -> Res<Option<FoundRow>> {
    let row = conn
        .prepare_cached(&format!("SELECT {COLUMNS} FROM found f WHERE f.id = ?1"))?
        .query_row(params![id], row_of)
        .optional()?
        .flatten();
    let Some(mut row) = row else { return Ok(None) };
    row.files = files_of(conn, &row.id)?;
    Ok(Some(row))
}

pub(crate) fn query(conn: &Connection, q: &FoundQuery) -> Res<FoundPage> {
    let limit = usize::try_from(q.limit.clamp(1, MAX_SCAN_LIMIT)).unwrap_or(1);
    let text = q.query.as_deref().map(str::trim).filter(|t| !t.is_empty());
    // Under 3 characters: the start of a title. Else the index, with short
    // terms checked here.
    let (expr, short, prefix) = match text {
        Some(t) if t.chars().count() < MIN_INDEXED_TERM => (None, Vec::new(), Some(super::fold_text(t))),
        Some(t) => {
            let (expr, short) = match_expr(t);
            (expr, short, None)
        }
        None => (None, Vec::new(), None),
    };
    let sql = format!(
        "SELECT {COLUMNS} FROM found f
         WHERE (f.thread_id IS NOT NULL
                OR f.native_id NOT IN (SELECT native_id FROM threads WHERE native_id IS NOT NULL
                                       UNION SELECT prior_native_id FROM threads WHERE prior_native_id IS NOT NULL))
           AND (?1 IS NULL OR f.agent = ?1)
           AND (?2 = 0 OR f.thread_id IS NULL)
           AND (?3 IS NULL OR f.rowid IN (SELECT rowid FROM found_text WHERE found_text MATCH ?3))
         ORDER BY f.updated_ms DESC, f.id"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut rows = stmt.query(params![
        q.agent.map(AgentId::as_str),
        i64::from(q.show == FoundShow::New),
        expr.as_deref()
    ])?;
    let mut page: Vec<FoundRow> = Vec::new();
    let mut total: u32 = 0;
    let mut more = false;
    while let Some(r) = rows.next()? {
        let Some(row) = row_of(r)? else { continue };
        if let Some(prefix) = &prefix
            && !super::fold_text(&row.title).starts_with(prefix.as_str())
        {
            continue;
        }
        if !short.is_empty() && !has_all(&format!("{} {}", row.title, row.folder_name()), &short) {
            continue;
        }
        total = total.saturating_add(1);
        if q.before_ms.is_some_and(|b| row.updated_ms >= b) {
            continue;
        }
        // A page ends after the rows that share its last time, so the next
        // one (before that time) misses none.
        match page.last() {
            Some(last) if page.len() >= limit && last.updated_ms != row.updated_ms => more = true,
            _ if more => {}
            _ => page.push(row),
        }
    }
    drop(rows);
    for row in &mut page {
        row.files = files_of(conn, &row.id)?;
    }
    let next_ms = if more { page.last().map(|r| r.updated_ms) } else { None };
    Ok(FoundPage { rows: page, next_ms, total })
}

pub(crate) fn all_files(conn: &Connection) -> Res<Vec<(String, FoundFile)>> {
    Ok(conn
        .prepare_cached("SELECT found_id, path, dev, ino, bytes, mtime_ms, stamp, packed FROM found_files")?
        .query_map([], |r| Ok((r.get(0)?, file_of(r, 1)?)))?
        .collect::<Result<_, _>>()?)
}

pub(crate) fn forget_files(conn: &Connection, files: &[(String, String)]) -> Res<()> {
    let mut stmt = conn.prepare_cached("DELETE FROM found_files WHERE found_id = ?1 AND path = ?2")?;
    for (id, path) in files {
        stmt.execute(params![id, path])?;
    }
    Ok(())
}

pub(crate) fn set_thread(conn: &Connection, found_id: &str, thread: Option<&str>) -> Res<()> {
    let changed = conn
        .prepare_cached("UPDATE found SET thread_id = ?2 WHERE id = ?1")?
        .execute(params![found_id, thread])?;
    if changed == 0 {
        return Err(not_found("no such session"));
    }
    Ok(())
}
