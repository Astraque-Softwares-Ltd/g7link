// SPDX-License-Identifier: MPL-2.0
//! Drafts and history (§Q4.10). A draft is a composer's unsent text, kept
//! per thread (or per folder for a new thread) so another device can pick it
//! up; `rev` orders the writes, and a stale one is refused with the stored
//! draft. History is the last prompts sent from this PC, never a phone's.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::json;

use crate::api::*;
use crate::host::Host;

/// The drafts, newest last.
#[derive(Debug, Default)]
pub struct Drafts {
    map: HashMap<String, Draft>,
    /// Keys, least recently set first.
    order: VecDeque<String>,
}

impl Drafts {
    /// From the store, oldest first.
    pub fn load(drafts: Vec<Draft>) -> Self {
        let mut out = Self::default();
        for draft in drafts {
            out.put(draft);
        }
        out
    }

    pub fn get(&self, key: &str) -> Option<&Draft> {
        self.map.get(key)
    }

    /// Oldest first, as stored.
    pub fn list(&self) -> Vec<Draft> {
        self.order.iter().filter_map(|k| self.map.get(k).cloned()).collect()
    }

    fn put(&mut self, draft: Draft) {
        self.order.retain(|k| k != &draft.key);
        self.order.push_back(draft.key.clone());
        self.map.insert(draft.key.clone(), draft);
        while self.order.len() > MAX_DRAFTS {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    /// Sets a draft when `rev` is the stored one's (0 for a new key).
    /// The new rev, or the stored draft the write conflicts with.
    pub fn set(
        &mut self,
        key: &str,
        text: String,
        images: Vec<ImageRef>,
        rev: u64,
        by: Option<Origin>,
    ) -> Result<u64, Option<Box<Draft>>> {
        let current = self.map.get(key).map_or(0, |d| d.rev);
        if rev != current {
            return Err(self.map.get(key).cloned().map(Box::new));
        }
        let rev = current + 1;
        self.put(Draft { key: key.to_owned(), text, images, rev, by });
        Ok(rev)
    }

    /// Its text was sent: the draft is emptied (its `rev` goes on, so a
    /// stale write still conflicts). The new rev, when there was one.
    pub fn sent(&mut self, key: &str, by: &Origin) -> Option<u64> {
        let draft = self.map.get_mut(key)?;
        if draft.text.is_empty() && draft.images.is_empty() {
            return None;
        }
        draft.text.clear();
        draft.images.clear();
        draft.rev += 1;
        draft.by = Some(by.clone());
        Some(draft.rev)
    }
}

/// The prompts sent from this PC, oldest first.
#[derive(Debug, Default)]
pub struct History {
    lines: VecDeque<String>,
}

impl History {
    pub fn load(lines: Vec<String>) -> Self {
        Self { lines: lines.into() }
    }

    /// Adds one; `true` when the oldest went and the file must be rewritten.
    pub fn push(&mut self, text: &str) -> bool {
        self.lines.push_back(cut(text, MAX_HISTORY_PROMPT).to_owned());
        let over = self.lines.len() > MAX_HISTORY;
        while self.lines.len() > MAX_HISTORY {
            self.lines.pop_front();
        }
        over
    }

    /// Newest first.
    pub fn newest(&self, limit: usize) -> Vec<String> {
        self.lines.iter().rev().take(limit).cloned().collect()
    }

    pub fn all(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }
}

impl Host {
    fn lock_drafts(&self) -> std::sync::MutexGuard<'_, Drafts> {
        self.drafts.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Writes the drafts once nothing changed for [`DRAFT_SAVE_MS`].
    fn save_drafts_later(self: &Arc<Self>) {
        let generation = self.drafts_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let host = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(DRAFT_SAVE_MS)).await;
            let Some(host) = host.upgrade() else { return };
            if host.drafts_generation.load(Ordering::SeqCst) == generation {
                host.store.save_drafts(host.lock_drafts().list());
            }
        });
    }

    /// Writes the drafts now (shutdown).
    pub(crate) fn save_drafts_now(&self) {
        self.drafts_generation.fetch_add(1, Ordering::SeqCst);
        self.store.save_drafts(self.lock_drafts().list());
    }

    /// Puts `text` and `images` in a thread's draft whatever was there
    /// (Edit from here): every client is told, the asker too.
    pub(crate) fn draft_refill(
        self: &Arc<Self>,
        key: &str,
        text: String,
        images: Vec<ImageRef>,
        by: Option<Origin>,
    ) {
        let set = {
            let mut drafts = self.lock_drafts();
            let rev = drafts.get(key).map_or(0, |d| d.rev);
            drafts.set(key, text, images, rev, by.clone())
        };
        if let Ok(rev) = set {
            self.save_drafts_later();
            self.hub.broadcast(&Event::DraftChanged(DraftChangedEvent { key: key.to_owned(), rev, by }));
        }
    }

    /// `draft.set`. `client`: the connection that set it, which isn't told.
    pub fn draft_set(
        self: &Arc<Self>,
        p: DraftSetParams,
        images: Vec<ImageRef>,
        client: u64,
    ) -> Result<u64, RpcError> {
        if !is_valid_draft_key(&p.key) {
            return Err(RpcError::new(ErrorCode::BadRequest, "Bad draft key."));
        }
        if p.text.len() > MAX_PROMPT || images.len() > MAX_IMAGES {
            return Err(RpcError::new(ErrorCode::BadRequest, "The draft is too long."));
        }
        if p.by.as_ref().is_some_and(|b| !b.is_valid()) {
            return Err(RpcError::new(ErrorCode::BadRequest, "Bad origin."));
        }
        let set = self.lock_drafts().set(&p.key, p.text, images, p.rev, p.by.clone());
        match set {
            Ok(rev) => {
                self.save_drafts_later();
                let changed = DraftChangedEvent { key: p.key, rev, by: p.by };
                self.hub.broadcast_except(&Event::DraftChanged(changed), client);
                Ok(rev)
            }
            Err(stored) => {
                let mut error = RpcError::new(ErrorCode::Conflict, "The draft changed elsewhere.");
                error.data = Some(json!({ "draft": stored }));
                Err(error)
            }
        }
    }

    pub fn draft_get(&self, key: &str) -> Result<Option<Draft>, RpcError> {
        if !is_valid_draft_key(key) {
            return Err(RpcError::new(ErrorCode::BadRequest, "Bad draft key."));
        }
        Ok(self.lock_drafts().get(key).cloned())
    }

    /// A message was sent from the composer of `key`: its draft is emptied
    /// and every client hears of it.
    pub(crate) fn draft_sent(self: &Arc<Self>, key: &str, by: &Origin) {
        let rev = self.lock_drafts().sent(key, by);
        if let Some(rev) = rev {
            self.save_drafts_later();
            let changed = DraftChangedEvent { key: key.to_owned(), rev, by: Some(by.clone()) };
            self.hub.broadcast(&Event::DraftChanged(changed));
        }
    }

    /// Keeps a prompt sent from this PC (not a phone's).
    pub(crate) fn remember_prompt(&self, text: &str, origin: &Origin) {
        if !matches!(origin.kind, OriginKind::Launcher | OriginKind::Cli) {
            return;
        }
        let mut history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        if history.push(text) {
            self.store.rewrite_history(history.all());
        } else {
            self.store.append_history(cut(text, MAX_HISTORY_PROMPT).to_owned());
        }
    }

    /// `history.list`: newest first.
    pub fn history_list(&self, limit: Option<u32>) -> Vec<String> {
        let limit = limit.unwrap_or(DEFAULT_HISTORY_LIMIT).clamp(1, MAX_HISTORY_LIMIT) as usize;
        self.history.lock().unwrap_or_else(|e| e.into_inner()).newest(limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: usize) -> String {
        format!("{n:032x}")
    }

    #[test]
    fn a_stale_rev_conflicts_with_the_stored_draft() {
        let mut drafts = Drafts::default();
        let k = key(1);
        assert_eq!(drafts.set(&k, "Fix the".into(), vec![], 0, None), Ok(1));
        assert_eq!(drafts.set(&k, "Fix the build".into(), vec![], 1, Some(Origin::launcher())), Ok(2));
        let stale = drafts.set(&k, "Fix the tests".into(), vec![], 1, Some(Origin::phone("p", "Pixel")));
        let stored = stale.unwrap_err().unwrap();
        assert_eq!((stored.text.as_str(), stored.rev), ("Fix the build", 2));
        assert_eq!(drafts.set(&key(2), "x".into(), vec![], 4, None), Err(None), "a new key takes rev 0");
        // Sent: emptied, and its rev goes on.
        assert_eq!(drafts.sent(&k, &Origin::launcher()), Some(3));
        assert_eq!(drafts.sent(&k, &Origin::launcher()), None, "nothing more to empty");
        assert_eq!(drafts.get(&k).unwrap().text, "");
        assert!(drafts.set(&k, "again".into(), vec![], 2, None).is_err());
        assert_eq!(drafts.set(&k, "again".into(), vec![], 3, None), Ok(4));
    }

    #[test]
    fn the_oldest_draft_goes_past_the_limit() {
        let mut drafts = Drafts::default();
        for n in 0..=MAX_DRAFTS {
            drafts.set(&key(n), format!("d{n}"), vec![], 0, None).unwrap();
        }
        assert!(drafts.get(&key(0)).is_none(), "the oldest was dropped");
        assert_eq!(drafts.list().len(), MAX_DRAFTS);
        // Setting one again makes it the newest.
        drafts.set(&key(1), "d1 again".into(), vec![], 1, None).unwrap();
        drafts.set(&key(MAX_DRAFTS + 1), "new".into(), vec![], 0, None).unwrap();
        assert!(drafts.get(&key(1)).is_some() && drafts.get(&key(2)).is_none());
        let reloaded = Drafts::load(drafts.list());
        assert_eq!(reloaded.list(), drafts.list());
    }

    #[test]
    fn history_keeps_the_newest_cut_prompts() {
        let mut history = History::default();
        let mut rewrote = 0;
        for n in 0..MAX_HISTORY + 3 {
            rewrote += usize::from(history.push(&format!("p{n}")));
        }
        assert_eq!(rewrote, 3);
        assert_eq!(history.all().len(), MAX_HISTORY);
        assert_eq!(history.newest(2), [format!("p{}", MAX_HISTORY + 2), format!("p{}", MAX_HISTORY + 1)]);
        history.push(&"é".repeat(MAX_HISTORY_PROMPT));
        assert!(history.newest(1)[0].len() <= MAX_HISTORY_PROMPT);
    }

    #[test]
    fn draft_keys_are_threads_or_new_in_a_folder() {
        assert!(is_valid_draft_key(&key(7)));
        assert!(is_valid_draft_key("new:f0123456789ab"));
        assert!(!is_valid_draft_key("new:../x"));
        assert!(!is_valid_draft_key("f0123456789ab"));
    }
}
