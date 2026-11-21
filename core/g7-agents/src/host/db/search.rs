// SPDX-License-Identifier: MPL-2.0
//! `threads.search` (§U3.4, reconcile §8).
//!
//! Terms of [`MIN_INDEXED_TERM`] characters or more go to the trigram index as
//! phrases joined with AND; shorter ones must then appear, folded, in the hit's
//! own text or its thread's title. A query of short terms only searches
//! thread titles. Hits come newest first, grouped by thread, at most
//! [`MAX_HITS_PER_THREAD`] a thread on a page, a title hit first.
//!
//! The index is contentless, so snippets and their marks are made here from
//! the item's own text, folded the way the tokenizer folds it (case, and the
//! diacritics of Latin letters: SQLite's own table, below).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, InterruptHandle, OptionalExtension, params};
use serde::de::DeserializeOwned;

use super::{Res, interrupted, invalid, uint};
use crate::api::{
    AgentId, Item, ItemKind, MAX_HITS_PER_THREAD, MAX_SEARCH_CANDIDATES, MAX_SEARCH_LIMIT, MAX_SEARCH_QUERY,
    MAX_SEARCH_TERMS, MAX_SNIPPET_CHARS, MAX_SNIPPET_MARKS, MAX_TITLE, MAX_TITLE_SCAN, MIN_INDEXED_TERM,
    SNIPPET_LEAD_CHARS, SearchHit, State,
};

/// Stops a search. The caller keeps one per client and stops it when that
/// client searches again; a statement that is running is interrupted.
#[derive(Clone, Default)]
pub struct Interrupt(Arc<Stop>);

#[derive(Default)]
struct Stop {
    stopped: AtomicBool,
    /// The connection a search runs on now.
    running: Mutex<Option<InterruptHandle>>,
}

impl fmt::Debug for Interrupt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Interrupt").field("stopped", &self.is_stopped()).finish_non_exhaustive()
    }
}

/// While a search runs on a connection; clears the registration when done.
struct Running<'a>(&'a Interrupt);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        *self.0.slot() = None;
    }
}

impl Interrupt {
    pub fn new() -> Self {
        Self::default()
    }

    fn slot(&self) -> MutexGuard<'_, Option<InterruptHandle>> {
        self.0.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stops the search: one not started yet fails at once, one running is
    /// interrupted at its next step.
    pub fn stop(&self) {
        self.0.stopped.store(true, Ordering::SeqCst);
        if let Some(handle) = self.slot().as_ref() {
            handle.interrupt();
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.0.stopped.load(Ordering::SeqCst)
    }

    /// Fails when stopped.
    pub(crate) fn check(&self) -> Res<()> {
        if self.is_stopped() { Err(interrupted()) } else { Ok(()) }
    }

    /// Registers `conn` as running this search. The registration and the
    /// check share the lock [`Interrupt::stop`] takes, so a stop can only
    /// interrupt this search's own statements; a stale interrupt is cleared
    /// by SQLite when the next statement starts on an idle connection.
    fn enter(&self, conn: &Connection) -> Res<Running<'_>> {
        let mut slot = self.slot();
        if self.is_stopped() {
            return Err(interrupted());
        }
        *slot = Some(conn.get_interrupt_handle());
        Ok(Running(self))
    }
}

/// A `threads.search` request, as the host passes it on.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub query: String,
    /// At most [`MAX_SEARCH_LIMIT`] hits.
    pub limit: u32,
    /// The `next` of the page before.
    pub cursor: Option<String>,
    /// Archived threads too.
    pub archived: bool,
    /// The threads the asking client may read; `None`: all of them.
    pub visible: Option<HashSet<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchPage {
    /// Newest first, grouped by thread. `more_in_thread` is the same on each
    /// hit of a thread.
    pub hits: Vec<SearchHit>,
    pub next: Option<String>,
    /// Every term was shorter than [`MIN_INDEXED_TERM`]: only titles were searched.
    pub titles_only: bool,
}

// ---- Folding, as SQLite's trigram tokenizer with `remove_diacritics 1` does ----

/// SQLite's `fts5_remove_diacritic` table (fts5_unicode2.c, public domain):
/// `(code point << 3) | range` keys, and the letter each range folds to (`0`:
/// a combining mark, dropped; the high bit: a letter with more than one
/// diacritic, folded too, as the trigram tokenizer asks).
const DIA: [u16; 126] = [
    0, 1797, 1848, 1859, 1891, 1928, 1940, 1995, 2024, 2040, 2060, 2110, 2168, 2206, 2264, 2286, 2344, 2383,
    2472, 2488, 2516, 2596, 2668, 2732, 2782, 2842, 2894, 2954, 2984, 3000, 3028, 3336, 3456, 3696, 3712,
    3728, 3744, 3766, 3832, 3896, 3912, 3928, 3944, 3968, 4008, 4040, 4056, 4106, 4138, 4170, 4202, 4234,
    4266, 4296, 4312, 4344, 4408, 4424, 4442, 4472, 4488, 4504, 6148, 6198, 6264, 6280, 6360, 6429, 6505,
    6529, 61448, 61468, 61512, 61534, 61592, 61610, 61642, 61672, 61688, 61704, 61726, 61784, 61800, 61816,
    61836, 61880, 61896, 61914, 61948, 61998, 62062, 62122, 62154, 62184, 62200, 62218, 62252, 62302, 62364,
    62410, 62442, 62478, 62536, 62554, 62584, 62604, 62640, 62648, 62656, 62664, 62730, 62766, 62830, 62890,
    62924, 62974, 63032, 63050, 63082, 63118, 63182, 63242, 63274, 63310, 63368, 63390,
];

const DIA_CHAR: [u8; 126] = [
    0, 97, 99, 101, 105, 110, 111, 117, 121, 121, 97, 99, 100, 101, 101, 103, 104, 105, 106, 107, 108, 110,
    111, 114, 115, 116, 117, 117, 119, 121, 122, 111, 117, 97, 105, 111, 117, 245, 225, 103, 107, 111, 239,
    106, 103, 110, 225, 97, 101, 105, 111, 114, 117, 115, 116, 104, 97, 101, 239, 111, 239, 121, 0, 0, 0, 0,
    0, 0, 0, 0, 97, 98, 227, 100, 100, 229, 101, 229, 102, 103, 104, 104, 105, 233, 107, 108, 236, 108, 109,
    110, 239, 112, 114, 242, 114, 115, 243, 116, 117, 245, 118, 119, 119, 120, 121, 122, 104, 116, 119, 121,
    97, 225, 225, 225, 101, 229, 229, 105, 111, 239, 239, 239, 117, 245, 245, 121,
];

fn remove_diacritic(c: u32) -> u32 {
    let key = (c << 3) | 7;
    let i = DIA.partition_point(|&d| u32::from(d) <= key).saturating_sub(1);
    let d = u32::from(DIA[i]);
    if c > (d >> 3) + (d & 7) { c } else { u32::from(DIA_CHAR[i] & 0x7f) }
}

/// One character folded; `None` for a combining mark the tokenizer drops.
fn fold_char(c: char) -> Option<char> {
    let mut lower = c.to_lowercase();
    let c = match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    };
    let code = u32::from(c);
    if !(128..65536).contains(&code) {
        return Some(c);
    }
    match remove_diacritic(code) {
        0 => None,
        folded => char::from_u32(folded),
    }
}

/// `s` folded for comparing: lower case, without the diacritics of Latin letters.
pub fn fold_text(s: &str) -> String {
    s.chars().filter_map(fold_char).collect()
}

/// `chars` folded, with the index in `chars` of each folded character.
fn fold_map(chars: &[char]) -> (Vec<char>, Vec<usize>) {
    let mut folded = Vec::with_capacity(chars.len());
    let mut at = Vec::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        if let Some(f) = fold_char(c) {
            folded.push(f);
            at.push(i);
        }
    }
    (folded, at)
}

fn find(hay: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

fn contains(hay: &[char], needle: &[char]) -> bool {
    find(hay, needle, 0).is_some()
}

/// A message's text as it is indexed and searched: Markdown link targets
/// dropped, their text kept (`[text](target)` → `text`, an image's `!` too).
pub fn searchable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let link = after.find(']').and_then(|close| {
            let target = after[close + 1..].strip_prefix('(')?;
            let end = target.find(')')?;
            Some((&after[..close], &target[end + 1..]))
        });
        match link {
            Some((label, tail)) => {
                let head = &rest[..open];
                out.push_str(head.strip_suffix('!').unwrap_or(head));
                out.push_str(label);
                rest = tail;
            }
            None => {
                out.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Runs of whitespace as one space, for a one-line snippet.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Up to [`MAX_SNIPPET_CHARS`] characters of `text` from at most
/// [`SNIPPET_LEAD_CHARS`] before its first match, and the matches in it as
/// (start, length) in UTF-16 units. Without a match: its start, no marks.
pub(crate) fn snippet(text: &str, terms: &[Vec<char>]) -> (String, Vec<(u16, u16)>) {
    let chars: Vec<char> = text.chars().collect();
    let (folded, at) = fold_map(&chars);
    let mut found: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        let mut from = 0;
        while let Some(p) = find(&folded, term, from) {
            found.push((at[p], at[p + term.len() - 1] + 1));
            from = p + term.len();
            if found.len() >= 64 {
                break;
            }
        }
    }
    found.sort_unstable();
    let start = found.first().map_or(0, |m| m.0.saturating_sub(SNIPPET_LEAD_CHARS));
    let end = (start + MAX_SNIPPET_CHARS).min(chars.len());
    let units = |a: usize, b: usize| -> u16 {
        u16::try_from(chars[a..b].iter().map(|c| c.len_utf16()).sum::<usize>()).unwrap_or(u16::MAX)
    };
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in found {
        if s >= end {
            break;
        }
        let e = e.min(end);
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    let marks = merged.iter().take(MAX_SNIPPET_MARKS).map(|&(s, e)| (units(start, s), units(s, e))).collect();
    (chars[start..end].iter().collect(), marks)
}

/// A term of the query, as typed and folded.
struct Term {
    text: String,
    folded: Vec<char>,
    indexed: bool,
}

fn terms(query: &str) -> Vec<Term> {
    crate::api::cut(query, MAX_SEARCH_QUERY)
        .split_whitespace()
        .take(MAX_SEARCH_TERMS)
        .filter_map(|t| {
            let folded: Vec<char> = t.chars().filter_map(fold_char).collect();
            (!folded.is_empty()).then(|| Term {
                indexed: t.chars().count() >= MIN_INDEXED_TERM,
                text: t.to_owned(),
                folded,
            })
        })
        .collect()
}

/// `"term"`, its quotes doubled, so nothing in it is FTS5 syntax. The
/// tokenizer folds it as it folded the text.
fn phrase(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

fn word<T: DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).ok()
}

/// What a hit says of its thread.
struct Head {
    title: String,
    folded_title: Vec<char>,
    agent: AgentId,
    state: State,
    archived: bool,
    updated_ms: u64,
    folder_label: String,
}

const HEAD_COLUMNS: &str = "id, title, agent, state, archived_ms IS NOT NULL, updated_ms, coalesce(json_extract(info, '$.folder_label'), '')";

fn head_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, Option<Head>)> {
    let id: String = row.get(0)?;
    let title: String = row.get(1)?;
    let agent: String = row.get(2)?;
    let state: String = row.get(3)?;
    let head = match (word::<AgentId>(&agent), word::<State>(&state)) {
        (Some(agent), Some(state)) => Some(Head {
            folded_title: title.chars().filter_map(fold_char).collect(),
            title: crate::api::cut(&title, MAX_TITLE).to_owned(),
            agent,
            state,
            archived: row.get(4)?,
            updated_ms: uint(row, 5)?,
            folder_label: row.get(6)?,
        }),
        _ => None,
    };
    Ok((id, head))
}

fn hit(
    thread_id: &str,
    head: &Head,
    seq: u64,
    kind: Option<ItemKind>,
    ts_ms: u64,
    snippet: (String, Vec<(u16, u16)>),
) -> SearchHit {
    SearchHit {
        thread_id: thread_id.to_owned(),
        title: head.title.clone(),
        agent: head.agent,
        folder_label: head.folder_label.clone(),
        state: head.state,
        archived: head.archived,
        seq,
        kind,
        ts_ms,
        snippet: snippet.0,
        marks: snippet.1,
        more_in_thread: 0,
    }
}

fn may_read(q: &SearchQuery, thread: &str) -> bool {
    q.visible.as_ref().is_none_or(|v| v.contains(thread))
}

fn page_limit(q: &SearchQuery) -> usize {
    usize::try_from(q.limit.clamp(1, MAX_SEARCH_LIMIT)).unwrap_or(1)
}

/// How many groups the earlier pages showed.
fn cursor(q: &SearchQuery) -> Res<usize> {
    match q.cursor.as_deref() {
        None => Ok(0),
        Some(c) => c
            .parse::<usize>()
            .ok()
            .filter(|&n| n <= MAX_SEARCH_CANDIDATES + MAX_TITLE_SCAN)
            .ok_or_else(|| invalid("bad search cursor")),
    }
}

pub(crate) fn search(conn: &Connection, q: &SearchQuery, stop: &Interrupt) -> Res<SearchPage> {
    let _running = stop.enter(conn)?;
    let terms = terms(&q.query);
    if terms.is_empty() {
        return Ok(SearchPage::default());
    }
    if terms.iter().any(|t| t.indexed) {
        messages(conn, q, &terms, stop)
    } else {
        titles(conn, q, &terms, stop)
    }
}

/// A query of short terms: thread titles only, newest first.
fn titles(conn: &Connection, q: &SearchQuery, terms: &[Term], stop: &Interrupt) -> Res<SearchPage> {
    let skip = cursor(q)?;
    let limit = page_limit(q);
    let folded: Vec<Vec<char>> = terms.iter().map(|t| t.folded.clone()).collect();
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {HEAD_COLUMNS} FROM threads ORDER BY updated_ms DESC, rowid DESC LIMIT ?1"
    ))?;
    let scan = i64::try_from(MAX_TITLE_SCAN).unwrap_or(i64::MAX);
    stop.check()?;
    let mut rows = stmt.query(params![scan])?;
    let mut hits = Vec::new();
    let mut matched = 0;
    let mut next = None;
    while let Some(row) = rows.next()? {
        stop.check()?;
        let (id, Some(head)) = head_of(row)? else { continue };
        if (head.archived && !q.archived)
            || !may_read(q, &id)
            || !folded.iter().all(|t| contains(&head.folded_title, t))
        {
            continue;
        }
        matched += 1;
        if matched <= skip {
            continue;
        }
        if hits.len() == limit {
            next = Some((matched - 1).to_string());
            break;
        }
        let snip = snippet(&head.title, &folded);
        hits.push(hit(&id, &head, 0, None, head.updated_ms, snip));
    }
    Ok(SearchPage { hits, next, titles_only: true })
}

/// An item that matched.
struct Candidate {
    rowid: i64,
    seq: u64,
    kind: Option<ItemKind>,
    ts_ms: u64,
}

/// A thread's hits on the way to a page.
struct Group {
    thread: String,
    newest: u64,
    title: bool,
    items: Vec<Candidate>,
}

/// An item's text as searched, on one line.
fn item_text(conn: &Connection, rowid: i64) -> Res<String> {
    let mut stmt = conn.prepare_cached("SELECT body FROM items WHERE rowid = ?1")?;
    let body: String = stmt.query_row(params![rowid], |r| r.get(0))?;
    let text = serde_json::from_str::<Item>(&body).ok().and_then(|i| i.text).unwrap_or_default();
    Ok(one_line(&searchable(&text)))
}

fn messages(conn: &Connection, q: &SearchQuery, terms: &[Term], stop: &Interrupt) -> Res<SearchPage> {
    let skip = cursor(q)?;
    let limit = page_limit(q);
    let expr = terms.iter().filter(|t| t.indexed).map(|t| phrase(&t.text)).collect::<Vec<_>>().join(" AND ");
    let short: Vec<&Vec<char>> = terms.iter().filter(|t| !t.indexed).map(|t| &t.folded).collect();
    let all: Vec<Vec<char>> = terms.iter().map(|t| t.folded.clone()).collect();
    let candidates_max = i64::try_from(MAX_SEARCH_CANDIDATES).unwrap_or(i64::MAX);

    let mut heads: HashMap<String, Option<Head>> = HashMap::new();
    let mut head_stmt = conn.prepare_cached(&format!("SELECT {HEAD_COLUMNS} FROM threads WHERE id = ?1"))?;
    let mut head = |heads: &mut HashMap<String, Option<Head>>, id: &str| -> Res<()> {
        if !heads.contains_key(id) {
            let h = head_stmt.query_row(params![id], head_of).optional()?.and_then(|(_, h)| h);
            heads.insert(id.to_owned(), h);
        }
        Ok(())
    };

    let mut groups: Vec<Group> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT i.rowid, i.thread_id, i.seq, i.kind, i.ts_ms FROM items i JOIN threads t ON t.id = i.thread_id
             WHERE i.rowid IN (SELECT rowid FROM item_text WHERE item_text MATCH ?1)
               AND i.cut = 0 AND (?2 OR t.archived_ms IS NULL)
             ORDER BY i.ts_ms DESC, i.rowid DESC LIMIT ?3",
        )?;
        stop.check()?;
        let mut rows = stmt.query(params![expr, q.archived, candidates_max])?;
        while let Some(row) = rows.next()? {
            stop.check()?;
            let thread: String = row.get(1)?;
            if !may_read(q, &thread) {
                continue;
            }
            let kind: String = row.get(3)?;
            let candidate =
                Candidate { rowid: row.get(0)?, seq: uint(row, 2)?, kind: word(&kind), ts_ms: uint(row, 4)? };
            head(&mut heads, &thread)?;
            let Some(Some(h)) = heads.get(&thread) else { continue };
            if !short.is_empty() {
                let text: Vec<char> =
                    item_text(conn, candidate.rowid)?.chars().filter_map(fold_char).collect();
                if !short.iter().all(|t| contains(&text, t) || contains(&h.folded_title, t)) {
                    continue;
                }
            }
            let at = *index.entry(thread.clone()).or_insert_with(|| {
                groups.push(Group { thread, newest: candidate.ts_ms, title: false, items: Vec::new() });
                groups.len() - 1
            });
            groups[at].items.push(candidate);
        }
    }
    {
        let mut stmt = conn.prepare_cached(
            "SELECT t.id FROM threads t
             WHERE t.rowid IN (SELECT rowid FROM title_text WHERE title_text MATCH ?1) AND (?2 OR t.archived_ms IS NULL)",
        )?;
        let ids: Vec<String> =
            stmt.query_map(params![expr, q.archived], |r| r.get(0))?.collect::<Result<_, _>>()?;
        for thread in ids {
            stop.check()?;
            if !may_read(q, &thread) {
                continue;
            }
            head(&mut heads, &thread)?;
            let Some(Some(h)) = heads.get(&thread) else { continue };
            if !short.iter().all(|t| contains(&h.folded_title, t)) {
                continue;
            }
            let updated = h.updated_ms;
            match index.get(&thread) {
                Some(&at) => {
                    groups[at].title = true;
                    groups[at].newest = groups[at].newest.max(updated);
                }
                None => {
                    index.insert(thread.clone(), groups.len());
                    groups.push(Group { thread, newest: updated, title: true, items: Vec::new() });
                }
            }
        }
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.newest));

    let mut hits: Vec<SearchHit> = Vec::new();
    let mut next = None;
    for (n, group) in groups.iter().enumerate().skip(skip) {
        stop.check()?;
        if hits.len() >= limit {
            next = Some(n.to_string());
            break;
        }
        let Some(Some(h)) = heads.get(&group.thread) else { continue };
        let total = usize::from(group.title) + group.items.len();
        let shown = total.min(MAX_HITS_PER_THREAD).min(limit - hits.len());
        let more = u32::try_from(total - shown).unwrap_or(u32::MAX);
        let first = hits.len();
        if group.title {
            hits.push(hit(&group.thread, h, 0, None, h.updated_ms, snippet(&h.title, &all)));
        }
        for c in &group.items {
            if hits.len() - first >= shown {
                break;
            }
            let text = item_text(conn, c.rowid)?;
            hits.push(hit(&group.thread, h, c.seq, c.kind, c.ts_ms, snippet(&text, &all)));
        }
        for hit in &mut hits[first..] {
            hit.more_in_thread = more;
        }
    }
    Ok(SearchPage { hits, next, titles_only: false })
}

/// A query as an index expression of its long terms (`None` without one),
/// and its short terms, folded, to check by hand.
pub(crate) fn match_expr(query: &str) -> (Option<String>, Vec<Vec<char>>) {
    let terms = terms(query);
    let expr = terms.iter().filter(|t| t.indexed).map(|t| phrase(&t.text)).collect::<Vec<_>>().join(" AND ");
    let short = terms.into_iter().filter(|t| !t.indexed).map(|t| t.folded).collect();
    ((!expr.is_empty()).then_some(expr), short)
}

/// Whether every one of `terms` appears in `text`, folded.
pub(crate) fn has_all(text: &str, terms: &[Vec<char>]) -> bool {
    let folded: Vec<char> = text.chars().filter_map(fold_char).collect();
    terms.iter().all(|t| contains(&folded, t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_matches_the_tokenizer() {
        assert_eq!(fold_text("Le Café Crème est PRÊT"), "le cafe creme est pret");
        assert_eq!(fold_text("e\u{301}"), "e", "a combining mark is dropped");
        assert_eq!(fold_text("東京"), "東京");
        // Whatever is folded here, the bundled tokenizer folds the same way.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(x, tokenize = 'trigram remove_diacritics 1', content = '')",
        )
        .unwrap();
        let samples =
            ["Ǖber Ễnd", "Ñandú Øre", "Straße", "ÀÉÎÕÜ àéîõü", "Łódź Żółć", "Crème brûlée", "Ελλάδα"];
        for (n, sample) in (1i64..).zip(samples) {
            conn.execute("INSERT INTO t (rowid, x) VALUES (?1, ?2)", params![n, sample]).unwrap();
            let folded = fold_text(sample);
            let hit: Option<i64> = conn
                .query_row("SELECT rowid FROM t WHERE t MATCH ?1", [phrase(&folded)], |r| r.get(0))
                .optional()
                .unwrap();
            assert_eq!(hit, Some(n), "{sample} folds to {folded}");
        }
    }

    #[test]
    fn link_targets_are_dropped() {
        assert_eq!(
            searchable("see [the docs](https://x.example/a) and ![a cat](cat.png)."),
            "see the docs and a cat."
        );
        assert_eq!(searchable("[not a link] (x) [open"), "[not a link] (x) [open");
        assert_eq!(searchable("a [b](c"), "a [b](c");
    }

    #[test]
    fn a_running_statement_is_interrupted() {
        let conn = Connection::open_in_memory().unwrap();
        let stop = Interrupt::new();
        let worker = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let _running = stop.enter(&conn).unwrap();
                conn.query_row(
                    "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n) SELECT count(*) FROM n",
                    [],
                    |r| r.get::<_, i64>(0),
                )
            })
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while stop.slot().is_none() {
            assert!(std::time::Instant::now() < deadline, "the search never started");
            std::thread::yield_now();
        }
        // An interrupt that lands before the statement starts is lost (SQLite
        // clears it when a statement starts on an idle connection): the
        // search's own checks cover that gap, so signal until it stops.
        while !worker.is_finished() {
            stop.stop();
            assert!(std::time::Instant::now() < deadline, "the statement wasn't interrupted");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let err = worker.join().unwrap().unwrap_err();
        assert_eq!(super::super::sql(err).kind(), std::io::ErrorKind::Interrupted);
        assert!(stop.slot().is_none(), "the registration ends with the search");
        assert!(
            stop.enter(&Connection::open_in_memory().unwrap()).is_err(),
            "a stopped search doesn't start"
        );
    }

    #[test]
    fn snippets_and_marks() {
        let terms = vec![fold_text("cafe").chars().collect::<Vec<_>>()];
        let (s, marks) = snippet("Le café crème", &terms);
        assert_eq!(s, "Le café crème");
        assert_eq!(marks, vec![(3, 4)]);
        let long = format!("{}needle{}", "x".repeat(100), "y".repeat(400));
        let terms = vec!["needle".chars().collect::<Vec<_>>()];
        let (s, marks) = snippet(&long, &terms);
        assert_eq!(s.chars().count(), MAX_SNIPPET_CHARS);
        assert!(s.starts_with(&"x".repeat(SNIPPET_LEAD_CHARS)));
        assert_eq!(marks, vec![(24, 6)]);
    }
}
