// SPDX-License-Identifier: MPL-2.0
//! Threads: their rows, loading them back, deleting, copying (a fork),
//! pruning past the limit, and message keys.

use std::collections::{BTreeMap, HashMap};
use std::io;

use rusqlite::{Connection, OptionalExtension, params};

use super::items::{self, kind_word};
use super::turns::{self, TurnRow};
use super::{Fail, Res, int, not_found, opt_uint, uint, uint32};
use crate::api::{
    Checkpoint, CheckpointWhy, FORK_ITEMS, Item, ItemKind, PlanState, SEND_KEY_MS, ThreadInfo, Via,
};

/// A thread's state the host keeps beside its [`ThreadInfo`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadCols {
    /// The `seq` its next item gets.
    pub next_seq: u64,
    /// The number its next turn gets; never reused, also after a rewind.
    pub next_turn: u32,
    /// Its working directory, only for a thread brought in from outside
    /// every agent folder. Never leaves the PC.
    pub cwd: Option<String>,
    /// When it was archived (with `ThreadInfo.archived`).
    pub archived_ms: Option<u64>,
    /// The last `seq` any client had on screen.
    pub read_seq: u64,
    /// "Mark unread".
    pub unread_mark: bool,
    /// The session it was brought in from (`found.id`).
    pub found_id: Option<String>,
    /// A fork the agent makes when the thread's next process starts (an
    /// encoded launch hint). Kept across a restart; never in [`ThreadInfo`],
    /// on the wire or in an export.
    pub pending_fork: Option<String>,
}

/// A thread read back, with what the host needs to take it up again.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedThread {
    /// As saved, with the native id the database holds (a rewind changes it).
    pub info: ThreadInfo,
    /// `next_seq` is past every stored item.
    pub cols: ThreadCols,
    /// The native id before the last rewind, while that rewind can be undone.
    pub prior_native_id: Option<String>,
    /// The largest `turn` among its items that are not cut: 0 for a thread
    /// stored before turns. New turns go on from `cols.next_turn`.
    pub max_turn: u32,
    /// Its `user` items that started a turn (not denials, not steered), not cut.
    pub users: u32,
    /// Its newest plan, when that one is still proposed.
    pub plan: Option<Item>,
}

fn word<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

/// Inserts or updates the thread's row, and its title in the index when that
/// changed. The native id it had before a rewind is the database's own and
/// is left as it is. Returns the row's rowid.
pub(crate) fn upsert(conn: &Connection, info: &ThreadInfo, cols: &ThreadCols) -> Res<i64> {
    let old_title: Option<String> = conn
        .prepare_cached("SELECT title FROM threads WHERE id = ?1")?
        .query_row(params![info.id], |r| r.get(0))
        .optional()?;
    let archived_ms = info.archived.then(|| cols.archived_ms.unwrap_or(info.updated_ms));
    let body = serde_json::to_string(info)?;
    let rowid: i64 = conn
        .prepare_cached(
            "INSERT INTO threads (id, agent, folder_id, cwd, title, state, created_ms, updated_ms, next_seq,
                 next_turn, native_id, pinned, archived_ms, snoozed_until_ms, read_seq, unread_mark,
                 forked_from, forked_turn, found_id, pending_fork, info)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
             ON CONFLICT (id) DO UPDATE SET agent = excluded.agent, folder_id = excluded.folder_id,
                 cwd = excluded.cwd, title = excluded.title, state = excluded.state,
                 created_ms = excluded.created_ms, updated_ms = excluded.updated_ms,
                 next_seq = excluded.next_seq, next_turn = excluded.next_turn, native_id = excluded.native_id,
                 pinned = excluded.pinned, archived_ms = excluded.archived_ms,
                 snoozed_until_ms = excluded.snoozed_until_ms, read_seq = excluded.read_seq,
                 unread_mark = excluded.unread_mark, forked_from = excluded.forked_from,
                 forked_turn = excluded.forked_turn, found_id = excluded.found_id,
                 pending_fork = excluded.pending_fork, info = excluded.info
             RETURNING rowid",
        )?
        .query_row(
            params![
                info.id,
                info.agent.as_str(),
                Some(info.folder_id.as_str()).filter(|f| !f.is_empty()),
                cols.cwd,
                info.title,
                word(&info.state),
                int(info.created_ms)?,
                int(info.updated_ms)?,
                int(cols.next_seq)?,
                cols.next_turn,
                info.native_id,
                info.pinned,
                archived_ms.map(int).transpose()?,
                info.snoozed_until_ms.map(int).transpose()?,
                int(cols.read_seq)?,
                cols.unread_mark,
                info.forked_from.as_ref().map(|f| f.thread_id.as_str()),
                info.forked_from.as_ref().and_then(|f| f.turn),
                cols.found_id,
                cols.pending_fork,
                body,
            ],
            |r| r.get(0),
        )?;
    if old_title.as_deref() != Some(info.title.as_str()) {
        conn.prepare_cached("INSERT OR REPLACE INTO title_text (rowid, title) VALUES (?1, ?2)")?
            .execute(params![rowid, info.title])?;
    }
    Ok(rowid)
}

/// The thread's `info` with `native_id` set to `native`.
pub(crate) fn info_with_native(info: &str, native: Option<&str>) -> Res<String> {
    let mut value: serde_json::Value = serde_json::from_str(info)?;
    if let Some(object) = value.as_object_mut() {
        match native {
            Some(n) => object.insert("native_id".to_owned(), serde_json::Value::String(n.to_owned())),
            None => object.remove("native_id"),
        };
    }
    Ok(serde_json::to_string(&value)?)
}

/// Item aggregates per thread.
#[derive(Default)]
struct Counts {
    max_seq: u64,
    /// Over every item and turn, cut ones too: the numbers taken.
    max_any_turn: u32,
    max_turn: u32,
    users: u32,
}

/// Every thread (`only` absent) or one.
pub(crate) fn load(conn: &Connection, only: Option<&str>) -> Res<Vec<LoadedThread>> {
    let filter = if only.is_some() { "WHERE thread_id = ?1" } else { "WHERE ?1 IS NULL" };
    let mut counts: HashMap<String, Counts> = HashMap::new();
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT thread_id, MAX(seq), coalesce(MAX(CASE WHEN cut = 0 THEN turn END), 0),
                    coalesce(SUM(CASE WHEN cut = 0 AND kind = 'user' AND json_extract(body, '$.deny_of') IS NULL
                                       AND coalesce(json_extract(body, '$.via'), '') <> ?2 THEN 1 ELSE 0 END), 0),
                    coalesce(MAX(turn), 0)
             FROM items {filter} GROUP BY thread_id"
        ))?;
        let mut rows = stmt.query(params![only, word(&Via::Steer)])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            counts.insert(
                id,
                Counts {
                    max_seq: uint(row, 1)?,
                    max_turn: uint32(row, 2)?,
                    users: uint32(row, 3)?,
                    max_any_turn: uint32(row, 4)?,
                },
            );
        }
        let mut stmt =
            conn.prepare(&format!("SELECT thread_id, MAX(turn) FROM turns {filter} GROUP BY thread_id"))?;
        let mut rows = stmt.query(params![only])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let c = counts.entry(id).or_default();
            c.max_any_turn = c.max_any_turn.max(uint32(row, 1)?);
        }
    }
    let mut plans: HashMap<String, Item> = HashMap::new();
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT i.thread_id, i.body FROM items i
             JOIN (SELECT thread_id, MAX(seq) AS seq FROM items {filter} AND kind = 'plan' AND cut = 0
                   GROUP BY thread_id) p ON p.thread_id = i.thread_id AND p.seq = i.seq"
        ))?;
        let mut rows = stmt.query(params![only])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let body: String = row.get(1)?;
            if let Ok(item) = serde_json::from_str::<Item>(&body)
                && item.plan_state == Some(PlanState::Proposed)
            {
                plans.insert(id, item);
            }
        }
    }
    let filter = if only.is_some() { "WHERE id = ?1" } else { "WHERE ?1 IS NULL" };
    let mut stmt = conn.prepare(&format!(
        "SELECT id, next_seq, next_turn, cwd, prior_native_id, archived_ms, read_seq, unread_mark, found_id,
                native_id, info, pending_fork
         FROM threads {filter} ORDER BY rowid"
    ))?;
    let mut rows = stmt.query(params![only])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let body: String = row.get(10)?;
        let Ok(mut info) = serde_json::from_str::<ThreadInfo>(&body) else {
            tracing::warn!(thread = %id, "a stored thread can't be read");
            continue;
        };
        info.native_id = row.get(9)?;
        let c = counts.remove(&id).unwrap_or_default();
        let cols = ThreadCols {
            next_seq: uint(row, 1)?.max(c.max_seq + 1).max(1),
            next_turn: uint32(row, 2)?.max(c.max_any_turn + 1),
            cwd: row.get(3)?,
            archived_ms: opt_uint(row, 5)?,
            read_seq: uint(row, 6)?,
            unread_mark: row.get(7)?,
            found_id: row.get(8)?,
            pending_fork: row.get(11)?,
        };
        out.push(LoadedThread {
            plan: plans.remove(&id),
            info,
            cols,
            prior_native_id: row.get(4)?,
            max_turn: c.max_turn,
            users: c.users,
        });
    }
    Ok(out)
}

fn exists(conn: &Connection, thread: &str) -> Res<bool> {
    Ok(conn
        .prepare_cached("SELECT 1 FROM threads WHERE id = ?1")?
        .query_row(params![thread], |_| Ok(()))
        .optional()?
        .is_some())
}

pub(crate) fn delete(conn: &Connection, thread: &str) -> Res<()> {
    let Some(rowid) = conn
        .prepare_cached("SELECT rowid FROM threads WHERE id = ?1")?
        .query_row(params![thread], |r| r.get::<_, i64>(0))
        .optional()?
    else {
        return Ok(());
    };
    let indexed: Vec<i64> = conn
        .prepare_cached("SELECT rowid FROM items WHERE thread_id = ?1 AND cut = 0")?
        .query_map(params![thread], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for item in indexed {
        items::unindex(conn, item)?;
    }
    conn.prepare_cached("DELETE FROM title_text WHERE rowid = ?1")?.execute(params![rowid])?;
    conn.prepare_cached("UPDATE found SET thread_id = NULL WHERE thread_id = ?1")?
        .execute(params![thread])?;
    conn.prepare_cached("DELETE FROM threads WHERE id = ?1")?.execute(params![thread])?;
    Ok(())
}

pub(crate) fn copy(conn: &Connection, from: &str, through: u32, new: &ThreadInfo) -> Res<()> {
    if exists(conn, &new.id)? {
        return Err(Fail::Io(io::Error::new(io::ErrorKind::AlreadyExists, "that thread exists")));
    }
    if !exists(conn, from)? {
        return Err(not_found("no such thread"));
    }
    // Items of a later turn never come along, nor anything from where it started.
    let boundary: Option<u64> = conn
        .prepare_cached("SELECT MIN(prompt_seq) FROM turns WHERE thread_id = ?1 AND turn > ?2 AND cut = 0")?
        .query_row(params![from, through], |r| opt_uint(r, 0))?;
    let boundary = boundary.map_or(Ok(i64::MAX), int)?;
    let limit = i64::try_from(FORK_ITEMS).unwrap_or(i64::MAX);
    let mut picked: Vec<(u64, serde_json::Value, Item, String)> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT seq, kind, body FROM items
             WHERE thread_id = ?1 AND cut = 0 AND seq < ?2 AND (turn IS NULL OR turn <= ?3)
             ORDER BY seq DESC LIMIT ?4",
        )?;
        let mut rows = stmt.query(params![from, boundary, through, limit])?;
        while let Some(row) = rows.next()? {
            let body: String = row.get(2)?;
            let (Ok(value), Ok(item)) =
                (serde_json::from_str::<serde_json::Value>(&body), serde_json::from_str::<Item>(&body))
            else {
                continue;
            };
            picked.push((uint(row, 0)?, value, item, row.get(1)?));
        }
    }
    picked.reverse();
    let mut seqs: BTreeMap<u64, u64> = BTreeMap::new();
    let mut max_turn = through;
    let mut copies = Vec::with_capacity(picked.len());
    for (n, (old, mut value, mut item, kind)) in (1u64..).zip(picked) {
        seqs.insert(old, n);
        item.seq = n;
        max_turn = max_turn.max(item.turn.unwrap_or(0));
        if let Some(object) = value.as_object_mut() {
            object.insert("seq".to_owned(), serde_json::Value::from(n));
        }
        copies.push((item, kind, serde_json::to_string(&value)?));
    }
    let count = u64::try_from(copies.len()).unwrap_or(u64::MAX);
    let cols =
        ThreadCols { next_seq: count + 1, next_turn: max_turn + 1, read_seq: count, ..ThreadCols::default() };
    upsert(conn, new, &cols)?;
    conn.prepare_cached("UPDATE threads SET forked_from = ?2, forked_turn = ?3 WHERE id = ?1")?
        .execute(params![new.id, from, through])?;
    for (item, kind, body) in &copies {
        items::insert(conn, &new.id, item, kind, body)?;
    }
    // Their turns, marks kept so the copy can be rewound and forked in its
    // turn; no restore points.
    let rows = turns::read_range(conn, from, through)?;
    for row in rows {
        let Some(&prompt_seq) = seqs.get(&row.prompt_seq) else { continue };
        let last_seq = row.last_seq.and_then(|last| seqs.range(..=last).next_back().map(|(_, &n)| n));
        let copy = TurnRow {
            prompt_seq,
            last_seq,
            checkpoint: Checkpoint::Missing,
            why: Some(CheckpointWhy::Before),
            pre_oid: None,
            post_oid: None,
            pre_head: None,
            post_head: None,
            files: 0,
            adds: 0,
            dels: 0,
            skipped: Vec::new(),
            cut: false,
            ..row
        };
        turns::upsert(conn, &new.id, &copy, None)?;
    }
    let approvals: Vec<(u64, String)> = conn
        .prepare_cached("SELECT ts_ms, record FROM approvals WHERE thread_id = ?1 ORDER BY rowid")?
        .query_map(params![from], |r| Ok((uint(r, 0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (ts_ms, record) in approvals {
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&record) else { continue };
        let Some(&seq) = value.get("tool_seq").and_then(serde_json::Value::as_u64).and_then(|s| seqs.get(&s))
        else {
            continue;
        };
        if let Some(object) = value.as_object_mut() {
            object.insert("tool_seq".to_owned(), serde_json::Value::from(seq));
        }
        items::insert_approval(conn, &new.id, ts_ms, &serde_json::to_string(&value)?)?;
    }
    Ok(())
}

/// The threads to remove so that `max` are left, in the order they go.
pub(crate) fn prunable(conn: &Connection, max: usize) -> Res<Vec<String>> {
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM threads", [], |r| r.get(0))?;
    let excess = total - i64::try_from(max).unwrap_or(i64::MAX);
    if excess <= 0 {
        return Ok(Vec::new());
    }
    let ids = conn
        .prepare_cached(
            "SELECT id FROM threads WHERE pinned = 0 AND state NOT IN ('working', 'waiting')
             ORDER BY archived_ms IS NULL, updated_ms, rowid LIMIT ?1",
        )?
        .query_map(params![excess], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

pub(crate) fn take_send_key(conn: &Connection, thread: &str, key: &str, now_ms: u64) -> Res<bool> {
    if !exists(conn, thread)? {
        return Err(not_found("no such thread"));
    }
    conn.prepare_cached("DELETE FROM send_keys WHERE thread_id = ?1 AND ts_ms < ?2")?
        .execute(params![thread, int(now_ms.saturating_sub(SEND_KEY_MS))?])?;
    let taken = conn
        .prepare_cached("INSERT OR IGNORE INTO send_keys (thread_id, key, ts_ms) VALUES (?1, ?2, ?3)")?
        .execute(params![thread, key, int(now_ms)?])?;
    Ok(taken == 1)
}

pub(crate) fn release_send_key(conn: &Connection, thread: &str, key: &str) -> Res<()> {
    conn.prepare_cached("DELETE FROM send_keys WHERE thread_id = ?1 AND key = ?2")?
        .execute(params![thread, key])?;
    Ok(())
}

/// The kind of a stored line as it was written (a later kind stays as it is).
pub(crate) fn raw_kind(item: &Item, raw: &str) -> String {
    #[derive(serde::Deserialize)]
    struct Kind {
        kind: String,
    }
    match serde_json::from_str::<Kind>(raw) {
        Ok(k) if item.kind == ItemKind::Unknown => k.kind,
        _ => kind_word(item.kind),
    }
}
