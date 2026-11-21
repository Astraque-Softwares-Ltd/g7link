// SPDX-License-Identifier: MPL-2.0
//! Items (each the item's JSON as the file store wrote it) and answered approvals.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};

use super::search::searchable;
use super::{ApprovalRecord, Res, int, uint};
use crate::api::{Item, ItemKind};

/// The notice that takes the place of the items left out past the limit.
pub(crate) const LEFT_OUT_ID: &str = "left-out";
pub(crate) const LEFT_OUT_TEXT: &str = "Earlier messages were left out.";
/// Trimming goes this many items below the limit, so it doesn't run on every write.
const TRIM_SLACK: usize = 200;
/// Answered approvals read for a thread, the newest.
const MAX_APPROVALS_READ: i64 = 10_000;

/// An item kind as stored.
pub(crate) fn kind_word(kind: ItemKind) -> String {
    match serde_json::to_value(kind) {
        Ok(serde_json::Value::String(s)) => s,
        _ => "unknown".to_owned(),
    }
}

/// The text indexed for an item: messages you sent, the agent's answers once
/// finished, and plans. Never tools, thinking, notices or questions.
pub(crate) fn index_text(item: &Item) -> Option<String> {
    let finished = match item.kind {
        ItemKind::User | ItemKind::Plan => true,
        ItemKind::Assistant => item.end_ms.is_some(),
        _ => false,
    };
    let text = item.text.as_deref().filter(|t| finished && !t.trim().is_empty())?;
    Some(searchable(text))
}

/// Whether a stored item of `kind` may have an index row.
fn maybe_indexed(kind: &str, ended: bool) -> bool {
    matches!(kind, "user" | "plan") || (kind == "assistant" && ended)
}

/// Inserts a new item row (not cut) with its index row. Returns its rowid.
pub(crate) fn insert(conn: &Connection, thread: &str, item: &Item, kind: &str, body: &str) -> Res<i64> {
    conn.prepare_cached(
        "INSERT INTO items (thread_id, seq, turn, kind, ts_ms, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?
    .execute(params![thread, int(item.seq)?, item.turn, kind, int(item.ts_ms)?, body])?;
    let rowid = conn.last_insert_rowid();
    if let Some(text) = index_text(item) {
        conn.prepare_cached("INSERT INTO item_text (rowid, text) VALUES (?1, ?2)")?
            .execute(params![rowid, text])?;
    }
    Ok(rowid)
}

/// Inserts `item`, or replaces the row of its `seq` unless that row has a
/// newer `rev`. Returns whether a row was added.
pub(crate) fn upsert(conn: &Connection, thread: &str, item: &Item, body: &str) -> Res<bool> {
    let kind = kind_word(item.kind);
    let old = conn
        .prepare_cached(
            "SELECT rowid, cut, kind, coalesce(json_extract(body, '$.rev'), 0), json_extract(body, '$.end_ms') IS NOT NULL
             FROM items WHERE thread_id = ?1 AND seq = ?2",
        )?
        .query_row(params![thread, int(item.seq)?], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, bool>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, bool>(4)?))
        })
        .optional()?;
    let Some((rowid, cut, old_kind, old_rev, old_ended)) = old else {
        insert(conn, thread, item, &kind, body)?;
        return Ok(true);
    };
    if i64::from(item.rev) < old_rev {
        return Ok(false);
    }
    conn.prepare_cached("UPDATE items SET turn = ?1, kind = ?2, ts_ms = ?3, body = ?4 WHERE rowid = ?5")?
        .execute(params![item.turn, kind, int(item.ts_ms)?, body, rowid])?;
    if !cut {
        match index_text(item) {
            Some(text) => {
                conn.prepare_cached("INSERT OR REPLACE INTO item_text (rowid, text) VALUES (?1, ?2)")?
                    .execute(params![rowid, text])?;
            }
            None if maybe_indexed(&old_kind, old_ended) => unindex(conn, rowid)?,
            None => {}
        }
    }
    Ok(false)
}

pub(crate) fn unindex(conn: &Connection, rowid: i64) -> Res<()> {
    conn.prepare_cached("DELETE FROM item_text WHERE rowid = ?1")?.execute(params![rowid])?;
    Ok(())
}

/// Indexes a stored item again from its body (after an undo).
pub(crate) fn reindex(conn: &Connection, rowid: i64, body: &str) -> Res<()> {
    if let Some(text) = serde_json::from_str::<Item>(body).ok().as_ref().and_then(index_text) {
        conn.prepare_cached("INSERT OR REPLACE INTO item_text (rowid, text) VALUES (?1, ?2)")?
            .execute(params![rowid, text])?;
    }
    Ok(())
}

/// Items stored for a thread, cut ones included.
pub(crate) fn count(conn: &Connection, thread: &str) -> Res<usize> {
    let n: i64 = conn
        .prepare_cached("SELECT COUNT(*) FROM items WHERE thread_id = ?1")?
        .query_row(params![thread], |r| r.get(0))?;
    Ok(usize::try_from(n).unwrap_or(0))
}

/// Removes the oldest items so the thread keeps fewer than `max`, and puts a
/// notice that says so first, at the `seq` of the newest one removed.
/// Returns the items left.
pub(crate) fn trim(conn: &Connection, thread: &str, max: usize) -> Res<usize> {
    let notice: Option<i64> = conn
        .prepare_cached(
            "SELECT rowid FROM items WHERE thread_id = ?1 AND kind = 'notice' AND json_extract(body, '$.id') = ?2",
        )?
        .query_row(params![thread, LEFT_OUT_ID], |r| r.get(0))
        .optional()?;
    let total = count(conn, thread)?;
    let real = total - usize::from(notice.is_some());
    let keep = max.saturating_sub(1 + TRIM_SLACK.min(max / 2)).max(1);
    if real <= keep {
        return Ok(total);
    }
    let remove = i64::try_from(real - keep).unwrap_or(i64::MAX);
    let victims: Vec<(i64, u64, u64, i64)> = conn
        .prepare_cached(
            "SELECT rowid, seq, ts_ms, coalesce(json_extract(body, '$.rev'), 0) FROM items
             WHERE thread_id = ?1 AND rowid IS NOT ?2 ORDER BY seq LIMIT ?3",
        )?
        .query_map(params![thread, notice, remove], |r| Ok((r.get(0)?, uint(r, 1)?, uint(r, 2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    let mut delete = conn.prepare_cached("DELETE FROM items WHERE rowid = ?1")?;
    for &(rowid, ..) in &victims {
        unindex(conn, rowid)?;
        delete.execute(params![rowid])?;
    }
    if let Some(rowid) = notice {
        delete.execute(params![rowid])?;
    }
    let Some(&(_, seq, ts_ms, rev)) = victims.last() else { return Ok(total) };
    let item = Item {
        text: Some(LEFT_OUT_TEXT.to_owned()),
        rev: u32::try_from(rev.saturating_add(1)).unwrap_or(u32::MAX),
        ..Item::new(seq, LEFT_OUT_ID, ItemKind::Notice, ts_ms)
    };
    let body = serde_json::to_string(&item)?;
    insert(conn, thread, &item, "notice", &body)?;
    Ok(keep + 1)
}

fn parse_all(rows: impl Iterator<Item = rusqlite::Result<String>>) -> Res<Vec<Item>> {
    let mut items = Vec::new();
    for body in rows {
        // A body this version can't read is skipped, as the file store skipped its line.
        if let Ok(item) = serde_json::from_str::<Item>(&body?) {
            items.push(item);
        }
    }
    Ok(items)
}

pub(crate) fn read_before(
    conn: &Connection,
    thread: &str,
    before: Option<u64>,
    limit: u32,
) -> Res<Vec<Item>> {
    let before = before.map_or(Ok(i64::MAX), int)?;
    let mut stmt = conn.prepare_cached(
        "SELECT body FROM (SELECT seq, body FROM items WHERE thread_id = ?1 AND cut = 0 AND seq < ?2
                           ORDER BY seq DESC LIMIT ?3) ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![thread, before, i64::from(limit)], |r| r.get(0))?;
    parse_all(rows)
}

pub(crate) fn read_after(conn: &Connection, thread: &str, after: u64, limit: u32) -> Res<Vec<Item>> {
    let mut stmt = conn.prepare_cached(
        "SELECT body FROM items WHERE thread_id = ?1 AND cut = 0 AND seq > ?2 ORDER BY seq LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![thread, int(after)?, i64::from(limit)], |r| r.get(0))?;
    parse_all(rows)
}

pub(crate) fn read_all(conn: &Connection, thread: &str) -> Res<BTreeMap<u64, Item>> {
    let mut stmt =
        conn.prepare_cached("SELECT body FROM items WHERE thread_id = ?1 AND cut = 0 ORDER BY seq")?;
    let rows = stmt.query_map(params![thread], |r| r.get(0))?;
    Ok(parse_all(rows)?.into_iter().map(|i| (i.seq, i)).collect())
}

pub(crate) fn insert_approval(conn: &Connection, thread: &str, ts_ms: u64, record: &str) -> Res<()> {
    conn.prepare_cached("INSERT INTO approvals (thread_id, ts_ms, record) VALUES (?1, ?2, ?3)")?
        .execute(params![thread, int(ts_ms)?, record])?;
    Ok(())
}

pub(crate) fn read_approvals(conn: &Connection, thread: &str) -> Res<Vec<ApprovalRecord>> {
    let mut stmt = conn.prepare_cached(
        "SELECT record FROM (SELECT rowid, record FROM approvals WHERE thread_id = ?1 ORDER BY rowid DESC LIMIT ?2)
         ORDER BY rowid",
    )?;
    let rows = stmt.query_map(params![thread, MAX_APPROVALS_READ], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for record in rows {
        if let Ok(record) = serde_json::from_str(&record?) {
            out.push(record);
        }
    }
    Ok(out)
}
