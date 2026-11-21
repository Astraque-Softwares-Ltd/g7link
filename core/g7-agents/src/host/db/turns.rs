// SPDX-License-Identifier: MPL-2.0
//! Turns, their files, and the rewind's cut: `cut`, `uncut`, `purge`.

use std::collections::BTreeMap;
use std::io;

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::threads::info_with_native;
use super::{Fail, Res, int, items, not_found, opt_uint, opt_uint32, uint, uint32};
use crate::api::{
    Checkpoint, CheckpointWhy, FileStatus, Item, ItemKind, MAX_TURN_FILES, State, TurnEnd, TurnFile, TurnInfo,
};

/// Large files left out of a restore point that a turn lists by name.
pub const MAX_SKIPPED: usize = 200;

/// A turn as stored (§U3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRow {
    /// 1-based per thread, never reused.
    pub turn: u32,
    /// The user item that started it.
    pub prompt_seq: u64,
    pub last_seq: Option<u64>,
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
    pub ended: Option<TurnEnd>,
    /// Claude: the `uuid` of its replayed prompt line; Codex: the turn id.
    pub native_mark: Option<String>,
    pub checkpoint: Checkpoint,
    /// With `checkpoint: missing`.
    pub why: Option<CheckpointWhy>,
    /// The restore points' commits, before and after.
    pub pre_oid: Option<String>,
    pub post_oid: Option<String>,
    /// `"<ref or 'detached'> <oid>"` at each capture.
    pub pre_head: Option<String>,
    pub post_head: Option<String>,
    pub files: u32,
    pub adds: u32,
    pub dels: u32,
    /// Large files left out of the restore point, at most [`MAX_SKIPPED`].
    pub skipped: Vec<String>,
    /// Rewound away, kept until the rewind can no longer be undone. Set by
    /// the database only; a saved row keeps the value stored.
    pub cut: bool,
}

impl TurnRow {
    /// A turn that has just started: its restore point is being recorded.
    pub fn new(turn: u32, prompt_seq: u64, started_ms: u64) -> Self {
        Self {
            turn,
            prompt_seq,
            last_seq: None,
            started_ms,
            ended_ms: None,
            ended: None,
            native_mark: None,
            checkpoint: Checkpoint::Pending,
            why: None,
            pre_oid: None,
            post_oid: None,
            pre_head: None,
            post_head: None,
            files: 0,
            adds: 0,
            dels: 0,
            skipped: Vec::new(),
            cut: false,
        }
    }

    /// As `thread.turns` sends it, before the host's verdicts (`can_*` false).
    pub fn info(&self) -> TurnInfo {
        TurnInfo {
            turn: self.turn,
            prompt_seq: self.prompt_seq,
            last_seq: self.last_seq,
            started_ms: self.started_ms,
            ended_ms: self.ended_ms,
            ended: self.ended,
            checkpoint: self.checkpoint,
            why: self.why,
            files: self.files,
            adds: self.adds,
            dels: self.dels,
            more_files: usize::try_from(self.files).is_ok_and(|f| f > MAX_TURN_FILES),
            skipped: u32::try_from(self.skipped.len()).unwrap_or(u32::MAX),
            can_fork: false,
            can_rewind: false,
            can_restore: false,
        }
    }
}

/// What a cut covers, or what an undo brought back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CutRange {
    /// The first and last `seq` it covers (`thread.cut`).
    pub from_seq: u64,
    pub to_seq: u64,
    /// Turns in it.
    pub turns: u32,
    /// Your messages and the agent's answers in it.
    pub messages: u32,
}

/// A turn's restore points in G7's own repository (reconcile §1).
pub fn turn_refs(thread: &str, turn: u32) -> [String; 2] {
    [format!("refs/g7/{thread}/{turn}/pre"), format!("refs/g7/{thread}/{turn}/post")]
}

/// The point a rewind restores files from on undo.
pub fn undo_ref(thread: &str) -> String {
    format!("refs/g7/{thread}/undo")
}

const COLUMNS: &str = "turn, prompt_seq, last_seq, started_ms, ended_ms, ended, native_mark, checkpoint, why,
     pre_oid, post_oid, pre_head, post_head, files, adds, dels, skipped, cut";

fn row_of(r: &Row<'_>) -> rusqlite::Result<TurnRow> {
    let ended: Option<String> = r.get(5)?;
    let checkpoint: String = r.get(7)?;
    let why: Option<String> = r.get(8)?;
    let skipped: Option<String> = r.get(16)?;
    Ok(TurnRow {
        turn: uint32(r, 0)?,
        prompt_seq: uint(r, 1)?,
        last_seq: opt_uint(r, 2)?,
        started_ms: uint(r, 3)?,
        ended_ms: opt_uint(r, 4)?,
        ended: ended.as_deref().and_then(TurnEnd::parse),
        native_mark: r.get(6)?,
        checkpoint: Checkpoint::parse(&checkpoint).unwrap_or_default(),
        why: why.as_deref().and_then(CheckpointWhy::parse),
        pre_oid: r.get(9)?,
        post_oid: r.get(10)?,
        pre_head: r.get(11)?,
        post_head: r.get(12)?,
        files: uint32(r, 13)?,
        adds: uint32(r, 14)?,
        dels: uint32(r, 15)?,
        skipped: skipped.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        cut: r.get(17)?,
    })
}

pub(crate) fn upsert(conn: &Connection, thread: &str, t: &TurnRow, files: Option<&[TurnFile]>) -> Res<()> {
    let skipped = (!t.skipped.is_empty())
        .then(|| serde_json::to_string(&t.skipped[..t.skipped.len().min(MAX_SKIPPED)]))
        .transpose()?;
    conn.prepare_cached(
        "INSERT INTO turns (thread_id, turn, prompt_seq, last_seq, started_ms, ended_ms, ended, native_mark,
             checkpoint, why, pre_oid, post_oid, pre_head, post_head, files, adds, dels, skipped)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
         ON CONFLICT (thread_id, turn) DO UPDATE SET prompt_seq = excluded.prompt_seq,
             last_seq = excluded.last_seq, started_ms = excluded.started_ms, ended_ms = excluded.ended_ms,
             ended = excluded.ended, native_mark = excluded.native_mark, checkpoint = excluded.checkpoint,
             why = excluded.why, pre_oid = excluded.pre_oid, post_oid = excluded.post_oid,
             pre_head = excluded.pre_head, post_head = excluded.post_head, files = excluded.files,
             adds = excluded.adds, dels = excluded.dels, skipped = excluded.skipped",
    )?
    .execute(params![
        thread,
        t.turn,
        int(t.prompt_seq)?,
        t.last_seq.map(int).transpose()?,
        int(t.started_ms)?,
        t.ended_ms.map(int).transpose()?,
        t.ended.map(TurnEnd::as_str),
        t.native_mark,
        t.checkpoint.as_str(),
        t.why.map(CheckpointWhy::as_str),
        t.pre_oid,
        t.post_oid,
        t.pre_head,
        t.post_head,
        t.files,
        t.adds,
        t.dels,
        skipped,
    ])?;
    if let Some(files) = files {
        conn.prepare_cached("DELETE FROM turn_files WHERE thread_id = ?1 AND turn = ?2")?
            .execute(params![thread, t.turn])?;
        let mut insert = conn.prepare_cached(
            "INSERT OR REPLACE INTO turn_files (thread_id, turn, path, status, from_path, adds, dels, binary)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for f in files.iter().take(MAX_TURN_FILES) {
            insert.execute(params![
                thread,
                t.turn,
                f.path,
                f.status.as_str(),
                f.from,
                f.adds,
                f.dels,
                f.binary
            ])?;
        }
    }
    Ok(())
}

pub(crate) fn read(conn: &Connection, thread: &str, before: Option<u32>, limit: u32) -> Res<Vec<TurnRow>> {
    let before = before.map_or(i64::MAX, i64::from);
    let rows = conn
        .prepare_cached(&format!(
            "SELECT {COLUMNS} FROM turns WHERE thread_id = ?1 AND cut = 0 AND turn < ?2 ORDER BY turn DESC LIMIT ?3"
        ))?
        .query_map(params![thread, before, i64::from(limit)], row_of)?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Turns up to `through`, not cut, oldest first.
pub(crate) fn read_range(conn: &Connection, thread: &str, through: u32) -> Res<Vec<TurnRow>> {
    let rows = conn
        .prepare_cached(&format!(
            "SELECT {COLUMNS} FROM turns WHERE thread_id = ?1 AND cut = 0 AND turn <= ?2 ORDER BY turn"
        ))?
        .query_map(params![thread, through], row_of)?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

pub(crate) fn read_one(conn: &Connection, thread: &str, turn: u32) -> Res<Option<TurnRow>> {
    Ok(conn
        .prepare_cached(&format!("SELECT {COLUMNS} FROM turns WHERE thread_id = ?1 AND turn = ?2"))?
        .query_row(params![thread, turn], row_of)
        .optional()?)
}

pub(crate) fn read_files(conn: &Connection, thread: &str, turn: u32) -> Res<Vec<TurnFile>> {
    let limit = i64::try_from(MAX_TURN_FILES).unwrap_or(i64::MAX);
    let files = conn
        .prepare_cached(
            "SELECT path, status, from_path, adds, dels, binary FROM turn_files
             WHERE thread_id = ?1 AND turn = ?2 ORDER BY path LIMIT ?3",
        )?
        .query_map(params![thread, turn, limit], |r| {
            let status: String = r.get(1)?;
            Ok(TurnFile {
                path: r.get(0)?,
                status: FileStatus::parse(&status).unwrap_or_default(),
                from: r.get(2)?,
                adds: opt_uint32(r, 3)?.unwrap_or(0),
                dels: opt_uint32(r, 4)?.unwrap_or(0),
                binary: r.get(5)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(files)
}

/// The turns of a thread from the file store, rebuilt from its items' turn
/// numbers: no restore points (`missing`, `before`), the last one ended as
/// the thread did.
pub(crate) fn rebuild(items: &BTreeMap<u64, Item>, state: State) -> Vec<TurnRow> {
    let mut by_turn: BTreeMap<u32, Vec<&Item>> = BTreeMap::new();
    for item in items.values() {
        if let Some(turn) = item.turn {
            by_turn.entry(turn).or_default().push(item);
        }
    }
    let last = by_turn.keys().next_back().copied();
    by_turn
        .into_iter()
        .filter_map(|(turn, items)| {
            let prompt = items
                .iter()
                .find(|i| {
                    i.kind == ItemKind::User && i.deny_of.is_none() && i.via != Some(crate::api::Via::Steer)
                })
                .or_else(|| items.first())?;
            let ended = if Some(turn) == last {
                match state {
                    State::Done => TurnEnd::Done,
                    State::Failed => TurnEnd::Failed,
                    State::Interrupted | State::Working | State::Waiting => TurnEnd::Stopped,
                }
            } else {
                TurnEnd::Done
            };
            Some(TurnRow {
                last_seq: items.iter().map(|i| i.seq).max(),
                ended_ms: items.iter().map(|i| i.end_ms.unwrap_or(i.ts_ms).max(i.ts_ms)).max(),
                ended: Some(ended),
                checkpoint: Checkpoint::Missing,
                why: Some(CheckpointWhy::Before),
                ..TurnRow::new(turn, prompt.seq, prompt.ts_ms)
            })
        })
        .collect()
}

/// Whether the thread has a cut that can still be undone.
fn has_cut(conn: &Connection, thread: &str) -> Res<bool> {
    Ok(conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM items WHERE thread_id = ?1 AND cut = 1)
                 OR EXISTS (SELECT 1 FROM turns WHERE thread_id = ?1 AND cut = 1)",
        )?
        .query_row(params![thread], |r| r.get(0))?)
}

pub(crate) fn preview_cut(conn: &Connection, thread: &str, from_turn: u32) -> Res<CutRange> {
    let prompt_seq = conn
        .prepare_cached("SELECT prompt_seq FROM turns WHERE thread_id = ?1 AND turn = ?2 AND cut = 0")?
        .query_row(params![thread, from_turn], |r| uint(r, 0))
        .optional()?
        .ok_or_else(|| not_found("no such turn"))?;
    let (to_seq, messages) = conn
        .prepare_cached(
            "SELECT MAX(seq), coalesce(SUM(kind IN ('user', 'assistant')), 0) FROM items
             WHERE thread_id = ?1 AND cut = 0 AND seq >= ?2",
        )?
        .query_row(params![thread, int(prompt_seq)?], |r| Ok((opt_uint(r, 0)?, uint32(r, 1)?)))?;
    let turns = conn
        .prepare_cached("SELECT COUNT(*) FROM turns WHERE thread_id = ?1 AND cut = 0 AND turn >= ?2")?
        .query_row(params![thread, from_turn], |r| uint32(r, 0))?;
    Ok(CutRange { from_seq: prompt_seq, to_seq: to_seq.unwrap_or(prompt_seq), turns, messages })
}

/// The thread's native id and its `info`.
fn native_of(conn: &Connection, thread: &str) -> Res<(Option<String>, Option<String>, String)> {
    conn.prepare_cached("SELECT native_id, prior_native_id, info FROM threads WHERE id = ?1")?
        .query_row(params![thread], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?
        .ok_or_else(|| not_found("no such thread"))
}

pub(crate) fn cut(conn: &Connection, thread: &str, from_turn: u32, native: &str) -> Res<CutRange> {
    let (old_native, _, info) = native_of(conn, thread)?;
    if has_cut(conn, thread)? {
        return Err(Fail::Io(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "an earlier rewind can still be undone",
        )));
    }
    let range = preview_cut(conn, thread, from_turn)?;
    let from_seq = int(range.from_seq)?;
    let cut_rows: Vec<i64> = conn
        .prepare_cached("SELECT rowid FROM items WHERE thread_id = ?1 AND cut = 0 AND seq >= ?2")?
        .query_map(params![thread, from_seq], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for rowid in cut_rows {
        items::unindex(conn, rowid)?;
    }
    conn.prepare_cached("UPDATE items SET cut = 1 WHERE thread_id = ?1 AND cut = 0 AND seq >= ?2")?
        .execute(params![thread, from_seq])?;
    conn.prepare_cached("UPDATE turns SET cut = 1 WHERE thread_id = ?1 AND turn >= ?2")?
        .execute(params![thread, from_turn])?;
    let native = Some(native).filter(|n| !n.is_empty());
    conn.prepare_cached("UPDATE threads SET prior_native_id = ?2, native_id = ?3, info = ?4 WHERE id = ?1")?
        .execute(params![thread, old_native, native, info_with_native(&info, native)?])?;
    Ok(range)
}

pub(crate) fn uncut(conn: &Connection, thread: &str) -> Res<CutRange> {
    let (_, prior, info) = native_of(conn, thread)?;
    if !has_cut(conn, thread)? {
        return Err(not_found("nothing to undo"));
    }
    let (from_seq, to_seq) = conn
        .prepare_cached("SELECT MIN(seq), MAX(seq) FROM items WHERE thread_id = ?1 AND cut = 1")?
        .query_row(params![thread], |r| Ok((opt_uint(r, 0)?, opt_uint(r, 1)?)))?;
    let mut last = to_seq.unwrap_or(0);
    // The notices the rewind wrote ("Rewound to here", what was restored).
    if let Some(to_seq) = to_seq {
        let notices: Vec<(i64, u64)> = conn
            .prepare_cached(
                "SELECT rowid, seq FROM items WHERE thread_id = ?1 AND cut = 0 AND kind = 'notice' AND seq > ?2",
            )?
            .query_map(params![thread, int(to_seq)?], |r| Ok((r.get(0)?, uint(r, 1)?)))?
            .collect::<Result<_, _>>()?;
        for (rowid, seq) in notices {
            items::unindex(conn, rowid)?;
            conn.prepare_cached("DELETE FROM items WHERE rowid = ?1")?.execute(params![rowid])?;
            last = last.max(seq);
        }
    }
    let back: Vec<(i64, String, String)> = conn
        .prepare_cached("SELECT rowid, kind, body FROM items WHERE thread_id = ?1 AND cut = 1")?
        .query_map(params![thread], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    let messages =
        u32::try_from(back.iter().filter(|(_, kind, _)| kind == "user" || kind == "assistant").count())
            .unwrap_or(u32::MAX);
    for (rowid, _, body) in &back {
        items::reindex(conn, *rowid, body)?;
    }
    conn.prepare_cached("UPDATE items SET cut = 0 WHERE thread_id = ?1 AND cut = 1")?
        .execute(params![thread])?;
    let turns = conn
        .prepare_cached("UPDATE turns SET cut = 0 WHERE thread_id = ?1 AND cut = 1")?
        .execute(params![thread])?;
    conn.prepare_cached(
        "UPDATE threads SET native_id = ?2, prior_native_id = NULL, info = ?3 WHERE id = ?1",
    )?
    .execute(params![thread, prior, info_with_native(&info, prior.as_deref())?])?;
    Ok(CutRange {
        from_seq: from_seq.unwrap_or(last),
        to_seq: last,
        turns: u32::try_from(turns).unwrap_or(u32::MAX),
        messages,
    })
}

/// Whether a stored turn still has refs: it was recorded and not pruned.
const HAS_REFS: &str = "checkpoint <> 'expired'
     AND (pre_oid IS NOT NULL OR post_oid IS NOT NULL OR checkpoint IN ('ready', 'pending'))";

pub(crate) fn purge(conn: &Connection, thread: &str) -> Res<Vec<String>> {
    let had = has_cut(conn, thread)?;
    let recorded: Vec<u32> = conn
        .prepare_cached(&format!("SELECT turn FROM turns WHERE thread_id = ?1 AND cut = 1 AND {HAS_REFS}"))?
        .query_map(params![thread], |r| uint32(r, 0))?
        .collect::<Result<_, _>>()?;
    conn.prepare_cached("DELETE FROM items WHERE thread_id = ?1 AND cut = 1")?.execute(params![thread])?;
    conn.prepare_cached("DELETE FROM turns WHERE thread_id = ?1 AND cut = 1")?.execute(params![thread])?;
    conn.prepare_cached("UPDATE threads SET prior_native_id = NULL WHERE id = ?1")?
        .execute(params![thread])?;
    let mut refs: Vec<String> = recorded.into_iter().flat_map(|t| turn_refs(thread, t)).collect();
    if had {
        refs.push(undo_ref(thread));
    }
    Ok(refs)
}

pub(crate) fn expire(conn: &Connection, thread: &str, keep: u32) -> Res<Vec<String>> {
    let Some(keep) = keep.checked_sub(1) else { return Ok(Vec::new()) };
    // The oldest turn that keeps its restore point.
    let oldest_kept: Option<u32> = conn
        .prepare_cached(
            "SELECT turn FROM turns WHERE thread_id = ?1 AND cut = 0 ORDER BY turn DESC LIMIT 1 OFFSET ?2",
        )?
        .query_row(params![thread, keep], |r| uint32(r, 0))
        .optional()?;
    let Some(oldest_kept) = oldest_kept else { return Ok(Vec::new()) };
    let old: Vec<u32> = conn
        .prepare_cached(&format!(
            "SELECT turn FROM turns WHERE thread_id = ?1 AND cut = 0 AND turn < ?2 AND {HAS_REFS}"
        ))?
        .query_map(params![thread, oldest_kept], |r| uint32(r, 0))?
        .collect::<Result<_, _>>()?;
    let mut update = conn.prepare_cached(
        "UPDATE turns SET checkpoint = 'expired', why = NULL WHERE thread_id = ?1 AND turn = ?2",
    )?;
    for &turn in &old {
        update.execute(params![thread, turn])?;
    }
    Ok(old.into_iter().flat_map(|t| turn_refs(thread, t)).collect())
}
