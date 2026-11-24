// SPDX-License-Identifier: GPL-3.0-or-later
//! Project scripts on the Agents page (phase 6, §X5.1): the Run menu in a
//! thread's header, the run panel beside the transcript, "Add to message",
//! and Setup's Scripts on each folder's card.
//!
//! A folder's scripts are what the user typed in Setup; the host keeps them
//! in its own `folders.json` and runs one only when a person asks (here, or
//! from a phone the folder lets). This keeps what the page shows of the open
//! thread's runs and asks the host to start, stop and read them. Output is
//! plain text: the host cleaned it, and the page never reads it as markup.
//!
//! Phones' requests (`docs/protocol/agents.md` §12) go to the host as that
//! phone (`by`); the host alone checks the folder's "Phones can run these".
//! A phone never gets a run's command: the wire has no field for it.

use g7_agents::api::{
    FoldersSetParams, RunBrief, RunPhase, ScriptOutputParams, ScriptRun, ScriptRunParams, ScriptStopParams,
    ScriptWanted,
};

use g7link_core::AgentScriptRun as WireRun;

use super::*;

// ---- Bounds ----

/// "Add to message" takes the newest lines: at most 40, and 2,000 characters.
const QUOTE_LINES: usize = 40;
const QUOTE_CHARS: usize = 2_000;
/// Runs the panel lists: what a thread can have running and kept.
const LISTED_RUNS: usize = api::MAX_RUNS_THREAD + api::MAX_RUNS_KEPT;

// ---- The rules (pure) ----

fn is_running(phase: RunPhase) -> bool {
    phase == RunPhase::Running
}

/// A script running for the thread, by name ("" for none): its folder's
/// header shows it. The host keeps `runs` current with `thread.state`.
pub(crate) fn running_name(info: &ThreadInfo) -> &str {
    info.runs.iter().find(|r| is_running(r.phase)).map_or("", |r| r.name.as_str())
}

/// The Run menu's rows: the folder's scripts in order, each with the run of
/// it that is running in this thread ("" for none).
pub(crate) fn menu_rows(folder: Option<&Folder>, runs: &[RunBrief]) -> Vec<Value> {
    let Some(folder) = folder else { return Vec::new() };
    folder
        .scripts
        .iter()
        .map(|script| {
            let run = runs.iter().find(|r| r.script_id == script.id && is_running(r.phase));
            json!({
                "id": script.id,
                "name": script.name,
                "command": script.command,
                "run": run.map(|r| r.id.clone()).unwrap_or_default(),
                "running": run.is_some(),
            })
        })
        .collect()
}

/// The panel's order: running first, then the newest.
pub(crate) fn sort_runs(runs: &mut [ScriptRun]) {
    runs.sort_by(|a, b| is_running(b.phase).cmp(&is_running(a.phase)).then(b.started_ms.cmp(&a.started_ms)));
}

/// `got` onto the run shown: lines that follow on are added, an overlap is
/// skipped, and a jump past what is shown counts as lines not kept. The
/// newest [`api::MAX_RUN_LINES`] and [`api::MAX_RUN_BYTES`] stay.
pub(crate) fn merge(shown: &mut ScriptRun, got: ScriptRun) {
    if got.id != shown.id {
        return;
    }
    let next = shown.from + shown.lines.len() as u64;
    if got.from > next {
        let lost = u32::try_from(got.from - next).unwrap_or(u32::MAX);
        shown.lines.clear();
        shown.from = got.from;
        shown.dropped = shown.dropped.saturating_add(lost);
        shown.lines.extend(got.lines);
    } else {
        let skip = usize::try_from(next - got.from).unwrap_or(usize::MAX);
        shown.lines.extend(got.lines.into_iter().skip(skip));
    }
    let mut over = shown.lines.len().saturating_sub(api::MAX_RUN_LINES);
    let mut bytes: usize = shown.lines.iter().map(String::len).sum();
    for line in &shown.lines[over..] {
        if bytes <= api::MAX_RUN_BYTES {
            break;
        }
        bytes -= line.len();
        over += 1;
    }
    let over = over.min(shown.lines.len());
    if over > 0 {
        let dropped_here: usize = shown.lines.drain(..over).count();
        shown.from += dropped_here as u64;
        shown.dropped = shown.dropped.saturating_add(u32::try_from(dropped_here).unwrap_or(u32::MAX));
    }
    shown.dropped = shown.dropped.max(got.dropped);
    shown.phase = got.phase;
    shown.exit = got.exit.or(shown.exit);
    shown.ended_ms = got.ended_ms.or(shown.ended_ms);
    if got.error.is_some() {
        shown.error = got.error;
    }
}

/// A `script.progress` leaves a gap after what is shown: the lines between
/// are read with `script.output { from }`, from this line on.
pub(crate) fn gap(shown: &ScriptRun, got: &ScriptRun) -> Option<u64> {
    let next = shown.from + shown.lines.len() as u64;
    (got.id == shown.id && got.from > next).then_some(next)
}

/// "Add to message": the newest lines (at most 40 and 2,000 characters)
/// in a fence longer than any run of backticks inside them, under
/// `heading` ("Output of <name>:"). `None` when there's no line.
pub(crate) fn quote_text(heading: &str, lines: &[String]) -> Option<String> {
    let mut kept: Vec<&str> = Vec::new();
    let mut chars = 0;
    for line in lines.iter().rev().take(QUOTE_LINES) {
        let n = line.chars().count() + usize::from(!kept.is_empty());
        if chars + n > QUOTE_CHARS {
            if kept.is_empty() {
                let room = QUOTE_CHARS;
                let start = line.char_indices().rev().nth(room.saturating_sub(1)).map_or(0, |(i, _)| i);
                kept.push(&line[start..]);
            }
            break;
        }
        chars += n;
        kept.push(line);
    }
    if kept.iter().all(|l| l.trim().is_empty()) {
        return None;
    }
    kept.reverse();
    let body = kept.join("\n");
    let (mut longest, mut run) = (0, 0);
    for c in body.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat((longest + 1).max(3));
    Some(format!("{heading}\n{fence}\n{body}\n{fence}"))
}

/// The composer's words with the quote after them.
pub(crate) fn with_quote(text: &str, quote: &str) -> String {
    let text = text.trim_end();
    if text.is_empty() { quote.to_owned() } else { format!("{text}\n\n{quote}") }
}

/// Setup's form as the page sends it: `[{ id?, name, command }]`. The host
/// checks the bounds; this only refuses what isn't the shape, and trims.
pub(crate) fn wanted(json: &str) -> Option<Vec<ScriptWanted>> {
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        id: Option<String>,
        name: String,
        command: String,
    }
    let rows: Vec<Row> = serde_json::from_str(json).ok()?;
    if rows.len() > api::MAX_SCRIPTS {
        return None;
    }
    Some(
        rows.into_iter()
            .map(|r| ScriptWanted {
                id: r.id.filter(|id| api::is_valid_script_id(id)),
                name: r.name.trim().to_owned(),
                command: r.command.trim().to_owned(),
            })
            .collect(),
    )
}

fn run_json(run: &ScriptRun) -> Value {
    let phone = (run.by.kind == OriginKind::Phone).then(|| run.by.device_name.clone().unwrap_or_default());
    json!({
        "id": run.id,
        "scriptId": run.script_id,
        "name": run.name,
        "phase": word(&run.phase),
        "exit": run.exit.map_or(Value::Null, Value::from),
        "by": phone.unwrap_or_default(),
        "startedMs": run.started_ms,
        "endedMs": run.ended_ms.unwrap_or(0),
        "reason": run.error.as_ref().map(|e| e.reason.clone()).unwrap_or_default(),
    })
}

// ---- What the page keeps ----

/// The open thread's runs.
#[derive(Debug, Default)]
pub(crate) struct Panel {
    pub thread_id: String,
    pub seq: u64,
    pub loading: bool,
    /// The thread's runs without their lines, running first, then the newest.
    pub runs: Vec<ScriptRun>,
    /// The run whose output shows.
    pub selected: Option<String>,
    /// The selected run with its lines.
    pub output: Option<ScriptRun>,
    /// `script.output` is being read for a gap; another gap reads again after.
    pub reading: bool,
    pub again: bool,
    /// The last refusal: its reason word and the script's name.
    pub error: Option<(String, String)>,
}

/// Everything scripts keep, in the bridge's state.
#[derive(Debug, Default)]
pub(crate) struct Scripts {
    /// The host has `cap::SCRIPTS`.
    pub cap: bool,
    /// The run panel beside the transcript is open.
    pub shown: bool,
    pub panel: Panel,
    /// Setup: the folder whose list is being saved.
    pub saving: Option<String>,
    /// Setup: the host's last refusal, by folder: its reason word.
    pub refused: Option<(String, String)>,
}

impl Scripts {
    /// The open thread changed: the panel starts over for another thread.
    fn follow_thread(&mut self, thread_id: &str) -> bool {
        if self.panel.thread_id == thread_id {
            return false;
        }
        self.panel = Panel { thread_id: thread_id.to_owned(), ..Default::default() };
        true
    }

    fn upsert(&mut self, run: &ScriptRun) {
        let runs = &mut self.panel.runs;
        let mut brief = run.clone();
        brief.lines = Vec::new();
        match runs.iter_mut().find(|r| r.id == run.id) {
            Some(r) => *r = brief,
            None => runs.push(brief),
        }
        sort_runs(runs);
        runs.truncate(LISTED_RUNS);
    }
}

// ---- The page's JSON ----

/// The open thread's Run menu and run panel (`{}` when no thread is open).
pub(super) fn runs_json(s: &BridgeState) -> Value {
    let sc = &s.scripts;
    let Some(open) = s.open.as_ref().filter(|o| o.kind == OpenKind::Thread) else { return json!({}) };
    let Some(info) = s.threads.get(&open.id) else { return json!({}) };
    let folder = s.folders.iter().find(|f| f.id == info.folder_id);
    let p = &sc.panel;
    let mine = p.thread_id == info.id;
    // The lines only while the panel shows them: they can be 256 KiB.
    let output =
        p.output.as_ref().filter(|o| sc.shown && mine && p.selected.as_deref() == Some(o.id.as_str()));
    json!({
        "available": sc.cap && info.readonly.is_none(),
        "shown": sc.shown && sc.cap,
        "folder": folder.map(|f| f.id.clone()).unwrap_or_default(),
        "folderLabel": info.folder_label,
        "missing": folder.is_some_and(|f| !f.exists),
        "scripts": menu_rows(folder, &info.runs),
        "running": info.runs.iter().filter(|r| is_running(r.phase)).count(),
        "loading": mine && p.loading && p.runs.is_empty(),
        "runs": if mine { p.runs.iter().map(run_json).collect::<Vec<_>>() } else { Vec::new() },
        "selected": if mine { p.selected.clone().unwrap_or_default() } else { String::new() },
        "output": output.map_or_else(|| json!({}), |o| json!({
            "lines": o.lines,
            "from": o.from,
            "dropped": o.dropped,
        })),
        "phones": folder.is_some_and(|f| f.phone_scripts),
        "error": p.error.as_ref().filter(|_| mine).map_or_else(|| json!({}), |(reason, name)| json!({
            "reason": reason, "name": name,
        })),
    })
}

/// Setup's part of a folder's card.
pub(super) fn folder_json(s: &BridgeState, f: &Folder) -> Value {
    let refused = s.scripts.refused.as_ref().filter(|(id, _)| *id == f.id).map(|(_, r)| r.clone());
    json!({
        "list": f.scripts.iter().map(|x| json!({ "id": x.id, "name": x.name, "command": x.command })).collect::<Vec<_>>(),
        "phones": f.phone_scripts,
        "saving": s.scripts.saving.as_deref() == Some(f.id.as_str()),
        "refused": refused.unwrap_or_default(),
    })
}

// ---- Talking to the host ----

/// The open thread's id, while it is a thread.
fn open_thread_id() -> Option<String> {
    let s = lock();
    s.open.as_ref().filter(|o| o.kind == OpenKind::Thread).map(|o| o.id.clone())
}

/// A refusal of a run, in the panel's line (the page words it).
fn refused(e: &ClientError, name: String) {
    let reason = source::reason_of(e);
    lock().scripts.panel.error = Some((reason.clone(), name));
    changed();
    if matches!(reason.as_str(), "unreachable" | "denied" | "unavailable") {
        source::tell(&reason);
    }
}

/// The host connected: whether it has scripts. A thread already open reads its runs.
pub(crate) fn connected(cap: bool) {
    lock().scripts.cap = cap;
    if cap {
        read_runs();
    }
}

/// Reads the open thread's runs, then the selected one's output.
pub(crate) fn read_runs() {
    let Some((thread_id, seq)) = ({
        let mut s = lock();
        let id = s.open.as_ref().filter(|o| o.kind == OpenKind::Thread).map(|o| o.id.clone());
        match id {
            Some(id) if s.scripts.cap => {
                s.scripts.follow_thread(&id);
                let p = &mut s.scripts.panel;
                p.seq += 1;
                p.loading = true;
                Some((id, p.seq))
            }
            _ => None,
        }
    }) else {
        return;
    };
    source::with_client(move |client| async move {
        let read = client.script_runs(thread_id.clone()).await;
        let output = {
            let mut s = lock();
            let p = &mut s.scripts.panel;
            if p.thread_id != thread_id || p.seq != seq {
                return;
            }
            p.loading = false;
            match read {
                Ok(mut runs) => {
                    runs.iter_mut().for_each(|r| r.lines = Vec::new());
                    sort_runs(&mut runs);
                    runs.truncate(LISTED_RUNS);
                    p.runs = runs;
                }
                Err(e) => tracing::debug!(error = %e, "a thread's runs weren't read"),
            }
            // The selected run stays; else the first (a running one, or the newest).
            if p.selected.as_ref().is_none_or(|id| !p.runs.iter().any(|r| &r.id == id)) {
                p.selected = p.runs.first().map(|r| r.id.clone());
            }
            let wanted = p.selected.clone().filter(|id| p.output.as_ref().is_none_or(|o| &o.id != id));
            if wanted.is_some() {
                p.output = None;
            }
            wanted
        };
        changed();
        if let Some(run) = output {
            read_output(run, None);
        }
    });
}

/// Reads a run's kept lines (`from`: from that line on) onto the output shown.
fn read_output(run: String, from: Option<u64>) {
    lock().scripts.panel.reading = true;
    source::with_client(move |client| async move {
        let read = client.script_output(ScriptOutputParams { run: run.clone(), from }).await;
        let again = {
            let mut s = lock();
            let p = &mut s.scripts.panel;
            p.reading = false;
            if p.selected.as_deref() != Some(run.as_str()) {
                return;
            }
            match read {
                Ok(got) => match p.output.as_mut().filter(|o| o.id == got.id) {
                    Some(shown) => merge(shown, got),
                    None => p.output = Some(got),
                },
                Err(e) => tracing::debug!(error = %e, "a run's output wasn't read"),
            }
            let again = std::mem::take(&mut p.again);
            again.then(|| p.output.as_ref().map(|o| o.from + o.lines.len() as u64)).flatten()
        };
        changed();
        if let Some(next) = again {
            read_output(run, Some(next));
        }
    });
}

/// `script.progress`: the run's row changes; the shown output takes the new
/// lines, or reads what a gap left out.
pub(crate) fn progress(run: ScriptRun) {
    let read = {
        let mut s = lock();
        if s.scripts.panel.thread_id != run.thread_id {
            return;
        }
        s.scripts.upsert(&run);
        let p = &mut s.scripts.panel;
        let selected = p.selected.as_deref() == Some(run.id.as_str());
        match p.output.as_mut().filter(|o| selected && o.id == run.id) {
            Some(shown) => match gap(shown, &run) {
                Some(_) if p.reading => {
                    p.again = true;
                    None
                }
                Some(next) => {
                    // Its phase is taken now; its lines when they're read.
                    shown.phase = run.phase;
                    shown.exit = run.exit.or(shown.exit);
                    shown.ended_ms = run.ended_ms.or(shown.ended_ms);
                    Some((run.id.clone(), next))
                }
                None => {
                    merge(shown, run);
                    None
                }
            },
            None => None,
        }
    };
    changed();
    if let Some((id, next)) = read {
        read_output(id, Some(next));
    }
}

/// A thread's state arrived: when its runs aren't the ones the panel lists,
/// they're read again.
pub(crate) fn thread_state(info: &ThreadInfo) {
    let stale = {
        let s = lock();
        let p = &s.scripts.panel;
        s.scripts.cap
            && p.thread_id == info.id
            && !p.loading
            && info.runs.iter().any(|b| !p.runs.iter().any(|r| r.id == b.id && r.phase == b.phase))
    };
    if stale {
        read_runs();
    }
}

/// A thread opened, or another one did: the panel follows it.
pub(crate) fn thread_opened() {
    let Some(id) = open_thread_id() else { return };
    let (fresh, wanted) = {
        let mut s = lock();
        let fresh = s.scripts.follow_thread(&id);
        let wanted =
            s.scripts.cap && (s.scripts.shown || s.threads.get(&id).is_some_and(|t| !t.runs.is_empty()));
        (fresh, wanted)
    };
    if fresh && wanted {
        read_runs();
    }
}

// ---- The page's requests ----

/// Opens or closes the run panel (`Ctrl+Shift+R`). Source control's panel
/// gives way: the two share the slot.
pub fn panel(open: bool) {
    {
        let mut s = lock();
        if s.scripts.shown == open {
            return;
        }
        s.scripts.shown = open;
        if open {
            s.source.shown = false;
        }
    }
    changed();
    if open {
        read_runs();
    }
}

/// The source-control panel opened: the run panel gives way.
pub(crate) fn give_way() {
    let mut s = lock();
    if std::mem::take(&mut s.scripts.shown) {
        drop(s);
        changed();
    }
}

/// Runs one of the open thread's folder's scripts; the panel opens on it.
pub fn run(script_id: String) {
    if !api::is_valid_script_id(&script_id) {
        return;
    }
    let Some(thread_id) = open_thread_id() else { return };
    let name = {
        let mut s = lock();
        if !s.scripts.cap {
            return;
        }
        let info = s.threads.get(&thread_id);
        if info.is_none_or(|t| t.readonly.is_some()) {
            return;
        }
        let name = info
            .and_then(|t| s.folders.iter().find(|f| f.id == t.folder_id))
            .and_then(|f| f.scripts.iter().find(|x| x.id == script_id))
            .map(|x| x.name.clone())
            .or_else(|| {
                s.scripts.panel.runs.iter().find(|r| r.script_id == script_id).map(|r| r.name.clone())
            })
            .unwrap_or_default();
        s.scripts.follow_thread(&thread_id);
        s.scripts.panel.error = None;
        s.scripts.shown = true;
        s.source.shown = false;
        name
    };
    changed();
    let params = ScriptRunParams { thread_id: thread_id.clone(), script_id, by: None };
    source::with_client(move |client| async move {
        match client.script_run(params).await {
            Ok(run) => {
                {
                    let mut s = lock();
                    if s.scripts.panel.thread_id == run.thread_id {
                        s.scripts.upsert(&run);
                        s.scripts.panel.selected = Some(run.id.clone());
                        s.scripts.panel.output = Some(run);
                    }
                }
                changed();
            }
            Err(e) => refused(&e, name),
        }
    });
}

/// Stops a run of the open thread.
pub fn stop(run: String) {
    if !api::is_valid_run_id(&run) {
        return;
    }
    let name = lock().scripts.panel.runs.iter().find(|r| r.id == run).map(|r| r.name.clone());
    let Some(name) = name else { return };
    source::with_client(move |client| async move {
        match client.script_stop(ScriptStopParams { run, by: None }).await {
            Ok(run) => progress(run),
            Err(e) => refused(&e, name),
        }
    });
}

/// Shows a run's output.
pub fn select(run: String) {
    {
        let mut s = lock();
        let p = &mut s.scripts.panel;
        if p.selected.as_deref() == Some(run.as_str()) || !p.runs.iter().any(|r| r.id == run) {
            return;
        }
        p.selected = Some(run.clone());
        p.output = None;
        p.again = false;
    }
    changed();
    read_output(run, None);
}

/// The kept lines of the run shown, as one text.
fn shown_lines(run: &str) -> Option<(String, Vec<String>)> {
    let s = lock();
    let o = s.scripts.panel.output.as_ref().filter(|o| o.id == run)?;
    Some((o.name.clone(), o.lines.clone()))
}

/// "Copy output": the kept lines of the run shown.
pub fn copy(run: String) {
    let Some((_, lines)) = shown_lines(&run) else { return };
    if lines.is_empty() {
        return;
    }
    super::copy(lines.join("\n"));
}

/// "Add to message": the newest lines go into the composer's draft under
/// "Output of <name>:". Nothing is sent.
pub fn quote(run: String) {
    let Some((name, lines)) = shown_lines(&run) else { return };
    let Some(quote) = quote_text(&tr!("Agents", "Output of %1:", name), &lines) else { return };
    {
        let mut s = lock();
        let Some(open) = s.open.as_mut().filter(|o| o.kind == OpenKind::Thread) else { return };
        let text = with_quote(&open.composer.text, &quote);
        open.composer.put(text);
    }
    changed();
}

// ---- Setup ----

/// Saves a folder's whole script list, in order (`json`: `[{ id?, name, command }]`).
pub fn save(folder_id: String, json: String) {
    let Some(list) = wanted(&json) else { return };
    {
        let mut s = lock();
        if !s.scripts.cap || !s.folders.iter().any(|f| f.id == folder_id) {
            return;
        }
        s.scripts.saving = Some(folder_id.clone());
        s.scripts.refused = None;
    }
    changed();
    let params = FoldersSetParams { id: folder_id.clone(), scripts: Some(list), ..Default::default() };
    set_folder(folder_id, params);
}

/// A folder's "Phones can run these".
pub fn set_phones(folder_id: String, on: bool) {
    if !lock().scripts.cap {
        return;
    }
    let params = FoldersSetParams { id: folder_id.clone(), phone_scripts: Some(on), ..Default::default() };
    set_folder(folder_id, params);
}

fn set_folder(folder_id: String, params: FoldersSetParams) {
    source::with_client(move |client| async move {
        let result = client.folders_set_with(params).await;
        {
            let mut s = lock();
            if s.scripts.saving.as_deref() == Some(folder_id.as_str()) {
                s.scripts.saving = None;
            }
            match result {
                Ok(folder) => {
                    if let Some(f) = s.folders.iter_mut().find(|f| f.id == folder.id) {
                        *f = folder;
                    }
                }
                Err(e) => {
                    let reason = source::reason_of(&e);
                    if matches!(reason.as_str(), "unreachable" | "denied" | "unavailable") {
                        drop(s);
                        source::tell(&reason);
                        changed();
                        return;
                    }
                    s.scripts.refused = Some((folder_id, reason));
                }
            }
        }
        changed();
    });
}

// ---- Phones (§12) ----

/// A run as a phone gets it: no command, no folder, the starting phone by
/// name. The core cuts its lines to the phone's bounds and sets `mine`.
pub(super) fn wire_run(run: &ScriptRun) -> WireRun {
    WireRun {
        id: run.id.clone(),
        thread: run.thread_id.clone(),
        script: run.script_id.clone(),
        name: run.name.clone(),
        phase: word(&run.phase),
        exit: run.exit,
        started_ms: run.started_ms,
        ended_ms: run.ended_ms,
        by: (run.by.kind == OriginKind::Phone).then(|| run.by.device_name.clone()).flatten(),
        mine: false,
        lines: run.lines.clone(),
        from: run.from,
        dropped: run.dropped,
        reason: run.error.as_ref().map(|e| e.reason.clone()),
    }
}

/// The phone a run was started from (`None`: this PC).
pub(super) fn run_by(run: &ScriptRun) -> Option<DeviceId> {
    (run.by.kind == OriginKind::Phone).then(|| run.by.device_id.as_deref()?.parse().ok()).flatten()
}

/// `script.run` for a phone: always as that phone, so the host checks the
/// folder's switch.
pub(crate) fn phone_run_params(
    from: &DeviceId,
    name: &str,
    thread: String,
    script: String,
) -> ScriptRunParams {
    ScriptRunParams { thread_id: thread, script_id: script, by: Some(Origin::phone(from.to_string(), name)) }
}

/// `script.stop` for a phone, as that phone.
pub(crate) fn phone_stop_params(from: &DeviceId, name: &str, run: String) -> ScriptStopParams {
    ScriptStopParams { run, by: Some(Origin::phone(from.to_string(), name)) }
}

fn run_reply(result: Result<ScriptRun, ClientError>) -> Result<AgentReply, AgentFailure> {
    match result {
        Ok(run) => Ok(AgentReply::ScriptRun { by: run_by(&run), run: wire_run(&run) }),
        Err(e) => source_relay::refused(e),
    }
}

/// Answers a phone's §12 request, after the core checked the `agents`
/// toggle and the limits.
pub(super) async fn phone(
    from: DeviceId,
    client: &Client,
    request: AgentRequest,
) -> Result<AgentReply, AgentFailure> {
    let name = device_name(&from);
    match request {
        AgentRequest::ScriptRun { thread, script } => {
            run_reply(client.script_run(phone_run_params(&from, &name, thread, script)).await)
        }
        AgentRequest::ScriptStop { run } => {
            run_reply(client.script_stop(phone_stop_params(&from, &name, run)).await)
        }
        AgentRequest::ScriptRuns { thread } => {
            let runs = client.script_runs(thread).await.map_err(|e| failure(&e))?;
            Ok(AgentReply::ScriptRuns(runs.iter().map(|r| (wire_run(r), run_by(r))).collect()))
        }
        AgentRequest::ScriptOutput { run } => {
            run_reply(client.script_output(ScriptOutputParams { run, from: None }).await)
        }
        _ => Err(AgentFailure::BadMessage),
    }
}

#[cfg(test)]
mod tests {
    use g7_agents::api::{RunError, Script};

    use super::*;

    fn a_run(id: &str, phase: RunPhase, started_ms: u64) -> ScriptRun {
        ScriptRun {
            id: id.into(),
            thread_id: "t".into(),
            folder_id: "f".into(),
            script_id: "s0000000a".into(),
            name: "dev".into(),
            command: "npm run dev".into(),
            phase,
            exit: None,
            started_ms,
            ended_ms: None,
            by: Origin::launcher(),
            lines: Vec::new(),
            from: 0,
            dropped: 0,
            error: None,
        }
    }

    fn lines(from: u64, n: u64) -> Vec<String> {
        (from..from + n).map(|i| format!("line {i}")).collect()
    }

    fn folder_with(scripts: &[(&str, &str, &str)]) -> Folder {
        Folder {
            id: "f".into(),
            label: "shop".into(),
            path: "/home/me/shop".into(),
            exists: true,
            added_ms: 1,
            mcp: true,
            mcp_seen: Vec::new(),
            checkpoints: true,
            follow_remote: false,
            scripts: scripts
                .iter()
                .map(|(id, name, command)| Script {
                    id: (*id).into(),
                    name: (*name).into(),
                    command: (*command).into(),
                })
                .collect(),
            phone_scripts: false,
        }
    }

    #[test]
    fn the_menu_lists_the_folders_scripts_with_the_run_of_each_here() {
        let folder = folder_with(&[("s0000000a", "dev", "npm run dev"), ("s0000000b", "test", "cargo test")]);
        let runs = [
            RunBrief {
                id: "0123456789abcdef".into(),
                script_id: "s0000000a".into(),
                name: "dev".into(),
                phase: RunPhase::Running,
                exit: None,
            },
            RunBrief {
                id: "fedcba9876543210".into(),
                script_id: "s0000000b".into(),
                name: "test".into(),
                phase: RunPhase::Failed,
                exit: Some(1),
            },
        ];
        let rows = menu_rows(Some(&folder), &runs);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["command"], "npm run dev", "the command shows in the menu");
        assert_eq!(rows[0]["run"], "0123456789abcdef");
        assert_eq!(rows[0]["running"], true);
        assert_eq!(rows[1]["run"], "", "an ended run isn't one to stop");
        assert_eq!(rows[1]["running"], false);
        assert!(menu_rows(None, &runs).is_empty(), "no folder, no scripts");
        assert!(menu_rows(Some(&folder_with(&[])), &runs).is_empty());
    }

    #[test]
    fn running_runs_come_first_then_the_newest() {
        let mut runs = vec![
            a_run("a", RunPhase::Done, 30),
            a_run("b", RunPhase::Running, 10),
            a_run("c", RunPhase::Stopped, 20),
        ];
        sort_runs(&mut runs);
        assert_eq!(runs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["b", "a", "c"]);
    }

    #[test]
    fn new_lines_follow_on_and_an_overlap_is_skipped() {
        let mut shown = ScriptRun { lines: lines(0, 3), ..a_run("r", RunPhase::Running, 1) };
        let got = ScriptRun { lines: lines(2, 3), from: 2, ..a_run("r", RunPhase::Running, 1) };
        assert_eq!(gap(&shown, &got), None);
        merge(&mut shown, got);
        assert_eq!(shown.lines, lines(0, 5));
        assert_eq!(shown.from, 0);
        let other = ScriptRun { lines: lines(5, 1), from: 5, ..a_run("x", RunPhase::Running, 1) };
        merge(&mut shown, other);
        assert_eq!(shown.lines.len(), 5, "another run's lines never mix in");
    }

    #[test]
    fn a_gap_is_read_from_the_first_line_missing() {
        let shown = ScriptRun { lines: lines(0, 10), ..a_run("r", RunPhase::Running, 1) };
        let got = ScriptRun { lines: lines(250, 200), from: 250, ..a_run("r", RunPhase::Running, 1) };
        assert_eq!(gap(&shown, &got), Some(10), "script.output from line 10");
        let follows = ScriptRun { lines: lines(10, 1), from: 10, ..a_run("r", RunPhase::Running, 1) };
        assert_eq!(gap(&shown, &follows), None);
    }

    #[test]
    fn lines_the_host_no_longer_keeps_are_counted_not_kept() {
        let mut shown = ScriptRun { lines: lines(0, 10), ..a_run("r", RunPhase::Running, 1) };
        let got = ScriptRun { lines: lines(40, 5), from: 40, ..a_run("r", RunPhase::Done, 1) };
        merge(&mut shown, got);
        assert_eq!(shown.from, 40);
        assert_eq!(shown.lines, lines(40, 5));
        assert_eq!(shown.dropped, 30);
        assert_eq!(shown.phase, RunPhase::Done, "the last word has the final phase");
    }

    #[test]
    fn the_panel_keeps_the_newest_lines_within_its_bounds() {
        let mut shown = a_run("r", RunPhase::Running, 1);
        let total = api::MAX_RUN_LINES as u64 + 500;
        merge(&mut shown, ScriptRun { lines: lines(0, total), ..a_run("r", RunPhase::Running, 1) });
        assert_eq!(shown.lines.len(), api::MAX_RUN_LINES);
        assert_eq!(shown.from, 500);
        assert_eq!(shown.dropped, 500);
        assert_eq!(shown.lines.last().map(String::as_str), Some(format!("line {}", total - 1).as_str()));
        let mut wide = a_run("w", RunPhase::Running, 1);
        let long: Vec<String> = (0..200).map(|_| "x".repeat(4096)).collect();
        merge(&mut wide, ScriptRun { lines: long, ..a_run("w", RunPhase::Running, 1) });
        assert!(wide.lines.iter().map(String::len).sum::<usize>() <= api::MAX_RUN_BYTES);
        assert_eq!(wide.from + wide.lines.len() as u64, 200);
    }

    #[test]
    fn a_failed_run_keeps_why() {
        let mut shown = a_run("r", RunPhase::Running, 1);
        let error = RunError { reason: "spawn".into(), message: "The shell couldn't be started.".into() };
        merge(&mut shown, ScriptRun { error: Some(error), exit: None, ..a_run("r", RunPhase::Failed, 1) });
        assert_eq!(shown.error.as_ref().map(|e| e.reason.as_str()), Some("spawn"));
        assert_eq!(run_json(&shown)["reason"], "spawn");
        assert_eq!(run_json(&shown)["exit"], Value::Null);
    }

    #[test]
    fn a_phones_run_says_which_phone() {
        let run = ScriptRun { by: Origin::phone("d", "Pixel"), ..a_run("r", RunPhase::Running, 1) };
        assert_eq!(run_json(&run)["by"], "Pixel");
        assert_eq!(run_json(&a_run("r", RunPhase::Running, 1))["by"], "", "this PC's says nothing");
    }

    #[test]
    fn add_to_message_takes_the_newest_lines_in_a_fence() {
        let all = lines(0, 100);
        let quote = quote_text("Output of dev:", &all).expect("a quote");
        let mut parts = quote.lines();
        assert_eq!(parts.next(), Some("Output of dev:"));
        assert_eq!(parts.next(), Some("```"));
        let body: Vec<&str> = quote.lines().skip(2).take_while(|l| *l != "```").collect();
        assert_eq!(body.len(), QUOTE_LINES);
        assert_eq!(body.first(), Some(&"line 60"));
        assert_eq!(body.last(), Some(&"line 99"));
        assert!(quote_text("Output of dev:", &[]).is_none(), "nothing to add");
        assert!(quote_text("Output of dev:", &["  ".into()]).is_none());
    }

    #[test]
    fn add_to_message_stays_within_two_thousand_characters() {
        let long: Vec<String> = (0..40).map(|i| format!("{i:02}{}", "é".repeat(98))).collect();
        let quote = quote_text("Output of dev:", &long).expect("a quote");
        let body: String = quote.lines().skip(2).take_while(|l| *l != "```").collect::<Vec<_>>().join("\n");
        assert!(body.chars().count() <= QUOTE_CHARS);
        assert!(body.ends_with(&long[39]), "the newest line is whole");
        let one = vec!["y".repeat(5_000)];
        let cut = quote_text("Output of dev:", &one).expect("a quote");
        let body: String = cut.lines().skip(2).take_while(|l| *l != "```").collect();
        assert_eq!(body.chars().count(), QUOTE_CHARS, "a single long line keeps its end");
    }

    #[test]
    fn a_fence_is_longer_than_any_backticks_in_the_output() {
        let quote =
            quote_text("Output of docs:", &["```rust".into(), "fn x() {}".into(), "```".into()]).unwrap();
        assert!(quote.contains("\n````\n```rust"));
        assert!(quote.ends_with("```\n````"));
        assert_eq!(with_quote("", "Q"), "Q");
        assert_eq!(with_quote("Why does this fail?  \n", "Q"), "Why does this fail?\n\nQ");
    }

    #[test]
    fn a_phones_requests_always_go_as_that_phone() {
        let phone = DeviceId([7; 32]);
        let run = phone_run_params(&phone, "Pixel", "t".into(), "s0000000a".into());
        let by = run.by.expect("a phone's by");
        assert_eq!(by.kind, OriginKind::Phone);
        assert_eq!(by.device_id.as_deref(), Some(phone.to_string().as_str()));
        assert_eq!(by.device_name.as_deref(), Some("Pixel"));
        let stop = phone_stop_params(&phone, "Pixel", "0123456789abcdef".into());
        assert_eq!(stop.by.map(|b| b.kind), Some(OriginKind::Phone));
    }

    #[test]
    fn a_run_reaches_a_phone_without_its_command() {
        let phone = DeviceId([7; 32]);
        let run = ScriptRun {
            by: Origin::phone(phone.to_string(), "Pixel"),
            lines: vec!["ready".into()],
            error: Some(RunError { reason: "signal".into(), message: "killed".into() }),
            ..a_run("0123456789abcdef", RunPhase::Failed, 1)
        };
        let wire = wire_run(&run);
        let json = serde_json::to_string(&wire).unwrap();
        assert!(!json.contains("npm run dev"), "the command never leaves: {json}");
        assert_eq!(wire.phase, "failed");
        assert_eq!(wire.by.as_deref(), Some("Pixel"));
        assert_eq!(wire.reason.as_deref(), Some("signal"));
        assert_eq!(run_by(&run), Some(phone));
        assert_eq!(run_by(&a_run("r", RunPhase::Running, 1)), None, "this PC's");
        assert_eq!(wire_run(&a_run("r", RunPhase::Running, 1)).by, None);
    }

    #[test]
    fn setups_form_is_read_as_the_list_the_host_checks() {
        let list = wanted(r#"[{"id":"s0000000a","name":" dev ","command":" npm run dev "},{"name":"test","command":"cargo test"},{"id":"../x","name":"x","command":"y"}]"#)
            .expect("the shape");
        assert_eq!(list[0].id.as_deref(), Some("s0000000a"));
        assert_eq!(list[0].name, "dev");
        assert_eq!(list[0].command, "npm run dev");
        assert_eq!(list[1].id, None, "a new one gets its id from the host");
        assert_eq!(list[2].id, None, "an id that isn't one is dropped");
        assert!(wanted("{}").is_none());
        let thirteen: Vec<Value> =
            (0..13).map(|i| json!({ "name": format!("s{i}"), "command": "true" })).collect();
        assert!(wanted(&Value::Array(thirteen).to_string()).is_none(), "up to 12");
    }
}
