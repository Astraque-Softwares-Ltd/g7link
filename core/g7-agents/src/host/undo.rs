// SPDX-License-Identifier: MPL-2.0
//! Turns and what can be undone (§U5, §U6): a thread's turns and their
//! restore points, the files a turn changed and their patches, rewind (its
//! preview, its apply and its undo) and fork.
//!
//! Rewind is two steps: a preview hands out a plan whose token is used
//! once, within [`REWIND_TOKEN_MS`], by the connection that asked. Files are
//! only ever put back for G7 Link on this PC, never for a phone or the
//! launcher.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::api::*;
use crate::host::agents::UserInput;
use crate::host::db::TurnRow;
use crate::host::store::now_ms;
use crate::host::threads::{Entry, Registry, err, random_hex};
use crate::host::{Host, fork, git};

/// Plans waiting to be applied, at most this many at once.
const MAX_PLANS: usize = 64;

/// What a `rewind.preview` promised, until `rewind.apply` takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRewind {
    /// The connection that asked.
    pub client: u64,
    pub thread_id: String,
    pub turn: u32,
    pub conversation: bool,
    /// The files part: what the preview found (apply refuses when the
    /// files changed since).
    pub files: Option<git::RestorePlan>,
    pub expires_ms: u64,
}

/// The plans handed out, by token.
#[derive(Debug, Default)]
pub struct RewindTokens {
    plans: HashMap<String, PendingRewind>,
}

impl RewindTokens {
    /// A new token for `plan`. Expired plans go first; past [`MAX_PLANS`]
    /// the one that expires soonest goes.
    pub fn issue(&mut self, plan: PendingRewind, now_ms: u64) -> String {
        self.plans.retain(|_, p| p.expires_ms > now_ms);
        if self.plans.len() >= MAX_PLANS
            && let Some(oldest) = self.plans.iter().min_by_key(|(_, p)| p.expires_ms).map(|(t, _)| t.clone())
        {
            self.plans.remove(&oldest);
        }
        let mut token = random_hex(16);
        while self.plans.contains_key(&token) {
            token = random_hex(16);
        }
        self.plans.insert(token.clone(), plan);
        token
    }

    /// The plan for `token`, once. Unknown, expired, used, or another
    /// connection's: `NOT_FOUND`, the same for each, and the plan stays
    /// with the connection it belongs to.
    pub fn take(&mut self, token: &str, client: u64, now_ms: u64) -> Result<PendingRewind, RpcError> {
        let gone = || err(ErrorCode::NotFound, "That rewind isn't there any more. Look again.");
        match self.plans.get(token) {
            Some(plan) if plan.client != client => Err(gone()),
            Some(plan) if plan.expires_ms <= now_ms => {
                self.plans.remove(token);
                Err(gone())
            }
            Some(_) => self.plans.remove(token).ok_or_else(gone),
            None => Err(gone()),
        }
    }

    /// A connection closed: its plans go.
    pub fn forget_client(&mut self, client: u64) {
        self.plans.retain(|_, p| p.client != client);
    }

    /// A thread changed under its plans (a turn started, it was deleted).
    pub fn forget_thread(&mut self, thread_id: &str) {
        self.plans.retain(|_, p| p.thread_id != thread_id);
    }
}

/// Files may be put back for this asker: G7 Link's own page on this PC,
/// never a phone (relayed through it) or the launcher.
pub fn may_restore_files(kind: ClientKind, by: Option<&Origin>) -> bool {
    kind == ClientKind::Link && by.is_none_or(|by| by.kind != OriginKind::Phone)
}

/// A `rewind.preview` that may go on: a turn named, at least one part
/// asked for, the files part only for G7 Link on this PC.
pub fn check_preview(p: &RewindPreviewParams, kind: ClientKind) -> Result<(), RpcError> {
    if p.by.as_ref().is_some_and(|by| !by.is_valid()) {
        return Err(err(ErrorCode::BadRequest, "Bad origin."));
    }
    if p.turn == 0 || !(p.conversation || p.files) {
        return Err(err(ErrorCode::BadRequest, "Name a turn, and the conversation, the files or both."));
    }
    if p.files && !may_restore_files(kind, p.by.as_ref()) {
        return Err(err(ErrorCode::Denied, "Files are put back from G7 Link on this PC."));
    }
    Ok(())
}

/// Two folders overlap: one contains, equals or is inside the other.
/// Both are compared as given, so pass canonical paths.
pub fn folders_overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// The shared-folder rule (§U6.4), for G7's own threads: another thread
/// whose folder overlaps `folder` runs now, or did something after
/// `since_ms` (when the target's restore point was made). Its last change
/// counts as "did something", so the rule errs towards refusing.
pub fn shared_by_threads(reg: &Registry, thread_id: &str, folder: &Path, since_ms: u64) -> bool {
    reg.threads.iter().filter(|(id, _)| id.as_str() != thread_id).any(|(_, entry)| {
        entry.cwd.as_deref().is_some_and(|cwd| folders_overlap(cwd, folder))
            && (entry.info.state.is_running() || entry.info.updated_ms > since_ms)
    })
}

/// The same rule for sessions started outside G7 that the host follows:
/// one open now, or written after `since_ms`, in an overlapping folder.
pub fn shared_by_sessions(sessions: &[Session], folder: &Path, since_ms: u64) -> bool {
    sessions.iter().any(|s| folders_overlap(Path::new(&s.cwd), folder) && (s.open || s.updated_ms > since_ms))
}

impl Host {
    /// `thread.turns`: newest first, turns rewound away left out, each with
    /// the host's verdicts for it.
    pub async fn thread_turns(
        &self,
        p: ThreadTurnsParams,
        kind: ClientKind,
    ) -> Result<Vec<TurnInfo>, RpcError> {
        let limit = p.limit.unwrap_or(DEFAULT_TURNS_LIMIT).clamp(1, MAX_TURNS_LIMIT) as usize;
        let (agent, idle) = self.turn_owner(&p.thread_id)?;
        let can = self.agent_info(agent).await.info.can;
        let thread = p.thread_id.clone();
        let rows = self
            .store
            .db(move |db| db.turns(&thread, None, u32::MAX))
            .await
            .map_err(|_| err(ErrorCode::Internal, "Couldn't read the thread's turns."))?;
        let verdicts = Verdicts { agent, can: &can, idle, link: kind == ClientKind::Link };
        Ok(rows
            .iter()
            .filter(|r| p.before_turn.is_none_or(|before| r.turn < before))
            .take(limit)
            .map(|r| verdicts.info(r, &rows))
            .collect())
    }

    /// The thread's agent, and whether it is idle; `NOT_FOUND` for none.
    fn turn_owner(&self, thread_id: &str) -> Result<(AgentId, bool), RpcError> {
        let reg = self.lock_reg();
        let entry = reg.threads.get(thread_id).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
        Ok((entry.info.agent, !entry.info.state.is_running()))
    }

    /// Where `thread_id`'s restore points are, for reading them.
    fn place_of_thread(&self, thread_id: &str) -> Result<git::Place, RpcError> {
        match self.place_for(&self.lock_reg(), thread_id) {
            Placed::At(place) => Ok(place),
            _ => Err(err(ErrorCode::Unavailable, "Changes weren't recorded for this turn.")),
        }
    }

    /// The refs a range's diff runs between: the first turn's `pre`, the
    /// last one's `post`; `UNAVAILABLE` unless every turn in it is ready.
    async fn range_refs(
        &self,
        thread_id: &str,
        range: TurnRange,
    ) -> Result<(String, String, Vec<TurnRow>), RpcError> {
        let thread = thread_id.to_owned();
        let rows = self
            .store
            .db(move |db| db.turns(&thread, None, u32::MAX))
            .await
            .map_err(|_| err(ErrorCode::Internal, "Couldn't read the thread's turns."))?;
        let span: Vec<TurnRow> =
            rows.into_iter().filter(|r| (range.first()..=range.last()).contains(&r.turn)).collect();
        let whole = range.last() - range.first() + 1;
        if span.len() != whole as usize || span.iter().any(|r| r.checkpoint != Checkpoint::Ready) {
            return Err(err(ErrorCode::Unavailable, "Changes weren't recorded for this turn."));
        }
        let refs = git::pre_ref(thread_id, range.first()).zip(git::post_ref(thread_id, range.last()));
        let (from, to) = refs.ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
        Ok((from, to, span))
    }

    /// `turn.files`: one turn's stored list, or a fresh diff for a range.
    pub async fn turn_files(&self, p: TurnFilesParams) -> Result<TurnFilesResult, RpcError> {
        let range =
            p.range().ok_or_else(|| err(ErrorCode::BadRequest, "Name a turn, or the first and last."))?;
        self.turn_owner(&p.thread_id)?;
        let (from, to, span) = self.range_refs(&p.thread_id, range).await?;
        if let TurnRange::One(turn) = range {
            let thread = p.thread_id.clone();
            let files = self
                .store
                .db(move |db| db.turn_files(&thread, turn))
                .await
                .map_err(|_| err(ErrorCode::Internal, "Couldn't read the turn's files."))?;
            let row = &span[0];
            return Ok(TurnFilesResult {
                more: row.files as usize
                    > files.iter().filter(|f| f.status != FileStatus::Unrecorded).count(),
                files,
                adds: row.adds,
                dels: row.dels,
            });
        }
        let place = self.place_of_thread(&p.thread_id)?;
        let stats = git::turn_stats(&place, &from, &to).await.map_err(git_error)?;
        Ok(TurnFilesResult { files: stats.rows, more: stats.more, adds: stats.adds, dels: stats.dels })
    }

    /// `turn.patch`: a page of one listed file's patch. The whole patch is
    /// made each time (bounded by git's own limits) and cut at line ends.
    pub async fn turn_patch(&self, p: TurnPatchParams) -> Result<TurnPatchResult, RpcError> {
        let range =
            p.range().ok_or_else(|| err(ErrorCode::BadRequest, "Name a turn, or the first and last."))?;
        if p.path.is_empty() || p.path.len() > MAX_DIFF_PATH {
            return Err(err(ErrorCode::BadRequest, "Bad path."));
        }
        // The path is looked up in the turn's own list, never taken on trust.
        let files = self
            .turn_files(TurnFilesParams {
                thread_id: p.thread_id.clone(),
                turn: p.turn,
                from_turn: p.from_turn,
                to_turn: p.to_turn,
            })
            .await?;
        if !files.files.iter().any(|f| f.path == p.path) {
            return Err(err(ErrorCode::BadRequest, "That file isn't in these changes."));
        }
        let (from, to, _) = self.range_refs(&p.thread_id, range).await?;
        let place = self.place_of_thread(&p.thread_id)?;
        let patch = git::patch(&place, &from, &to, &p.path).await.map_err(git_error)?;
        if patch.binary || patch.unrecorded {
            return Ok(TurnPatchResult {
                binary: patch.binary,
                truncated: patch.truncated,
                ..Default::default()
            });
        }
        let (page, next) =
            patch.page(p.offset.unwrap_or(0)).ok_or_else(|| err(ErrorCode::BadRequest, "Bad offset."))?;
        Ok(TurnPatchResult { patch: page.to_owned(), next, truncated: patch.truncated, binary: false })
    }

    /// `rewind.preview`: what a rewind to before `turn` would do. Nothing
    /// changes but the snapshot of the files a files part takes.
    pub async fn rewind_preview(
        self: &Arc<Self>,
        p: RewindPreviewParams,
        client: u64,
        kind: ClientKind,
    ) -> Result<RewindPlan, RpcError> {
        check_preview(&p, kind)?;
        let thread = p.thread_id.clone();
        let (agent, idle) = self.turn_owner(&thread)?;
        if !idle {
            return Err(err(ErrorCode::Busy, "Wait until it finishes."));
        }
        let rows = self.live_turns(&thread).await?;
        let target = rows.iter().find(|r| r.turn == p.turn).cloned();
        let target = target.ok_or_else(|| err(ErrorCode::NotFound, "No such turn."))?;
        let can = self.agent_info(agent).await.info.can;
        let mut blocked = None;
        let mut cut = None;
        if p.conversation {
            let copy =
                p.turn == 1 || Verdicts { agent, can: &can, idle, link: false }.copies(&rows, p.turn - 1);
            if copy {
                let (owned, turn) = (thread.clone(), p.turn);
                let range = self.store.db(move |db| db.cut_preview(&owned, turn)).await;
                let range = range.map_err(|_| err(ErrorCode::Internal, "Couldn't read the thread."))?;
                cut = Some(RewindCut { turns: range.turns, messages: range.messages });
            } else {
                blocked = Some(RewindBlock::Agent);
            }
        }
        let mut files = None;
        if p.files {
            match self.plan_files(&thread, &target, &rows).await {
                Ok(plan) => files = Some(plan),
                Err(block) => blocked = Some(block),
            }
        }
        if cut.is_none() && files.is_none() {
            return Err(err(
                ErrorCode::Unavailable,
                block_text(blocked.unwrap_or(RewindBlock::Agent), agent),
            ));
        }
        let now = now_ms();
        let expires_ms = now + REWIND_TOKEN_MS;
        let wire = files.as_ref().map(git::RestorePlan::wire);
        let pending = PendingRewind {
            client,
            thread_id: thread.clone(),
            turn: p.turn,
            conversation: cut.is_some(),
            files,
            expires_ms,
        };
        let token = self.rewinds.lock().unwrap_or_else(|e| e.into_inner()).issue(pending, now);
        Ok(RewindPlan { token, expires_ms, thread_id: thread, turn: p.turn, cut, files: wire, blocked })
    }

    /// The thread's turns that weren't rewound away, newest first.
    async fn live_turns(&self, thread: &str) -> Result<Vec<TurnRow>, RpcError> {
        let owned = thread.to_owned();
        self.store
            .db(move |db| db.turns(&owned, None, u32::MAX))
            .await
            .map_err(|_| err(ErrorCode::Internal, "Couldn't read the thread's turns."))
    }

    /// The files part of a rewind to before `target`: refused while the
    /// folder is shared (§U6.4), without a restore point, or when git says so.
    async fn plan_files(
        self: &Arc<Self>,
        thread: &str,
        target: &TurnRow,
        rows: &[TurnRow],
    ) -> Result<git::RestorePlan, RewindBlock> {
        let (place, folder) = {
            let reg = self.lock_reg();
            let place = match self.place_for(&reg, thread) {
                Placed::At(place) => place,
                Placed::Off => return Err(RewindBlock::Off),
                Placed::Nowhere => return Err(RewindBlock::NotGit),
            };
            if shared_by_threads(&reg, thread, place.folder(), target.started_ms) {
                return Err(RewindBlock::Shared);
            }
            let folder = place.folder().to_owned();
            (place, folder)
        };
        let sessions = self.sessions_list(MAX_SESSIONS).await;
        if shared_by_sessions(&sessions, &folder, target.started_ms) {
            return Err(RewindBlock::Shared);
        }
        match target.checkpoint {
            Checkpoint::Expired => return Err(RewindBlock::Expired),
            Checkpoint::NotGit => return Err(RewindBlock::NotGit),
            _ if target.pre_oid.is_none() => {
                return Err(if target.why == Some(CheckpointWhy::Off) {
                    RewindBlock::Off
                } else {
                    RewindBlock::NoCheckpoint
                });
            }
            _ => {}
        }
        let pre = git::pre_ref(thread, target.turn).ok_or(RewindBlock::NoCheckpoint)?;
        let last_post =
            rows.iter().find(|r| r.post_oid.is_some()).and_then(|r| git::post_ref(thread, r.turn));
        git::plan_restore(&place, thread, &pre, last_post.as_deref(), &target.skipped)
            .await
            .map_err(git::Blocked::block)
    }

    /// `rewind.apply`, for the connection that asked for the plan.
    pub async fn rewind_apply(
        self: &Arc<Self>,
        p: RewindApplyParams,
        client: u64,
        kind: ClientKind,
    ) -> Result<RewindApplyResult, RpcError> {
        if !p.by.is_valid() {
            return Err(err(ErrorCode::BadRequest, "Bad origin."));
        }
        let pending =
            self.rewinds.lock().unwrap_or_else(|e| e.into_inner()).take(&p.token, client, now_ms())?;
        if pending.files.is_some() && !may_restore_files(kind, Some(&p.by)) {
            return Err(err(ErrorCode::Denied, "Files are put back from G7 Link on this PC."));
        }
        let thread = pending.thread_id.clone();
        let (agent, idle) = self.turn_owner(&thread)?;
        if !idle {
            return Err(err(ErrorCode::Busy, "Wait until it finishes."));
        }
        let rows = self.live_turns(&thread).await?;
        let target = rows.iter().find(|r| r.turn == pending.turn).cloned();
        let target = target.ok_or_else(|| err(ErrorCode::NotFound, "No such turn."))?;
        // The prompt goes back to the composer: read before it is cut.
        let prompt = if p.refill { self.item_at(&thread, target.prompt_seq).await } else { None };
        let place = self.place_any(&self.lock_reg(), &thread);
        let mut left = Vec::new();
        let mut restored = 0u32;
        if let (Some(plan), Some(place)) = (&pending.files, &place) {
            left = git::apply_restore(place, plan, p.include_later).await.map_err(|e| match e {
                git::GitError::Changed | git::GitError::Busy => {
                    err(ErrorCode::Conflict, "The files changed. Look again.")
                }
                git::GitError::Gone => err(ErrorCode::Unavailable, "This restore point is no longer kept."),
                _ => err(ErrorCode::Internal, "Git couldn't put the files back."),
            })?;
            let changed = plan.files.iter().filter(|f| p.include_later || !f.later).count();
            restored = u32::try_from(changed.saturating_sub(left.len())).unwrap_or(u32::MAX);
        } else if pending.files.is_some() {
            return Err(err(ErrorCode::Unavailable, "Restore points are off for this folder."));
        }
        let files = pending.files.is_some();
        if pending.conversation
            && let Err(e) = self.cut_conversation(&thread, agent, &rows, pending.turn).await
        {
            // The thread is as it was: its files too.
            if files && let Some(place) = &place {
                self.put_back(place, &thread).await;
            }
            return Err(e);
        }
        let until_ms = now_ms() + UNDO_MS;
        let thread_info = {
            let mut reg = self.lock_reg();
            let entry =
                reg.threads.get_mut(&thread).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
            entry.info.undo = Some(UndoInfo { until_ms, files });
            let (kind, text) = match (pending.conversation, p.refill) {
                (false, _) => (
                    NoticeKind::FilesRestored,
                    format!(
                        "Restored the files to before this message. {} still remembers what it did.",
                        agent.display_name()
                    ),
                ),
                (true, true) => (NoticeKind::EditFromHere, "Rewound to here".to_owned()),
                (true, false) if files => {
                    (NoticeKind::Rewound, format!("Rewound to here. Restored {restored} files."))
                }
                (true, false) => (NoticeKind::Rewound, "Rewound to here".to_owned()),
            };
            let notice = crate::host::threads::Entry::marked_notice(entry, &text, kind);
            crate::host::threads::emit_state(self, entry);
            crate::host::threads::emit_item(self, &thread, &notice);
            entry.info.clone()
        };
        if let Some(item) = prompt {
            self.draft_refill(&thread, item.text.unwrap_or_default(), item.images, Some(p.by.clone()));
        }
        let left = left
            .into_iter()
            .take(MAX_REWIND_LEFT)
            .map(|l| RewindLeft { path: l.path, why: cut(l.why.text(), MAX_LEFT_WHY).to_owned() })
            .collect();
        Ok(RewindApplyResult { thread: thread_info, left })
    }

    /// One stored item, before a cut hides it.
    async fn item_at(&self, thread: &str, seq: u64) -> Option<Item> {
        let owned = thread.to_owned();
        let items = self.store.db(move |db| db.read_items(&owned, Some(seq + 1), 1)).await.ok()?;
        items.into_iter().find(|i| i.seq == seq && i.kind == ItemKind::User)
    }

    /// Puts the files back as they were before the last rewind changed them.
    async fn put_back(&self, place: &git::Place, thread: &str) -> bool {
        let Some(undo) = git::undo_ref(thread) else { return false };
        match git::plan_restore(place, thread, &undo, None, &[]).await {
            Ok(plan) => git::apply_restore(place, &plan, true).await.is_ok(),
            Err(_) => false,
        }
    }

    /// Cuts turns `from_turn`… from the conversation: the agent continues
    /// from a copy of its session ending before them (§U6.2, §U6.3 step 3-4).
    async fn cut_conversation(
        self: &Arc<Self>,
        thread: &str,
        agent: AgentId,
        rows: &[TurnRow],
        from_turn: u32,
    ) -> Result<(), RpcError> {
        // What the copy is made from, and how.
        let through = if from_turn == 1 {
            None
        } else {
            Some(fork::through(agent, rows, from_turn - 1).ok_or_else(|| {
                err(
                    ErrorCode::Unavailable,
                    format!("{} can only be forked from its latest message.", agent.display_name()),
                )
            })?)
        };
        let (native, pending, prior) = match &through {
            // Before the first turn: the next message starts a new session.
            None => {
                let prior = self.lock_reg().threads.get(thread).and_then(|e| e.info.native_id.clone());
                (String::new(), None, prior)
            }
            Some(through) => {
                let (job, pending_from, prior) = {
                    let reg = self.lock_reg();
                    let entry =
                        reg.threads.get(thread).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
                    let pending_from = entry.pending_fork.as_deref().and_then(fork::LaunchHint::decode);
                    let job = entry
                        .info
                        .native_id
                        .is_some()
                        .then(|| fork::fork_native(self, entry, through.mark()));
                    (job, pending_from, entry.info.native_id.clone())
                };
                match (job, pending_from) {
                    (Some(job), _) => match job.await? {
                        fork::Forked::Now(id) => (id, None, prior),
                        // Claude: the copy names itself when it first runs.
                        fork::Forked::AtNextStart(hint) => {
                            (prior.clone().unwrap_or_default(), Some(hint.encode()), prior)
                        }
                    },
                    // A fork not yet run: a new hint from the same source.
                    (None, Some(from)) => {
                        let hint = fork::LaunchHint::new(agent, &from.from, through.mark())
                            .ok_or_else(|| err(ErrorCode::Unavailable, "There is nothing to rewind yet."))?;
                        (String::new(), Some(hint.encode()), None)
                    }
                    (None, None) => {
                        return Err(err(ErrorCode::Unavailable, "There is nothing to rewind yet."));
                    }
                }
            }
        };
        // One undoable rewind at a time: an older one is made final first.
        let (owned, keep) = (thread.to_owned(), native.clone());
        let cut_range = self
            .store
            .db(move |db| match db.cut(&owned, from_turn, &keep) {
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let refs = db.purge_cut(&owned)?;
                    db.cut(&owned, from_turn, &keep).map(|range| (range, refs))
                }
                other => other.map(|range| (range, Vec::new())),
            })
            .await
            .map_err(|_| err(ErrorCode::Internal, "Couldn't rewind the thread."))?;
        let (range, purged) = cut_range;
        let place = self.place_any(&self.lock_reg(), thread);
        if let Some(place) = place {
            tokio::spawn(async move {
                for prefix in purged {
                    let _ = git::delete_refs(&place, &prefix).await;
                }
            });
        }
        let mut reg = self.lock_reg();
        let entry = reg.threads.get_mut(thread).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
        entry.kept.prior_native_id = prior;
        entry.info.native_id = (!native.is_empty()).then_some(native);
        entry.pending_fork = pending;
        entry.info.turns = from_turn - 1;
        entry.row = None;
        self.hub.broadcast(&Event::ThreadCut(ThreadCutEvent {
            thread_id: thread.to_owned(),
            from_seq: range.from_seq,
            to_seq: range.to_seq,
            restored: false,
        }));
        Ok(())
    }

    /// `rewind.undo`: the last rewind is taken back while it can be.
    pub async fn rewind_undo(
        self: &Arc<Self>,
        p: RewindUndoParams,
        kind: ClientKind,
    ) -> Result<ThreadInfo, RpcError> {
        if !p.by.is_valid() {
            return Err(err(ErrorCode::BadRequest, "Bad origin."));
        }
        let thread = p.thread_id.clone();
        let (undo, idle, place) = {
            let reg = self.lock_reg();
            let entry =
                reg.threads.get(&thread).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
            (entry.info.undo, !entry.info.state.is_running(), self.place_any(&reg, &thread))
        };
        let undo = undo.ok_or_else(|| err(ErrorCode::NotFound, "There is nothing to undo."))?;
        if !idle {
            return Err(err(ErrorCode::Busy, "Wait until it finishes."));
        }
        if undo.files {
            if !may_restore_files(kind, Some(&p.by)) {
                return Err(err(ErrorCode::Denied, "Files are put back from G7 Link on this PC."));
            }
            let place =
                place.ok_or_else(|| err(ErrorCode::Unavailable, "This restore point is no longer kept."))?;
            if !self.put_back(&place, &thread).await {
                return Err(err(ErrorCode::Conflict, "The files couldn't be put back. Look again."));
            }
        }
        // A files-only rewind cut nothing: there is nothing to bring back.
        let owned = thread.clone();
        let range = self
            .store
            .db(move |db| db.uncut(&owned))
            .await
            .ok()
            .filter(|r| r.to_seq >= r.from_seq && r.to_seq > 0);
        let mut reg = self.lock_reg();
        let entry =
            reg.threads.get_mut(&thread).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
        if range.is_some() {
            entry.info.native_id = entry.kept.prior_native_id.take().or(entry.info.native_id.take());
            entry.pending_fork = None;
        }
        entry.info.undo = None;
        crate::host::threads::emit_state(self, entry);
        let info = entry.info.clone();
        drop(reg);
        if let Some(range) = range {
            self.hub.broadcast(&Event::ThreadCut(ThreadCutEvent {
                thread_id: thread,
                from_seq: range.from_seq,
                to_seq: range.to_seq,
                restored: true,
            }));
        }
        Ok(info)
    }

    /// An undo that has run out (24 h) or that a new turn ended: the cut
    /// turns and their restore points go for good.
    pub(crate) fn end_undo(&self, entry: &mut Entry) {
        if entry.info.undo.take().is_none() {
            return;
        }
        entry.kept.prior_native_id = None;
        let thread = entry.info.id.clone();
        let store = self.store.clone();
        let place = self.place_for_entry(entry);
        tokio::spawn(async move {
            let owned = thread.clone();
            if let Ok(refs) = store.db(move |db| db.purge_cut(&owned)).await
                && let Some(place) = place
            {
                for prefix in refs {
                    let _ = git::delete_refs(&place, &prefix).await;
                }
            }
        });
    }

    /// `entry`'s place, from what it holds (its folder must still be listed).
    fn place_for_entry(&self, entry: &Entry) -> Option<git::Place> {
        let checkpoints = git::checkpoints_dir(&self.config.data_dir);
        git::Place::new(self.git.clone()?, &checkpoints, &entry.info.folder_id, entry.cwd.as_deref()?)
    }

    /// Undos past their time end, in every thread (the 5 s tick).
    pub(crate) fn expire_undos(&self) {
        let now = now_ms();
        let mut reg = self.lock_reg();
        for entry in reg.threads.values_mut().filter(|e| e.info.undo.is_some_and(|u| u.until_ms <= now)) {
            self.end_undo(entry);
            crate::host::threads::emit_state(self, entry);
        }
    }

    /// `thread.fork`: a new thread whose conversation is a copy of this one
    /// through `turn` (the latest when absent). Nothing runs now: the copy
    /// is made when the new thread's first message is sent (reconcile §5).
    pub async fn thread_fork(self: &Arc<Self>, p: ThreadForkParams) -> Result<ThreadInfo, RpcError> {
        if !p.by.is_valid() || p.turn == Some(0) {
            return Err(err(ErrorCode::BadRequest, "Bad fork."));
        }
        let thread = p.thread_id.clone();
        let (agent, idle) = self.turn_owner(&thread)?;
        if !idle {
            return Err(err(ErrorCode::Busy, "Wait until it finishes."));
        }
        let rows = self.live_turns(&thread).await?;
        let last = p
            .turn
            .or_else(|| rows.first().map(|r| r.turn))
            .ok_or_else(|| err(ErrorCode::Unavailable, "There is nothing to fork yet."))?;
        if !rows.iter().any(|r| r.turn == last) {
            return Err(err(ErrorCode::NotFound, "No such turn."));
        }
        let can = self.agent_info(agent).await.info.can;
        let through = fork::through(agent, &rows, last).filter(|t| match t {
            fork::Through::Latest => can.contains(&Can::Fork),
            fork::Through::Mark(_) => can.contains(&Can::ForkAt),
        });
        let through = through.ok_or_else(|| {
            err(
                ErrorCode::Unavailable,
                format!("{} can only be forked from its latest message.", agent.display_name()),
            )
        })?;
        let (new, hint, worktree_dir) = {
            let reg = self.lock_reg();
            if reg.threads.len() >= MAX_THREADS_STORED {
                return Err(err(ErrorCode::Limit, "G7 keeps up to 5,000 threads. Delete some first."));
            }
            let entry =
                reg.threads.get(&thread).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
            if entry.cwd.is_none() || entry.info.readonly.is_some() {
                return Err(err(ErrorCode::Denied, "Its folder isn't one of your agent folders."));
            }
            let hint = match (
                fork::launch_hint(entry, through.mark()),
                entry.pending_fork.as_deref().and_then(fork::LaunchHint::decode),
            ) {
                (Ok(hint), _) => hint,
                (Err(_), Some(from)) => fork::LaunchHint::new(agent, &from.from, through.mark())
                    .ok_or_else(|| err(ErrorCode::Unavailable, "There is nothing to fork yet."))?,
                (Err(e), None) => return Err(e),
            };
            let mut id = random_hex(16);
            while reg.threads.contains_key(&id) {
                id = random_hex(16);
            }
            let now = now_ms();
            let parent = &entry.info;
            let mode = if p.by.kind == OriginKind::Phone && !parent.mode.phone_may() {
                Mode::Supervised
            } else {
                parent.mode
            };
            let new = ThreadInfo {
                id,
                state: State::Done,
                mode,
                origin: p.by.clone(),
                created_ms: now,
                updated_ms: now,
                state_since_ms: now,
                last_seq: 0,
                pending: 0,
                summary: None,
                error: None,
                native_id: None,
                step: None,
                plan: None,
                continued: false,
                answered_model: None,
                queued: Vec::new(),
                queue_held: false,
                context: None,
                asking: 0,
                pinned: false,
                archived: false,
                unread: false,
                snoozed_until_ms: None,
                turns: last,
                forked_from: Some(Box::new(ForkedFrom {
                    thread_id: thread.clone(),
                    turn: p.turn,
                    title: parent.title.clone(),
                })),
                found: false,
                readonly: None,
                undo: None,
                wrote: None,
                ..parent.clone()
            };
            // A worktree thread's fork works in the same worktree.
            (new, hint, entry.worktree_dir.clone())
        };
        let (from, info) = (thread.clone(), new.clone());
        let loaded = self
            .store
            .db(move |db| {
                db.copy_thread(&from, last, &info)?;
                db.load_thread(&info.id)
            })
            .await
            .map_err(|_| err(ErrorCode::Internal, "Couldn't copy the thread."))?
            .ok_or_else(|| err(ErrorCode::Internal, "Couldn't copy the thread."))?;
        let folder_label = new.folder_label.clone();
        let info = {
            let mut reg = self.lock_reg();
            let folders = reg.folders.clone();
            let mut entry = crate::host::threads::Registry::entry_from(loaded, &folders);
            if entry.info.worktree.is_some() {
                entry.worktree_dir = worktree_dir;
                entry.cwd = reg.work_dir(&entry);
            }
            entry.pending_fork = Some(hint.encode());
            entry.info.forked_from = new.forked_from.clone();
            // After the copied items, so it shows under them (§U6.5).
            let notice = crate::host::threads::Entry::marked_notice(
                &mut entry,
                &format!("Forked from here. The files in {folder_label} are as they are now."),
                NoticeKind::Forked,
            );
            crate::host::threads::emit_state(self, &mut entry);
            crate::host::threads::emit_item(self, &entry.info.id.clone(), &notice);
            let info = entry.info.clone();
            reg.threads.insert(info.id.clone(), entry);
            info
        };
        if p.by.kind == OriginKind::Phone {
            crate::host::threads::notify(
                self,
                crate::host::notify::Note::PhoneStarted { thread: info.clone(), continued: false },
            );
        }
        Ok(info)
    }
}

// ---- Turns and their restore points (§U5.2, §U5.3) ----

/// What a client is told it may do with each turn.
pub(crate) struct Verdicts<'a> {
    pub agent: AgentId,
    /// The agent's abilities.
    pub can: &'a [Can],
    /// The thread neither works nor waits.
    pub idle: bool,
    /// The asker is G7 Link on this PC (files may be put back for it).
    pub link: bool,
}

impl Verdicts<'_> {
    /// A copy of the session through turn `last` can be made.
    pub fn copies(&self, rows: &[TurnRow], last: u32) -> bool {
        match fork::through(self.agent, rows, last) {
            Some(fork::Through::Latest) => self.can.contains(&Can::Fork),
            Some(fork::Through::Mark(_)) => self.can.contains(&Can::ForkAt),
            None => false,
        }
    }

    /// `row` as a client sees it; `rows` are the thread's turns not rewound away.
    pub fn info(&self, row: &TurnRow, rows: &[TurnRow]) -> TurnInfo {
        let copy = |last: u32| self.copies(rows, last);
        TurnInfo {
            can_fork: self.idle && copy(row.turn),
            can_rewind: self.idle && (row.turn == 1 || copy(row.turn - 1)),
            can_restore: self.idle
                && self.link
                && row.checkpoint == Checkpoint::Ready
                && row.pre_oid.is_some(),
            ..row.info()
        }
    }
}

/// Where a thread's restore points would be taken.
pub(crate) enum Placed {
    At(git::Place),
    /// The folder's switch is off.
    Off,
    /// No git, no folder, or not a place git can use.
    Nowhere,
}

/// A restore point to take before a turn's message goes to the agent.
pub(crate) struct PreCapture {
    place: git::Place,
    refname: String,
    lock: Arc<tokio::sync::Mutex<()>>,
    turn: u32,
}

/// Why a rewind can't be done, in the words of §U9.4.
fn block_text(block: RewindBlock, agent: AgentId) -> String {
    match block {
        RewindBlock::Shared => "Another thread worked in this folder since then.".to_owned(),
        RewindBlock::Busy => {
            "Git is in the middle of something in this folder. Finish it, then try again.".to_owned()
        }
        RewindBlock::NoCheckpoint => "No restore point for this turn.".to_owned(),
        RewindBlock::NotGit => "This isn't a git repository, so G7 can't undo file changes here.".to_owned(),
        RewindBlock::Expired => "This restore point is no longer kept.".to_owned(),
        RewindBlock::Off => "Restore points are off for this folder.".to_owned(),
        RewindBlock::Agent => format!("{} can only be forked from its latest message.", agent.display_name()),
    }
}

/// A git failure as a client hears it.
fn git_error(e: git::GitError) -> RpcError {
    match e {
        git::GitError::Busy => err(ErrorCode::Busy, "Git is busy in this folder."),
        git::GitError::Gone | git::GitError::NotFound => {
            err(ErrorCode::Unavailable, "This restore point is no longer kept.")
        }
        git::GitError::Changed => err(ErrorCode::Conflict, "The files changed. Look again."),
        _ => err(ErrorCode::Unavailable, "Git couldn't read the changes."),
    }
}

impl Host {
    /// Where `thread_id`'s restore points go: G7's repository for its
    /// folder, unless git is missing or the folder's switch is off.
    pub(crate) fn place_for(&self, reg: &Registry, thread_id: &str) -> Placed {
        let off = reg
            .threads
            .get(thread_id)
            .and_then(|e| reg.folders.iter().find(|f| f.id == e.info.folder_id))
            .is_some_and(|f| !f.checkpoints);
        if off {
            return Placed::Off;
        }
        self.place_any(reg, thread_id).map_or(Placed::Nowhere, Placed::At)
    }

    /// Where `thread_id`'s restore points are, whatever its folder's switch
    /// says now (to read or delete the ones already taken).
    pub(crate) fn place_any(&self, reg: &Registry, thread_id: &str) -> Option<git::Place> {
        let entry = reg.threads.get(thread_id)?;
        let folder = reg.folders.iter().find(|f| f.id == entry.info.folder_id)?;
        git::Place::new(
            self.git.clone()?,
            &git::checkpoints_dir(&self.config.data_dir),
            &folder.id,
            entry.cwd.as_deref()?,
        )
    }

    /// A folder's own place (its whole repository).
    pub(crate) fn folder_place(&self, folder: &Folder) -> Option<git::Place> {
        let checkpoints = git::checkpoints_dir(&self.config.data_dir);
        git::Place::new(self.git.clone()?, &checkpoints, &folder.id, Path::new(&folder.path))
    }

    /// Turns that were running when the host stopped: each closes as
    /// stopped, with a second restore point taken now (§U5.3).
    pub(crate) fn recover_turns(self: &Arc<Self>, threads: Vec<String>) {
        if threads.is_empty() {
            return;
        }
        let host = self.clone();
        tokio::spawn(async move {
            for thread in threads {
                let owned = thread.clone();
                let Ok(rows) = host.store.db(move |db| db.turns(&owned, None, 1)).await else { continue };
                let Some(mut row) = rows.into_iter().next().filter(|r| r.ended.is_none()) else { continue };
                row.ended = Some(TurnEnd::Stopped);
                row.ended_ms = Some(now_ms());
                let placed = {
                    let reg = host.lock_reg();
                    row.last_seq = reg.threads.get(&thread).map(|e| e.info.last_seq);
                    host.place_any(&reg, &thread).map_or(Placed::Nowhere, Placed::At)
                };
                let row = host.settle_turn(&thread, row, placed).await;
                if let Some(entry) = host.lock_reg().threads.get_mut(&thread) {
                    entry.row = Some(row);
                }
            }
        });
    }

    /// Refs of threads the database no longer has, in every folder's
    /// repository, are deleted (§U5.6): at start.
    pub(crate) fn sweep_orphan_refs(self: &Arc<Self>) {
        let (places, known): (Vec<git::Place>, std::collections::HashSet<String>) = {
            let reg = self.lock_reg();
            (
                reg.folders.iter().filter_map(|f| self.folder_place(f)).collect(),
                reg.threads.keys().cloned().collect(),
            )
        };
        tokio::spawn(async move {
            for place in places {
                if !place.repo().is_dir() {
                    continue;
                }
                let Ok(ids) = git::thread_ids(&place).await else { continue };
                for id in ids.iter().filter(|id| !known.contains(*id)) {
                    if let Some(prefix) = git::thread_refs(id) {
                        let _ = git::delete_refs(&place, &prefix).await;
                    }
                }
            }
        });
    }

    /// The row of the turn `entry` just began. A turn that starts with a
    /// person's message (`placed` given) gets a restore point first: what
    /// it needs is returned, and the message must wait for it. Other turns
    /// (one the agent started for a steer, a denial's words) get none.
    pub(crate) fn open_turn(
        &self,
        entry: &mut Entry,
        prompt_seq: u64,
        placed: Option<Placed>,
    ) -> Option<PreCapture> {
        // A new turn ends what the last rewind could undo.
        if entry.info.undo.is_some() {
            self.end_undo(entry);
            crate::host::threads::emit_state(self, entry);
        }
        let thread = entry.info.id.clone();
        let mut row = TurnRow::new(entry.turn_no(), prompt_seq, now_ms());
        let pre = match placed {
            None => {
                row.checkpoint = Checkpoint::Missing;
                None
            }
            Some(Placed::Off) => {
                row.checkpoint = Checkpoint::Missing;
                row.why = Some(CheckpointWhy::Off);
                None
            }
            Some(Placed::Nowhere) => {
                row.checkpoint = Checkpoint::NotGit;
                None
            }
            Some(Placed::At(place)) => git::pre_ref(&thread, row.turn).map(|refname| PreCapture {
                place,
                refname,
                lock: entry.captures.clone(),
                turn: row.turn,
            }),
        };
        if pre.is_none() && row.checkpoint == Checkpoint::Pending {
            row.checkpoint = Checkpoint::NotGit;
        }
        self.save_turn(&thread, &row, None);
        self.hub.broadcast(&Event::ThreadTurn(ThreadTurnEvent { thread_id: thread, turn: row.info() }));
        entry.row = Some(row);
        pre
    }

    fn save_turn(&self, thread_id: &str, row: &TurnRow, files: Option<Vec<TurnFile>>) {
        let (thread, row) = (thread_id.to_owned(), row.clone());
        self.store.db_later("save a turn", move |db| db.save_turn(&thread, &row, files.as_deref()));
    }

    /// Takes `pre`'s restore point (at most [`git::PRE_BUDGET`], after the
    /// last turn's own has settled), then gives the agent `input`, unless the
    /// turn ended meanwhile.
    pub(crate) fn deliver_after(
        self: &Arc<Self>,
        thread_id: &str,
        pre: PreCapture,
        input: UserInput,
        bin: Option<std::path::PathBuf>,
    ) {
        let host = self.clone();
        let thread = thread_id.to_owned();
        tokio::spawn(async move {
            let guard = pre.lock.clone().lock_owned().await;
            let captured = git::capture(&pre.place, &pre.refname, git::PRE_BUDGET).await;
            let mut reg = host.lock_reg();
            let Some(entry) = reg.threads.get_mut(&thread) else { return };
            if let Some(row) = entry.row.as_mut().filter(|r| r.turn == pre.turn) {
                match captured {
                    git::Capture::Ready { oid, head, skipped, .. } => {
                        row.pre_oid = Some(oid);
                        row.pre_head = Some(head);
                        row.skipped = skipped;
                        row.skipped.truncate(crate::host::db::MAX_SKIPPED);
                    }
                    git::Capture::Missing(why) => {
                        row.checkpoint = Checkpoint::Missing;
                        row.why = Some(why);
                    }
                    git::Capture::NotGit => row.checkpoint = Checkpoint::NotGit,
                }
                let row = row.clone();
                host.save_turn(&thread, &row, None);
                if row.checkpoint != Checkpoint::Pending {
                    let event = ThreadTurnEvent { thread_id: thread.clone(), turn: row.info() };
                    host.hub.broadcast(&Event::ThreadTurn(event));
                }
            }
            drop(guard);
            if entry.turn_no() == pre.turn && entry.info.state.is_running() {
                host.deliver_input(&mut reg, &thread, input, bin.as_deref());
            }
        });
    }

    /// The agent's mark of the running turn (`Step::TurnMark`): kept once.
    pub(crate) fn turn_marked(&self, entry: &mut Entry, mark: String) {
        let Some(row) = entry.row.as_mut().filter(|r| r.native_mark.is_none()) else { return };
        if fork::is_mark(&mark) {
            row.native_mark = Some(mark);
            let row = row.clone();
            self.save_turn(&entry.info.id, &row, None);
        }
    }

    /// The running turn ended with `outcome`: its row closes, and its
    /// second restore point and what changed follow in the background
    /// (at most [`git::POST_BUDGET`]).
    pub(crate) fn close_turn(self: &Arc<Self>, reg: &Registry, thread_id: &str, outcome: Outcome) {
        let placed = self.place_for(reg, thread_id);
        let Some(entry) = reg.threads.get(thread_id) else { return };
        let Some(mut row) = entry.row.clone().filter(|r| r.ended.is_none()) else { return };
        row.ended = Some(match outcome {
            Outcome::Done => TurnEnd::Done,
            Outcome::Failed => TurnEnd::Failed,
            Outcome::Interrupted => TurnEnd::Stopped,
        });
        row.ended_ms = Some(now_ms());
        row.last_seq = Some(entry.info.last_seq);
        let (lock, agent) = (entry.captures.clone(), entry.info.agent);
        self.save_turn(thread_id, &row, None);
        self.hub.broadcast(&Event::ThreadTurn(ThreadTurnEvent {
            thread_id: thread_id.to_owned(),
            turn: row.info(),
        }));
        let host = self.clone();
        let thread = thread_id.to_owned();
        tokio::spawn(async move {
            // After this turn's first restore point, whenever that settles.
            let _guard = lock.lock_owned().await;
            let settled = {
                let mut reg = host.lock_reg();
                let current =
                    reg.threads.get_mut(&thread).and_then(|e| e.row.as_mut()).filter(|r| r.turn == row.turn);
                if let Some(current) = current {
                    current.ended = row.ended;
                    current.ended_ms = row.ended_ms;
                    current.last_seq = row.last_seq;
                    row = current.clone();
                }
                row.clone()
            };
            let row = host.settle_turn(&thread, settled, placed).await;
            if let Some(current) = host
                .lock_reg()
                .threads
                .get_mut(&thread)
                .and_then(|e| e.row.as_mut())
                .filter(|r| r.turn == row.turn)
            {
                *current = row.clone();
            }
            host.announce_turns(&thread, agent, &[row.turn, row.turn.saturating_sub(1)]).await;
        });
    }

    /// Takes a closed turn's second restore point and what changed.
    async fn settle_turn(self: &Arc<Self>, thread: &str, mut row: TurnRow, placed: Placed) -> TurnRow {
        let refs = git::pre_ref(thread, row.turn).zip(git::post_ref(thread, row.turn));
        let (Placed::At(place), Some((pre, post)), Some(_)) = (placed, refs, row.pre_oid.clone()) else {
            if row.checkpoint == Checkpoint::Pending {
                row.checkpoint = Checkpoint::Missing;
            }
            self.save_turn(thread, &row, None);
            return row;
        };
        let mut files = None;
        match git::capture(&place, &post, git::POST_BUDGET).await {
            git::Capture::Ready { oid, head, skipped, .. } => {
                row.post_oid = Some(oid);
                row.post_head = Some(head);
                for path in skipped {
                    if row.skipped.len() < crate::host::db::MAX_SKIPPED && !row.skipped.contains(&path) {
                        row.skipped.push(path);
                    }
                }
                match git::turn_stats(&place, &pre, &post).await {
                    Ok(stats) => {
                        row.checkpoint = Checkpoint::Ready;
                        row.why = None;
                        (row.files, row.adds, row.dels) = (stats.files, stats.adds, stats.dels);
                        files = Some(stats.rows);
                    }
                    Err(_) => {
                        row.checkpoint = Checkpoint::Missing;
                        row.why = Some(CheckpointWhy::Refused);
                    }
                }
            }
            git::Capture::Missing(why) => {
                row.checkpoint = Checkpoint::Missing;
                row.why = Some(why);
            }
            git::Capture::NotGit => row.checkpoint = Checkpoint::NotGit,
        }
        self.save_turn(thread, &row, files);
        // Only the newest restore points are kept.
        let owned = thread.to_owned();
        if let Ok(refs) = self.store.db(move |db| db.expire_turns(&owned, KEPT_RESTORE_POINTS)).await {
            for prefix in refs {
                let _ = git::delete_refs(&place, &prefix).await;
            }
        }
        row
    }

    /// Sends `thread.turn` for `turns` as they are now, with the verdicts
    /// an idle thread has (each client still hides them while it works).
    pub(crate) async fn announce_turns(&self, thread: &str, agent: AgentId, turns: &[u32]) {
        let owned = thread.to_owned();
        let Ok(rows) = self.store.db(move |db| db.turns(&owned, None, u32::MAX)).await else { return };
        let can = self.agent_info(agent).await.info.can;
        let idle = self.lock_reg().threads.get(thread).is_some_and(|e| !e.info.state.is_running());
        let verdicts = Verdicts { agent, can: &can, idle, link: true };
        for row in rows.iter().filter(|r| turns.contains(&r.turn)) {
            let mut turn = verdicts.info(row, &rows);
            // `can_restore` is per client: an event says what the PC may do.
            turn.can_restore = turn.can_restore && row.pre_oid.is_some();
            self.hub.broadcast(&Event::ThreadTurn(ThreadTurnEvent { thread_id: thread.to_owned(), turn }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(client: u64, expires_ms: u64) -> PendingRewind {
        PendingRewind {
            client,
            thread_id: "a".repeat(32),
            turn: 2,
            conversation: true,
            files: None,
            expires_ms,
        }
    }

    #[test]
    fn a_plan_is_taken_once_by_its_own_connection_in_time() {
        let mut tokens = RewindTokens::default();
        let token = tokens.issue(plan(1, 1_000), 0);
        assert!(is_valid_thread_id(&token), "32 hex digits");
        assert_eq!(tokens.take(&token, 2, 10).unwrap_err().code, ErrorCode::NotFound, "another connection");
        assert_eq!(tokens.take(&token, 1, 10).unwrap(), plan(1, 1_000), "it stayed with its own");
        assert_eq!(tokens.take(&token, 1, 10).unwrap_err().code, ErrorCode::NotFound, "used once");
        let late = tokens.issue(plan(1, 1_000), 0);
        assert_eq!(tokens.take(&late, 1, 1_000).unwrap_err().code, ErrorCode::NotFound, "expired");
        let closed = tokens.issue(plan(3, 1_000), 0);
        tokens.forget_client(3);
        assert!(tokens.take(&closed, 3, 10).is_err(), "its connection closed");
        let changed = tokens.issue(plan(1, 1_000), 0);
        tokens.forget_thread(&"a".repeat(32));
        assert!(tokens.take(&changed, 1, 10).is_err(), "its thread changed");
    }

    #[test]
    fn plans_are_bounded() {
        let mut tokens = RewindTokens::default();
        let first = tokens.issue(plan(1, 500), 0);
        for n in 0..MAX_PLANS as u64 {
            tokens.issue(plan(1, 1_000 + n), 0);
        }
        assert_eq!(tokens.plans.len(), MAX_PLANS);
        assert!(tokens.take(&first, 1, 10).is_err(), "the soonest to expire went");
    }

    #[test]
    fn only_g7_link_on_this_pc_puts_files_back() {
        let ask = |files: bool, by: Option<Origin>| RewindPreviewParams {
            thread_id: "a".repeat(32),
            turn: 1,
            conversation: !files,
            files,
            by,
        };
        assert!(check_preview(&ask(true, None), ClientKind::Link).is_ok());
        assert!(check_preview(&ask(true, Some(Origin::launcher())), ClientKind::Link).is_ok());
        let phone = Some(Origin::phone("p", "Pixel"));
        assert_eq!(
            check_preview(&ask(true, phone.clone()), ClientKind::Link).unwrap_err().code,
            ErrorCode::Denied
        );
        assert!(
            check_preview(&ask(false, phone), ClientKind::Link).is_ok(),
            "the conversation, from a phone"
        );
        assert_eq!(
            check_preview(&ask(true, None), ClientKind::Launcher).unwrap_err().code,
            ErrorCode::Denied
        );
        assert!(check_preview(&ask(false, None), ClientKind::Launcher).is_ok());
        let nothing = RewindPreviewParams { conversation: false, ..ask(false, None) };
        assert_eq!(check_preview(&nothing, ClientKind::Link).unwrap_err().code, ErrorCode::BadRequest);
        let no_turn = RewindPreviewParams { turn: 0, ..ask(false, None) };
        assert_eq!(check_preview(&no_turn, ClientKind::Link).unwrap_err().code, ErrorCode::BadRequest);
    }

    #[test]
    fn folders_overlap_when_one_holds_the_other() {
        let at = Path::new;
        assert!(folders_overlap(at("/home/me/app"), at("/home/me/app")));
        assert!(folders_overlap(at("/home/me/app/src"), at("/home/me/app")));
        assert!(folders_overlap(at("/home/me/app"), at("/home/me/app/src")));
        assert!(!folders_overlap(at("/home/me/app"), at("/home/me/application")), "by component, not text");
        assert!(!folders_overlap(at("/home/me/app"), at("/home/me/web")));
    }

    #[test]
    fn a_session_shares_the_folder_while_open_or_once_written_since() {
        let session = |cwd: &str, open: bool, updated_ms: u64| Session {
            id: format!("sc{}", "ab".repeat(16)),
            agent: AgentId::Claude,
            title: "t".into(),
            cwd: cwd.into(),
            folder_id: None,
            folder_label: "app".into(),
            state: SessionState::Idle,
            state_guess: false,
            state_since_ms: 0,
            waiting_for: None,
            source: SessionSource::Terminal,
            open,
            updated_ms,
            last_seq: 0,
            step: None,
            summary: None,
            can_continue: false,
            blocked: None,
            limit: None,
        };
        let folder = Path::new("/home/me/app");
        assert!(shared_by_sessions(&[session("/home/me/app/sub", true, 0)], folder, 100));
        assert!(shared_by_sessions(&[session("/home/me", false, 101)], folder, 100));
        assert!(!shared_by_sessions(&[session("/home/me/app", false, 100)], folder, 100), "before the point");
        assert!(!shared_by_sessions(&[session("/home/me/web", true, 900)], folder, 100));
    }

    #[tokio::test]
    async fn another_thread_in_the_folder_shares_it() {
        let dir = tempfile::tempdir().unwrap();
        let host = crate::host::test_host(dir.path(), APPROVAL_LIFETIME_MS);
        // A test thread works in /tmp.
        let (busy, _, _) = host.test_thread(AgentId::Claude);
        let reg = host.lock_reg();
        let other = "f".repeat(32);
        assert!(shared_by_threads(&reg, &other, Path::new("/tmp/app"), u64::MAX), "it runs now");
        assert!(!shared_by_threads(&reg, &busy, Path::new("/tmp"), u64::MAX), "not itself");
        assert!(!shared_by_threads(&reg, &other, Path::new("/elsewhere"), 0));
    }
}
