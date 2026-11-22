// SPDX-License-Identifier: MPL-2.0
//! Project scripts in the host (phase 6, §X3): a folder's own commands, typed
//! in G7 Link and kept in `folders.json` (never read from the folder itself),
//! run when a person asks from a thread. One run of a script per place at a
//! time, four running in a thread and eight in the host; each keeps its
//! newest output in memory and nothing is stored.
//!
//! Who may ask is checked here on every request (§X3.4), the folder's
//! "Phones can run these" switch read each time. Nothing starts a run but a
//! person's `script.run`: no timer, turn, agent, queued message or bridge.
//! The process itself is `spawn`'s.

pub mod spawn;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::api::*;
use crate::host::Host;
use crate::host::notify::Note;
use crate::host::scm::{Lines, Stop, clean};
use crate::host::source::DONE_ON_LINK;
use crate::host::store::now_ms;
use crate::host::threads::{emit_state, err, notify, random_hex};
use spawn::Ended;

/// The methods that are this module's.
pub const METHODS: [&str; 4] =
    [method::SCRIPT_RUN, method::SCRIPT_STOP, method::SCRIPT_RUNS, method::SCRIPT_OUTPUT];

/// Refusal words (`data.reason`).
pub const NO_SCRIPT: &str = "no_script";
pub const SCRIPT_RUNNING: &str = "script_running";
pub const RUNS: &str = "runs";
pub const FOLDER_MISSING: &str = "folder_missing";
pub const PHONE_SCRIPTS_OFF: &str = "phone_scripts_off";

/// Lines in one second past which the rest of that second is only counted.
pub const FLOOD_LINES: u32 = 5_000;
const FLOOD_WINDOW: Duration = Duration::from_secs(1);
const PROGRESS_EVERY: Duration = Duration::from_millis(1000 / JOB_EVENTS_PER_SECOND as u64);
const KEPT: Duration = Duration::from_millis(RUN_KEPT_MS);
const STOP_GRACE: Duration = Duration::from_millis(RUN_STOP_GRACE_MS);
/// At shutdown every run's `KILL` is sent by then, and the runs are waited
/// for until [`SHUTDOWN_WAIT`].
const SHUTDOWN_KILL: Duration = Duration::from_secs(4);
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

const NO_RUN: &str = "That run isn't kept any more.";
const PHONE_OFF: &str = "This folder's scripts can't be run from a phone.";
const READ_ONLY: &str = "This thread can be read, not continued.";
const SPAWN_FAILED: &str = "The shell couldn't be started.";
const SIGNALLED: &str = "A signal ended it.";

// ---- A folder's list (§X3.1) ----

/// Why `folders.set`'s scripts were refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bad {
    Name,
    Command,
    Duplicate,
    Many,
}

impl Bad {
    /// Its `data.reason`.
    pub fn word(self) -> &'static str {
        match self {
            Bad::Name => "name",
            Bad::Command => "command",
            Bad::Duplicate => "duplicate",
            Bad::Many => "many",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Bad::Name => "Give it a name of up to 40 bytes.",
            Bad::Command => "Type the command on one line, up to 1024 bytes.",
            Bad::Duplicate => "A script with that name exists.",
            Bad::Many => "Up to 12 scripts.",
        }
    }

    pub fn refusal(self) -> RpcError {
        refusal(ErrorCode::BadRequest, self.word(), self.message())
    }
}

/// A name as kept: 1 to [`MAX_SCRIPT_NAME`] bytes, trimmed, no control or
/// direction character.
pub fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SCRIPT_NAME
        && name.trim() == name
        && !name.chars().any(|c| c.is_control() || is_stripped_char(c))
}

/// A command as kept: 1 to [`MAX_SCRIPT_COMMAND`] bytes, trimmed, one line.
/// Nothing else is looked at: it is the user's own shell line.
pub fn command_ok(command: &str) -> bool {
    !command.is_empty()
        && command.len() <= MAX_SCRIPT_COMMAND
        && command.trim() == command
        && !command.contains(['\n', '\r', '\0'])
}

/// A stored folder's scripts as the host keeps them: each in bounds, each id
/// and name once, at most [`MAX_SCRIPTS`].
pub fn loaded(scripts: Vec<Script>) -> Vec<Script> {
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    scripts
        .into_iter()
        .filter(|s| is_valid_script_id(&s.id) && name_ok(&s.name) && command_ok(&s.command))
        .filter(|s| ids.insert(s.id.clone()) && names.insert(s.name.to_lowercase()))
        .take(MAX_SCRIPTS)
        .collect()
}

/// The list `folders.set` asks for, in its order: names and commands
/// trimmed; an entry whose `id` is one of `old`'s keeps it, any other gets a
/// new one.
pub fn wanted(wanted: Vec<ScriptWanted>, old: &[Script]) -> Result<Vec<Script>, Bad> {
    if wanted.len() > MAX_SCRIPTS {
        return Err(Bad::Many);
    }
    let mut names = HashSet::new();
    let mut taken: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(wanted.len());
    let mut fresh = Vec::new();
    for want in wanted {
        let name = want.name.trim();
        if !name_ok(name) {
            return Err(Bad::Name);
        }
        let command = want.command.trim();
        if !command_ok(command) {
            return Err(Bad::Command);
        }
        if !names.insert(name.to_lowercase()) {
            return Err(Bad::Duplicate);
        }
        let id = want.id.filter(|id| old.iter().any(|s| s.id == *id) && !taken.contains(id));
        if let Some(id) = &id {
            taken.insert(id.clone());
        } else {
            fresh.push(out.len());
        }
        out.push(Script { id: id.unwrap_or_default(), name: name.to_owned(), command: command.to_owned() });
    }
    for at in fresh {
        let id = loop {
            let id = format!("s{}", random_hex(SCRIPT_ID_HEX / 2));
            if !taken.contains(&id) && !old.iter().any(|s| s.id == id) {
                break id;
            }
        };
        taken.insert(id.clone());
        out[at].id = id;
    }
    Ok(out)
}

// ---- Who may ask (§X3.4) ----

/// A request's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Want {
    Run,
    Stop,
    /// `script.runs` and `script.output`.
    Read,
}

/// Why a request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// "That is done from G7 Link on this PC."
    OnLink,
    /// A phone, and the folder's "Phones can run these" is off.
    PhoneOff,
}

/// Whether a connection of `kind`, asking for a phone (`phone`) or not, may
/// do `want`. `switch`: the folder's "Phones can run these" now;
/// `started_here`: the run is one this phone started. A phone reaches the
/// host only through G7 Link.
pub fn may(
    kind: ClientKind,
    phone: bool,
    want: Want,
    switch: bool,
    started_here: bool,
) -> Result<(), Refused> {
    if matches!(kind, ClientKind::Bridge | ClientKind::Cli) || (phone && kind != ClientKind::Link) {
        return Err(Refused::OnLink);
    }
    if kind == ClientKind::Test && want != Want::Read {
        return Err(Refused::OnLink);
    }
    match want {
        _ if !phone => Ok(()),
        Want::Read => Ok(()),
        Want::Run if switch => Ok(()),
        Want::Stop if switch || started_here => Ok(()),
        Want::Run | Want::Stop => Err(Refused::PhoneOff),
    }
}

fn refused(why: Refused) -> RpcError {
    match why {
        Refused::OnLink => err(ErrorCode::Denied, DONE_ON_LINK),
        Refused::PhoneOff => refusal(ErrorCode::Denied, PHONE_SCRIPTS_OFF, PHONE_OFF),
    }
}

fn refusal(code: ErrorCode, reason: &str, message: &str) -> RpcError {
    RpcError { data: Some(json!({ "reason": reason })), ..RpcError::new(code, message) }
}

/// `by`, checked: whether it is a phone's. A phone's names its device.
fn phone_of(by: Option<&Origin>) -> Result<bool, RpcError> {
    match by {
        Some(b) if !b.is_valid() => Err(err(ErrorCode::BadRequest, "Bad origin.")),
        Some(b) if b.kind == OriginKind::Phone && b.device_id.as_deref().is_none_or(str::is_empty) => {
            Err(err(ErrorCode::BadRequest, "Bad origin."))
        }
        Some(b) => Ok(b.kind == OriginKind::Phone),
        None => Ok(false),
    }
}

// ---- Runs ----

struct Slot {
    /// Without its lines: those are in `lines`.
    run: ScriptRun,
    /// Its output; lines dropped in a flood are counted there too.
    lines: Lines,
    stop: Stop,
    /// Where it runs: one run of a script each.
    cwd: PathBuf,
    ended: Option<Instant>,
}

impl Slot {
    fn running(&self) -> bool {
        self.run.phase == RunPhase::Running
    }

    fn kept_at(&self, now: Instant) -> bool {
        self.ended.is_none_or(|at| now.saturating_duration_since(at) < KEPT)
    }

    fn dropped(&self) -> u32 {
        self.lines.dropped()
    }

    /// The run with its kept lines numbered `from` and later.
    fn snapshot(&self, from: u64) -> ScriptRun {
        let (lines, next) = self.lines.since(from);
        let first = next.saturating_sub(lines.len() as u64);
        ScriptRun { lines, from: first, dropped: self.dropped(), ..self.run.clone() }
    }

    /// [`Slot::snapshot`] with only its newest lines within an event's bounds.
    fn event(&self, from: u64) -> ScriptRun {
        let (mut lines, next) = self.lines.since(from);
        let (mut count, mut bytes) = (0usize, 0usize);
        let keep = lines
            .iter()
            .rev()
            .take_while(|line| {
                count += 1;
                bytes += line.len();
                count <= MAX_RUN_EVENT_LINES && bytes <= MAX_RUN_EVENT_BYTES
            })
            .count();
        lines.drain(..lines.len() - keep);
        let first = next.saturating_sub(lines.len() as u64);
        ScriptRun { lines, from: first, dropped: self.dropped(), ..self.run.clone() }
    }

    /// The run without its lines.
    fn bare(&self) -> ScriptRun {
        ScriptRun { lines: Vec::new(), from: self.lines.next(), dropped: self.dropped(), ..self.run.clone() }
    }

    fn end(&mut self, phase: RunPhase, exit: Option<i32>, error: Option<RunError>) {
        self.ended = Some(Instant::now());
        self.run.ended_ms = Some(now_ms());
        self.run.phase = phase;
        self.run.exit = exit;
        self.run.error = error;
    }
}

/// The host's runs.
#[derive(Default)]
pub struct Scripts {
    runs: Mutex<HashMap<String, Slot>>,
    /// Set at shutdown: every stop's `KILL` is sent by then.
    hurry: Arc<Mutex<Option<Instant>>>,
}

impl std::fmt::Debug for Scripts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scripts").field("runs", &self.lock().len()).finish_non_exhaustive()
    }
}

impl Scripts {
    /// Taken after the registry's lock, never before it.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Slot>> {
        self.runs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs running now.
    pub fn running(&self) -> usize {
        self.lock().values().filter(|s| s.running()).count()
    }

    /// `ThreadInfo.runs` of a thread: running ones, then ended ones still
    /// kept, newest first in each; at most [`MAX_RUN_BRIEFS`].
    pub(crate) fn briefs(&self, thread_id: &str) -> Vec<RunBrief> {
        let now = Instant::now();
        let runs = self.lock();
        let mut mine: Vec<&Slot> =
            runs.values().filter(|s| s.run.thread_id == thread_id && s.kept_at(now)).collect();
        mine.sort_by(|a, b| {
            b.running()
                .cmp(&a.running())
                .then(b.run.started_ms.cmp(&a.run.started_ms))
                .then_with(|| b.run.id.cmp(&a.run.id))
        });
        mine.into_iter().take(MAX_RUN_BRIEFS).map(|s| s.run.brief()).collect()
    }

    /// Stops the running runs `which` picks. Whether there was one.
    fn stop_where(&self, which: impl Fn(&ScriptRun) -> bool) -> bool {
        let mut any = false;
        for slot in self.lock().values().filter(|s| s.running() && which(&s.run)) {
            slot.stop.stop();
            any = true;
        }
        any
    }

    /// A thread is deleted: its runs stop.
    pub(crate) fn stop_thread(&self, thread_id: &str) -> bool {
        self.stop_where(|run| run.thread_id == thread_id)
    }

    /// A folder is removed: its runs stop.
    pub(crate) fn stop_folder(&self, folder_id: &str) -> bool {
        self.stop_where(|run| run.folder_id == folder_id)
    }

    /// Ended runs past [`RUN_KEPT_MS`] at `now`, and the oldest past
    /// [`MAX_RUNS_KEPT`], are forgotten. The threads whose runs changed.
    fn sweep(runs: &mut HashMap<String, Slot>, now: Instant) -> HashSet<String> {
        let mut changed = HashSet::new();
        runs.retain(|_, slot| {
            let kept = slot.kept_at(now);
            if !kept {
                changed.insert(slot.run.thread_id.clone());
            }
            kept
        });
        let mut ended: Vec<(Instant, String)> =
            runs.iter().filter_map(|(id, s)| Some((s.ended?, id.clone()))).collect();
        if ended.len() > MAX_RUNS_KEPT {
            ended.sort();
            let excess = ended.len() - MAX_RUNS_KEPT;
            for (_, id) in ended.into_iter().take(excess) {
                if let Some(slot) = runs.remove(&id) {
                    changed.insert(slot.run.thread_id);
                }
            }
        }
        changed
    }

    /// How long a stop waits before `KILL`: [`RUN_STOP_GRACE_MS`], less at shutdown.
    fn grace(hurry: &Mutex<Option<Instant>>) -> Duration {
        match *hurry.lock().unwrap_or_else(PoisonError::into_inner) {
            Some(by) => STOP_GRACE.min(by.saturating_duration_since(Instant::now())),
            None => STOP_GRACE,
        }
    }
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params).map_err(|e| err(ErrorCode::BadRequest, e.to_string()))
}

fn to_value<T: Serialize>(value: &T) -> Result<Value, RpcError> {
    serde_json::to_value(value).map_err(|e| err(ErrorCode::Internal, e.to_string()))
}

fn no_run() -> RpcError {
    err(ErrorCode::NotFound, NO_RUN)
}

impl Host {
    /// One of [`METHODS`], for a connection of `kind`: who it is is checked first.
    pub(crate) async fn script_call(
        self: &Arc<Self>,
        kind: ClientKind,
        method_name: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        match method_name {
            method::SCRIPT_RUN => {
                may(kind, false, Want::Run, true, true).map_err(refused)?;
                let p: ScriptRunParams = parse(params)?;
                to_value(&ScriptRunResult { run: self.script_run(kind, p).await? })
            }
            method::SCRIPT_STOP => {
                may(kind, false, Want::Stop, true, true).map_err(refused)?;
                let p: ScriptStopParams = parse(params)?;
                to_value(&ScriptRunResult { run: self.script_stop(kind, &p)? })
            }
            method::SCRIPT_RUNS => {
                may(kind, false, Want::Read, true, true).map_err(refused)?;
                let p: ScriptRunsParams = parse(params)?;
                to_value(&ScriptRunsResult { runs: self.script_runs(&p.thread_id)? })
            }
            method::SCRIPT_OUTPUT => {
                may(kind, false, Want::Read, true, true).map_err(refused)?;
                let p: ScriptOutputParams = parse(params)?;
                to_value(&ScriptRunResult { run: self.script_output(&p)? })
            }
            _ => Err(err(ErrorCode::BadRequest, UNKNOWN_METHOD)),
        }
    }

    /// `script.run`: a person's request only. The run as it starts, or
    /// `failed` when its shell couldn't be started.
    async fn script_run(
        self: &Arc<Self>,
        kind: ClientKind,
        p: ScriptRunParams,
    ) -> Result<ScriptRun, RpcError> {
        let phone = phone_of(p.by.as_ref())?;
        may(kind, phone, Want::Run, true, false).map_err(refused)?;
        if !is_valid_thread_id(&p.thread_id) || !is_valid_script_id(&p.script_id) {
            return Err(err(ErrorCode::BadRequest, "Bad id."));
        }
        let by = p.by.clone().unwrap_or_else(Origin::launcher);
        let (id, command, cwd, stop, lines, thread) = {
            let reg = self.lock_reg();
            let entry =
                reg.threads.get(&p.thread_id).ok_or_else(|| err(ErrorCode::NotFound, "No such thread."))?;
            if entry.info.readonly.is_some() {
                return Err(err(ErrorCode::Denied, READ_ONLY));
            }
            let missing = || refusal(ErrorCode::Unavailable, FOLDER_MISSING, "This folder is missing.");
            let folder = reg.folders.iter().find(|f| f.id == entry.info.folder_id).ok_or_else(missing)?;
            may(kind, phone, Want::Run, folder.phone_scripts, false).map_err(refused)?;
            let script = folder.scripts.iter().find(|s| s.id == p.script_id).ok_or_else(|| {
                refusal(ErrorCode::NotFound, NO_SCRIPT, "That script isn't in this folder's list any more.")
            })?;
            let cwd = if entry.info.worktree.is_some() {
                entry.worktree_dir.clone()
            } else {
                Some(PathBuf::from(&folder.path))
            };
            let cwd = cwd.filter(|dir| dir.is_dir()).ok_or_else(missing)?;
            let mut runs = self.scripts.lock();
            if let Some(same) =
                runs.values().find(|s| s.running() && s.run.script_id == script.id && s.cwd == cwd)
            {
                let mut busy = refusal(
                    ErrorCode::Busy,
                    SCRIPT_RUNNING,
                    &format!("\u{201c}{}\u{201d} is already running here.", script.name),
                );
                busy.data = Some(json!({ "reason": SCRIPT_RUNNING, "run": same.run.id }));
                return Err(busy);
            }
            let here = runs.values().filter(|s| s.running() && s.run.thread_id == p.thread_id).count();
            let all = runs.values().filter(|s| s.running()).count();
            if here >= MAX_RUNS_THREAD || all >= MAX_RUNS_HOST {
                return Err(refusal(ErrorCode::Limit, RUNS, "Too many scripts are running. Stop one first."));
            }
            let mut id = random_hex(RUN_ID_LEN / 2);
            while runs.contains_key(&id) {
                id = random_hex(RUN_ID_LEN / 2);
            }
            let (stop, lines) = (Stop::new(), Lines::with_caps(MAX_RUN_LINES, MAX_RUN_BYTES));
            let run = ScriptRun {
                id: id.clone(),
                thread_id: p.thread_id.clone(),
                folder_id: folder.id.clone(),
                script_id: script.id.clone(),
                name: script.name.clone(),
                command: script.command.clone(),
                phase: RunPhase::Running,
                exit: None,
                started_ms: now_ms(),
                ended_ms: None,
                by: by.clone(),
                lines: Vec::new(),
                from: 0,
                dropped: 0,
                error: None,
            };
            let slot = Slot { run, lines: lines.clone(), stop: stop.clone(), cwd: cwd.clone(), ended: None };
            runs.insert(id.clone(), slot);
            (id, script.command.clone(), cwd, stop, lines, entry.info.clone())
        };
        let shell = spawn::shell(&self.config.env);
        let started = spawn::start(&shell, &command, &cwd, &self.config.env).await;
        let running = match started {
            Ok(running) => {
                if let Some(pid) = running.pid() {
                    self.ports_run_started(&thread.id, &id, pid);
                }
                Some(running)
            }
            Err(e) => {
                self.run_not_started(&id, &e);
                None
            }
        };
        // Its start is told before its task can tell its end.
        let run = self.scripts.lock().get(&id).map(|s| s.event(0)).ok_or_else(no_run)?;
        self.runs_changed(&thread.id);
        self.hub.broadcast(&Event::ScriptProgress(ScriptProgressEvent { run: run.clone() }));
        if let Some(running) = running {
            self.watch_run(id.clone(), running, stop, lines);
        }
        if phone && let Some(device) = by.device_name.clone().or(by.device_id.clone()) {
            notify(self, Note::PhoneScript { thread, device, script: run.name.clone() });
        }
        Ok(ScriptRun { lines: Vec::new(), ..run })
    }

    /// The run's task: its output kept, cleaned, as it arrives; its new
    /// lines sent at most [`JOB_EVENTS_PER_SECOND`] a second; its end.
    fn watch_run(self: &Arc<Self>, id: String, running: spawn::Running, stop: Stop, lines: Lines) {
        let host = Arc::downgrade(self);
        let hurry = self.scripts.hurry.clone();
        let home = crate::host::env::lookup(&self.config.env, "HOME")
            .and_then(|h| h.to_str())
            .filter(|h| h.starts_with('/'))
            .map(str::to_owned);
        tokio::spawn(async move {
            let kept = lines.clone();
            let mut window = (Instant::now(), 0u32);
            let sink = move |raw: Vec<u8>| {
                let now = Instant::now();
                if now.saturating_duration_since(window.0) >= FLOOD_WINDOW {
                    window = (now, 0);
                }
                window.1 = window.1.saturating_add(1);
                if window.1 > FLOOD_LINES {
                    kept.count_dropped(1);
                    return;
                }
                kept.push(&clean::line(&raw, home.as_deref()));
            };
            let finish = running.finish(&stop, || Scripts::grace(&hurry), sink);
            let mut finish = std::pin::pin!(finish);
            let mut tick = tokio::time::interval(PROGRESS_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            let mut sent = 0u64;
            let ended = loop {
                tokio::select! {
                    ended = &mut finish => break ended,
                    _ = tick.tick() => {
                        if lines.next() > sent && let Some(host) = host.upgrade() {
                            sent = host.run_progress(&id, sent);
                        }
                    }
                }
            };
            if let Some(host) = host.upgrade() {
                host.end_run(&id, ended, sent);
            }
        });
    }

    /// Sends the run's lines numbered `from` and later; the number after them.
    fn run_progress(&self, id: &str, from: u64) -> u64 {
        let (run, next) = {
            let runs = self.scripts.lock();
            let Some(slot) = runs.get(id) else { return from };
            (slot.event(from), slot.lines.next())
        };
        if !run.lines.is_empty() {
            self.hub.broadcast(&Event::ScriptProgress(ScriptProgressEvent { run }));
        }
        next
    }

    fn end_run(&self, id: &str, ended: Ended, sent: u64) {
        let (run, swept) = {
            let mut runs = self.scripts.lock();
            let Some(slot) = runs.get_mut(id) else { return };
            match ended {
                Ended::Exit(0) => slot.end(RunPhase::Done, Some(0), None),
                Ended::Exit(code) => slot.end(RunPhase::Failed, Some(code), None),
                Ended::Signal => slot.end(
                    RunPhase::Failed,
                    None,
                    Some(RunError { reason: "signal".into(), message: SIGNALLED.into() }),
                ),
                Ended::Stopped => slot.end(RunPhase::Stopped, None, None),
            }
            let run = slot.event(sent);
            (run, Scripts::sweep(&mut runs, Instant::now()))
        };
        self.ports_run_ended(&run.thread_id, id);
        self.runs_changed(&run.thread_id);
        for thread in swept.iter().filter(|t| **t != run.thread_id) {
            self.runs_changed(thread);
        }
        // Last: a client that hears the end finds the thread already says so.
        self.hub.broadcast(&Event::ScriptProgress(ScriptProgressEvent { run }));
    }

    /// The shell couldn't be started: the run is still kept, `failed`, with
    /// one line saying so.
    fn run_not_started(&self, id: &str, e: &std::io::Error) {
        let mut runs = self.scripts.lock();
        let Some(slot) = runs.get_mut(id) else { return };
        slot.lines.push(&clean_cut(&format!("{SPAWN_FAILED} {e}"), MAX_ERROR_MESSAGE).0);
        slot.end(
            RunPhase::Failed,
            None,
            Some(RunError { reason: "spawn".into(), message: SPAWN_FAILED.into() }),
        );
    }

    /// `script.stop`: by a PC client, or by the phone that started it, or
    /// any phone while its folder's switch is on. A run that ended already is
    /// answered as it is.
    fn script_stop(&self, kind: ClientKind, p: &ScriptStopParams) -> Result<ScriptRun, RpcError> {
        let phone = phone_of(p.by.as_ref())?;
        may(kind, phone, Want::Stop, true, true).map_err(refused)?;
        if !is_valid_run_id(&p.run) {
            return Err(no_run());
        }
        let reg = self.lock_reg();
        let runs = self.scripts.lock();
        let slot = runs.get(&p.run).filter(|s| s.kept_at(Instant::now())).ok_or_else(no_run)?;
        let switch = reg.folders.iter().any(|f| f.id == slot.run.folder_id && f.phone_scripts);
        let started_here = phone
            && slot.run.by.kind == OriginKind::Phone
            && slot.run.by.device_id.is_some()
            && slot.run.by.device_id == p.by.as_ref().and_then(|b| b.device_id.clone());
        may(kind, phone, Want::Stop, switch, started_here).map_err(refused)?;
        slot.stop.stop();
        Ok(slot.bare())
    }

    /// `script.runs`: a thread's runs still kept, newest first, without lines.
    fn script_runs(&self, thread_id: &str) -> Result<Vec<ScriptRun>, RpcError> {
        let reg = self.lock_reg();
        if !is_valid_thread_id(thread_id) || !reg.threads.contains_key(thread_id) {
            return Err(err(ErrorCode::NotFound, "No such thread."));
        }
        let now = Instant::now();
        let runs = self.scripts.lock();
        let mut list: Vec<ScriptRun> = runs
            .values()
            .filter(|s| s.run.thread_id == thread_id && s.kept_at(now))
            .map(Slot::bare)
            .collect();
        list.sort_by(|a, b| b.started_ms.cmp(&a.started_ms).then_with(|| b.id.cmp(&a.id)));
        Ok(list)
    }

    /// `script.output`: the run with its kept lines numbered `from` and later.
    fn script_output(&self, p: &ScriptOutputParams) -> Result<ScriptRun, RpcError> {
        if !is_valid_run_id(&p.run) {
            return Err(no_run());
        }
        let runs = self.scripts.lock();
        let slot = runs.get(&p.run).filter(|s| s.kept_at(Instant::now())).ok_or_else(no_run)?;
        Ok(slot.snapshot(p.from.unwrap_or(0)))
    }

    /// The thread's `runs` brought up to date: `thread.state` when they changed.
    pub(crate) fn runs_changed(&self, thread_id: &str) {
        let mut reg = self.lock_reg();
        let Some(entry) = reg.threads.get_mut(thread_id) else { return };
        let briefs = self.scripts.briefs(thread_id);
        if entry.info.runs != briefs {
            entry.info.runs = briefs;
            emit_state(self, entry);
        }
    }

    /// Every few seconds: ended runs past their time are forgotten.
    pub(crate) fn scripts_tick(&self) {
        let swept = Scripts::sweep(&mut self.scripts.lock(), Instant::now());
        for thread in swept {
            self.runs_changed(&thread);
        }
    }

    /// At shutdown, before the threads: every run is stopped, `KILL`ed
    /// within four seconds, and waited for up to five in all.
    pub(crate) async fn stop_runs(&self) {
        *self.scripts.hurry.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(Instant::now() + SHUTDOWN_KILL);
        self.scripts.stop_where(|_| true);
        let until = Instant::now() + SHUTDOWN_WAIT;
        while self.scripts.running() > 0 && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kind, phone, want, switch, started here: allowed.
    type Row = (ClientKind, bool, Want, bool, bool, Result<(), Refused>);

    #[test]
    fn who_may_ask_is_the_table() {
        use ClientKind::*;
        use Want::*;
        let cases: &[Row] = &[
            (Link, false, Run, false, false, Ok(())),
            (Link, false, Stop, false, false, Ok(())),
            (Link, false, Read, false, false, Ok(())),
            (Launcher, false, Run, false, false, Ok(())),
            (Launcher, false, Stop, false, false, Ok(())),
            (Launcher, false, Read, false, false, Ok(())),
            (Link, true, Run, true, false, Ok(())),
            (Link, true, Run, false, false, Err(Refused::PhoneOff)),
            (Link, true, Stop, false, true, Ok(())),
            (Link, true, Stop, true, false, Ok(())),
            (Link, true, Stop, false, false, Err(Refused::PhoneOff)),
            (Link, true, Read, false, false, Ok(())),
            (Launcher, true, Run, true, false, Err(Refused::OnLink)),
            (Launcher, true, Read, true, false, Err(Refused::OnLink)),
            (Test, false, Read, false, false, Ok(())),
            (Test, false, Run, true, false, Err(Refused::OnLink)),
            (Test, false, Stop, true, true, Err(Refused::OnLink)),
            (Test, true, Read, true, true, Err(Refused::OnLink)),
        ];
        for (kind, phone, want, switch, here, allowed) in cases {
            assert_eq!(
                may(*kind, *phone, *want, *switch, *here),
                *allowed,
                "{kind:?} {phone} {want:?} {switch}"
            );
        }
        for kind in [Bridge, Cli] {
            for want in [Run, Stop, Read] {
                for phone in [false, true] {
                    assert_eq!(may(kind, phone, want, true, true), Err(Refused::OnLink), "{kind:?} {want:?}");
                }
            }
        }
    }

    fn wanted_one(name: &str, command: &str) -> ScriptWanted {
        ScriptWanted { id: None, name: name.into(), command: command.into() }
    }

    #[test]
    fn a_folders_list_keeps_its_bounds() {
        let list = wanted(vec![wanted_one("  dev ", " npm run dev  "), wanted_one("Test", "make test")], &[])
            .unwrap();
        assert_eq!((list[0].name.as_str(), list[0].command.as_str()), ("dev", "npm run dev"));
        assert!(list.iter().all(|s| is_valid_script_id(&s.id)));
        assert_ne!(list[0].id, list[1].id);
        // A known id keeps it, in the new order; an unknown one gets a new one.
        let again = wanted(
            vec![
                ScriptWanted { id: Some(list[1].id.clone()), ..wanted_one("test", "make check") },
                ScriptWanted { id: Some("s00000000".into()), ..wanted_one("lint", "make lint") },
            ],
            &list,
        )
        .unwrap();
        assert_eq!(again[0].id, list[1].id);
        assert!(is_valid_script_id(&again[1].id) && again[1].id != "s00000000");
        // The same id twice: the second gets a new one.
        let twice = wanted(
            vec![
                ScriptWanted { id: Some(list[0].id.clone()), ..wanted_one("a", "x") },
                ScriptWanted { id: Some(list[0].id.clone()), ..wanted_one("b", "y") },
            ],
            &list,
        )
        .unwrap();
        assert_ne!(twice[0].id, twice[1].id);

        let bad = |w: Vec<ScriptWanted>| wanted(w, &[]).unwrap_err();
        assert_eq!(bad(vec![wanted_one("  ", "x")]), Bad::Name);
        assert_eq!(bad(vec![wanted_one(&"n".repeat(MAX_SCRIPT_NAME + 1), "x")]), Bad::Name);
        assert_eq!(bad(vec![wanted_one("a\u{7}b", "x")]), Bad::Name);
        assert_eq!(bad(vec![wanted_one("a\u{202e}b", "x")]), Bad::Name);
        assert_eq!(bad(vec![wanted_one("a", "")]), Bad::Command);
        assert_eq!(bad(vec![wanted_one("a", "make\nrm -rf x")]), Bad::Command);
        assert_eq!(bad(vec![wanted_one("a", "a\rb")]), Bad::Command);
        assert_eq!(bad(vec![wanted_one("a", "a\0b")]), Bad::Command);
        assert_eq!(bad(vec![wanted_one("a", &"c".repeat(MAX_SCRIPT_COMMAND + 1))]), Bad::Command);
        assert_eq!(bad(vec![wanted_one("Dev", "x"), wanted_one("dev", "y")]), Bad::Duplicate);
        assert_eq!(bad((0..=MAX_SCRIPTS).map(|n| wanted_one(&format!("s{n}"), "x")).collect()), Bad::Many);
        assert_eq!(
            wanted((0..MAX_SCRIPTS).map(|n| wanted_one(&format!("s{n}"), "x")).collect(), &[]).unwrap().len(),
            12
        );
        assert!(command_ok(&"c".repeat(MAX_SCRIPT_COMMAND)) && name_ok(&"n".repeat(MAX_SCRIPT_NAME)));
        assert!(command_ok("FOO='a b' \"$(pwd)\" && -x --y | z\ttab"), "the user's own line");
        assert!(wanted(Vec::new(), &list).unwrap().is_empty());
        for (bad, word) in [
            (Bad::Name, "name"),
            (Bad::Command, "command"),
            (Bad::Duplicate, "duplicate"),
            (Bad::Many, "many"),
        ] {
            let e = bad.refusal();
            assert_eq!((e.code, e.data), (ErrorCode::BadRequest, Some(json!({ "reason": word }))));
        }
    }

    #[test]
    fn a_stored_list_drops_what_is_out_of_bounds() {
        let ok = |id: &str, name: &str| Script { id: id.into(), name: name.into(), command: "make".into() };
        let mut stored = vec![
            ok("s00000001", "dev"),
            ok("bad", "x"),
            ok("s00000002", " padded"),
            ok("s00000003", &"n".repeat(MAX_SCRIPT_NAME + 1)),
            Script { command: "a\nb".into(), ..ok("s00000004", "two lines") },
            Script { command: String::new(), ..ok("s00000005", "empty") },
            ok("s00000001", "same id"),
            ok("s00000006", "DEV"),
        ];
        stored.extend((10..30).map(|n| ok(&format!("s000000{n}"), &format!("n{n}"))));
        let kept = loaded(stored);
        assert_eq!(kept.len(), MAX_SCRIPTS);
        assert_eq!(kept[0].name, "dev");
        assert_eq!(kept[1].name, "n10", "everything between was out of bounds");
    }

    fn slot(thread: &str, ended: Option<Instant>, started_ms: u64) -> Slot {
        Slot {
            run: ScriptRun {
                id: random_hex(RUN_ID_LEN / 2),
                thread_id: thread.into(),
                folder_id: "f000000000001".into(),
                script_id: "s00000001".into(),
                name: "dev".into(),
                command: "x".into(),
                phase: if ended.is_some() { RunPhase::Done } else { RunPhase::Running },
                exit: None,
                started_ms,
                ended_ms: None,
                by: Origin::launcher(),
                lines: Vec::new(),
                from: 0,
                dropped: 0,
                error: None,
            },
            lines: Lines::with_caps(MAX_RUN_LINES, MAX_RUN_BYTES),
            stop: Stop::new(),
            cwd: PathBuf::from("/tmp"),
            ended,
        }
    }

    #[test]
    fn ended_runs_go_after_thirty_minutes_and_past_sixteen() {
        let t0 = Instant::now();
        let mut runs = HashMap::new();
        let running = slot("a", None, 1);
        runs.insert(running.run.id.clone(), running);
        for n in 0..20u32 {
            let s = slot(
                if n < 10 { "b" } else { "c" },
                Some(t0 + Duration::from_secs(u64::from(n))),
                u64::from(n),
            );
            runs.insert(s.run.id.clone(), s);
        }
        let changed = Scripts::sweep(&mut runs, t0 + Duration::from_secs(30));
        assert_eq!(runs.len(), 1 + MAX_RUNS_KEPT, "the four oldest ended go");
        assert_eq!(changed, HashSet::from(["b".to_owned()]));
        let changed = Scripts::sweep(&mut runs, t0 + KEPT + Duration::from_secs(14));
        assert_eq!(runs.len(), 1 + 5, "ended at 15 to 19 s are still kept: {}", runs.len());
        assert!(changed.contains("b") && changed.contains("c"));
        Scripts::sweep(&mut runs, t0 + KEPT + Duration::from_secs(3600));
        assert_eq!(runs.len(), 1, "a running run is never swept");
    }

    #[test]
    fn a_threads_runs_list_running_first_then_newest() {
        let scripts = Scripts::default();
        {
            let mut runs = scripts.lock();
            for (thread, ended, started) in
                [("a", Some(Instant::now()), 5), ("a", None, 1), ("a", None, 2), ("b", None, 9)]
            {
                let s = slot(thread, ended, started);
                runs.insert(s.run.id.clone(), s);
            }
            for n in 0..10 {
                let s = slot("a", Some(Instant::now()), 10 + n);
                runs.insert(s.run.id.clone(), s);
            }
        }
        let briefs = scripts.briefs("a");
        assert_eq!(briefs.len(), MAX_RUN_BRIEFS);
        assert_eq!(briefs[0].phase, RunPhase::Running);
        assert_eq!(briefs[1].phase, RunPhase::Running);
        assert!(briefs[2..].iter().all(|b| b.phase == RunPhase::Done));
        assert!(scripts.stop_thread("a") && !scripts.stop_thread("z"));
        assert!(
            scripts
                .lock()
                .values()
                .filter(|s| s.run.thread_id == "a" && s.running())
                .all(|s| s.stop.is_stopped())
        );
        assert!(scripts.lock().values().filter(|s| s.run.thread_id == "b").all(|s| !s.stop.is_stopped()));
        assert!(scripts.stop_folder("f000000000001"));
    }

    #[test]
    fn a_loaded_thread_has_no_runs() {
        let brief = RunBrief {
            id: "0123456789abcdef".into(),
            script_id: "s00000001".into(),
            name: "dev".into(),
            phase: RunPhase::Running,
            exit: None,
        };
        let info = ThreadInfo { runs: vec![brief], ..crate::host::threads::test_info(AgentId::Claude) };
        let loaded = crate::host::db::LoadedThread {
            info: serde_json::from_value(serde_json::to_value(&info).unwrap()).unwrap(),
            cols: crate::host::db::ThreadCols { next_seq: 1, next_turn: 1, ..Default::default() },
            prior_native_id: None,
            max_turn: 0,
            users: 0,
            plan: None,
        };
        let entry = crate::host::threads::Registry::entry_from(loaded, &[]);
        assert!(entry.info.runs.is_empty(), "no run outlives its host");
    }

    #[tokio::test]
    async fn a_phones_run_is_told_on_the_pc_once_and_the_switch_is_read_each_time() {
        let dir = tempfile::tempdir().unwrap();
        let (host, mut notes) = crate::host::test_host_noting(dir.path(), APPROVAL_LIFETIME_MS);
        host.test_folder(dir.path());
        let script = Script { id: "s00000001".into(), name: "build".into(), command: "exit 0".into() };
        {
            let mut reg = host.lock_reg();
            reg.folders[0].scripts = vec![script.clone()];
            reg.folders[0].phone_scripts = true;
        }
        let (thread, _generation, _cmds) = host.test_thread(AgentId::Claude);
        let phone = json!({"kind": "phone", "device_id": "dev1", "device_name": "Pixel 8 Pro"});
        let ask = |by: Value| json!({"thread_id": thread, "script_id": script.id, "by": by});
        let ran = host.script_call(ClientKind::Link, method::SCRIPT_RUN, ask(phone.clone())).await.unwrap();
        let run: ScriptRunResult = serde_json::from_value(ran).unwrap();
        assert_eq!(run.run.by.device_id.as_deref(), Some("dev1"));
        let mut told = Vec::new();
        let until = Instant::now() + Duration::from_secs(5);
        while host.scripts.running() > 0 && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        host.script_call(ClientKind::Link, method::SCRIPT_RUN, ask(Value::Null)).await.unwrap();
        while let Ok(note) = notes.try_recv() {
            if let Note::PhoneScript { device, script, thread: info } = note {
                told.push((device, script, info.id));
            }
        }
        assert_eq!(told, [("Pixel 8 Pro".to_owned(), "build".to_owned(), thread.clone())]);

        // Off: the phone is refused at once; the PC is not.
        host.lock_reg().folders[0].phone_scripts = false;
        let refused =
            host.script_call(ClientKind::Link, method::SCRIPT_RUN, ask(phone.clone())).await.unwrap_err();
        assert_eq!(
            (refused.code, refused.data),
            (ErrorCode::Denied, Some(json!({"reason": PHONE_SCRIPTS_OFF})))
        );
        assert_eq!(refused.message, PHONE_OFF);
        for kind in [ClientKind::Launcher, ClientKind::Cli, ClientKind::Test, ClientKind::Bridge] {
            let e = host.script_call(kind, method::SCRIPT_RUN, ask(phone.clone())).await.unwrap_err();
            assert_eq!((e.code, e.message.as_str()), (ErrorCode::Denied, DONE_ON_LINK), "{kind:?}");
        }
        assert!(notes.try_recv().is_err(), "nothing more was told");
        host.stop_runs().await;
    }

    #[test]
    fn an_event_carries_the_newest_lines_within_its_bounds() {
        let s = slot("a", None, 1);
        for n in 0..500 {
            s.lines.push(&format!("line {n}"));
        }
        let event = s.event(0);
        assert_eq!(event.lines.len(), MAX_RUN_EVENT_LINES);
        assert_eq!(event.from, 300);
        assert_eq!(event.lines[0], "line 300");
        let rest = s.event(450);
        assert_eq!((rest.from, rest.lines.len()), (450, 50));
        let wide = slot("a", None, 1);
        for _ in 0..200 {
            wide.lines.push(&"é".repeat(256));
        }
        let event = wide.event(0);
        assert!(event.lines.iter().map(String::len).sum::<usize>() <= MAX_RUN_EVENT_BYTES);
        assert_eq!(event.lines.len(), MAX_RUN_EVENT_BYTES / 512);
        assert_eq!(event.from, 200 - event.lines.len() as u64);
        let none = s.event(500);
        assert!(none.lines.is_empty() && none.from == 500);
        assert_eq!(s.bare().from, 500);
    }
}
