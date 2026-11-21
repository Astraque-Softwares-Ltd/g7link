// SPDX-License-Identifier: GPL-3.0-or-later
//! A phone's reply entered into a session started on this PC
//! (`plans/reply-on-pc.md`, docs/protocol/agents.md §8): the PC finds the
//! one window the session runs in, brings it forward, proves that the
//! terminal in front is the session's, pastes the text as one bracketed
//! paste (PRIMARY and Shift+Insert) and presses Enter.
//!
//! Refusing is the default: each step is a check that must pass, and what
//! isn't known is a refusal with a reason the phone can show. Nothing is
//! typed before the proof; nothing but the paste chord and Enter is ever
//! pressed. G7 never switches tabs, clicks, or types a probe.
//!
//! The sequence runs over four small traits ([`Host`], [`Windows`],
//! [`Keys`], [`Desk`]) so every refusal is tested on fakes; the live ones
//! are at the end. The text is never logged or stored: it lives in memory
//! for one request, and on PRIMARY until Enter.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use g7_agents::{
    api::{self, ErrorCode, PlaceRefusal, SessionPlaceResult, SessionSource, SessionState, cap},
    client::{Client, ClientError},
};
use g7link_core::{AgentFailure, AgentReply, AgentReplyTarget, AgentSessionReplied, DeviceId};

/// Most bytes in a reply, after it's cleaned.
pub(crate) const MAX_TEXT: usize = 8192;
/// How long the window has to come forward.
const FOCUS_WAIT: Duration = Duration::from_secs(1);
const FOCUS_POLL: Duration = Duration::from_millis(50);
/// Between the paste and the checks before Enter (S5: Claude's status file
/// says `waiting` within about 50 ms of a question).
const SETTLE: Duration = Duration::from_millis(250);
/// Input at the PC this recent is someone there (§3.2).
pub(crate) const OWNER_AWAY: Duration = Duration::from_secs(5);
/// A confirmation nonce works once, this long.
const NONCE_LIFE: Duration = Duration::from_secs(120);
/// A confirmed window is remembered this long.
const CONFIRMED_FOR: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_NONCES: usize = 32;
const MAX_CONFIRMED: usize = 256;
const MAX_PHONES: usize = 16;
/// Rate (§3.5): checks a second, one looked-at attempt a second, one entered
/// reply per session in [`SESSION_GAP`], and entered replies a minute and an hour.
const CHECKS_PER_SECOND: usize = 2;
const SESSION_GAP: Duration = Duration::from_secs(3);
const PER_MINUTE: usize = 6;
const PER_HOUR: usize = 60;
const SECOND: Duration = Duration::from_secs(1);
const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(60 * 60);
const MAX_RETRY_MS: u64 = 60_000;
/// A mark shorter than this, without whitespace, proves nothing.
const MIN_MARK_CHARS: usize = 16;
/// Bytes in a target's `app`, `agent` and `folder`.
const MAX_TARGET_BYTES: usize = 64;

// ---- What the request and the answer are ----

/// A phone's `agent.session.reply`.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Ask {
    pub session: String,
    pub text: Option<String>,
    pub check: bool,
    pub confirm: Option<String>,
}

impl std::fmt::Debug for Ask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ask")
            .field("session", &self.session)
            .field("text_len", &self.text.as_ref().map(String::len))
            .field("check", &self.check)
            .field("confirm", &self.confirm.is_some())
            .finish()
    }
}

/// Where a reply goes, as the phone shows it: "Terminal · Claude · g7linux".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub app: String,
    pub agent: String,
    pub folder: String,
    pub kind: &'static str,
}

/// Nothing was typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    Locked,
    InUse,
    Confirm,
    NeedsYou,
    Busy,
    Ended,
    NoProcess,
    NoWindow,
    Ambiguous,
    Tab,
    Place,
    Text,
    Rate,
    Setup,
}

impl Refused {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Refused::Locked => "locked",
            Refused::InUse => "in_use",
            Refused::Confirm => "confirm",
            Refused::NeedsYou => "needs_you",
            Refused::Busy => "busy",
            Refused::Ended => "ended",
            Refused::NoProcess => "no_process",
            Refused::NoWindow => "no_window",
            Refused::Ambiguous => "ambiguous",
            Refused::Tab => "tab",
            Refused::Place => "place",
            Refused::Text => "text",
            Refused::Rate => "rate",
            Refused::Setup => "setup",
        }
    }
}

/// The reply didn't go through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failed {
    /// The window didn't come forward: nothing was typed.
    Focus,
    /// The text was pasted and not sent.
    Interrupted,
    /// A key couldn't be pressed; the paste may have landed.
    Input,
}

impl Failed {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Failed::Focus => "focus",
            Failed::Interrupted => "interrupted",
            Failed::Input => "input",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A check found nothing in the way.
    Ready,
    /// Enter was pressed in the proven window.
    Sent,
    Refused(Refused),
    Failed(Failed),
}

impl Outcome {
    pub(crate) fn result(self) -> &'static str {
        match self {
            Outcome::Ready => "ready",
            Outcome::Sent => "sent",
            Outcome::Refused(_) => "refused",
            Outcome::Failed(_) => "failed",
        }
    }

    pub(crate) fn reason(self) -> Option<&'static str> {
        match self {
            Outcome::Ready | Outcome::Sent => None,
            Outcome::Refused(r) => Some(r.word()),
            Outcome::Failed(f) => Some(f.word()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Answer {
    pub outcome: Outcome,
    pub target: Option<Target>,
    pub nonce: Option<String>,
    pub retry_ms: Option<u32>,
    /// The terminal's executable, for the log line only.
    pub emulator: Option<String>,
}

impl Answer {
    fn of(outcome: Outcome) -> Self {
        Answer { outcome, target: None, nonce: None, retry_ms: None, emulator: None }
    }

    fn refused(reason: Refused) -> Self {
        Answer::of(Outcome::Refused(reason))
    }

    fn failed(reason: Failed) -> Self {
        Answer::of(Outcome::Failed(reason))
    }

    /// Where it was for: the target, and the terminal for the log.
    fn at(mut self, resolved: &Resolved) -> Self {
        self.target = Some(resolved.target.clone());
        self.emulator = Some(resolved.emulator.exe_name.clone());
        self
    }
}

/// The request itself can't be answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    /// The text breaks §2.5, or both or neither of `text` and `check`.
    BadMessage,
    /// No such session.
    NotFound,
    /// Another reply is being entered on this PC.
    Busy,
    /// The agent host couldn't be asked.
    Internal,
}

// ---- What the sequence asks of the PC ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Agent {
    Claude,
    Codex,
}

impl Agent {
    fn name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude",
            Agent::Codex => "Codex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Terminal,
    DesktopApp,
    Other,
}

/// The session's state as followed (§2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Now {
    Idle,
    Working,
    /// The agent reported it waits for the user.
    NeedsYou,
    /// Guessed from the file.
    MayNeedYou,
    Ended,
}

/// A process above the agent: its parent, grandparent, and so on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ancestor {
    pub pid: u32,
    pub start_ticks: u64,
    pub comm: String,
    /// The file name of its executable, never a path.
    pub exe_name: String,
}

/// Where a session's process runs, from the host's `session.place`.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Place {
    pub agent: Agent,
    pub source: Source,
    pub state: Now,
    /// The state came from Claude's status file, not a guess.
    pub state_exact: bool,
    pub pid: u32,
    pub start_ticks: u64,
    /// The agent's terminal (`tty_nr`); `None`: it has none.
    pub tty: Option<u32>,
    /// The agent is its terminal's foreground job.
    pub foreground: bool,
    /// Parent first, up to 32.
    pub chain: Vec<Ancestor>,
    /// Per ancestor pid: how many distinct terminals its descendants hold.
    pub ptys_under: Vec<(u32, u32)>,
    /// Runs of the session's newest text, whitespace removed. Never logged.
    pub marks: Vec<String>,
    /// The folder's label, never a path.
    pub folder: String,
}

impl std::fmt::Debug for Place {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Place")
            .field("agent", &self.agent)
            .field("source", &self.source)
            .field("state", &self.state)
            .field("state_exact", &self.state_exact)
            .field("pid", &self.pid)
            .field("tty", &self.tty)
            .field("foreground", &self.foreground)
            .field("chain", &self.chain.len())
            .field("marks", &self.marks.len())
            .finish_non_exhaustive()
    }
}

/// What the host says about a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Found {
    Place(Place),
    /// No session with this id.
    Unknown,
    /// No process has it.
    Ended,
    /// Two live processes have it.
    Ambiguous,
    /// It runs somewhere G7 can't reply into: the Claude or Codex app, or a
    /// Codex that doesn't name its session.
    Elsewhere,
}

/// The agent host couldn't be asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HostDown;

pub(crate) trait Host {
    fn place(&self, session: &str) -> Result<Found, HostDown>;
}

/// A window as the window backend lists it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Win {
    pub id: u64,
    pub pid: u32,
    pub class: String,
    /// Compared in memory to find a window, never logged.
    pub title: String,
    pub focused: bool,
    pub fullscreen: bool,
    pub minimized: bool,
    /// A normal window, not a dialog or a menu.
    pub normal: bool,
}

impl std::fmt::Debug for Win {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Win")
            .field("id", &self.id)
            .field("pid", &self.pid)
            .field("class", &self.class)
            .field("focused", &self.focused)
            .field("fullscreen", &self.fullscreen)
            .field("minimized", &self.minimized)
            .field("normal", &self.normal)
            .finish_non_exhaustive()
    }
}

/// A window of a terminal app on the accessibility bus, and the text of the
/// one terminal it shows (Ptyxis: the selected tab's).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Frame {
    pub title: String,
    pub active: bool,
    pub text: String,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame").field("active", &self.active).field("text_len", &self.text.len()).finish()
    }
}

/// The window backend: windows, focus, PRIMARY and terminal text.
pub(crate) trait Windows {
    /// Lists windows with `fullscreen` and `minimized`, and writes PRIMARY
    /// alone (the companion extension's additions).
    fn ready(&self) -> bool;
    /// `None` when the backend didn't answer.
    fn list(&self) -> Option<Vec<Win>>;
    fn activate(&self, id: u64) -> bool;
    fn set_primary(&self, text: &str) -> bool;
    /// Clears PRIMARY if it still holds `expected`.
    fn clear_primary(&self, expected: &str);
    /// The text of the focused terminal in the active window of the app
    /// with `pid`; `None` when there's no such terminal or it can't be read.
    fn terminal_text(&self, pid: u32) -> Option<String>;
    /// Each window of the app with `pid` that shows one terminal.
    fn terminal_frames(&self, pid: u32) -> Option<Vec<Frame>>;
    /// The app's name from its desktop entry.
    fn app_name(&self, class: &str, pid: u32) -> Option<String>;
}

/// The keys a reply presses.
pub(crate) trait Keys {
    /// The virtual keyboard exists (made now when it doesn't).
    fn ready(&self) -> bool;
    /// Shift+Insert: pastes PRIMARY in the terminal.
    fn paste(&self) -> bool;
    fn enter(&self) -> bool;
}

/// The PC around the window: the lock, the owner, the clock.
pub(crate) trait Desk {
    /// `None` when logind didn't say.
    fn locked(&self) -> Option<bool>;
    /// Someone used this PC's keyboard or mouse in the last [`OWNER_AWAY`],
    /// not counting a phone's own input; `None` when the desktop didn't say.
    fn owner_active(&self) -> Option<bool>;
    /// A phone's input is being typed now.
    fn phone_typing(&self) -> bool;
    fn wait(&self, how_long: Duration);
}

/// The four, together.
#[derive(Debug)]
pub(crate) struct Pc<'a, H, W, K, D> {
    pub host: &'a H,
    pub windows: &'a W,
    pub keys: &'a K,
    pub desk: &'a D,
}

// ---- The text (§2.5) ----

/// The text cleaned for a terminal: CRLF and CR to LF, a tab to four
/// spaces, trailing whitespace trimmed. `Err(BadMessage)` for an empty or
/// too long text, a control character other than LF (no `ESC` can end the
/// paste early) or a bidirectional control; `Ok(Err(Text))` when it starts
/// with `!` or `/`, which the agent reads as a command (§9.1).
pub(crate) fn clean(text: &str) -> Result<Result<String, Refused>, Error> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ");
    let text = text.trim_end();
    if text.trim_start().is_empty() || text.len() > MAX_TEXT {
        return Err(Error::BadMessage);
    }
    if text.chars().any(|c| (c.is_control() && c != '\n') || is_bidi_control(c)) {
        return Err(Error::BadMessage);
    }
    if text.trim_start().starts_with(['!', '/']) {
        return Ok(Err(Refused::Text));
    }
    Ok(Ok(text.to_owned()))
}

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

// ---- Rate and confirmations (§3.3, §3.5) ----

/// What a confirmation holds for: this phone, this session, this window of
/// this run of this terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Bound {
    phone: String,
    session: String,
    window: u64,
    emulator: u32,
    emulator_start: u64,
}

#[derive(Debug)]
struct Nonce {
    value: String,
    bound: Bound,
    at: Instant,
}

#[derive(Debug, Default)]
struct Pace {
    checks: VecDeque<Instant>,
    attempt: Option<Instant>,
    /// Replies entered in the last hour, and their session.
    entered: VecDeque<(Instant, String)>,
}

/// The PC's memory of replies: nonces, confirmed windows and the pace of
/// each phone. In memory only: a restart asks again.
#[derive(Debug, Default)]
pub(crate) struct Ledger {
    nonces: VecDeque<Nonce>,
    confirmed: VecDeque<(Bound, Instant)>,
    phones: VecDeque<(String, Pace)>,
}

fn wait_ms(wait: Duration) -> u32 {
    u32::try_from(wait.as_millis().clamp(1, u128::from(MAX_RETRY_MS))).unwrap_or(1)
}

impl Ledger {
    fn pace(&mut self, phone: &str) -> &mut Pace {
        if let Some(at) = self.phones.iter().position(|(p, _)| p == phone) {
            let entry = self.phones.remove(at).expect("an index just found");
            self.phones.push_back(entry);
        } else {
            if self.phones.len() >= MAX_PHONES {
                self.phones.pop_front();
            }
            self.phones.push_back((phone.to_owned(), Pace::default()));
        }
        &mut self.phones.back_mut().expect("just pushed").1
    }

    /// At most [`CHECKS_PER_SECOND`] checks a second; `Err` holds how long to wait.
    fn check(&mut self, phone: &str, now: Instant) -> Result<(), Duration> {
        let pace = self.pace(phone);
        pace.checks.retain(|at| now.saturating_duration_since(*at) < SECOND);
        if pace.checks.len() >= CHECKS_PER_SECOND {
            let oldest = pace.checks.front().copied().unwrap_or(now);
            return Err(SECOND.saturating_sub(now.saturating_duration_since(oldest)));
        }
        pace.checks.push_back(now);
        Ok(())
    }

    /// Whether an attempt to enter a reply is looked at now: one a second
    /// (but `exempt`, a send with a live confirmation), and within what was
    /// entered lately.
    fn attempt(&mut self, phone: &str, session: &str, now: Instant, exempt: bool) -> Result<(), Duration> {
        let pace = self.pace(phone);
        if !exempt
            && let Some(last) = pace.attempt
            && now.saturating_duration_since(last) < SECOND
        {
            return Err(SECOND - now.saturating_duration_since(last));
        }
        pace.attempt = Some(now);
        pace.entered.retain(|(at, _)| now.saturating_duration_since(*at) < HOUR);
        let since = |at: &Instant| now.saturating_duration_since(*at);
        if let Some((at, _)) = pace.entered.iter().rev().find(|(_, s)| s == session)
            && since(at) < SESSION_GAP
        {
            return Err(SESSION_GAP - since(at));
        }
        let last_minute: Vec<&Instant> =
            pace.entered.iter().map(|(at, _)| at).filter(|at| since(at) < MINUTE).collect();
        if last_minute.len() >= PER_MINUTE {
            return Err(MINUTE.saturating_sub(since(last_minute[last_minute.len() - PER_MINUTE])));
        }
        if pace.entered.len() >= PER_HOUR {
            return Err(HOUR.saturating_sub(since(&pace.entered[pace.entered.len() - PER_HOUR].0)));
        }
        Ok(())
    }

    /// A reply was entered: keys were pressed.
    fn entered(&mut self, phone: &str, session: &str, now: Instant) {
        let pace = self.pace(phone);
        pace.entered.push_back((now, session.to_owned()));
        while pace.entered.len() > PER_HOUR {
            pace.entered.pop_front();
        }
    }

    fn prune(&mut self, now: Instant) {
        self.nonces.retain(|n| now.saturating_duration_since(n.at) < NONCE_LIFE);
        self.confirmed.retain(|(_, at)| now.saturating_duration_since(*at) < CONFIRMED_FOR);
    }

    fn is_confirmed(&mut self, bound: &Bound, now: Instant) -> bool {
        self.prune(now);
        self.confirmed.iter().any(|(b, _)| b == bound)
    }

    /// A new one-time nonce for `bound`.
    fn mint(&mut self, bound: Bound, now: Instant) -> String {
        self.prune(now);
        let value = data_encoding::HEXLOWER.encode(&rand::random::<[u8; 16]>());
        if self.nonces.len() >= MAX_NONCES {
            self.nonces.pop_front();
        }
        self.nonces.push_back(Nonce { value: value.clone(), bound, at: now });
        value
    }

    /// Whether `nonce` is live for this phone and session (it may not match
    /// the window: that's for [`Ledger::redeem`] to say).
    fn holds(&mut self, phone: &str, session: &str, nonce: &str, now: Instant) -> bool {
        self.prune(now);
        self.nonces.iter().any(|n| n.value == nonce && n.bound.phone == phone && n.bound.session == session)
    }

    /// Spends `nonce` (once, matching or not); true when it was for
    /// `bound`, which is then confirmed.
    fn redeem(&mut self, nonce: &str, bound: &Bound, now: Instant) -> bool {
        self.prune(now);
        let Some(at) = self.nonces.iter().position(|n| n.value == nonce) else { return false };
        let spent = self.nonces.remove(at).expect("an index just found");
        if spent.bound != *bound {
            return false;
        }
        self.confirmed.retain(|(b, _)| !(b.phone == bound.phone && b.session == bound.session));
        if self.confirmed.len() >= MAX_CONFIRMED {
            self.confirmed.pop_front();
        }
        self.confirmed.push_back((bound.clone(), now));
        true
    }
}

/// The PC's replies: the ledger, and the one reply being entered.
#[derive(Debug, Default)]
pub(crate) struct Replies {
    ledger: Mutex<Ledger>,
    entering: Mutex<()>,
}

impl Replies {
    fn ledger(&self) -> std::sync::MutexGuard<'_, Ledger> {
        self.ledger.lock().unwrap_or_else(|e| e.into_inner())
    }
}

// ---- Finding the window (§1.3, §1.4, §1.5, §2.2) ----

/// A terminal G7 knows how to paste into: its executable, its window
/// classes, and the name shown when its desktop entry can't be read.
#[derive(Debug)]
struct Terminal {
    exe: &'static str,
    classes: &'static [&'static str],
    name: &'static str,
}

/// v1 (§9.1): the VTE terminals, where Shift+Insert pastes PRIMARY.
const TERMINALS: [Terminal; 2] = [
    Terminal {
        exe: "ptyxis",
        classes: &["org.gnome.Ptyxis", "org.gnome.Ptyxis.Devel", "ptyxis"],
        name: "Ptyxis",
    },
    Terminal {
        exe: "gnome-terminal-server",
        classes: &["org.gnome.Terminal", "gnome-terminal", "gnome-terminal-server"],
        name: "Terminal",
    },
];

/// Above the agent, these keep G7 from the window: a multiplexer, a remote
/// shell or an editor's terminal panel (§1.3).
const IN_BETWEEN: [&str; 11] = [
    "tmux",
    "zellij",
    "screen",
    "sshd",
    "sshd-session",
    "mosh-server",
    "code",
    "code-insiders",
    "codium",
    "cursor",
    "windsurf",
];

/// The program of [`IN_BETWEEN`] an ancestor is.
fn in_between(ancestor: &Ancestor) -> Option<&'static str> {
    let comm = ancestor.comm.split(':').next().unwrap_or_default().trim();
    IN_BETWEEN.iter().copied().find(|name| ancestor.exe_name == *name || comm == *name)
}

/// The editors of [`IN_BETWEEN`]: their terminal is a panel of an app.
const EDITORS: [&str; 5] = ["code", "code-insiders", "codium", "cursor", "windsurf"];

/// How the phone names a program of [`IN_BETWEEN`] that has no window of
/// its own to name it by.
fn program_name(program: &str) -> &'static str {
    match program {
        "tmux" => "tmux",
        "zellij" => "Zellij",
        "screen" => "Screen",
        "sshd" | "sshd-session" => "SSH",
        "mosh-server" => "Mosh",
        "code" => "VS Code",
        "code-insiders" => "VS Code Insiders",
        "codium" => "VSCodium",
        "cursor" => "Cursor",
        _ => "Windsurf",
    }
}

fn known_terminal(exe_name: &str, class: &str) -> Option<&'static Terminal> {
    TERMINALS.iter().find(|t| t.exe == exe_name && t.classes.iter().any(|c| c.eq_ignore_ascii_case(class)))
}

/// Whether the terminal's text shows one of the session's marks. Both are
/// compared without whitespace, so line wrapping can't break a match.
pub(crate) fn shows_a_mark(screen: &str, marks: &[String]) -> bool {
    let flat: String = screen.chars().filter(|c| !c.is_whitespace()).collect();
    marks.iter().any(|mark| {
        let mark: String = mark.chars().filter(|c| !c.is_whitespace()).collect();
        mark.chars().count() >= MIN_MARK_CHARS && flat.contains(&mark)
    })
}

/// The terminal that holds the session, and the window it's entered in.
#[derive(Debug, Clone)]
struct Resolved {
    window: u64,
    class: String,
    emulator: Ancestor,
    /// The window can only be showing this session's terminal (§1.4).
    structural: bool,
    target: Target,
}

fn cut(text: &str, max: usize) -> String {
    let end = (0..=max.min(text.len())).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    text[..end].to_owned()
}

/// A folder's label for the phone: never a path.
fn folder_label(folder: &str) -> String {
    cut(folder.rsplit('/').find(|part| !part.is_empty()).unwrap_or_default(), MAX_TARGET_BYTES)
}

/// Steps 2 and 4's checks of the session itself: open, one process, in
/// a terminal, at a prompt.
fn session_allows(place: &Place) -> Result<(), Refused> {
    if place.source != Source::Terminal {
        return Err(Refused::Place);
    }
    match place.state {
        Now::Ended => return Err(Refused::Ended),
        Now::NeedsYou | Now::MayNeedYou => return Err(Refused::NeedsYou),
        Now::Idle => {}
        // Claude queues a message sent mid-turn (O2), when its status file
        // says it's working; a guessed state is idle only (§9.1, S5). Codex
        // reports nothing a file reader can trust mid-turn.
        Now::Working if place.agent == Agent::Claude && place.state_exact => {}
        Now::Working => return Err(Refused::Busy),
    }
    if place.tty.is_none() {
        return Err(Refused::Place);
    }
    if !place.foreground {
        return Err(Refused::NoProcess);
    }
    Ok(())
}

/// Step 2: the session's one process, or why there's none.
fn placed<H: Host>(host: &H, session: &str) -> Result<Result<Place, Answer>, Error> {
    Ok(match host.place(session).map_err(|HostDown| Error::Internal)? {
        Found::Place(place) => Ok(place),
        Found::Unknown => return Err(Error::NotFound),
        Found::Ended => Err(Answer::refused(Refused::Ended)),
        Found::Ambiguous => Err(Answer::refused(Refused::Ambiguous)),
        Found::Elsewhere => Err(Answer::refused(Refused::Place)),
    })
}

/// Step 3: nobody at the PC, nothing in front that mustn't be covered.
fn pc_allows<D: Desk>(desk: &D, windows: &[Win], target_pid: Option<u32>) -> Result<(), Refused> {
    if desk.phone_typing() || desk.owner_active() != Some(false) {
        return Err(Refused::InUse);
    }
    // A film, a presentation or a call; a fullscreen terminal holding the
    // session is fine.
    if windows.iter().any(|w| w.focused && w.fullscreen && Some(w.pid) != target_pid) {
        return Err(Refused::InUse);
    }
    Ok(())
}

/// Step 4: the emulator above the agent, a terminal G7 knows, and the one
/// window of it to enter the reply in.
fn resolve<W: Windows>(windows: &W, place: &Place, listed: &[Win]) -> Result<Resolved, Refused> {
    let mut emulator = None;
    for ancestor in &place.chain {
        if in_between(ancestor).is_some() {
            return Err(Refused::Place);
        }
        if listed.iter().any(|w| w.pid == ancestor.pid) {
            emulator = Some(ancestor.clone());
            break;
        }
    }
    let emulator = emulator.ok_or(Refused::NoWindow)?;
    let own: Vec<&Win> = listed.iter().filter(|w| w.pid == emulator.pid && w.normal).collect();
    let first = own.first().ok_or(Refused::NoWindow)?;
    let terminal = known_terminal(&emulator.exe_name, &first.class).ok_or(Refused::Place)?;
    if own.iter().any(|w| known_terminal(&emulator.exe_name, &w.class).is_none()) {
        return Err(Refused::Place);
    }
    let ptys = place.ptys_under.iter().find(|(pid, _)| *pid == emulator.pid).map(|(_, n)| *n);
    // A process found by its command line may have switched sessions inside
    // (Codex's `/resume`, a Claude without its status file): only the
    // screen can say which one it shows.
    let by_status_file = place.agent == Agent::Claude && place.state_exact;
    let structural = by_status_file && own.len() == 1 && ptys == Some(1);
    let window = if structural { first.id } else { pick_window(windows, emulator.pid, &own, &place.marks)? };
    let class = own.iter().find(|w| w.id == window).map(|w| w.class.clone()).unwrap_or_default();
    let app = windows
        .app_name(&class, emulator.pid)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| terminal.name.to_owned());
    let target = target(place, &app, "terminal");
    Ok(Resolved { window, class, emulator, structural, target })
}

fn target(place: &Place, app: &str, kind: &'static str) -> Target {
    Target {
        app: cut(app, MAX_TARGET_BYTES),
        agent: place.agent.name().to_owned(),
        folder: folder_label(&place.folder),
        kind,
    }
}

/// A `place` refusal, naming where the session runs when something was
/// found above it: the app whose window that is (VS Code, a terminal G7
/// doesn't type into), or the program in between that has none (tmux,
/// SSH). The phone words its answer by it.
fn refused_place<W: Windows>(windows: &W, place: &Place, listed: Option<&[Win]>) -> Answer {
    let listed = match listed {
        Some(listed) => listed.to_vec(),
        None => windows.list().unwrap_or_default(),
    };
    Answer { target: place_target(windows, place, &listed), ..Answer::refused(Refused::Place) }
}

fn place_target<W: Windows>(windows: &W, place: &Place, listed: &[Win]) -> Option<Target> {
    let mut between = None;
    for ancestor in &place.chain {
        between = between.or(in_between(ancestor));
        let Some(window) = listed.iter().find(|w| w.pid == ancestor.pid && w.normal) else { continue };
        let app = windows
            .app_name(&window.class, window.pid)
            .filter(|name| !name.trim().is_empty())
            .or_else(|| between.map(|program| program_name(program).to_owned()))
            .unwrap_or_else(|| ancestor.exe_name.clone());
        let editor = between.is_some_and(|program| EDITORS.contains(&program))
            || EDITORS.contains(&ancestor.exe_name.as_str());
        return (!app.trim().is_empty())
            .then(|| target(place, &app, if editor { "app" } else { "terminal" }));
    }
    let program = between?;
    let kind = if EDITORS.contains(&program) { "app" } else { "terminal" };
    Some(target(place, program_name(program), kind))
}

/// Of a terminal's windows, the one showing the session: the one in front
/// if it does, else the single one whose terminal shows a mark. Read from
/// the accessibility bus without bringing any forward.
fn pick_window<W: Windows>(windows: &W, pid: u32, own: &[&Win], marks: &[String]) -> Result<u64, Refused> {
    let frames = windows.terminal_frames(pid).filter(|f| !f.is_empty()).ok_or(Refused::Ambiguous)?;
    let showing: Vec<&Frame> = frames.iter().filter(|f| shows_a_mark(&f.text, marks)).collect();
    if let [only] = own {
        // One window, several tabs: it's in front only if its selected tab is.
        return if showing.is_empty() { Err(Refused::Tab) } else { Ok(only.id) };
    }
    if let Some(front) = own.iter().find(|w| w.focused)
        && showing.iter().any(|f| f.active && f.title == front.title)
    {
        return Ok(front.id);
    }
    let [frame] = showing.as_slice() else { return Err(Refused::Ambiguous) };
    let titled: Vec<&&Win> = own.iter().filter(|w| w.title == frame.title).collect();
    match titled.as_slice() {
        [window] => Ok(window.id),
        _ => Err(Refused::Ambiguous),
    }
}

fn focused_on<W: Windows>(windows: &W, resolved: &Resolved) -> bool {
    windows.list().is_some_and(|list| {
        list.iter().any(|w| {
            w.id == resolved.window
                && w.focused
                && w.pid == resolved.emulator.pid
                && w.class == resolved.class
        })
    })
}

/// Clears PRIMARY on the way out, if it still holds the reply.
struct Primary<'a, W: Windows> {
    windows: &'a W,
    text: &'a str,
}

impl<W: Windows> Drop for Primary<'_, W> {
    fn drop(&mut self) {
        self.windows.clear_primary(self.text);
    }
}

impl<W: Windows> std::fmt::Debug for Primary<'_, W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Primary").finish_non_exhaustive()
    }
}

// ---- The sequence (§2.4) ----

/// Answers a phone's reply or check. `phone` is the asking phone's id.
pub(crate) fn answer<H: Host, W: Windows, K: Keys, D: Desk>(
    pc: &Pc<'_, H, W, K, D>,
    replies: &Replies,
    phone: &str,
    ask: &Ask,
) -> Result<Answer, Error> {
    let answer = match (&ask.text, ask.check) {
        (None, true) => check(pc, replies, phone, &ask.session)?,
        (Some(text), false) => send(pc, replies, phone, ask, text)?,
        _ => return Err(Error::BadMessage),
    };
    tracing::info!(
        session = %ask.session,
        check = ask.check,
        result = answer.outcome.result(),
        reason = answer.outcome.reason().unwrap_or_default(),
        emulator = answer.emulator.as_deref().unwrap_or_default(),
        "a phone's reply into a session"
    );
    Ok(answer)
}

fn rate(wait: Duration) -> Answer {
    Answer { retry_ms: Some(wait_ms(wait)), ..Answer::refused(Refused::Rate) }
}

/// Steps 1 to 4 without bringing anything forward: `ready`, or why not.
fn check<H: Host, W: Windows, K: Keys, D: Desk>(
    pc: &Pc<'_, H, W, K, D>,
    replies: &Replies,
    phone: &str,
    session: &str,
) -> Result<Answer, Error> {
    if let Err(wait) = replies.ledger().check(phone, Instant::now()) {
        return Ok(rate(wait));
    }
    let place = match placed(pc.host, session)? {
        Ok(place) => place,
        Err(answer) => return Ok(answer),
    };
    Ok(match ready(pc, &place) {
        Ok(resolved) => Answer::of(Outcome::Ready).at(&resolved),
        Err(answer) => *answer,
    })
}

/// Steps 2 (the session's state), 3 and 4.
fn ready<H: Host, W: Windows, K: Keys, D: Desk>(
    pc: &Pc<'_, H, W, K, D>,
    place: &Place,
) -> Result<Resolved, Box<Answer>> {
    let refused = |reason| Box::new(Answer::refused(reason));
    session_allows(place).map_err(|reason| match reason {
        Refused::Place => Box::new(refused_place(pc.windows, place, None)),
        reason => refused(reason),
    })?;
    // Before the extension: GNOME turns it off while the screen is locked.
    if pc.desk.locked() != Some(false) {
        return Err(refused(Refused::Locked));
    }
    if !pc.windows.ready() {
        return Err(refused(Refused::Setup));
    }
    let listed = pc.windows.list().ok_or_else(|| refused(Refused::Setup))?;
    // Step 3 before 4: nothing is read from the screen while someone's there.
    let in_chain = |pid: u32| place.chain.iter().any(|a| a.pid == pid);
    let front_pid = listed.iter().find(|w| w.focused).map(|w| w.pid).filter(|pid| in_chain(*pid));
    pc_allows(pc.desk, &listed, front_pid).map_err(refused)?;
    resolve(pc.windows, place, &listed).map_err(|reason| match reason {
        Refused::Place => Box::new(refused_place(pc.windows, place, Some(&listed))),
        reason => refused(reason),
    })
}

fn send<H: Host, W: Windows, K: Keys, D: Desk>(
    pc: &Pc<'_, H, W, K, D>,
    replies: &Replies,
    phone: &str,
    ask: &Ask,
    text: &str,
) -> Result<Answer, Error> {
    // Step 1: the text, then the pace.
    let text = match clean(text)? {
        Ok(text) => text,
        Err(reason) => return Ok(Answer::refused(reason)),
    };
    let now = Instant::now();
    {
        let mut ledger = replies.ledger();
        let exempt = ask.confirm.as_deref().is_some_and(|n| ledger.holds(phone, &ask.session, n, now));
        if let Err(wait) = ledger.attempt(phone, &ask.session, now, exempt) {
            return Ok(rate(wait));
        }
    }
    let Ok(_entering) = replies.entering.try_lock() else { return Err(Error::Busy) };

    // Step 2: the host.
    let place = match placed(pc.host, &ask.session)? {
        Ok(place) => place,
        Err(answer) => return Ok(answer),
    };
    // Steps 3 and 4.
    let resolved = match ready(pc, &place) {
        Ok(resolved) => resolved,
        Err(answer) => return Ok(*answer),
    };
    let bound = Bound {
        phone: phone.to_owned(),
        session: ask.session.clone(),
        window: resolved.window,
        emulator: resolved.emulator.pid,
        emulator_start: resolved.emulator.start_ticks,
    };
    {
        let mut ledger = replies.ledger();
        let confirmed = ledger.is_confirmed(&bound, now)
            || ask.confirm.as_deref().is_some_and(|nonce| ledger.redeem(nonce, &bound, now));
        if !confirmed {
            let nonce = ledger.mint(bound, now);
            return Ok(Answer { nonce: Some(nonce), ..Answer::refused(Refused::Confirm).at(&resolved) });
        }
    }
    if !pc.keys.ready() {
        return Ok(Answer::refused(Refused::Setup).at(&resolved));
    }

    // Step 5: bring it forward, never while locked.
    if pc.desk.locked() != Some(false) {
        return Ok(Answer::refused(Refused::Locked).at(&resolved));
    }
    if !pc.windows.activate(resolved.window) {
        return Ok(Answer::failed(Failed::Focus).at(&resolved));
    }
    let mut waited = Duration::ZERO;
    while !focused_on(pc.windows, &resolved) {
        if waited >= FOCUS_WAIT {
            return Ok(Answer::failed(Failed::Focus).at(&resolved));
        }
        pc.desk.wait(FOCUS_POLL);
        waited += FOCUS_POLL;
    }

    // Step 6: the proof, now that it's in front.
    if !resolved.structural {
        match pc.windows.terminal_text(resolved.emulator.pid) {
            Some(screen) if shows_a_mark(&screen, &place.marks) => {}
            Some(_) => return Ok(Answer::refused(Refused::Tab).at(&resolved)),
            None => return Ok(Answer::refused(Refused::Ambiguous).at(&resolved)),
        }
    }

    // Step 7: PRIMARY, still in front, the chord.
    if !pc.windows.set_primary(&text) {
        return Ok(Answer::refused(Refused::Setup).at(&resolved));
    }
    let _primary = Primary { windows: pc.windows, text: &text };
    if !focused_on(pc.windows, &resolved) {
        return Ok(Answer::failed(Failed::Focus).at(&resolved));
    }
    replies.ledger().entered(phone, &ask.session, Instant::now());
    if !pc.keys.paste() {
        return Ok(Answer::failed(Failed::Input).at(&resolved));
    }

    // Step 8: settle, then everything that can have changed.
    pc.desk.wait(SETTLE);
    let interrupted = Answer::failed(Failed::Interrupted).at(&resolved);
    if !focused_on(pc.windows, &resolved) || pc.desk.locked() != Some(false) {
        return Ok(interrupted);
    }
    match pc.host.place(&ask.session) {
        Ok(Found::Place(again))
            if again.pid == place.pid
                && again.start_ticks == place.start_ticks
                && session_allows(&again).is_ok() => {}
        _ => return Ok(interrupted),
    }

    // Step 9: Enter.
    if !pc.keys.enter() {
        return Ok(Answer::failed(Failed::Input).at(&resolved));
    }
    Ok(Answer::of(Outcome::Sent).at(&resolved))
}

// ---- This PC ----

/// A session's place as the host's `session.place` answers it.
fn found(result: SessionPlaceResult, folder: &str) -> Found {
    let place = match result {
        SessionPlaceResult::Place(place) => place,
        SessionPlaceResult::Refused(refused) => {
            return match refused.reason {
                PlaceRefusal::Ended => Found::Ended,
                PlaceRefusal::Ambiguous => Found::Ambiguous,
                // A word a newer host sends is a refusal too.
                PlaceRefusal::Place | PlaceRefusal::Unknown => Found::Elsewhere,
            };
        }
    };
    let state = match place.state {
        SessionState::Idle => Now::Idle,
        SessionState::Working => Now::Working,
        SessionState::NeedsYou if place.state_exact => Now::NeedsYou,
        SessionState::NeedsYou | SessionState::NeedsYouGuess => Now::MayNeedYou,
        SessionState::Ended => Now::Ended,
    };
    Found::Place(Place {
        agent: match place.agent {
            api::AgentId::Claude => Agent::Claude,
            api::AgentId::Codex => Agent::Codex,
        },
        source: match place.source {
            SessionSource::Terminal => Source::Terminal,
            SessionSource::DesktopApp => Source::DesktopApp,
            SessionSource::Other => Source::Other,
        },
        state,
        state_exact: place.state_exact,
        pid: place.pid,
        start_ticks: place.start_ticks,
        tty: place.tty,
        foreground: place.foreground,
        ptys_under: place.chain.iter().filter_map(|a| Some((a.pid, a.ptys_under?))).collect(),
        chain: place
            .chain
            .into_iter()
            .map(|a| Ancestor {
                pid: a.pid,
                start_ticks: a.start_ticks,
                comm: a.comm,
                exe_name: a.exe_name.unwrap_or_default(),
            })
            .collect(),
        marks: place.marks,
        folder: folder.to_owned(),
    })
}

/// The agent host, asked from the reply's own thread.
#[derive(Debug)]
struct LiveHost {
    client: Client,
    runtime: tokio::runtime::Handle,
    folder: String,
}

impl Host for LiveHost {
    fn place(&self, session: &str) -> Result<Found, HostDown> {
        match self.runtime.block_on(self.client.session_place(session.to_owned())) {
            Ok(result) => Ok(found(result, &self.folder)),
            Err(ClientError::Remote(e)) if e.code == ErrorCode::NotFound => Ok(Found::Unknown),
            Err(error) => {
                tracing::info!(%error, "the agent host didn't place a session");
                Err(HostDown)
            }
        }
    }
}

/// The answer as the core passes it to the phone.
fn wire(answer: Answer) -> AgentSessionReplied {
    AgentSessionReplied {
        result: answer.outcome.result().to_owned(),
        reason: answer.outcome.reason().map(str::to_owned),
        target: answer.target.map(|t| AgentReplyTarget {
            app: t.app,
            agent: t.agent,
            folder: t.folder,
            kind: t.kind.to_owned(),
        }),
        nonce: answer.nonce,
        retry_ms: answer.retry_ms.map(u64::from),
    }
}

fn wire_error(error: Error) -> AgentFailure {
    match error {
        Error::BadMessage => AgentFailure::BadMessage,
        Error::NotFound => AgentFailure::NotFound,
        Error::Busy => AgentFailure::Busy,
        Error::Internal => AgentFailure::Internal,
    }
}

/// A phone's `agent.session.reply` (`text: None` is a check), after the
/// core checked the capability, the toggles and the limits. `folder` is
/// the session's folder label when the page's list has it. Runs on a
/// thread of its own: it waits on windows, the accessibility bus and the
/// keyboard, and asks the host from there.
pub(crate) async fn phone(
    from: DeviceId,
    client: &Client,
    folder: Option<String>,
    session: String,
    text: Option<String>,
    confirm: Option<String>,
) -> Result<AgentReply, AgentFailure> {
    // A host older than `session.place` can't say where a session runs.
    if !client.host().has(cap::PLACE) {
        return Ok(AgentReply::Replied(wire(Answer::refused(Refused::Setup))));
    }
    let folder = match folder {
        Some(folder) => folder,
        None => client
            .sessions_list(Some(g7link_core::agent::MAX_SESSIONS as u32))
            .await
            .ok()
            .and_then(|list| list.into_iter().find(|s| s.id == session).map(|s| s.folder_label))
            .unwrap_or_default(),
    };
    let host = LiveHost { client: client.clone(), runtime: tokio::runtime::Handle::current(), folder };
    let ask = Ask { session, check: text.is_none(), text, confirm };
    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new().name("agent-reply".into()).spawn(move || {
        let pc = Pc { host: &host, windows: &LiveWindows, keys: &LiveKeys, desk: &LiveDesk };
        let _ = tx.send(answer(&pc, replies(), &from.to_string(), &ask));
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "can't enter a phone's reply");
        return Err(AgentFailure::Internal);
    }
    match rx.await {
        Ok(Ok(answer)) => Ok(AgentReply::Replied(wire(answer))),
        Ok(Err(error)) => Err(wire_error(error)),
        Err(_) => Err(AgentFailure::Internal),
    }
}

/// Offers `agent.reply` again when what it needs comes or goes (the
/// extension enabled after login, or updated). One thread for the app's
/// lifetime.
pub(crate) fn watch_setup() {
    const EVERY: Duration = Duration::from_secs(60);
    let spawned = std::thread::Builder::new().name("agent-reply-setup".into()).spawn(|| {
        loop {
            std::thread::sleep(EVERY);
            // Locked, the extension is off: the phone shows "unlock", not "set up".
            if !super::host_installed() || crate::sys::input::lock_state() != Some(false) {
                continue;
            }
            let before = *SETUP.lock().unwrap_or_else(|e| e.into_inner());
            if before.map(|s| s == Setup::Ready) != Some(offered()) {
                crate::core_host::refresh_capabilities();
            }
        }
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "can't follow what replies need");
    }
}

static REPLIES: OnceLock<Replies> = OnceLock::new();

/// The PC's one ledger.
pub(crate) fn replies() -> &'static Replies {
    REPLIES.get_or_init(Replies::default)
}

/// GNOME Shell's companion extension, the accessibility bus and the
/// desktop entries.
#[derive(Debug)]
pub(crate) struct LiveWindows;

impl Windows for LiveWindows {
    fn ready(&self) -> bool {
        crate::sys::shell_clipboard::has_primary() && crate::sys::shell_windows::has_states()
    }

    fn list(&self) -> Option<Vec<Win>> {
        let listed = crate::sys::shell_windows::list()?;
        Some(
            listed
                .into_iter()
                .map(|w| Win {
                    id: w.id,
                    pid: w.pid,
                    class: w.class,
                    title: w.title,
                    focused: w.focused,
                    fullscreen: w.fullscreen,
                    minimized: w.minimized,
                    normal: w.normal,
                })
                .collect(),
        )
    }

    fn activate(&self, id: u64) -> bool {
        crate::sys::shell_windows::activate(id)
    }

    fn set_primary(&self, text: &str) -> bool {
        crate::sys::shell_clipboard::set_primary(text)
    }

    fn clear_primary(&self, expected: &str) {
        crate::sys::shell_clipboard::clear_primary(expected);
    }

    fn terminal_text(&self, pid: u32) -> Option<String> {
        crate::sys::text_focus::focused_terminal_text(pid)
    }

    fn terminal_frames(&self, pid: u32) -> Option<Vec<Frame>> {
        let frames = crate::sys::text_focus::terminal_frames(pid)?;
        Some(frames.into_iter().map(|f| Frame { title: f.title, active: f.active, text: f.text }).collect())
    }

    fn app_name(&self, class: &str, pid: u32) -> Option<String> {
        crate::sys::app_entry::identify(class, pid).map(|app| app.name.clone())
    }
}

/// The virtual keyboard only: never the portal or a helper program.
#[derive(Debug)]
pub(crate) struct LiveKeys;

impl Keys for LiveKeys {
    fn ready(&self) -> bool {
        crate::sys::input::reply_keyboard()
    }

    fn paste(&self) -> bool {
        // Counted as a phone's input, so the idle check of the next reply
        // doesn't take it for someone at the PC.
        let _ours = crate::sys::text_focus::injecting();
        crate::sys::input::press_reply(crate::sys::input::ReplyKey::Paste)
    }

    fn enter(&self) -> bool {
        let _ours = crate::sys::text_focus::injecting();
        crate::sys::input::press_reply(crate::sys::input::ReplyKey::Enter)
    }
}

#[derive(Debug)]
pub(crate) struct LiveDesk;

impl Desk for LiveDesk {
    fn locked(&self) -> Option<bool> {
        crate::sys::input::lock_state()
    }

    fn owner_active(&self) -> Option<bool> {
        crate::sys::text_focus::someone_at_pc(OWNER_AWAY)
    }

    fn phone_typing(&self) -> bool {
        crate::sys::text_focus::phone_typing()
    }

    fn wait(&self, how_long: Duration) {
        std::thread::sleep(how_long);
    }
}

/// Whether this PC can enter replies now: the extension lists windows with
/// their states and writes PRIMARY alone, and the virtual keyboard can be
/// made. Asks the desktop (blocking); the answer is kept for [`setup`].
pub(crate) fn offered() -> bool {
    let extension = crate::sys::shell_clipboard::has_primary() && crate::sys::shell_windows::has_states();
    let keyboard = crate::sys::input::reply_keyboard_possible();
    let setup = match (extension, keyboard) {
        (true, true) => Setup::Ready,
        (false, _) => Setup::Extension,
        (true, false) => Setup::Keyboard,
    };
    *SETUP.lock().unwrap_or_else(|e| e.into_inner()) = Some(setup);
    setup == Setup::Ready
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    Ready,
    /// The companion extension is missing or too old.
    Extension,
    /// No virtual keyboard can be made.
    Keyboard,
}

static SETUP: Mutex<Option<Setup>> = Mutex::new(None);

/// What [`offered`] found last, for Settings: `ready`, `extension`,
/// `keyboard`, or empty when it hasn't been asked.
pub(crate) fn setup() -> &'static str {
    match *SETUP.lock().unwrap_or_else(|e| e.into_inner()) {
        Some(Setup::Ready) => "ready",
        Some(Setup::Extension) => "extension",
        Some(Setup::Keyboard) => "keyboard",
        None => "",
    }
}

#[cfg(test)]
#[path = "reply_tests.rs"]
mod tests;
